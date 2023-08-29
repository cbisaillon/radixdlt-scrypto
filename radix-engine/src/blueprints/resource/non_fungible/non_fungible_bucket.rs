use crate::blueprints::resource::*;
use crate::errors::ApplicationError;
use crate::errors::RuntimeError;
use crate::kernel::kernel_api::KernelNodeApi;
use crate::types::*;
use radix_engine_interface::api::field_api::LockFlags;
use radix_engine_interface::api::{ClientApi, FieldValue, ACTOR_REF_OUTER, ACTOR_STATE_SELF};
use radix_engine_interface::blueprints::resource::*;

pub struct NonFungibleBucketBlueprint;

impl NonFungibleBucketBlueprint {
    pub fn take<Y>(amount: &Decimal, api: &mut Y) -> Result<Bucket, RuntimeError>
    where
        Y: ClientApi<RuntimeError>,
    {
        Self::take_advanced(amount, WithdrawStrategy::Exact, api)
    }

    pub fn take_advanced<Y>(
        amount: &Decimal,
        withdraw_strategy: WithdrawStrategy,
        api: &mut Y,
    ) -> Result<Bucket, RuntimeError>
    where
        Y: ClientApi<RuntimeError>,
    {
        let handle = api.actor_open_field(
            ACTOR_STATE_SELF,
            NonFungibleBucketField::Liquid.into(),
            LockFlags::MUTABLE,
        )?;

        let mut liquid: LiquidNonFungibleResource = api.field_read_typed(handle)?;

        // Early exit if input amount is obviously wrong
        // This is to prevent for_withdrawal from overflowing in case a bad amount is sent in
        {
            if amount.is_negative() {
                return Err(RuntimeError::ApplicationError(
                    ApplicationError::BucketError(BucketError::InvalidAmount),
                ));
            }
            let bucket_amount_plus_one = liquid
                .amount()
                .safe_add(Decimal::ONE)
                .ok_or_else(|| BucketError::DecimalOverflow)?;
            if amount > &bucket_amount_plus_one {
                return Err(RuntimeError::ApplicationError(
                    ApplicationError::BucketError(BucketError::ResourceError(
                        ResourceError::InsufficientBalance,
                    )),
                ));
            }
        }

        // Apply withdraw strategy
        let amount = amount.for_withdrawal(0, withdraw_strategy);

        // Check amount
        let n = check_non_fungible_amount(&amount).map_err(|_| {
            RuntimeError::ApplicationError(ApplicationError::BucketError(
                BucketError::InvalidAmount,
            ))
        })?;

        // Take
        let taken = liquid
            .take_by_amount(n)
            .map_err(BucketError::ResourceError)
            .map_err(|e| RuntimeError::ApplicationError(ApplicationError::BucketError(e)))?;
        api.field_write_typed(handle, &liquid)?;
        api.field_close(handle)?;

        // Create node
        let bucket = NonFungibleResourceManagerBlueprint::create_bucket(taken.into_ids(), api)?;
        Ok(bucket)
    }

    pub fn take_non_fungibles<Y>(
        ids: &IndexSet<NonFungibleLocalId>,
        api: &mut Y,
    ) -> Result<Bucket, RuntimeError>
    where
        Y: ClientApi<RuntimeError>,
    {
        // Take
        let taken = Self::internal_take(ids, api)?;

        // Create node
        let bucket = NonFungibleResourceManagerBlueprint::create_bucket(taken.into_ids(), api)?;
        Ok(bucket)
    }

    pub fn put<Y>(bucket: Bucket, api: &mut Y) -> Result<(), RuntimeError>
    where
        Y: ClientApi<RuntimeError>,
    {
        // Drop other bucket
        let other_bucket = drop_non_fungible_bucket(bucket.0.as_node_id(), api)?;

        // Put
        Self::internal_put(other_bucket.liquid, api)?;

        Ok(())
    }

    pub fn get_non_fungible_local_ids<Y>(
        api: &mut Y,
    ) -> Result<IndexSet<NonFungibleLocalId>, RuntimeError>
    where
        Y: ClientApi<RuntimeError>,
    {
        let mut ids = Self::liquid_non_fungible_local_ids(api)?;
        ids.extend(Self::locked_non_fungible_local_ids(api)?);
        Ok(ids)
    }

    pub fn contains_non_fungible<Y>(
        id: NonFungibleLocalId,
        api: &mut Y,
    ) -> Result<bool, RuntimeError>
    where
        Y: KernelNodeApi + ClientApi<RuntimeError>,
    {
        let ids = Self::liquid_non_fungible_local_ids(api)?;
        if ids.contains(&id) {
            return Ok(true);
        }

        let ids = Self::locked_non_fungible_local_ids(api)?;
        if ids.contains(&id) {
            return Ok(true);
        }

        Ok(false)
    }

    pub fn get_amount<Y>(api: &mut Y) -> Result<Decimal, RuntimeError>
    where
        Y: KernelNodeApi + ClientApi<RuntimeError>,
    {
        let amount = Self::liquid_amount(api)?
            .safe_add(Self::locked_amount(api)?)
            .unwrap();

        Ok(amount)
    }

    pub fn get_resource_address<Y>(api: &mut Y) -> Result<ResourceAddress, RuntimeError>
    where
        Y: ClientApi<RuntimeError>,
    {
        let resource_address =
            ResourceAddress::new_or_panic(api.actor_get_node_id(ACTOR_REF_OUTER)?.into());

        Ok(resource_address)
    }

    pub fn create_proof_of_non_fungibles<Y>(
        receiver: &NodeId,
        ids: IndexSet<NonFungibleLocalId>,
        api: &mut Y,
    ) -> Result<Proof, RuntimeError>
    where
        Y: KernelNodeApi + ClientApi<RuntimeError>,
    {
        Self::lock_non_fungibles(&ids, api)?;

        let proof_info = ProofMoveableSubstate { restricted: false };
        let proof_evidence = NonFungibleProofSubstate::new(
            ids.clone(),
            indexmap!(
                LocalRef::Bucket(Reference(receiver.clone())) => ids
            ),
        )
        .map_err(|e| {
            RuntimeError::ApplicationError(ApplicationError::BucketError(BucketError::ProofError(
                e,
            )))
        })?;
        let proof_id = api.new_simple_object(
            NON_FUNGIBLE_PROOF_BLUEPRINT,
            indexmap! {
                0u8 => FieldValue::new(&proof_info),
                1u8 => FieldValue::new(&proof_evidence),
            },
        )?;
        Ok(Proof(Own(proof_id)))
    }

    pub fn create_proof_of_all<Y>(receiver: &NodeId, api: &mut Y) -> Result<Proof, RuntimeError>
    where
        Y: KernelNodeApi + ClientApi<RuntimeError>,
    {
        Self::create_proof_of_non_fungibles(receiver, Self::get_non_fungible_local_ids(api)?, api)
    }

    //===================
    // Protected method
    //===================

    pub fn lock_non_fungibles<Y>(
        ids: &IndexSet<NonFungibleLocalId>,
        api: &mut Y,
    ) -> Result<(), RuntimeError>
    where
        Y: KernelNodeApi + ClientApi<RuntimeError>,
    {
        let handle = api.actor_open_field(
            ACTOR_STATE_SELF,
            NonFungibleBucketField::Locked.into(),
            LockFlags::MUTABLE,
        )?;
        let mut locked: LockedNonFungibleResource = api.field_read_typed(handle)?;

        // Take from liquid if needed
        let delta: IndexSet<NonFungibleLocalId> = ids
            .iter()
            .cloned()
            .filter(|id| !locked.ids.contains_key(id))
            .collect();
        Self::internal_take(&delta, api)?;

        // Increase lock count
        for id in ids {
            locked.ids.entry(id.clone()).or_default().add_assign(1);
        }

        api.field_write_typed(handle, &locked)?;

        // Issue proof
        Ok(())
    }

    pub fn unlock_non_fungibles<Y>(
        ids: IndexSet<NonFungibleLocalId>,
        api: &mut Y,
    ) -> Result<(), RuntimeError>
    where
        Y: KernelNodeApi + ClientApi<RuntimeError>,
    {
        let handle = api.actor_open_field(
            ACTOR_STATE_SELF,
            NonFungibleBucketField::Locked.into(),
            LockFlags::MUTABLE,
        )?;
        let mut locked: LockedNonFungibleResource = api.field_read_typed(handle)?;

        let mut liquid_non_fungibles = IndexSet::<NonFungibleLocalId>::new();
        for id in ids {
            let cnt = locked
                .ids
                .remove(&id)
                .expect("Attempted to unlock non-fungible that was not locked");
            if cnt > 1 {
                locked.ids.insert(id, cnt - 1);
            } else {
                liquid_non_fungibles.insert(id);
            }
        }

        api.field_write_typed(handle, &locked)?;

        Self::internal_put(LiquidNonFungibleResource::new(liquid_non_fungibles), api)
    }

    //===================
    // Helper methods
    //===================

    fn liquid_amount<Y>(api: &mut Y) -> Result<Decimal, RuntimeError>
    where
        Y: ClientApi<RuntimeError>,
    {
        let handle = api.actor_open_field(
            ACTOR_STATE_SELF,
            NonFungibleBucketField::Liquid.into(),
            LockFlags::read_only(),
        )?;
        let substate_ref: LiquidNonFungibleResource = api.field_read_typed(handle)?;
        let amount = substate_ref.amount();
        api.field_close(handle)?;
        Ok(amount)
    }

    fn locked_amount<Y>(api: &mut Y) -> Result<Decimal, RuntimeError>
    where
        Y: ClientApi<RuntimeError>,
    {
        let handle = api.actor_open_field(
            ACTOR_STATE_SELF,
            NonFungibleBucketField::Locked.into(),
            LockFlags::read_only(),
        )?;
        let substate_ref: LockedNonFungibleResource = api.field_read_typed(handle)?;
        let amount = substate_ref.amount();
        api.field_close(handle)?;
        Ok(amount)
    }

    fn liquid_non_fungible_local_ids<Y>(
        api: &mut Y,
    ) -> Result<IndexSet<NonFungibleLocalId>, RuntimeError>
    where
        Y: ClientApi<RuntimeError>,
    {
        let handle = api.actor_open_field(
            ACTOR_STATE_SELF,
            NonFungibleBucketField::Liquid.into(),
            LockFlags::read_only(),
        )?;
        let substate_ref: LiquidNonFungibleResource = api.field_read_typed(handle)?;
        let ids = substate_ref.ids().clone();
        api.field_close(handle)?;
        Ok(ids)
    }

    fn locked_non_fungible_local_ids<Y>(
        api: &mut Y,
    ) -> Result<IndexSet<NonFungibleLocalId>, RuntimeError>
    where
        Y: ClientApi<RuntimeError>,
    {
        let handle = api.actor_open_field(
            ACTOR_STATE_SELF,
            NonFungibleBucketField::Locked.into(),
            LockFlags::read_only(),
        )?;
        let substate_ref: LockedNonFungibleResource = api.field_read_typed(handle)?;
        let ids = substate_ref.ids();
        api.field_close(handle)?;
        Ok(ids)
    }

    fn internal_take<Y>(
        ids: &IndexSet<NonFungibleLocalId>,
        api: &mut Y,
    ) -> Result<LiquidNonFungibleResource, RuntimeError>
    where
        Y: ClientApi<RuntimeError>,
    {
        let handle = api.actor_open_field(
            ACTOR_STATE_SELF,
            NonFungibleBucketField::Liquid.into(),
            LockFlags::MUTABLE,
        )?;
        let mut substate: LiquidNonFungibleResource = api.field_read_typed(handle)?;
        let taken = substate
            .take_by_ids(ids)
            .map_err(BucketError::ResourceError)
            .map_err(|e| RuntimeError::ApplicationError(ApplicationError::BucketError(e)))?;
        api.field_write_typed(handle, &substate)?;
        api.field_close(handle)?;
        Ok(taken)
    }

    fn internal_put<Y>(resource: LiquidNonFungibleResource, api: &mut Y) -> Result<(), RuntimeError>
    where
        Y: ClientApi<RuntimeError>,
    {
        if resource.is_empty() {
            return Ok(());
        }

        let handle = api.actor_open_field(
            ACTOR_STATE_SELF,
            NonFungibleBucketField::Liquid.into(),
            LockFlags::MUTABLE,
        )?;
        let mut substate: LiquidNonFungibleResource = api.field_read_typed(handle)?;
        substate.put(resource).map_err(|e| {
            RuntimeError::ApplicationError(ApplicationError::BucketError(
                BucketError::ResourceError(e),
            ))
        })?;
        api.field_write_typed(handle, &substate)?;
        api.field_close(handle)?;
        Ok(())
    }
}

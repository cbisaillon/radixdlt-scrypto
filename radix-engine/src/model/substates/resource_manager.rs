use crate::model::ResourceMethodRule::{Protected, Public};
use crate::model::{convert, InvokeError, MethodAuthorization};
use crate::model::{NonFungible, Resource, ResourceManagerError};
use crate::types::AccessRule::*;
use crate::types::ResourceMethodAuthKey::*;
use crate::types::*;

#[derive(Debug, Clone, TypeId, Encode, Decode, PartialEq, Eq)]
pub struct ResourceManagerSubstate {
    pub resource_type: ResourceType,
    pub metadata: HashMap<String, String>,
    pub method_table: HashMap<ResourceManagerMethod, ResourceMethodRule>,
    pub vault_method_table: HashMap<VaultMethod, ResourceMethodRule>,
    pub bucket_method_table: HashMap<BucketMethod, ResourceMethodRule>,
    pub authorization: HashMap<ResourceMethodAuthKey, MethodAccessRule>,
    pub total_supply: Decimal,
    pub nf_store_id: Option<NonFungibleStoreId>,
    pub resource_address: Option<ResourceAddress>, // always set after instantiation
}

impl ResourceManagerSubstate {
    pub fn new(
        resource_type: ResourceType,
        metadata: HashMap<String, String>,
        mut auth: HashMap<ResourceMethodAuthKey, (AccessRule, Mutability)>,
        nf_store_id: Option<NonFungibleStoreId>,
    ) -> Result<ResourceManagerSubstate, InvokeError<ResourceManagerError>> {
        let mut vault_method_table: HashMap<VaultMethod, ResourceMethodRule> = HashMap::new();
        vault_method_table.insert(VaultMethod::LockFee, Protected(Withdraw));
        vault_method_table.insert(VaultMethod::Take, Protected(Withdraw));
        vault_method_table.insert(VaultMethod::Put, Protected(Deposit));
        vault_method_table.insert(VaultMethod::GetAmount, Public);
        vault_method_table.insert(VaultMethod::GetResourceAddress, Public);
        vault_method_table.insert(VaultMethod::GetNonFungibleIds, Public);
        vault_method_table.insert(VaultMethod::CreateProof, Public);
        vault_method_table.insert(VaultMethod::CreateProofByAmount, Public);
        vault_method_table.insert(VaultMethod::CreateProofByIds, Public);
        vault_method_table.insert(VaultMethod::TakeNonFungibles, Protected(Withdraw));

        let bucket_method_table: HashMap<BucketMethod, ResourceMethodRule> = HashMap::new();

        let mut method_table: HashMap<ResourceManagerMethod, ResourceMethodRule> = HashMap::new();
        method_table.insert(ResourceManagerMethod::Mint, Protected(Mint));
        method_table.insert(
            ResourceManagerMethod::UpdateMetadata,
            Protected(UpdateMetadata),
        );
        method_table.insert(ResourceManagerMethod::CreateBucket, Public);
        method_table.insert(ResourceManagerMethod::GetMetadata, Public);
        method_table.insert(ResourceManagerMethod::GetResourceType, Public);
        method_table.insert(ResourceManagerMethod::GetTotalSupply, Public);
        method_table.insert(ResourceManagerMethod::CreateVault, Public);
        method_table.insert(ResourceManagerMethod::Burn, Protected(Burn));
        method_table.insert(ResourceManagerMethod::SetResourceAddress, Public);

        // Non Fungible methods
        method_table.insert(
            ResourceManagerMethod::UpdateNonFungibleData,
            Protected(UpdateNonFungibleData),
        );
        method_table.insert(ResourceManagerMethod::NonFungibleExists, Public);
        method_table.insert(ResourceManagerMethod::GetNonFungible, Public);

        let mut authorization: HashMap<ResourceMethodAuthKey, MethodAccessRule> = HashMap::new();
        for (auth_entry_key, default) in [
            (Mint, (DenyAll, LOCKED)),
            (Burn, (DenyAll, LOCKED)),
            (Withdraw, (AllowAll, LOCKED)),
            (Deposit, (AllowAll, LOCKED)),
            (UpdateMetadata, (DenyAll, LOCKED)),
            (UpdateNonFungibleData, (DenyAll, LOCKED)),
        ] {
            let entry = auth.remove(&auth_entry_key).unwrap_or(default);
            authorization.insert(auth_entry_key, MethodAccessRule::new(entry));
        }

        let resource_manager = ResourceManagerSubstate {
            resource_type,
            metadata,
            method_table,
            vault_method_table,
            bucket_method_table,
            authorization,
            total_supply: 0.into(),
            nf_store_id,
            resource_address: None,
        };

        Ok(resource_manager)
    }

    pub fn get_auth(
        &self,
        method: ResourceManagerMethod,
        args: &ScryptoValue,
    ) -> &MethodAuthorization {
        match &method {
            ResourceManagerMethod::UpdateAuth => {
                // FIXME we can't assume the input always match the function identifier
                // especially for the auth module code path
                let input: ResourceManagerUpdateAuthInvocation = scrypto_decode(&args.raw).unwrap();
                match self.authorization.get(&input.method) {
                    None => &MethodAuthorization::Unsupported,
                    Some(entry) => {
                        entry.get_update_auth(MethodAccessRuleMethod::Update(input.access_rule))
                    }
                }
            }
            ResourceManagerMethod::LockAuth => {
                // FIXME we can't assume the input always match the function identifier
                // especially for the auth module code path
                let input: ResourceManagerLockAuthInvocation = scrypto_decode(&args.raw).unwrap();
                match self.authorization.get(&input.method) {
                    None => &MethodAuthorization::Unsupported,
                    Some(entry) => entry.get_update_auth(MethodAccessRuleMethod::Lock()),
                }
            }
            ResourceManagerMethod::Burn => self
                .authorization
                .get(&ResourceMethodAuthKey::Burn)
                .unwrap_or_else(|| panic!("Authorization for {:?} not specified", method))
                .get_method_auth(),
            _ => match self.method_table.get(&method) {
                None => &MethodAuthorization::Unsupported,
                Some(Public) => &MethodAuthorization::AllowAll,
                Some(Protected(method)) => self
                    .authorization
                    .get(method)
                    .unwrap_or_else(|| panic!("Authorization for {:?} not specified", method))
                    .get_method_auth(),
            },
        }
    }

    pub fn check_amount(&self, amount: Decimal) -> Result<(), InvokeError<ResourceManagerError>> {
        let divisibility = self.resource_type.divisibility();

        if amount.is_negative()
            || amount.0 % I256::from(10i128.pow((18 - divisibility).into())) != I256::from(0)
        {
            Err(InvokeError::Error(ResourceManagerError::InvalidAmount(
                amount,
                divisibility,
            )))
        } else {
            Ok(())
        }
    }

    pub fn get_vault_auth(&self, vault_fn: VaultMethod) -> &MethodAuthorization {
        match self.vault_method_table.get(&vault_fn) {
            None => &MethodAuthorization::Unsupported,
            Some(Public) => &MethodAuthorization::AllowAll,
            Some(Protected(auth_key)) => self
                .authorization
                .get(auth_key)
                .unwrap_or_else(|| panic!("Authorization for {:?} not specified", vault_fn))
                .get_method_auth(),
        }
    }

    pub fn get_bucket_auth(&self, bucket_method: BucketMethod) -> &MethodAuthorization {
        match self.bucket_method_table.get(&bucket_method) {
            None => &MethodAuthorization::Unsupported,
            Some(Public) => &MethodAuthorization::AllowAll,
            Some(Protected(method)) => self
                .authorization
                .get(method)
                .unwrap_or_else(|| panic!("Authorization for {:?} not specified", bucket_method))
                .get_method_auth(),
        }
    }

    pub fn burn(&mut self, amount: Decimal) {
        self.total_supply -= amount;
    }

    pub fn update_metadata(
        &mut self,
        new_metadata: HashMap<String, String>,
    ) -> Result<(), InvokeError<ResourceManagerError>> {
        self.metadata = new_metadata;

        Ok(())
    }

    pub fn set_resource_address(
        &mut self,
        resource_address: ResourceAddress,
    ) -> Result<(), InvokeError<ResourceManagerError>> {
        if self.resource_address.is_some() {
            return Err(InvokeError::Error(
                ResourceManagerError::ResourceAddressAlreadySet,
            ));
        }

        self.resource_address = Some(resource_address);

        Ok(())
    }

    pub fn mint(
        &mut self,
        mint_params: MintParams,
        self_address: ResourceAddress,
    ) -> Result<(Resource, HashMap<NonFungibleId, NonFungible>), InvokeError<ResourceManagerError>>
    {
        match mint_params {
            MintParams::Fungible { amount } => self.mint_fungible(amount, self_address),
            MintParams::NonFungible { entries } => self.mint_non_fungibles(entries, self_address),
        }
    }

    pub fn mint_fungible(
        &mut self,
        amount: Decimal,
        self_address: ResourceAddress,
    ) -> Result<(Resource, HashMap<NonFungibleId, NonFungible>), InvokeError<ResourceManagerError>>
    {
        if let ResourceType::Fungible { divisibility } = self.resource_type {
            // check amount
            self.check_amount(amount)?;

            // Practically impossible to overflow the Decimal type with this limit in place.
            if amount > dec!("1000000000000000000") {
                return Err(InvokeError::Error(
                    ResourceManagerError::MaxMintAmountExceeded,
                ));
            }

            self.total_supply += amount;

            Ok((
                Resource::new_fungible(self_address, divisibility, amount),
                HashMap::new(),
            ))
        } else {
            Err(InvokeError::Error(
                ResourceManagerError::ResourceTypeDoesNotMatch,
            ))
        }
    }

    pub fn mint_non_fungibles(
        &mut self,
        entries: HashMap<NonFungibleId, (Vec<u8>, Vec<u8>)>,
        self_address: ResourceAddress,
    ) -> Result<(Resource, HashMap<NonFungibleId, NonFungible>), InvokeError<ResourceManagerError>>
    {
        // check resource type
        if !matches!(self.resource_type, ResourceType::NonFungible) {
            return Err(InvokeError::Error(
                ResourceManagerError::ResourceTypeDoesNotMatch,
            ));
        }

        // check amount
        let amount: Decimal = entries.len().into();
        self.check_amount(amount)?;

        self.total_supply += amount;

        // Allocate non-fungibles
        let mut ids = BTreeSet::new();
        let mut non_fungibles = HashMap::new();
        for (id, data) in entries {
            let non_fungible = NonFungible::new(data.0, data.1);
            ids.insert(id.clone());
            non_fungibles.insert(id, non_fungible);
        }

        Ok((Resource::new_non_fungible(self_address, ids), non_fungibles))
    }
}

#[derive(Debug, Clone, TypeId, Encode, Decode, PartialEq, Eq)]
pub enum ResourceMethodRule {
    Public,
    Protected(ResourceMethodAuthKey),
}

#[derive(Debug, Clone, TypeId, Encode, Decode, PartialEq, Eq)]
pub struct MethodAccessRule {
    pub auth: MethodAuthorization,
    pub update_auth: MethodAuthorization,
}

pub enum MethodAccessRuleMethod {
    Lock(),
    Update(AccessRule),
}

/// Converts soft authorization rule to a hard authorization rule.
/// Currently required as all auth is defined by soft authorization rules.
macro_rules! convert_auth {
    ($auth:expr) => {
        convert(&Type::Unit, &ScryptoValue::unit(), &$auth)
    };
}

impl MethodAccessRule {
    // TODO: turn this into a proper node, i.e. id generation and invocation support

    pub fn new(entry: (AccessRule, Mutability)) -> Self {
        MethodAccessRule {
            auth: convert_auth!(entry.0),
            update_auth: match entry.1 {
                Mutability::LOCKED => MethodAuthorization::DenyAll,
                Mutability::MUTABLE(method_auth) => convert_auth!(method_auth),
            },
        }
    }

    pub fn get_method_auth(&self) -> &MethodAuthorization {
        &self.auth
    }

    pub fn get_update_auth(&self, method: MethodAccessRuleMethod) -> &MethodAuthorization {
        match method {
            MethodAccessRuleMethod::Lock() | MethodAccessRuleMethod::Update(_) => &self.update_auth,
        }
    }

    pub fn main(
        &mut self,
        method: MethodAccessRuleMethod,
    ) -> Result<ScryptoValue, InvokeError<ResourceManagerError>> {
        match method {
            MethodAccessRuleMethod::Lock() => self.lock(),
            MethodAccessRuleMethod::Update(method_auth) => {
                self.update(method_auth);
            }
        }

        Ok(ScryptoValue::from_typed(&()))
    }

    fn update(&mut self, method_auth: AccessRule) {
        self.auth = convert_auth!(method_auth)
    }

    fn lock(&mut self) {
        self.update_auth = MethodAuthorization::DenyAll;
    }
}
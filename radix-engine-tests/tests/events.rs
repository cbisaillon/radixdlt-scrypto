use radix_engine::blueprints::consensus_manager::{
    ClaimXrdEvent, EpochChangeEvent, RegisterValidatorEvent, RoundChangeEvent, StakeEvent,
    UnregisterValidatorEvent, UnstakeEvent, UpdateAcceptingStakeDelegationStateEvent,
};
use radix_engine::blueprints::package::PackageError;
use radix_engine::blueprints::resource::*;
use radix_engine::errors::{
    ApplicationError, PayloadValidationAgainstSchemaError, RuntimeError, SystemError,
};
use radix_engine::system::node_modules::metadata::SetMetadataEvent;
use radix_engine::types::*;
use radix_engine_interface::api::node_modules::auth::{RoleDefinition, ToRoleEntry};
use radix_engine_interface::api::node_modules::metadata::MetadataValue;
use radix_engine_interface::api::node_modules::ModuleConfig;
use radix_engine_interface::api::ObjectModuleId;
use radix_engine_interface::blueprints::account::ACCOUNT_TRY_DEPOSIT_BATCH_OR_ABORT_IDENT;
use radix_engine_interface::blueprints::consensus_manager::*;
use radix_engine_interface::{burn_roles, metadata, metadata_init, mint_roles, recall_roles};
use scrypto::prelude::{AccessRule, FromPublicKey};
use scrypto::NonFungibleData;
use scrypto_unit::*;
use transaction::model::InstructionV1;
use transaction::prelude::*;
use transaction::signing::secp256k1::Secp256k1PrivateKey;

#[test]
fn create_proof_emits_correct_events() {
    // Arrange
    let mut test_runner = TestRunnerBuilder::new().build();
    let (pk, _, account) = test_runner.new_allocated_account();

    // Act
    let manifest = ManifestBuilder::new()
        .lock_fee(FAUCET, dec!(500))
        .create_proof_from_account_of_amount(account, XRD, dec!(1))
        .build();
    let receipt =
        test_runner.execute_manifest(manifest, vec![NonFungibleGlobalId::from_public_key(&pk)]);

    // Assert
    let events = &receipt.expect_commit_success().application_events;
    for event in events {
        let name = test_runner.event_name(&event.0);
        println!("{:?} - {}", event.0, name);
    }
    assert!(match events.get(0) {
        Some((
            event_identifier @ EventTypeIdentifier(Emitter::Method(_, ObjectModuleId::Main), ..),
            ref event_data,
        )) if test_runner.is_event_name_equal::<fungible_vault::LockFeeEvent>(event_identifier)
            && is_decoded_equal(
                &fungible_vault::LockFeeEvent { amount: 500.into() },
                event_data
            ) =>
            true,
        _ => false,
    });
    assert!(match events.get(1) {
        Some((
            event_identifier @ EventTypeIdentifier(Emitter::Method(_, ObjectModuleId::Main), ..),
            ref event_data,
        )) if test_runner.is_event_name_equal::<BurnFungibleResourceEvent>(event_identifier)
            && is_decoded_equal(
                &BurnFungibleResourceEvent {
                    amount: receipt.expect_commit_success().fee_summary.to_burn_amount()
                },
                event_data
            ) =>
            true,
        _ => false,
    });
}

//=========
// Scrypto
//=========

#[test]
fn scrypto_cant_emit_unregistered_event() {
    // Arrange
    let mut test_runner = TestRunnerBuilder::new().without_trace().build();
    let package_address = test_runner.compile_and_publish("./tests/blueprints/events");

    let manifest = ManifestBuilder::new()
        .call_function(
            package_address,
            "ScryptoEvents",
            "emit_unregistered_event",
            manifest_args!(12u64),
        )
        .build();

    // Act
    let receipt = test_runner.execute_manifest_ignoring_fee(manifest, vec![]);

    // Assert
    receipt.expect_specific_failure(|e| match e {
        RuntimeError::SystemError(SystemError::PayloadValidationAgainstSchemaError(
            PayloadValidationAgainstSchemaError::EventDoesNotExist(event),
        )) if event.eq("UnregisteredEvent") => true,
        _ => false,
    });
}

#[test]
fn scrypto_can_emit_registered_events() {
    // Arrange
    let mut test_runner = TestRunnerBuilder::new().build();
    let package_address = test_runner.compile_and_publish("./tests/blueprints/events");

    let manifest = ManifestBuilder::new()
        .lock_fee(FAUCET, 500)
        .call_function(
            package_address,
            "ScryptoEvents",
            "emit_registered_event",
            manifest_args!(12u64),
        )
        .build();

    // Act
    let receipt = test_runner.execute_manifest(manifest, vec![]);

    // Assert
    let events = receipt.expect_commit(true).application_events.clone();
    for event in &events {
        let name = test_runner.event_name(&event.0);
        println!("{:?} - {}", event.0, name);
    }
    assert_eq!(events.len(), 3); // Three events: lock fee, registered event and burn fee
    assert!(match events.get(0) {
        Some((
            event_identifier @ EventTypeIdentifier(Emitter::Method(_, ObjectModuleId::Main), ..),
            ref event_data,
        )) if test_runner.is_event_name_equal::<fungible_vault::LockFeeEvent>(event_identifier)
            && is_decoded_equal(
                &fungible_vault::LockFeeEvent { amount: 500.into() },
                event_data
            ) =>
            true,
        _ => false,
    });
    assert!(match events.get(1) {
        Some((
            event_identifier @ EventTypeIdentifier(Emitter::Function(blueprint_id), ..),
            ref event_data,
        )) if test_runner.is_event_name_equal::<RegisteredEvent>(event_identifier)
            && is_decoded_equal(&RegisteredEvent { number: 12 }, event_data)
            && blueprint_id.package_address == package_address
            && blueprint_id.blueprint_name.eq("ScryptoEvents") =>
            true,
        _ => false,
    });
}

#[test]
fn cant_publish_a_package_with_non_struct_or_enum_event() {
    // Arrange
    let mut test_runner = TestRunnerBuilder::new().without_trace().build();

    let (code, definition) = Compile::compile("./tests/blueprints/events_invalid");
    let manifest = ManifestBuilder::new()
        .lock_fee(FAUCET, 500)
        .publish_package_advanced(None, code, definition, BTreeMap::new(), OwnerRole::None)
        .build();

    // Act
    let receipt = test_runner.execute_manifest(manifest, vec![]);

    // Assert
    receipt.expect_specific_failure(|runtime_error| {
        matches!(
            runtime_error,
            RuntimeError::ApplicationError(ApplicationError::PackageError(
                PackageError::InvalidEventSchema,
            )),
        )
    });
}

#[test]
fn local_type_index_with_misleading_name_fails() {
    // Arrange
    let mut test_runner = TestRunnerBuilder::new().without_trace().build();

    let (code, mut definition) = Compile::compile("./tests/blueprints/events");
    let blueprint_setup = definition.blueprints.get_mut("ScryptoEvents").unwrap();
    blueprint_setup.schema.events.event_schema.insert(
        "HelloHelloEvent".to_string(),
        blueprint_setup
            .schema
            .events
            .event_schema
            .get("RegisteredEvent")
            .unwrap()
            .clone(),
    );

    let manifest = ManifestBuilder::new()
        .lock_fee(FAUCET, 500)
        .publish_package_advanced(None, code, definition, BTreeMap::new(), OwnerRole::None)
        .build();

    // Act
    let receipt = test_runner.execute_manifest(manifest, vec![]);

    // Assert
    receipt.expect_specific_failure(|runtime_error| {
        matches!(
            runtime_error,
            RuntimeError::ApplicationError(ApplicationError::PackageError(
                PackageError::EventNameMismatch { .. },
            )),
        )
    });
}

//=======
// Vault
//=======

#[test]
fn locking_fee_against_a_vault_emits_correct_events() {
    // Arrange
    let mut test_runner = TestRunnerBuilder::new().without_trace().build();

    let manifest = ManifestBuilder::new().lock_fee(FAUCET, 500).build();

    // Act
    let receipt = test_runner.execute_manifest(manifest, vec![]);

    // Assert
    {
        let events = receipt.expect_commit(true).clone().application_events;
        for event in &events {
            let name = test_runner.event_name(&event.0);
            println!("{:?} - {}", event.0, name);
        }
        assert_eq!(events.len(), 2); // Two events: lock fee and burn fee
        assert!(match events.get(0) {
            Some((
                event_identifier
                @ EventTypeIdentifier(Emitter::Method(_, ObjectModuleId::Main), ..),
                ref event_data,
            )) if test_runner
                .is_event_name_equal::<fungible_vault::LockFeeEvent>(event_identifier)
                && is_decoded_equal(
                    &fungible_vault::LockFeeEvent { amount: 500.into() },
                    event_data
                ) =>
                true,
            _ => false,
        });
    }
}

#[test]
fn vault_fungible_recall_emits_correct_events() {
    // Arrange
    let mut test_runner = TestRunnerBuilder::new().without_trace().build();
    let (_, _, account) = test_runner.new_account(false);
    let recallable_resource_address = test_runner.create_recallable_token(account);
    let vault_id = test_runner.get_component_vaults(account, recallable_resource_address)[0];

    let manifest = ManifestBuilder::new()
        .lock_fee(FAUCET, 500)
        .recall(InternalAddress::new_or_panic(vault_id.into()), 1)
        .try_deposit_batch_or_abort(account, None)
        .build();

    // Act
    let receipt = test_runner.execute_manifest(manifest, vec![]);

    // Assert
    {
        let events = receipt.expect_commit(true).clone().application_events;
        for event in &events {
            let name = test_runner.event_name(&event.0);
            println!("{:?} - {}", event.0, name);
        }
        assert_eq!(events.len(), 4);
        assert!(match events.get(0) {
            Some((
                event_identifier
                @ EventTypeIdentifier(Emitter::Method(_, ObjectModuleId::Main), ..),
                ref event_data,
            )) if test_runner
                .is_event_name_equal::<fungible_vault::LockFeeEvent>(event_identifier)
                && is_decoded_equal(
                    &fungible_vault::LockFeeEvent { amount: 500.into() },
                    event_data
                ) =>
                true,
            _ => false,
        });
        assert!(match events.get(1) {
            Some((
                event_identifier
                @ EventTypeIdentifier(Emitter::Method(_, ObjectModuleId::Main), ..),
                ref event_data,
            )) if test_runner
                .is_event_name_equal::<fungible_vault::RecallEvent>(event_identifier)
                && is_decoded_equal(&fungible_vault::RecallEvent::new(1.into()), event_data) =>
                true,
            _ => false,
        });
        assert!(match events.get(2) {
            Some((
                event_identifier
                @ EventTypeIdentifier(Emitter::Method(_, ObjectModuleId::Main), ..),
                ref event_data,
            )) if test_runner
                .is_event_name_equal::<fungible_vault::DepositEvent>(event_identifier)
                && is_decoded_equal(&fungible_vault::DepositEvent::new(1.into()), event_data) =>
                true,
            _ => false,
        });
    }
}

#[test]
fn vault_non_fungible_recall_emits_correct_events() {
    // Arrange
    let mut test_runner = TestRunnerBuilder::new().without_trace().build();
    let (_, _, account) = test_runner.new_account(false);
    let (recallable_resource_address, non_fungible_local_id) = {
        let id = NonFungibleLocalId::Integer(IntegerNonFungibleLocalId::new(1));

        let manifest = ManifestBuilder::new()
            .lock_fee(FAUCET, 500)
            .create_non_fungible_resource(
                OwnerRole::None,
                NonFungibleIdType::Integer,
                false,
                NonFungibleResourceRoles {
                    recall_roles: recall_roles! {
                        recaller => rule!(allow_all);
                        recaller_updater => rule!(deny_all);
                    },
                    ..Default::default()
                },
                metadata!(),
                Some([(id.clone(), EmptyStruct {})]),
            )
            .try_deposit_batch_or_abort(account, None)
            .build();
        let receipt = test_runner.execute_manifest(manifest, vec![]);
        (receipt.expect_commit(true).new_resource_addresses()[0], id)
    };
    let vault_id = test_runner.get_component_vaults(account, recallable_resource_address)[0];

    let manifest = ManifestBuilder::new()
        .lock_fee(FAUCET, 500)
        .recall(InternalAddress::new_or_panic(vault_id.into()), 1)
        .try_deposit_batch_or_abort(account, None)
        .build();

    // Act
    let receipt = test_runner.execute_manifest(manifest, vec![]);

    // Assert
    {
        let events = receipt.expect_commit(true).clone().application_events;
        for event in &events {
            let name = test_runner.event_name(&event.0);
            println!("{:?} - {}", event.0, name);
        }
        assert_eq!(events.len(), 4);
        assert!(match events.get(0) {
            Some((
                event_identifier
                @ EventTypeIdentifier(Emitter::Method(_, ObjectModuleId::Main), ..),
                ref event_data,
            )) if test_runner
                .is_event_name_equal::<fungible_vault::LockFeeEvent>(event_identifier)
                && is_decoded_equal(
                    &fungible_vault::LockFeeEvent { amount: 500.into() },
                    event_data
                ) =>
                true,
            _ => false,
        });
        assert!(match events.get(1) {
            Some((
                event_identifier
                @ EventTypeIdentifier(Emitter::Method(_, ObjectModuleId::Main), ..),
                ref event_data,
            )) if test_runner
                .is_event_name_equal::<non_fungible_vault::RecallEvent>(event_identifier)
                && is_decoded_equal(
                    &non_fungible_vault::RecallEvent::new(btreeset!(NonFungibleLocalId::integer(
                        1
                    ))),
                    event_data
                ) =>
                true,
            _ => false,
        });
        assert!(match events.get(2) {
            Some((
                event_identifier
                @ EventTypeIdentifier(Emitter::Method(_, ObjectModuleId::Main), ..),
                ref event_data,
            )) if test_runner
                .is_event_name_equal::<non_fungible_vault::DepositEvent>(event_identifier)
                && is_decoded_equal(
                    &non_fungible_vault::DepositEvent::new([non_fungible_local_id.clone()].into()),
                    event_data
                ) =>
                true,
            _ => false,
        });
    }
}

//==================
// Resource Manager
//==================

#[test]
fn resource_manager_new_vault_emits_correct_events() {
    // Arrange
    let mut test_runner = TestRunnerBuilder::new().without_trace().build();
    let (_, _, account) = test_runner.new_account(false);

    let manifest = ManifestBuilder::new()
        .lock_fee(FAUCET, 500)
        .create_fungible_resource(
            OwnerRole::None,
            false,
            18,
            FungibleResourceRoles::default(),
            metadata!(),
            Some(1.into()),
        )
        .try_deposit_batch_or_abort(account, None)
        .build();

    // Act
    let receipt = test_runner.execute_manifest(manifest, vec![]);

    // Assert
    {
        let events = receipt.expect_commit(true).clone().application_events;
        for event in &events {
            let name = test_runner.event_name(&event.0);
            println!("{:?} - {}", event.0, name);
        }
        assert_eq!(events.len(), 5);
        assert!(match events.get(0) {
            Some((
                event_identifier
                @ EventTypeIdentifier(Emitter::Method(_, ObjectModuleId::Main), ..),
                ref event_data,
            )) if test_runner
                .is_event_name_equal::<fungible_vault::LockFeeEvent>(event_identifier)
                && is_decoded_equal(
                    &fungible_vault::LockFeeEvent { amount: 500.into() },
                    event_data
                ) =>
                true,
            _ => false,
        });
        assert!(match events.get(1) {
            Some((
                event_identifier @ EventTypeIdentifier(
                    Emitter::Method(_node_id, ObjectModuleId::Main),
                    ..,
                ),
                ..,
            )) if test_runner
                .is_event_name_equal::<MintFungibleResourceEvent>(event_identifier) =>
                true,
            _ => false,
        });
        assert!(match events.get(2) {
            Some((
                event_identifier @ EventTypeIdentifier(
                    Emitter::Method(_node_id, ObjectModuleId::Main),
                    ..,
                ),
                ..,
            )) if test_runner.is_event_name_equal::<VaultCreationEvent>(event_identifier) => true,
            _ => false,
        });
        assert!(match events.get(3) {
            Some((
                event_identifier
                @ EventTypeIdentifier(Emitter::Method(_, ObjectModuleId::Main), ..),
                ref event_data,
            )) if test_runner
                .is_event_name_equal::<fungible_vault::DepositEvent>(event_identifier)
                && is_decoded_equal(&fungible_vault::DepositEvent::new(1.into()), event_data) =>
                true,
            _ => false,
        });
    }
}

#[test]
fn resource_manager_mint_and_burn_fungible_resource_emits_correct_events() {
    // Arrange
    let mut test_runner = TestRunnerBuilder::new().without_trace().build();
    let (_, _, account) = test_runner.new_account(false);
    let resource_address = {
        let manifest = ManifestBuilder::new()
            .lock_fee(FAUCET, 500)
            .create_fungible_resource(
                OwnerRole::None,
                false,
                18,
                FungibleResourceRoles {
                    mint_roles: mint_roles! {
                        minter => rule!(allow_all);
                        minter_updater => rule!(deny_all);
                    },
                    burn_roles: burn_roles! {
                        burner => rule!(allow_all);
                        burner_updater => rule!(deny_all);
                    },
                    ..Default::default()
                },
                metadata!(),
                None,
            )
            .try_deposit_batch_or_abort(account, None)
            .build();
        let receipt = test_runner.execute_manifest(manifest, vec![]);
        receipt.expect_commit(true).new_resource_addresses()[0]
    };

    let manifest = ManifestBuilder::new()
        .lock_fee(FAUCET, 500)
        .mint_fungible(resource_address, 10)
        .burn_from_worktop(10, resource_address)
        .build();

    // Act
    let receipt = test_runner.execute_manifest(manifest, vec![]);

    // Assert
    {
        let events = receipt.expect_commit(true).clone().application_events;
        for event in &events {
            let name = test_runner.event_name(&event.0);
            println!("{:?} - {}", event.0, name);
        }
        assert_eq!(events.len(), 4); // Four events: vault lock fee, resource manager mint fungible, resource manager burn fungible, burn fee
        assert!(match events.get(0) {
            Some((
                event_identifier
                @ EventTypeIdentifier(Emitter::Method(_, ObjectModuleId::Main), ..),
                ref event_data,
            )) if test_runner
                .is_event_name_equal::<fungible_vault::LockFeeEvent>(event_identifier)
                && is_decoded_equal(
                    &fungible_vault::LockFeeEvent { amount: 500.into() },
                    event_data
                ) =>
                true,
            _ => false,
        });
        assert!(match events.get(1) {
            Some((
                event_identifier
                @ EventTypeIdentifier(Emitter::Method(_, ObjectModuleId::Main), ..),
                ref event_data,
            )) if test_runner
                .is_event_name_equal::<MintFungibleResourceEvent>(event_identifier)
                && is_decoded_equal(
                    &MintFungibleResourceEvent { amount: 10.into() },
                    event_data
                ) =>
                true,
            _ => false,
        });
        assert!(match events.get(2) {
            Some((
                event_identifier
                @ EventTypeIdentifier(Emitter::Method(_, ObjectModuleId::Main), ..),
                ref event_data,
            )) if test_runner
                .is_event_name_equal::<BurnFungibleResourceEvent>(event_identifier)
                && is_decoded_equal(
                    &BurnFungibleResourceEvent { amount: 10.into() },
                    event_data
                ) =>
                true,
            _ => false,
        });
    }
}

#[test]
fn resource_manager_mint_and_burn_non_fungible_resource_emits_correct_events() {
    // Arrange
    let mut test_runner = TestRunnerBuilder::new().without_trace().build();
    let (_, _, account) = test_runner.new_account(false);
    let resource_address = {
        let manifest = ManifestBuilder::new()
            .lock_fee(FAUCET, 500)
            .create_non_fungible_resource(
                OwnerRole::None,
                NonFungibleIdType::Integer,
                false,
                NonFungibleResourceRoles {
                    mint_roles: mint_roles! {
                        minter => rule!(allow_all);
                        minter_updater => rule!(deny_all);
                    },
                    burn_roles: burn_roles! {
                        burner => rule!(allow_all);
                        burner_updater => rule!(deny_all);
                    },
                    ..Default::default()
                },
                metadata!(),
                None::<BTreeMap<NonFungibleLocalId, EmptyStruct>>,
            )
            .try_deposit_batch_or_abort(account, None)
            .build();
        let receipt = test_runner.execute_manifest(manifest, vec![]);
        receipt.expect_commit(true).new_resource_addresses()[0]
    };

    let id = NonFungibleLocalId::Integer(IntegerNonFungibleLocalId::new(1));
    let manifest = ManifestBuilder::new()
        .lock_fee(FAUCET, 500)
        .mint_non_fungible(resource_address, [(id.clone(), EmptyStruct {})])
        .burn_from_worktop(1, resource_address)
        .build();

    // Act
    let receipt = test_runner.execute_manifest(manifest, vec![]);

    // Assert
    {
        let events = receipt.expect_commit(true).clone().application_events;
        for event in &events {
            let name = test_runner.event_name(&event.0);
            println!("{:?} - {}", event.0, name);
        }
        assert_eq!(events.len(), 4); // Four events: vault lock fee, resource manager mint non-fungible, resource manager burn non-fungible, burn fee
        assert!(match events.get(0) {
            Some((
                event_identifier
                @ EventTypeIdentifier(Emitter::Method(_, ObjectModuleId::Main), ..),
                ref event_data,
            )) if test_runner
                .is_event_name_equal::<fungible_vault::LockFeeEvent>(event_identifier)
                && is_decoded_equal(
                    &fungible_vault::LockFeeEvent { amount: 500.into() },
                    event_data
                ) =>
                true,
            _ => false,
        });
        assert!(match events.get(1) {
            Some((
                event_identifier
                @ EventTypeIdentifier(Emitter::Method(_, ObjectModuleId::Main), ..),
                ref event_data,
            )) if test_runner
                .is_event_name_equal::<MintNonFungibleResourceEvent>(event_identifier)
                && is_decoded_equal(
                    &MintNonFungibleResourceEvent {
                        ids: [id.clone()].into()
                    },
                    event_data
                ) =>
                true,
            _ => false,
        });
        assert!(match events.get(2) {
            Some((
                event_identifier
                @ EventTypeIdentifier(Emitter::Method(_, ObjectModuleId::Main), ..),
                ref event_data,
            )) if test_runner
                .is_event_name_equal::<BurnNonFungibleResourceEvent>(event_identifier)
                && is_decoded_equal(
                    &BurnNonFungibleResourceEvent {
                        ids: [id.clone()].into()
                    },
                    event_data
                ) =>
                true,
            _ => false,
        });
    }
}

#[test]
fn vault_take_non_fungibles_by_amount_emits_correct_event() {
    // Arrange
    let mut test_runner = TestRunnerBuilder::new().without_trace().build();
    let (public_key, _, account) = test_runner.new_account(false);
    let resource_address = {
        let manifest = ManifestBuilder::new()
            .lock_fee(test_runner.faucet_component(), dec!("100"))
            .create_non_fungible_resource(
                OwnerRole::None,
                NonFungibleIdType::Integer,
                true,
                NonFungibleResourceRoles {
                    mint_roles: Some(MintRoles {
                        minter: Some(rule!(allow_all)),
                        minter_updater: None,
                    }),
                    ..Default::default()
                },
                Default::default(),
                None::<BTreeMap<NonFungibleLocalId, EmptyStruct>>,
            )
            .call_method(
                account,
                ACCOUNT_TRY_DEPOSIT_BATCH_OR_ABORT_IDENT,
                manifest_args!(
                    ManifestExpression::EntireWorktop,
                    Option::<ResourceOrNonFungible>::None
                ),
            )
            .build();
        let receipt = test_runner.execute_manifest(manifest, vec![]);
        receipt.expect_commit(true).new_resource_addresses()[0]
    };

    let id = NonFungibleLocalId::Integer(IntegerNonFungibleLocalId::new(1));
    let id2 = NonFungibleLocalId::Integer(IntegerNonFungibleLocalId::new(2));
    let manifest = ManifestBuilder::new()
        .lock_fee(test_runner.faucet_component(), dec!("10"))
        .mint_non_fungible(
            resource_address,
            [(id.clone(), EmptyStruct {}), (id2.clone(), EmptyStruct {})],
        )
        .try_deposit_batch_or_abort(account, None)
        .withdraw_from_account(account, resource_address, dec!("2"))
        .try_deposit_batch_or_abort(account, None)
        .build();

    // Act
    let receipt = test_runner.execute_manifest(
        manifest,
        vec![NonFungibleGlobalId::from_public_key(&public_key)],
    );

    // Assert
    {
        let events = receipt.expect_commit(true).clone().application_events;
        for event in &events {
            let name = test_runner.event_name(&event.0);
            println!("{:?} - {}", event.0, name);
        }
        assert_eq!(events.len(), 7);
        assert!(match events.get(0) {
            Some((
                event_identifier
                @ EventTypeIdentifier(Emitter::Method(_, ObjectModuleId::Main), ..),
                ref event_data,
            )) if test_runner
                .is_event_name_equal::<fungible_vault::LockFeeEvent>(event_identifier)
                && is_decoded_equal(
                    &fungible_vault::LockFeeEvent { amount: 10.into() },
                    event_data
                ) =>
                true,
            _ => false,
        });
        assert!(match events.get(1) {
            Some((
                event_identifier
                @ EventTypeIdentifier(Emitter::Method(_, ObjectModuleId::Main), ..),
                ref event_data,
            )) if test_runner
                .is_event_name_equal::<MintNonFungibleResourceEvent>(event_identifier)
                && is_decoded_equal(
                    &MintNonFungibleResourceEvent {
                        ids: [id.clone(), id2.clone()].into()
                    },
                    event_data
                ) =>
                true,
            _ => false,
        });
        assert!(match events.get(2) {
            Some((
                event_identifier
                @ EventTypeIdentifier(Emitter::Method(_, ObjectModuleId::Main), ..),
                _,
            )) if test_runner.is_event_name_equal::<VaultCreationEvent>(event_identifier) => true,
            _ => false,
        });
        assert!(match events.get(3) {
            Some((
                event_identifier
                @ EventTypeIdentifier(Emitter::Method(_, ObjectModuleId::Main), ..),
                ref event_data,
            )) if test_runner
                .is_event_name_equal::<non_fungible_vault::DepositEvent>(event_identifier)
                && is_decoded_equal(
                    &non_fungible_vault::DepositEvent::new([id.clone(), id2.clone()].into()),
                    event_data
                ) =>
                true,
            _ => false,
        });
        assert!(match events.get(4) {
            Some((
                event_identifier
                @ EventTypeIdentifier(Emitter::Method(_, ObjectModuleId::Main), ..),
                ref event_data,
            )) if test_runner
                .is_event_name_equal::<non_fungible_vault::WithdrawEvent>(event_identifier)
                && is_decoded_equal(
                    &non_fungible_vault::WithdrawEvent::new([id.clone(), id2.clone()].into()),
                    event_data
                ) =>
                true,
            _ => false,
        });
        assert!(match events.get(5) {
            Some((
                event_identifier
                @ EventTypeIdentifier(Emitter::Method(_, ObjectModuleId::Main), ..),
                ref event_data,
            )) if test_runner
                .is_event_name_equal::<non_fungible_vault::DepositEvent>(event_identifier)
                && is_decoded_equal(
                    &non_fungible_vault::DepositEvent::new([id.clone(), id2.clone()].into()),
                    event_data
                ) =>
                true,
            _ => false,
        });
    }
}

//===============
// Consensus Manager
//===============

#[test]
fn consensus_manager_round_update_emits_correct_event() {
    let genesis = CustomGenesis::default(
        Epoch::of(1),
        CustomGenesis::default_consensus_manager_config().with_epoch_change_condition(
            EpochChangeCondition {
                min_round_count: 100, // we do not want the "epoch change" event here
                max_round_count: 100,
                target_duration_millis: 1000,
            },
        ),
    );
    let mut test_runner = TestRunnerBuilder::new()
        .with_custom_genesis(genesis)
        .build();

    // Act
    let receipt = test_runner.execute_validator_transaction(vec![InstructionV1::CallMethod {
        address: CONSENSUS_MANAGER.into(),
        method_name: CONSENSUS_MANAGER_NEXT_ROUND_IDENT.to_string(),
        args: to_manifest_value_and_unwrap!(&ConsensusManagerNextRoundInput::successful(
            Round::of(1),
            0,
            180000i64,
        )),
    }]);

    // Assert
    {
        let events = receipt.expect_commit(true).clone().application_events;
        for event in &events {
            let name = test_runner.event_name(&event.0);
            println!("{:?} - {}", event.0, name);
        }
        assert_eq!(events.len(), 2); // Two events: round change event, burn fee
        assert!(match events.get(0) {
            Some((
                event_identifier
                @ EventTypeIdentifier(Emitter::Method(_, ObjectModuleId::Main), ..),
                ref event_data,
            )) if test_runner.is_event_name_equal::<RoundChangeEvent>(event_identifier)
                && is_decoded_equal(
                    &RoundChangeEvent {
                        round: Round::of(1)
                    },
                    event_data
                ) =>
                true,
            _ => false,
        });
    }
}

#[test]
fn consensus_manager_epoch_update_emits_epoch_change_event() {
    let genesis_epoch = Epoch::of(3);
    let initial_epoch = genesis_epoch.next();
    let rounds_per_epoch = 5;
    let genesis = CustomGenesis::default(
        genesis_epoch,
        CustomGenesis::default_consensus_manager_config().with_epoch_change_condition(
            EpochChangeCondition {
                min_round_count: rounds_per_epoch,
                max_round_count: rounds_per_epoch,
                target_duration_millis: 1000,
            },
        ),
    );
    let mut test_runner = TestRunnerBuilder::new()
        .with_custom_genesis(genesis)
        .build();

    // Prepare: skip a few rounds, right to the one just before epoch change
    test_runner.advance_to_round(Round::of(rounds_per_epoch - 1));

    // Act: perform the most usual successful next round
    let receipt = test_runner.execute_validator_transaction(vec![InstructionV1::CallMethod {
        address: CONSENSUS_MANAGER.into(),
        method_name: CONSENSUS_MANAGER_NEXT_ROUND_IDENT.to_string(),
        args: to_manifest_value_and_unwrap!(&ConsensusManagerNextRoundInput::successful(
            Round::of(rounds_per_epoch),
            0,
            180000i64,
        )),
    }]);

    // Assert
    {
        let events = receipt.expect_commit(true).clone().application_events;
        let epoch_change_events = events
            .into_iter()
            .filter(|(id, _data)| test_runner.is_event_name_equal::<EpochChangeEvent>(id))
            .map(|(_id, data)| scrypto_decode::<EpochChangeEvent>(&data).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(epoch_change_events.len(), 1);
        let event = epoch_change_events.first().unwrap();
        assert_eq!(event.epoch, initial_epoch.next());
    }
}

#[test]
fn consensus_manager_epoch_update_emits_xrd_minting_event() {
    // Arrange: some validator, and a degenerate 1-round epoch config, to advance it easily
    let emission_xrd = dec!("13.37");
    let validator_key = Secp256k1PrivateKey::from_u64(1u64).unwrap().public_key();
    let genesis = CustomGenesis::single_validator_and_staker(
        validator_key,
        Decimal::one(),
        ComponentAddress::virtual_account_from_public_key(&validator_key),
        Epoch::of(4),
        CustomGenesis::default_consensus_manager_config()
            .with_epoch_change_condition(EpochChangeCondition {
                min_round_count: 1,
                max_round_count: 1, // deliberate, to go through rounds/epoch without gaps
                target_duration_millis: 0,
            })
            .with_total_emission_xrd_per_epoch(emission_xrd),
    );
    let mut test_runner = TestRunnerBuilder::new()
        .with_custom_genesis(genesis)
        .build();

    // Act
    let receipt = test_runner.execute_validator_transaction(vec![InstructionV1::CallMethod {
        address: CONSENSUS_MANAGER.into(),
        method_name: CONSENSUS_MANAGER_NEXT_ROUND_IDENT.to_string(),
        args: to_manifest_value_and_unwrap!(&ConsensusManagerNextRoundInput::successful(
            Round::of(1),
            0,
            180000i64,
        )),
    }]);

    // Assert
    let result = receipt.expect_commit_success();
    assert_eq!(
        test_runner.extract_events_of_type::<MintFungibleResourceEvent>(result),
        vec![
            MintFungibleResourceEvent {
                amount: emission_xrd
            }, // we mint XRD (because of emission)
            MintFungibleResourceEvent {
                amount: emission_xrd
            } // we stake them all immediately because of validator fee = 100% (and thus mint stake units)
        ]
    );
}

//===========
// Validator
//===========

#[test]
fn validator_registration_emits_correct_event() {
    // Arrange
    let initial_epoch = Epoch::of(5);
    let pub_key = Secp256k1PrivateKey::from_u64(1u64).unwrap().public_key();
    let genesis = CustomGenesis::default(
        initial_epoch,
        CustomGenesis::default_consensus_manager_config(),
    );
    let mut test_runner = TestRunnerBuilder::new()
        .with_custom_genesis(genesis)
        .build();
    let (account_pk, _, account) = test_runner.new_account(false);

    // Act
    let validator_address = test_runner.new_validator_with_pub_key(pub_key, account);
    let manifest = ManifestBuilder::new()
        .lock_fee(FAUCET, 500)
        .create_proof_from_account_of_non_fungibles(
            account,
            VALIDATOR_OWNER_BADGE,
            &btreeset!(NonFungibleLocalId::bytes(validator_address.as_node_id().0).unwrap()),
        )
        .register_validator(validator_address)
        .build();
    let receipt = test_runner.execute_manifest(
        manifest,
        vec![NonFungibleGlobalId::from_public_key(&account_pk)],
    );

    // Assert
    {
        let events = receipt.expect_commit(true).clone().application_events;
        for event in &events {
            let name = test_runner.event_name(&event.0);
            println!("{:?} - {}", event.0, name);
        }
        assert_eq!(events.len(), 3);
        assert!(match events.get(0) {
            Some((
                event_identifier
                @ EventTypeIdentifier(Emitter::Method(_, ObjectModuleId::Main), ..),
                ref event_data,
            )) if test_runner
                .is_event_name_equal::<fungible_vault::LockFeeEvent>(event_identifier)
                && is_decoded_equal(
                    &fungible_vault::LockFeeEvent { amount: 500.into() },
                    event_data
                ) =>
                true,
            _ => false,
        });
        assert!(match events.get(1) {
            Some((
                event_identifier
                @ EventTypeIdentifier(Emitter::Method(_, ObjectModuleId::Main), ..),
                ..,
            )) if test_runner.is_event_name_equal::<RegisterValidatorEvent>(event_identifier) =>
                true,
            _ => false,
        });
    }
}

#[test]
fn validator_unregistration_emits_correct_event() {
    // Arrange
    let initial_epoch = Epoch::of(5);
    let pub_key = Secp256k1PrivateKey::from_u64(1u64).unwrap().public_key();
    let genesis = CustomGenesis::default(
        initial_epoch,
        CustomGenesis::default_consensus_manager_config(),
    );
    let mut test_runner = TestRunnerBuilder::new()
        .with_custom_genesis(genesis)
        .build();
    let (account_pk, _, account) = test_runner.new_account(false);

    let validator_address = test_runner.new_validator_with_pub_key(pub_key, account);
    let manifest = ManifestBuilder::new()
        .lock_fee(FAUCET, 500)
        .create_proof_from_account_of_non_fungibles(
            account,
            VALIDATOR_OWNER_BADGE,
            &btreeset!(NonFungibleLocalId::bytes(validator_address.as_node_id().0).unwrap()),
        )
        .register_validator(validator_address)
        .build();
    let receipt = test_runner.execute_manifest(
        manifest,
        vec![NonFungibleGlobalId::from_public_key(&account_pk)],
    );
    receipt.expect_commit_success();

    // Act
    let manifest = ManifestBuilder::new()
        .lock_fee(FAUCET, 500)
        .create_proof_from_account_of_non_fungibles(
            account,
            VALIDATOR_OWNER_BADGE,
            &btreeset!(NonFungibleLocalId::bytes(validator_address.as_node_id().0).unwrap()),
        )
        .unregister_validator(validator_address)
        .build();
    let receipt = test_runner.execute_manifest(
        manifest,
        vec![NonFungibleGlobalId::from_public_key(&account_pk)],
    );

    // Assert
    {
        let events = receipt.expect_commit(true).clone().application_events;
        for event in &events {
            let name = test_runner.event_name(&event.0);
            println!("{:?} - {}", event.0, name);
        }
        assert_eq!(events.len(), 3);
        assert!(match events.get(0) {
            Some((
                event_identifier
                @ EventTypeIdentifier(Emitter::Method(_, ObjectModuleId::Main), ..),
                ref event_data,
            )) if test_runner
                .is_event_name_equal::<fungible_vault::LockFeeEvent>(event_identifier)
                && is_decoded_equal(
                    &fungible_vault::LockFeeEvent { amount: 500.into() },
                    event_data
                ) =>
                true,
            _ => false,
        });
        assert!(match events.get(1) {
            Some((
                event_identifier
                @ EventTypeIdentifier(Emitter::Method(_, ObjectModuleId::Main), ..),
                ..,
            )) if test_runner.is_event_name_equal::<UnregisterValidatorEvent>(event_identifier) =>
                true,
            _ => false,
        });
    }
}

#[test]
fn validator_staking_emits_correct_event() {
    // Arrange
    let initial_epoch = Epoch::of(5);
    let pub_key = Secp256k1PrivateKey::from_u64(1u64).unwrap().public_key();
    let genesis = CustomGenesis::default(
        initial_epoch,
        CustomGenesis::default_consensus_manager_config(),
    );
    let mut test_runner = TestRunnerBuilder::new()
        .with_custom_genesis(genesis)
        .build();
    let (account_pk, _, account) = test_runner.new_account(false);

    let validator_address = test_runner.new_validator_with_pub_key(pub_key, account);
    let manifest = ManifestBuilder::new()
        .lock_fee(FAUCET, 500)
        .create_proof_from_account_of_non_fungibles(
            account,
            VALIDATOR_OWNER_BADGE,
            &btreeset!(NonFungibleLocalId::bytes(validator_address.as_node_id().0).unwrap()),
        )
        .register_validator(validator_address)
        .build();
    let receipt = test_runner.execute_manifest(
        manifest,
        vec![NonFungibleGlobalId::from_public_key(&account_pk)],
    );
    receipt.expect_commit_success();

    // Act
    let manifest = ManifestBuilder::new()
        .lock_fee(FAUCET, 500)
        .create_proof_from_account_of_non_fungibles(
            account,
            VALIDATOR_OWNER_BADGE,
            &btreeset!(NonFungibleLocalId::bytes(validator_address.as_node_id().0).unwrap()),
        )
        .withdraw_from_account(account, XRD, 100)
        .take_all_from_worktop(XRD, "stake")
        .stake_validator_as_owner(validator_address, "stake")
        .try_deposit_batch_or_abort(account, None)
        .build();
    let receipt = test_runner.execute_manifest(
        manifest,
        vec![NonFungibleGlobalId::from_public_key(&account_pk)],
    );

    // Assert
    {
        let events = receipt.expect_commit(true).clone().application_events;
        for event in &events {
            let name = test_runner.event_name(&event.0);
            println!("{:?} - {}", event.0, name);
        }
        assert_eq!(events.len(), 8);
        assert!(match events.get(0) {
            Some((
                event_identifier
                @ EventTypeIdentifier(Emitter::Method(_, ObjectModuleId::Main), ..),
                ref event_data,
            )) if test_runner
                .is_event_name_equal::<fungible_vault::LockFeeEvent>(event_identifier)
                && is_decoded_equal(
                    &fungible_vault::LockFeeEvent { amount: 500.into() },
                    event_data
                ) =>
                true,
            _ => false,
        });
        assert!(match events.get(1) {
            Some((
                event_identifier
                @ EventTypeIdentifier(Emitter::Method(_, ObjectModuleId::Main), ..),
                ref event_data,
            )) if test_runner
                .is_event_name_equal::<fungible_vault::WithdrawEvent>(event_identifier)
                && is_decoded_equal(
                    &fungible_vault::WithdrawEvent::new(100.into()),
                    event_data
                ) =>
                true,
            _ => false,
        });
        assert!(match events.get(2) {
            Some((
                event_identifier
                @ EventTypeIdentifier(Emitter::Method(_, ObjectModuleId::Main), ..),
                ..,
            )) if test_runner
                .is_event_name_equal::<MintFungibleResourceEvent>(event_identifier) =>
                true,
            _ => false,
        });
        assert!(match events.get(3) {
            Some((
                event_identifier
                @ EventTypeIdentifier(Emitter::Method(_, ObjectModuleId::Main), ..),
                ref event_data,
            )) if test_runner
                .is_event_name_equal::<fungible_vault::DepositEvent>(event_identifier)
                && is_decoded_equal(&fungible_vault::DepositEvent::new(100.into()), event_data) =>
                true,
            _ => false,
        });
        assert!(match events.get(4) {
            Some((
                event_identifier
                @ EventTypeIdentifier(Emitter::Method(_, ObjectModuleId::Main), ..),
                ref event_data,
            )) if test_runner.is_event_name_equal::<StakeEvent>(event_identifier)
                && is_decoded_equal(
                    &StakeEvent {
                        xrd_staked: 100.into()
                    },
                    event_data
                ) =>
                true,
            _ => false,
        });
        assert!(match events.get(5) {
            Some((
                event_identifier @ EventTypeIdentifier(
                    Emitter::Method(_node_id, ObjectModuleId::Main),
                    ..,
                ),
                ..,
            )) if test_runner.is_event_name_equal::<VaultCreationEvent>(event_identifier) => true,
            _ => false,
        });
        assert!(match events.get(6) {
            Some((
                event_identifier
                @ EventTypeIdentifier(Emitter::Method(_, ObjectModuleId::Main), ..),
                ..,
            )) if test_runner
                .is_event_name_equal::<fungible_vault::DepositEvent>(event_identifier) =>
                true,
            _ => false,
        });
    }
}

#[test]
fn validator_unstake_emits_correct_events() {
    // Arrange
    let initial_epoch = Epoch::of(5);
    let num_unstake_epochs = 1;
    let validator_pub_key = Secp256k1PrivateKey::from_u64(2u64).unwrap().public_key();
    let account_pub_key = Secp256k1PrivateKey::from_u64(1u64).unwrap().public_key();
    let account_with_su = ComponentAddress::virtual_account_from_public_key(&account_pub_key);
    let genesis = CustomGenesis::single_validator_and_staker(
        validator_pub_key,
        Decimal::from(10),
        account_with_su,
        initial_epoch,
        CustomGenesis::default_consensus_manager_config()
            .with_num_unstake_epochs(num_unstake_epochs),
    );
    let mut test_runner = TestRunnerBuilder::new()
        .with_custom_genesis(genesis)
        .build();
    let validator_address = test_runner.get_active_validator_with_key(&validator_pub_key);
    let validator_substate = test_runner.get_validator_info(validator_address);

    // Act
    let manifest = ManifestBuilder::new()
        .lock_fee(FAUCET, 500)
        .withdraw_from_account(account_with_su, validator_substate.stake_unit_resource, 1)
        .take_all_from_worktop(validator_substate.stake_unit_resource, "stake_units")
        .unstake_validator(validator_address, "stake_units")
        .try_deposit_batch_or_abort(account_with_su, None)
        .build();
    let receipt = test_runner.execute_manifest(
        manifest,
        vec![NonFungibleGlobalId::from_public_key(&account_pub_key)],
    );
    receipt.expect_commit_success();
    test_runner.set_current_epoch(initial_epoch.after(1 + num_unstake_epochs));

    // Assert
    {
        let events = receipt.expect_commit(true).clone().application_events;
        for event in &events {
            let name = test_runner.event_name(&event.0);
            println!("{:?} - {}", event.0, name);
        }
        assert_eq!(events.len(), 10);
        assert!(match events.get(0) {
            Some((
                event_identifier
                @ EventTypeIdentifier(Emitter::Method(_, ObjectModuleId::Main), ..),
                ref event_data,
            )) if test_runner
                .is_event_name_equal::<fungible_vault::LockFeeEvent>(event_identifier)
                && is_decoded_equal(
                    &fungible_vault::LockFeeEvent { amount: 500.into() },
                    event_data
                ) =>
                true,
            _ => false,
        });
        assert!(match events.get(1) {
            Some((
                event_identifier
                @ EventTypeIdentifier(Emitter::Method(_, ObjectModuleId::Main), ..),
                ref event_data,
            )) if test_runner
                .is_event_name_equal::<fungible_vault::WithdrawEvent>(event_identifier)
                && is_decoded_equal(&fungible_vault::WithdrawEvent::new(1.into()), event_data) =>
                true,
            _ => false,
        });
        assert!(match events.get(2) {
            Some((
                event_identifier
                @ EventTypeIdentifier(Emitter::Method(_, ObjectModuleId::Main), ..),
                ref event_data,
            )) if test_runner
                .is_event_name_equal::<BurnFungibleResourceEvent>(event_identifier)
                && is_decoded_equal(
                    &BurnFungibleResourceEvent { amount: 1.into() },
                    event_data
                ) =>
                true,
            _ => false,
        });
        assert!(match events.get(3) {
            Some((
                event_identifier
                @ EventTypeIdentifier(Emitter::Method(_, ObjectModuleId::Main), ..),
                ..,
            )) if test_runner
                .is_event_name_equal::<fungible_vault::WithdrawEvent>(event_identifier) =>
                true,
            _ => false,
        });
        assert!(match events.get(4) {
            Some((
                event_identifier
                @ EventTypeIdentifier(Emitter::Method(_, ObjectModuleId::Main), ..),
                ..,
            )) if test_runner
                .is_event_name_equal::<fungible_vault::DepositEvent>(event_identifier) =>
                true,
            _ => false,
        });
        assert!(match events.get(5) {
            Some((
                event_identifier @ EventTypeIdentifier(
                    Emitter::Method(node_id, ObjectModuleId::Main),
                    ..,
                ),
                ..,
            )) if test_runner
                .is_event_name_equal::<MintNonFungibleResourceEvent>(event_identifier)
                && node_id == validator_substate.claim_nft.as_node_id() =>
                true,
            _ => false,
        });
        assert!(match events.get(6) {
            Some((
                event_identifier
                @ EventTypeIdentifier(Emitter::Method(_, ObjectModuleId::Main), ..),
                ..,
            )) if test_runner.is_event_name_equal::<UnstakeEvent>(event_identifier) => true,
            _ => false,
        });
        assert!(match events.get(7) {
            Some((
                event_identifier @ EventTypeIdentifier(
                    Emitter::Method(_node_id, ObjectModuleId::Main),
                    ..,
                ),
                ..,
            )) if test_runner.is_event_name_equal::<VaultCreationEvent>(event_identifier) => true,
            _ => false,
        });
        assert!(match events.get(8) {
            Some((
                event_identifier
                @ EventTypeIdentifier(Emitter::Method(_, ObjectModuleId::Main), ..),
                ..,
            )) if test_runner
                .is_event_name_equal::<non_fungible_vault::DepositEvent>(event_identifier) =>
                true,
            _ => false,
        });
    }
}

#[test]
fn validator_claim_xrd_emits_correct_events() {
    // Arrange
    let initial_epoch = Epoch::of(5);
    let num_unstake_epochs = 1;
    let validator_pub_key = Secp256k1PrivateKey::from_u64(2u64).unwrap().public_key();
    let account_pub_key = Secp256k1PrivateKey::from_u64(1u64).unwrap().public_key();
    let account_with_su = ComponentAddress::virtual_account_from_public_key(&account_pub_key);
    let genesis = CustomGenesis::single_validator_and_staker(
        validator_pub_key,
        Decimal::from(10),
        account_with_su,
        initial_epoch,
        CustomGenesis::default_consensus_manager_config()
            .with_num_unstake_epochs(num_unstake_epochs),
    );
    let mut test_runner = TestRunnerBuilder::new()
        .with_custom_genesis(genesis)
        .build();
    let validator_address = test_runner.get_active_validator_with_key(&validator_pub_key);
    let validator_substate = test_runner.get_validator_info(validator_address);
    let manifest = ManifestBuilder::new()
        .lock_fee(FAUCET, 500)
        .withdraw_from_account(account_with_su, validator_substate.stake_unit_resource, 1)
        .take_all_from_worktop(validator_substate.stake_unit_resource, "stake_units")
        .unstake_validator(validator_address, "stake_units")
        .try_deposit_batch_or_abort(account_with_su, None)
        .build();
    let receipt = test_runner.execute_manifest(
        manifest,
        vec![NonFungibleGlobalId::from_public_key(&account_pub_key)],
    );
    receipt.expect_commit_success();
    test_runner.set_current_epoch(initial_epoch.after(1 + num_unstake_epochs));

    // Act
    let manifest = ManifestBuilder::new()
        .lock_fee(FAUCET, 500)
        .withdraw_from_account(account_with_su, validator_substate.claim_nft, 1)
        .take_all_from_worktop(validator_substate.claim_nft, "unstake_nft")
        .claim_xrd(validator_address, "unstake_nft")
        .try_deposit_batch_or_abort(account_with_su, None)
        .build();
    let receipt = test_runner.execute_manifest(
        manifest,
        vec![NonFungibleGlobalId::from_public_key(&account_pub_key)],
    );

    // Assert
    {
        let events = receipt.expect_commit(true).clone().application_events;
        for event in &events {
            let name = test_runner.event_name(&event.0);
            println!("{:?} - {}", event.0, name);
        }
        assert_eq!(events.len(), 8);
        assert!(match events.get(0) {
            Some((
                event_identifier
                @ EventTypeIdentifier(Emitter::Method(_, ObjectModuleId::Main), ..),
                ref event_data,
            )) if test_runner
                .is_event_name_equal::<fungible_vault::LockFeeEvent>(event_identifier)
                && is_decoded_equal(
                    &fungible_vault::LockFeeEvent { amount: 500.into() },
                    event_data
                ) =>
                true,
            _ => false,
        });
        assert!(match events.get(1) {
            Some((
                event_identifier
                @ EventTypeIdentifier(Emitter::Method(_, ObjectModuleId::Main), ..),
                ..,
            )) if test_runner
                .is_event_name_equal::<non_fungible_vault::WithdrawEvent>(event_identifier) =>
                true,
            _ => false,
        });
        assert!(match events.get(2) {
            Some((
                event_identifier
                @ EventTypeIdentifier(Emitter::Method(_, ObjectModuleId::Main), ..),
                ..,
            )) if test_runner
                .is_event_name_equal::<BurnNonFungibleResourceEvent>(event_identifier) =>
                true,
            _ => false,
        });
        assert!(match events.get(3) {
            Some((
                event_identifier
                @ EventTypeIdentifier(Emitter::Method(_, ObjectModuleId::Main), ..),
                ..,
            )) if test_runner
                .is_event_name_equal::<fungible_vault::WithdrawEvent>(event_identifier) =>
                true,
            _ => false,
        });
        assert!(match events.get(4) {
            Some((
                event_identifier
                @ EventTypeIdentifier(Emitter::Method(_, ObjectModuleId::Main), ..),
                ..,
            )) if test_runner.is_event_name_equal::<ClaimXrdEvent>(event_identifier) => true,
            _ => false,
        });
        assert!(match events.get(5) {
            Some((
                event_identifier @ EventTypeIdentifier(
                    Emitter::Method(_node_id, ObjectModuleId::Main),
                    ..,
                ),
                ..,
            )) if test_runner.is_event_name_equal::<VaultCreationEvent>(event_identifier) => true,
            _ => false,
        });
        assert!(match events.get(6) {
            Some((
                event_identifier
                @ EventTypeIdentifier(Emitter::Method(_, ObjectModuleId::Main), ..),
                ..,
            )) if test_runner
                .is_event_name_equal::<fungible_vault::DepositEvent>(event_identifier) =>
                true,
            _ => false,
        });
    }
}

#[test]
fn validator_update_stake_delegation_status_emits_correct_event() {
    // Arrange
    let initial_epoch = Epoch::of(5);
    let genesis = CustomGenesis::default(
        initial_epoch,
        CustomGenesis::default_consensus_manager_config(),
    );
    let mut test_runner = TestRunnerBuilder::new()
        .with_custom_genesis(genesis)
        .build();
    let (pub_key, _, account) = test_runner.new_account(false);

    let validator_address = test_runner.new_validator_with_pub_key(pub_key, account);
    let manifest = ManifestBuilder::new()
        .lock_fee(FAUCET, 500)
        .create_proof_from_account_of_non_fungibles(
            account,
            VALIDATOR_OWNER_BADGE,
            &btreeset!(NonFungibleLocalId::bytes(validator_address.as_node_id().0).unwrap()),
        )
        .register_validator(validator_address)
        .build();
    let receipt = test_runner.execute_manifest(
        manifest,
        vec![NonFungibleGlobalId::from_public_key(&pub_key)],
    );
    receipt.expect_commit_success();

    // Act
    let manifest = ManifestBuilder::new()
        .lock_fee(FAUCET, 500)
        .create_proof_from_account_of_non_fungibles(
            account,
            VALIDATOR_OWNER_BADGE,
            &btreeset!(NonFungibleLocalId::bytes(validator_address.as_node_id().0).unwrap()),
        )
        .call_method(
            validator_address,
            VALIDATOR_UPDATE_ACCEPT_DELEGATED_STAKE_IDENT,
            ValidatorUpdateAcceptDelegatedStakeInput {
                accept_delegated_stake: false,
            },
        )
        .build();
    let receipt = test_runner.execute_manifest(
        manifest,
        vec![NonFungibleGlobalId::from_public_key(&pub_key)],
    );

    // Assert
    {
        let events = receipt.expect_commit(true).clone().application_events;
        for event in &events {
            let name = test_runner.event_name(&event.0);
            println!("{:?} - {}", event.0, name);
        }
        assert_eq!(events.len(), 3);
        assert!(match events.get(0) {
            Some((
                event_identifier
                @ EventTypeIdentifier(Emitter::Method(_, ObjectModuleId::Main), ..),
                ref event_data,
            )) if test_runner
                .is_event_name_equal::<fungible_vault::LockFeeEvent>(event_identifier)
                && is_decoded_equal(
                    &fungible_vault::LockFeeEvent { amount: 500.into() },
                    event_data
                ) =>
                true,
            _ => false,
        });
        assert!(match events.get(1) {
            Some((
                event_identifier
                @ EventTypeIdentifier(Emitter::Method(_, ObjectModuleId::Main), ..),
                ref event_data,
            )) if test_runner.is_event_name_equal::<UpdateAcceptingStakeDelegationStateEvent>(
                event_identifier
            ) && is_decoded_equal(
                &UpdateAcceptingStakeDelegationStateEvent {
                    accepts_delegation: false
                },
                event_data
            ) =>
                true,
            _ => false,
        });
    }
}

//==========
// Metadata
//==========

#[test]
fn setting_metadata_emits_correct_events() {
    // Arrange
    let mut test_runner = TestRunnerBuilder::new().without_trace().build();
    let resource_address = create_all_allowed_resource(&mut test_runner);

    let manifest = ManifestBuilder::new()
        .lock_fee(FAUCET, 500)
        .set_metadata(resource_address, "key", MetadataValue::I32(1))
        .build();

    // Act
    let receipt = test_runner.execute_manifest(manifest, vec![]);

    // Assert
    {
        let events = receipt.expect_commit(true).clone().application_events;
        for event in &events {
            let name = test_runner.event_name(&event.0);
            println!("{:?} - {}", event.0, name);
        }
        assert_eq!(events.len(), 3);
        assert!(match events.get(0) {
            Some((
                event_identifier
                @ EventTypeIdentifier(Emitter::Method(_, ObjectModuleId::Main), ..),
                ref event_data,
            )) if test_runner
                .is_event_name_equal::<fungible_vault::LockFeeEvent>(event_identifier)
                && is_decoded_equal(
                    &fungible_vault::LockFeeEvent { amount: 500.into() },
                    event_data
                ) =>
                true,
            _ => false,
        });
        assert!(match events.get(1) {
            Some((
                event_identifier @ EventTypeIdentifier(
                    Emitter::Method(_, ObjectModuleId::Metadata),
                    ..,
                ),
                ..,
            )) if test_runner.is_event_name_equal::<SetMetadataEvent>(event_identifier) => true,
            _ => false,
        });
    }
}

//=========
// Account
//=========

#[test]
fn create_account_events_can_be_looked_up() {
    // Arrange
    let mut test_runner = TestRunnerBuilder::new().without_trace().build();

    // Act
    let manifest = ManifestBuilder::new()
        .new_account_advanced(OwnerRole::Fixed(AccessRule::AllowAll))
        .build();
    let receipt = test_runner.execute_manifest_ignoring_fee(manifest, vec![]);

    // Assert
    {
        let events = receipt.expect_commit(true).clone().application_events;
        for (event_id, _) in events {
            let _name = test_runner.event_name(&event_id);
        }
    }
}

//=========
// Helpers
//=========

#[derive(ScryptoSbor, NonFungibleData, ManifestSbor)]
struct EmptyStruct {}

#[derive(ScryptoSbor, PartialEq, Eq, PartialOrd, Ord)]
struct RegisteredEvent {
    number: u64,
}

fn is_decoded_equal<T: ScryptoDecode + PartialEq>(expected: &T, actual: &[u8]) -> bool {
    scrypto_decode::<T>(&actual).unwrap() == *expected
}

fn create_all_allowed_resource(test_runner: &mut DefaultTestRunner) -> ResourceAddress {
    let manifest = ManifestBuilder::new()
        .create_fungible_resource(
            OwnerRole::Fixed(AccessRule::AllowAll),
            false,
            18,
            FungibleResourceRoles {
                mint_roles: mint_roles! {
                    minter => rule!(allow_all);
                    minter_updater => rule!(deny_all);
                },
                burn_roles: burn_roles! {
                    burner => rule!(allow_all);
                    burner_updater => rule!(deny_all);
                },
                recall_roles: recall_roles! {
                    recaller => rule!(allow_all);
                    recaller_updater => rule!(deny_all);
                },
                ..Default::default()
            },
            metadata!(),
            None,
        )
        .build();
    let receipt = test_runner.execute_manifest_ignoring_fee(manifest, vec![]);
    receipt.expect_commit(true).new_resource_addresses()[0]
}

#[test]
fn mint_burn_events_should_match_total_supply_for_fungible_resource() {
    let mut test_runner = TestRunnerBuilder::new()
        .without_trace()
        .collect_events()
        .build();
    let (pk, _, account) = test_runner.new_allocated_account();

    // Create
    let resource_address = test_runner.create_freely_mintable_and_burnable_fungible_resource(
        OwnerRole::None,
        Some(dec!(100)),
        18,
        account,
    );

    // Mint
    let manifest = ManifestBuilder::new()
        .lock_fee_from_faucet()
        .mint_fungible(resource_address, dec!(30))
        .deposit_batch(account)
        .build();
    test_runner
        .execute_manifest(manifest, vec![NonFungibleGlobalId::from_public_key(&pk)])
        .expect_commit_success();

    // Burn
    let manifest = ManifestBuilder::new()
        .lock_fee_from_faucet()
        .withdraw_from_account(account, resource_address, dec!(10))
        .burn_all_from_worktop(resource_address)
        .build();
    test_runner
        .execute_manifest(manifest, vec![NonFungibleGlobalId::from_public_key(&pk)])
        .expect_commit_success();

    // Assert
    let mut total_supply = Decimal::ZERO;
    let mut total_mint_amount = Decimal::ZERO;
    let mut total_burn_amount = Decimal::ZERO;
    for component in test_runner.find_all_components() {
        let balance = test_runner.get_component_balance(component, resource_address);
        total_supply += balance;
        println!("{:?}, {}", component, balance);
    }
    for tx_events in test_runner.collected_events() {
        for event in tx_events {
            match &event.0 .0 {
                Emitter::Method(x, _) if x.eq(resource_address.as_node_id()) => {}
                _ => {
                    continue;
                }
            }
            let actual_type_name = test_runner.event_name(&event.0);
            match actual_type_name.as_str() {
                "MintFungibleResourceEvent" => {
                    total_mint_amount += scrypto_decode::<MintFungibleResourceEvent>(&event.1)
                        .unwrap()
                        .amount;
                }
                "BurnFungibleResourceEvent" => {
                    total_burn_amount += scrypto_decode::<BurnFungibleResourceEvent>(&event.1)
                        .unwrap()
                        .amount;
                }
                _ => {}
            }
        }
    }
    println!("Total supply: {}", total_supply);
    println!("Total mint amount: {}", total_mint_amount);
    println!("Total burn amount: {}", total_burn_amount);
    assert_eq!(total_supply, total_mint_amount - total_burn_amount);

    // Query total supply from the resource manager
    let receipt = test_runner.execute_manifest(
        ManifestBuilder::new()
            .lock_fee_from_faucet()
            .call_method(resource_address, "get_total_supply", manifest_args!())
            .build(),
        vec![],
    );
    assert_eq!(
        Some(total_supply),
        receipt.expect_commit_success().output::<Option<Decimal>>(1)
    );
}

#[test]
fn mint_burn_events_should_match_total_supply_for_non_fungible_resource() {
    let mut test_runner = TestRunnerBuilder::new()
        .without_trace()
        .collect_events()
        .build();
    let (pk, _, account) = test_runner.new_allocated_account();

    // Create
    let resource_address = test_runner.create_freely_mintable_and_burnable_non_fungible_resource(
        OwnerRole::None,
        NonFungibleIdType::Integer,
        Some(vec![
            (NonFungibleLocalId::integer(1), EmptyNonFungibleData {}),
            (NonFungibleLocalId::integer(2), EmptyNonFungibleData {}),
            (NonFungibleLocalId::integer(3), EmptyNonFungibleData {}),
        ]),
        account,
    );

    // Mint
    let manifest = ManifestBuilder::new()
        .lock_fee_from_faucet()
        .mint_non_fungible(
            resource_address,
            vec![
                (NonFungibleLocalId::integer(4), EmptyNonFungibleData {}),
                (NonFungibleLocalId::integer(5), EmptyNonFungibleData {}),
            ],
        )
        .deposit_batch(account)
        .build();
    test_runner
        .execute_manifest(manifest, vec![NonFungibleGlobalId::from_public_key(&pk)])
        .expect_commit_success();

    // Burn
    let manifest = ManifestBuilder::new()
        .lock_fee_from_faucet()
        .withdraw_non_fungibles_from_account(
            account,
            resource_address,
            &btreeset!(NonFungibleLocalId::integer(4)),
        )
        .burn_all_from_worktop(resource_address)
        .build();
    test_runner
        .execute_manifest(manifest, vec![NonFungibleGlobalId::from_public_key(&pk)])
        .expect_commit_success();

    // Assert
    let mut total_supply = Decimal::ZERO;
    let mut total_mint_non_fungibles = BTreeSet::new();
    let mut total_burn_non_fungibles = BTreeSet::new();
    for component in test_runner.find_all_components() {
        let balance = test_runner.get_component_balance(component, resource_address);
        total_supply += balance;
        println!("{:?}, {}", component, balance);
    }
    for tx_events in test_runner.collected_events() {
        for event in tx_events {
            match &event.0 .0 {
                Emitter::Method(x, _) if x.eq(resource_address.as_node_id()) => {}
                _ => {
                    continue;
                }
            }
            let actual_type_name = test_runner.event_name(&event.0);
            match actual_type_name.as_str() {
                "MintNonFungibleResourceEvent" => {
                    total_mint_non_fungibles.extend(
                        scrypto_decode::<MintNonFungibleResourceEvent>(&event.1)
                            .unwrap()
                            .ids,
                    );
                }
                "BurnNonFungibleResourceEvent" => {
                    total_burn_non_fungibles.extend(
                        scrypto_decode::<BurnNonFungibleResourceEvent>(&event.1)
                            .unwrap()
                            .ids,
                    );
                }
                _ => {}
            }
        }
    }
    println!("Total supply: {}", total_supply);
    println!("Total mint: {:?}", total_mint_non_fungibles);
    println!("Total burn: {:?}", total_burn_non_fungibles);
    total_mint_non_fungibles.retain(|x| !total_burn_non_fungibles.contains(x));
    assert_eq!(total_supply, total_mint_non_fungibles.len().into());

    // Query total supply from the resource manager
    let receipt = test_runner.execute_manifest(
        ManifestBuilder::new()
            .lock_fee_from_faucet()
            .call_method(resource_address, "get_total_supply", manifest_args!())
            .build(),
        vec![],
    );
    assert_eq!(
        Some(total_supply),
        receipt.expect_commit_success().output::<Option<Decimal>>(1)
    );
}

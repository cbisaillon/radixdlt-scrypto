use radix_engine::types::*;
use radix_engine_interface::blueprints::account::ACCOUNT_DEPOSIT_BATCH_IDENT;
use scrypto_unit::*;
use transaction::builder::ManifestBuilder;
use transaction::model::InstructionV1;

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

const PACKAGE_ADDRESS_PLACE_HOLDER: [u8; NodeId::LENGTH] = [
    13, 144, 99, 24, 198, 49, 140, 100, 247, 152, 202, 204, 99, 24, 198, 49, 140, 247, 189, 241,
    172, 105, 67, 234, 38, 49, 140, 99, 24, 198,
];

#[test]
fn test_static_package_address() {
    // Arrange
    let mut test_runner = TestRunner::builder().build();
    let package_address1 =
        test_runner.compile_and_publish("./tests/blueprints/static_dependencies");

    let (mut code, mut definition) = Compile::compile("./tests/blueprints/static_dependencies");
    let place_holder: GlobalAddress =
        PackageAddress::new_or_panic(PACKAGE_ADDRESS_PLACE_HOLDER).into();
    for (_, blueprint) in &mut definition.schema.blueprints {
        if blueprint.dependencies.contains(&place_holder) {
            blueprint.dependencies.remove(&place_holder);
            blueprint.dependencies.insert(package_address1.into());
        }
    }

    let start = find_subsequence(&code, &PACKAGE_ADDRESS_PLACE_HOLDER).unwrap();
    code[start..start + PACKAGE_ADDRESS_PLACE_HOLDER.len()]
        .copy_from_slice(package_address1.as_ref());
    let package_address2 = test_runner.publish_package(
        code,
        definition,
        BTreeMap::new(),
        BTreeMap::new(),
        OwnerRole::None,
    );

    let manifest = ManifestBuilder::new()
        .lock_fee(test_runner.faucet_component(), 10.into())
        .call_function(
            package_address2,
            "Sample",
            "call_external_package",
            manifest_args!(),
        )
        .build();
    let receipt = test_runner.execute_manifest(manifest, vec![]);

    // Assert
    receipt.expect_commit_success();
}

#[test]
fn test_static_component_address() {
    // Arrange
    let mut test_runner = TestRunner::builder().build();
    let package_address = test_runner.compile_and_publish("./tests/blueprints/static_dependencies");
    let (key, _priv, account) = test_runner.new_account(false);

    // Act
    let manifest = ManifestBuilder::new()
        .lock_fee(account, 10.into())
        .call_function(
            package_address,
            "FaucetCall",
            "call_faucet_lock_fee",
            manifest_args!(),
        )
        .build();
    let receipt =
        test_runner.execute_manifest(manifest, vec![NonFungibleGlobalId::from_public_key(&key)]);

    // Assert
    receipt.expect_commit_success();
}

const PRE_ALLOCATED: [u8; NodeId::LENGTH] = [
    192, 0, 0, 0, 0, 1, 1, 1, 1, 0, 0, 0, 0, 1, 1, 1, 1, 0, 0, 0, 1, 1, 1, 1, 0, 0, 0, 0, 1, 1,
];

#[test]
fn static_component_should_be_callable() {
    // Arrange
    let mut test_runner = TestRunner::builder().build();
    let package_address = test_runner.compile_and_publish("./tests/blueprints/static_dependencies");
    let receipt = test_runner.execute_system_transaction_with_preallocated_ids(
        vec![InstructionV1::CallFunction {
            package_address,
            blueprint_name: "Preallocated".to_string(),
            function_name: "new".to_string(),
            args: manifest_args!(PRE_ALLOCATED, "my_secret".to_string()),
        }],
        indexset!(NodeId::from(PRE_ALLOCATED)),
        btreeset!(),
    );
    receipt.expect_commit_success();

    // Act
    let package_address2 = test_runner.compile_and_publish_retain_blueprints(
        "./tests/blueprints/static_dependencies2",
        |blueprint, _| blueprint.eq("PreallocatedCall"),
    );
    let manifest = ManifestBuilder::new()
        .lock_fee(test_runner.faucet_component(), 10.into())
        .call_function(
            package_address2,
            "PreallocatedCall",
            "call_preallocated",
            manifest_args!(),
        )
        .build();
    let receipt = test_runner.execute_manifest(manifest, vec![]);

    // Assert
    let result = receipt.expect_commit_success();
    let output = result.outcome.expect_success();
    output[1].expect_return_value(&"my_secret".to_string());
}

const PRE_ALLOCATED_RESOURCE: [u8; NodeId::LENGTH] = [
    93, 0, 0, 0, 0, 1, 1, 1, 1, 0, 0, 0, 0, 1, 1, 1, 1, 0, 0, 0, 1, 1, 1, 1, 0, 0, 0, 0, 1, 1,
];

#[test]
fn static_resource_should_be_callable() {
    // Arrange
    let mut test_runner = TestRunner::builder().build();
    let (key, _priv, account) = test_runner.new_account(false);
    let receipt = test_runner.execute_system_transaction_with_preallocated_ids(
        vec![
            InstructionV1::CallFunction {
                package_address: RESOURCE_PACKAGE,
                blueprint_name: "FungibleResourceManager".to_string(),
                function_name: "create_with_initial_supply_and_address".to_string(),
                args: manifest_decode(
                    &manifest_encode(
                        &FungibleResourceManagerCreateWithInitialSupplyAndAddressInput {
                            divisibility: 0u8,
                            metadata: btreemap!(),
                            access_rules: btreemap!(),
                            initial_supply: Decimal::from(10),
                            resource_address: PRE_ALLOCATED_RESOURCE,
                        },
                    )
                    .unwrap(),
                )
                .unwrap(),
            },
            InstructionV1::CallMethod {
                address: account.into(),
                method_name: ACCOUNT_DEPOSIT_BATCH_IDENT.to_string(),
                args: manifest_args!(ManifestExpression::EntireWorktop),
            },
        ],
        indexset!(NodeId::from(PRE_ALLOCATED_RESOURCE)),
        btreeset!(NonFungibleGlobalId::from_public_key(&key)),
    );
    receipt.expect_commit_success();

    // Act
    let package_address2 = test_runner.compile_and_publish_retain_blueprints(
        "./tests/blueprints/static_dependencies2",
        |blueprint, _| blueprint.eq("SomeResource"),
    );
    let manifest = ManifestBuilder::new()
        .lock_fee(test_runner.faucet_component(), 10.into())
        .call_function(
            package_address2,
            "SomeResource",
            "call_some_resource_total_supply",
            manifest_args!(),
        )
        .build();
    let receipt = test_runner.execute_manifest(manifest, vec![]);

    // Assert
    let result = receipt.expect_commit_success();
    let output = result.outcome.expect_success();
    output[1].expect_return_value(&Decimal::from(10));
}
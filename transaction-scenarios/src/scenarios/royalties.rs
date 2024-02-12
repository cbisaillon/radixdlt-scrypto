use radix_engine_interface::blueprints::package::{PACKAGE_BLUEPRINT, PackageDefinition};
use crate::internal_prelude::*;

#[derive(Default)]
pub struct RoyaltiesState {
    pub royalty_package_address: Option<PackageAddress>,
    pub no_royalty_component_address: Option<ComponentAddress>,
    pub xrd_royalty_component_address: Option<ComponentAddress>,
    pub usd_royalty_component_address: Option<ComponentAddress>,
}

pub enum RoyaltiesScenarioCreator {}

impl ScenarioCreator for RoyaltiesScenarioCreator {
    type Config = ();
    type State = RoyaltiesState;

    fn create_with_config_and_state(
        core: ScenarioCore,
        config: Self::Config,
        start_state: Self::State,
    ) -> Box<dyn ScenarioInstance> {
        let metadata = ScenarioMetadata {
            logical_name: "royalties",
        };

        #[allow(unused_variables)]
        ScenarioBuilder::new(core, metadata, config, start_state)
            .successful_transaction_with_result_handler(
                |core, state, _| {
                    let code = include_bytes!("../../../assets/royalties.wasm").to_vec();
                    let schema = manifest_decode::<PackageDefinition>(
                        include_bytes!("../../../assets/royalties.rpd")
                    ).unwrap();

                    core.next_transaction_with_faucet_lock_fee(
                        "royalties--publish-package",
                        |builder| {
                            builder
                                .allocate_global_address(
                                    PACKAGE_PACKAGE,
                                    PACKAGE_BLUEPRINT,
                                    "package_address_reservation",
                                    "package_address",
                                )
                                .with_name_lookup(|builder, namer| {
                                    let package_address = namer.named_address("package_address");
                                    builder
                                        .publish_package_advanced(
                                            Some("package_address_reservation".to_owned()),
                                            code.to_vec(),
                                            schema,
                                            MetadataInit::default(),
                                            OwnerRole::None,
                                        )
                                })
                        },
                        vec![],
                    )
                },
                |core, config, state, result| {
                    state.royalty_package_address = Some(result.new_package_addresses()[0]);
                    Ok(())
                },
            )
            .successful_transaction_with_result_handler(
                |core, _, state| {
                    core.next_transaction_with_faucet_lock_fee(
                        "royalties--instantiate-components",
                        |builder| {
                            let pkg_address = state.royalty_package_address.unwrap();
                            builder
                                .call_function(pkg_address, "RoyaltiesBp", "new", manifest_args!())
                                .call_function(pkg_address, "RoyaltiesBp", "new", manifest_args!())
                                .call_function(pkg_address, "RoyaltiesBp", "new", manifest_args!())
                        },
                        vec![]
                    )
                },
                |core, config, state, result| {
                    state.no_royalty_component_address = Some(result.new_component_addresses()[0]);
                    state.xrd_royalty_component_address = Some(result.new_component_addresses()[1]);
                    state.usd_royalty_component_address = Some(result.new_component_addresses()[2]);
                    Ok(())
                },
            )
            .successful_transaction(
                |core, _, state| {
                    core.next_transaction_with_faucet_lock_fee(
                        "royalties--set-components-royalty",
                        |mut builder| {
                            let without = state.no_royalty_component_address.unwrap();
                            builder = builder
                                .set_component_royalty(without, "call_no_package_royalty", RoyaltyAmount::Free)
                                .set_component_royalty(without, "call_xrd_package_royalty", RoyaltyAmount::Free)
                                .set_component_royalty(without, "call_usd_package_royalty", RoyaltyAmount::Free);
                            let with_xrd = state.xrd_royalty_component_address.unwrap();
                            builder = builder
                                .set_component_royalty(with_xrd, "call_no_package_royalty", RoyaltyAmount::Xrd(17.into()))
                                .set_component_royalty(with_xrd, "call_xrd_package_royalty", RoyaltyAmount::Xrd(18.into()))
                                .set_component_royalty(with_xrd, "call_usd_package_royalty", RoyaltyAmount::Xrd(19.into()));
                            let with_usd = state.usd_royalty_component_address.unwrap();
                             builder
                                .set_component_royalty(with_usd, "call_no_package_royalty", RoyaltyAmount::Usd(2.into()))
                                .set_component_royalty(with_usd, "call_xrd_package_royalty", RoyaltyAmount::Usd(3.into()))
                                .set_component_royalty(with_usd, "call_usd_package_royalty", RoyaltyAmount::Usd(4.into()))
                        },
                        vec![],
                    )
                }
            )
            .successful_transaction(
                |core, _, state| {
                    core.next_transaction_with_faucet_lock_fee(
                        "royalties--call_all_components_all_methods",
                        |mut builder| {
                            for instance in [
                                state.no_royalty_component_address.unwrap(),
                                state.xrd_royalty_component_address.unwrap(),
                                state.usd_royalty_component_address.unwrap(),
                            ] {
                                builder = builder
                                    .call_method(instance, "call_no_package_royalty", manifest_args!())
                                    .call_method(instance, "call_xrd_package_royalty", manifest_args!())
                                    .call_method(instance, "call_usd_package_royalty", manifest_args!());
                            }
                            builder
                        },
                        vec![],
                    )
                }
            )
            .finalize(|core, config, state| -> Result<_, ScenarioError> {
                Ok(ScenarioOutput {
                    interesting_addresses: DescribedAddresses::new()
                        .add("royalty_package_address", state.royalty_package_address.unwrap())
                        .add("no_royalty_component_address", state.no_royalty_component_address.unwrap())
                        .add("xrd_royalty_component_address", state.xrd_royalty_component_address.unwrap())
                        .add("usd_royalty_component_address", state.usd_royalty_component_address.unwrap()),
                })
            })
    }
}

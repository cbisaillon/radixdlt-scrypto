use sbor::rust::borrow::ToOwned;
use sbor::rust::collections::*;
use sbor::rust::vec;
use sbor::*;
use scrypto::buffer::*;
use scrypto::constants::*;
use scrypto::crypto::*;
use scrypto::engine::types::*;
use scrypto::resource::ResourceMethodAuthKey::Withdraw;
use scrypto::resource::LOCKED;
use scrypto::rule;

use crate::engine::Track;
use crate::engine::TrackReceipt;
use crate::ledger::{ReadableSubstateStore, WriteableSubstateStore};
use crate::model::ValidatedPackage;

#[derive(TypeId, Encode, Decode)]
struct SystemComponentState {
    xrd: scrypto::resource::Vault,
}

const XRD_SYMBOL: &str = "XRD";
const XRD_NAME: &str = "Radix";
const XRD_DESCRIPTION: &str = "The Radix Public Network's native token, used to pay the network's required transaction fees and to secure the network through staking to its validator nodes.";
const XRD_URL: &str = "https://tokens.radixdlt.com";
const XRD_MAX_SUPPLY: i128 = 24_000_000_000i128;
const XRD_VAULT_ID: VaultId = (Hash([0u8; 32]), 0);
const XRD_VAULT: scrypto::resource::Vault = scrypto::resource::Vault(XRD_VAULT_ID);

const SYSTEM_COMPONENT_NAME: &str = "System";

use crate::model::*;

fn create_genesis(mut track: Track) -> TrackReceipt {
    let system_package =
        extract_package(include_bytes!("../../../assets/system.wasm").to_vec()).unwrap();
    let validated_system_package = ValidatedPackage::new(system_package).unwrap();
    track.create_uuid_substate(
        SubstateId::Package(SYSTEM_PACKAGE),
        validated_system_package,
    );

    let account_package =
        extract_package(include_bytes!("../../../assets/account.wasm").to_vec()).unwrap();
    let validated_account_package = ValidatedPackage::new(account_package).unwrap();
    track.create_uuid_substate(
        SubstateId::Package(ACCOUNT_PACKAGE),
        validated_account_package,
    );

    // Radix token resource address
    let mut metadata = HashMap::new();
    metadata.insert("symbol".to_owned(), XRD_SYMBOL.to_owned());
    metadata.insert("name".to_owned(), XRD_NAME.to_owned());
    metadata.insert("description".to_owned(), XRD_DESCRIPTION.to_owned());
    metadata.insert("url".to_owned(), XRD_URL.to_owned());

    let mut resource_auth = HashMap::new();
    resource_auth.insert(Withdraw, (rule!(allow_all), LOCKED));

    let mut xrd_resource_manager = ResourceManager::new(
        ResourceType::Fungible { divisibility: 18 },
        metadata,
        resource_auth,
    )
    .unwrap();
    let minted_xrd = xrd_resource_manager
        .mint_fungible(XRD_MAX_SUPPLY.into(), RADIX_TOKEN.clone())
        .unwrap();
    track.create_uuid_substate(
        SubstateId::ResourceManager(RADIX_TOKEN),
        xrd_resource_manager,
    );

    let mut ecdsa_resource_auth = HashMap::new();
    ecdsa_resource_auth.insert(Withdraw, (rule!(allow_all), LOCKED));
    let ecdsa_token = ResourceManager::new(
        ResourceType::NonFungible,
        HashMap::new(),
        ecdsa_resource_auth,
    )
    .unwrap();
    track.create_uuid_substate(SubstateId::ResourceManager(ECDSA_TOKEN), ecdsa_token);

    let system_token =
        ResourceManager::new(ResourceType::NonFungible, HashMap::new(), HashMap::new()).unwrap();
    track.create_uuid_substate(SubstateId::ResourceManager(SYSTEM_TOKEN), system_token);

    let system_vault = Vault::new(minted_xrd);
    track.create_uuid_substate(SubstateId::Vault(XRD_VAULT_ID), system_vault);

    let system_component = Component::new(SYSTEM_PACKAGE, SYSTEM_COMPONENT_NAME.to_owned(), vec![]);
    let system_component_state =
        ComponentState::new(scrypto_encode(&SystemComponentState { xrd: XRD_VAULT }));
    track.create_uuid_substate(
        SubstateId::ComponentInfo(SYSTEM_COMPONENT, true),
        system_component,
    );
    track.create_uuid_substate(
        SubstateId::ComponentState(SYSTEM_COMPONENT),
        system_component_state,
    );
    track.create_uuid_substate(SubstateId::System, System { epoch: 0 });

    track.commit();
    track.to_receipt()
}

pub fn bootstrap<S>(mut substate_store: S) -> S
where
    S: ReadableSubstateStore + WriteableSubstateStore + 'static,
{
    if substate_store
        .get_substate(&SubstateId::Package(SYSTEM_PACKAGE))
        .is_none()
    {
        let track = Track::new(&substate_store);
        let receipt = create_genesis(track);
        receipt.state_updates.commit(&mut substate_store);
    }
    substate_store
}

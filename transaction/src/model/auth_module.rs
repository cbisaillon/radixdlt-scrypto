use radix_engine_lib::crypto::PublicKey;
use radix_engine_lib::resource::{NonFungibleAddress, NonFungibleId};
use scrypto::constants::SYSTEM_TOKEN;
use scrypto::resource::non_fungible::FromPublicKey;

pub struct AuthModule;

// TODO: Integrate with AuthModule in radix-engine
impl AuthModule {
    pub fn system_role_non_fungible_address() -> NonFungibleAddress {
        NonFungibleAddress::new(SYSTEM_TOKEN, NonFungibleId::from_u32(1))
    }

    pub fn validator_role_non_fungible_address() -> NonFungibleAddress {
        NonFungibleAddress::new(SYSTEM_TOKEN, NonFungibleId::from_u32(0))
    }

    pub fn pk_non_fungibles(signer_public_keys: &[PublicKey]) -> Vec<NonFungibleAddress> {
        signer_public_keys
            .iter()
            .map(NonFungibleAddress::from_public_key)
            .collect()
    }
}

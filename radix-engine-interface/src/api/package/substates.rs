use crate::abi::*;
use crate::api::types::*;
use crate::data::types::Own;
use radix_engine_derive::*;
use sbor::rust::collections::*;
use sbor::rust::fmt::{Debug, Formatter};

pub const RESOURCE_MANAGER_PACKAGE_CODE_ID: u8 = 0u8;
pub const IDENTITY_PACKAGE_CODE_ID: u8 = 1u8;
pub const EPOCH_MANAGER_PACKAGE_CODE_ID: u8 = 2u8;
pub const CLOCK_PACKAGE_CODE_ID: u8 = 3u8;
pub const ACCOUNT_PACKAGE_CODE_ID: u8 = 4u8;
pub const ACCESS_CONTROLLER_PACKAGE_CODE_ID: u8 = 5u8;

/// A collection of blueprints, compiled and published as a single unit.
#[derive(Debug, Clone, PartialEq, Eq, ScryptoCategorize, ScryptoEncode, ScryptoDecode)]
pub struct NativePackageInfoSubstate {
    pub native_package_code_id: u8,
    pub dependent_resources: BTreeSet<ResourceAddress>,
}

/// A collection of blueprints, compiled and published as a single unit.
#[derive(Clone, Categorize, Encode, Decode, PartialEq, Eq)]
pub struct PackageInfoSubstate {
    pub code: Vec<u8>,
    pub blueprint_abis: BTreeMap<String, BlueprintAbi>,
}

#[derive(Debug, Clone, PartialEq, Eq, ScryptoCategorize, ScryptoEncode, ScryptoDecode)]
pub struct PackageRoyaltyConfigSubstate {
    pub royalty_config: BTreeMap<String, RoyaltyConfig>,
}

impl Debug for PackageInfoSubstate {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("PackageInfoSubstate")
            .field("blueprint_abis", &self.blueprint_abis)
            .finish()
    }
}

impl PackageInfoSubstate {
    pub fn blueprint_abi(&self, blueprint_name: &str) -> Option<&BlueprintAbi> {
        self.blueprint_abis.get(blueprint_name)
    }

    // TODO: Reorganize structure
    pub fn fn_abi(&self, export_name: &str) -> Option<&Fn> {
        for (_, abi) in &self.blueprint_abis {
            for function in &abi.fns {
                if export_name.eq(&function.export_name) {
                    return Some(function);
                }
            }
        }

        return None;
    }

    pub fn code(&self) -> &[u8] {
        &self.code
    }
}

#[derive(Debug, Clone, PartialEq, Eq, ScryptoCategorize, ScryptoEncode, ScryptoDecode)]
pub struct PackageRoyaltyAccumulatorSubstate {
    pub royalty: Own,
}
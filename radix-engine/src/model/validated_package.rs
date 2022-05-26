use sbor::rust::collections::HashMap;
use sbor::rust::string::String;
use sbor::rust::vec::Vec;
use sbor::*;
use scrypto::abi::{Function, Method};
use scrypto::buffer::scrypto_decode;
use scrypto::component::PackageFunction;
use scrypto::values::ScryptoValue;

use crate::engine::SystemApi;
use crate::wasm::*;

/// A collection of blueprints, compiled and published as a single unit.
#[derive(Debug, Clone, TypeId, Encode, Decode)]
pub struct ValidatedPackage {
    code: Vec<u8>,
    blueprints: HashMap<String, (Type, Vec<Function>, Vec<Method>)>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PackageError {
    InvalidRequestData(DecodeError),
    BlueprintNotFound,
    WasmValidationError(WasmValidationError),
    MethodNotFound(String),
}

impl ValidatedPackage {
    /// Validates and creates a package
    pub fn new(package: scrypto::prelude::Package) -> Result<Self, WasmValidationError> {
        let code = package.code;
        let mut wasm_engine = WasmiEngine::new();
        wasm_engine.validate(&code)?;

        // TODO will replace this with static ABIs
        let module = wasm_engine.instantiate(&code);
        let mut blueprints = HashMap::new();
        for mut method_name in package.blueprints {
            method_name.push_str("_abi");

            let rtn = module
                .invoke_export(
                    &method_name,
                    &ScryptoValue::unit(),
                    &mut NopScryptoRuntime {},
                )
                .map_err(|_| WasmValidationError::UnableToExportBlueprintAbi)?;

            let abi: (Type, Vec<Function>, Vec<Method>) =
                scrypto_decode(&rtn.raw).map_err(|_| WasmValidationError::InvalidBlueprintAbi)?;

            if let Type::Struct { name, fields: _ } = &abi.0 {
                blueprints.insert(name.clone(), abi);
            } else {
                return Err(WasmValidationError::InvalidBlueprintAbi);
            }
        }

        Ok(Self {
            blueprints,
            code,
        })
    }

    pub fn code(&self) -> &[u8] {
        &self.code
    }

    pub fn blueprint_abi(
        &self,
        blueprint_name: &str,
    ) -> Option<&(Type, Vec<Function>, Vec<Method>)> {
        self.blueprints.get(blueprint_name)
    }

    pub fn contains_blueprint(&self, blueprint_name: &str) -> bool {
        self.blueprints.contains_key(blueprint_name)
    }

    pub fn load_blueprint_schema(&self, blueprint_name: &str) -> Result<&Type, PackageError> {
        self.blueprint_abi(blueprint_name)
            .map(|v| &v.0)
            .ok_or(PackageError::BlueprintNotFound)
    }

    pub fn static_main<S: SystemApi>(
        call_data: ScryptoValue,
        system_api: &mut S,
    ) -> Result<ScryptoValue, PackageError> {
        let function: PackageFunction =
            scrypto_decode(&call_data.raw).map_err(|e| PackageError::InvalidRequestData(e))?;
        match function {
            PackageFunction::Publish(package) => {
                let package =
                    ValidatedPackage::new(package).map_err(PackageError::WasmValidationError)?;
                let package_address = system_api.create_package(package);
                Ok(ScryptoValue::from_value(&package_address))
            }
        }
    }
}

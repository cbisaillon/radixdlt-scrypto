use super::system_modules::auth::{convert_contextless, Authentication};
use super::system_modules::costing::CostingReason;
use crate::errors::{
    ApplicationError, CreateObjectError, InvalidDropNodeAccess, InvalidModuleSet,
    InvalidModuleType, KernelError, RuntimeError,
};
use crate::errors::{SystemError, SystemUpstreamError};
use crate::kernel::actor::{Actor, InstanceContext};
use crate::kernel::call_frame::RefType;
use crate::kernel::kernel_api::*;
use crate::system::node_init::ModuleInit;
use crate::system::node_modules::type_info::{TypeInfoBlueprint, TypeInfoSubstate};
use crate::system::system_callback::{SystemConfig, SystemInvocation};
use crate::system::system_callback_api::SystemCallbackObject;
use crate::system::system_modules::costing::FIXED_LOW_FEE;
use crate::system::system_modules::events::EventError;
use crate::system::system_modules::execution_trace::{BucketSnapshot, ProofSnapshot};
use crate::track::interface::NodeSubstates;
use crate::types::*;
use radix_engine_interface::api::index_api::ClientIndexApi;
use radix_engine_interface::api::key_value_store_api::ClientKeyValueStoreApi;
use radix_engine_interface::api::node_modules::auth::*;
use radix_engine_interface::api::node_modules::metadata::*;
use radix_engine_interface::api::node_modules::royalty::*;
use radix_engine_interface::api::object_api::ObjectModuleId;
use radix_engine_interface::api::sorted_index_api::SortedKey;
use radix_engine_interface::api::substate_lock_api::LockFlags;
use radix_engine_interface::api::*;
use radix_engine_interface::blueprints::access_controller::*;
use radix_engine_interface::blueprints::account::*;
use radix_engine_interface::blueprints::clock::CLOCK_BLUEPRINT;
use radix_engine_interface::blueprints::epoch_manager::*;
use radix_engine_interface::blueprints::identity::*;
use radix_engine_interface::blueprints::package::*;
use radix_engine_interface::blueprints::resource::*;
use radix_engine_interface::schema::{BlueprintSchema, KeyValueStoreSchema};
use resources_tracker_macro::trace_resources;
use sbor::rust::string::ToString;
use sbor::rust::vec::Vec;

/// Provided to upper layer for invoking lower layer service
pub struct SystemService<'a, Y: KernelApi<SystemConfig<V>>, V: SystemCallbackObject> {
    pub api: &'a mut Y,
    pub phantom: PhantomData<V>,
}

impl<'a, Y, V> SystemService<'a, Y, V>
where
    Y: KernelApi<SystemConfig<V>>,
    V: SystemCallbackObject,
{
    pub fn new(api: &'a mut Y) -> Self {
        Self {
            api,
            phantom: PhantomData::default(),
        }
    }

    pub fn get_node_type_info(&mut self, node_id: &NodeId) -> Option<TypeInfoSubstate> {
        // This is to solve the bootstrapping problem.
        // TODO: Can be removed if we flush bootstrap state updates without transactional execution.
        if node_id.eq(RADIX_TOKEN.as_node_id()) {
            return Some(TypeInfoSubstate::Object(ObjectInfo {
                blueprint: Blueprint {
                    package_address: RESOURCE_PACKAGE,
                    blueprint_name: FUNGIBLE_RESOURCE_MANAGER_BLUEPRINT.to_string(),
                },
                global: true,
                outer_object: None,
            }));
        } else if node_id.eq(ECDSA_SECP256K1_SIGNATURE_VIRTUAL_BADGE.as_node_id())
            || node_id.eq(EDDSA_ED25519_SIGNATURE_VIRTUAL_BADGE.as_node_id())
            || node_id.eq(SYSTEM_TRANSACTION_BADGE.as_node_id())
            || node_id.eq(PACKAGE_VIRTUAL_BADGE.as_node_id())
            || node_id.eq(GLOBAL_ACTOR_VIRTUAL_BADGE.as_node_id())
            || node_id.eq(PACKAGE_OWNER_BADGE.as_node_id())
            || node_id.eq(VALIDATOR_OWNER_BADGE.as_node_id())
            || node_id.eq(IDENTITY_OWNER_BADGE.as_node_id())
            || node_id.eq(ACCOUNT_OWNER_BADGE.as_node_id())
        {
            return Some(TypeInfoSubstate::Object(ObjectInfo {
                blueprint: Blueprint {
                    package_address: RESOURCE_PACKAGE,
                    blueprint_name: NON_FUNGIBLE_RESOURCE_MANAGER_BLUEPRINT.to_string(),
                },
                global: true,
                outer_object: None,
            }));
        }

        self.api
            .kernel_lock_substate(
                node_id,
                SysModuleId::TypeInfo.into(),
                &TypeInfoOffset::TypeInfo.into(),
                LockFlags::read_only(),
            )
            .and_then(|lock_handle| {
                self.api
                    .kernel_read_substate(lock_handle)
                    .and_then(|x| Ok(x.as_typed::<TypeInfoSubstate>().unwrap()))
                    .and_then(|substate| {
                        self.api
                            .kernel_drop_lock(lock_handle)
                            .and_then(|_| Ok(substate))
                    })
            })
            .ok()
    }

    fn new_object_internal(
        &mut self,
        blueprint_ident: &str,
        fields: Vec<Vec<u8>>,
        package_address: PackageAddress,
        instance_context: Option<InstanceContext>,
    ) -> Result<NodeId, RuntimeError> {
        let blueprint = Blueprint::new(&package_address, blueprint_ident);
        let expected_blueprint_parent = self.verify_blueprint_fields(&blueprint, &fields)?;
        let outer_object = if let Some(parent) = &expected_blueprint_parent {
            match instance_context {
                Some(context) if context.instance_blueprint.eq(parent) => Some(context.instance),
                _ => {
                    return Err(RuntimeError::SystemError(
                        SystemError::InvalidChildObjectCreation,
                    ));
                }
            }
        } else {
            None
        };

        let node_id = {
            let entity_type = match (package_address, blueprint_ident) {
                (RESOURCE_PACKAGE, FUNGIBLE_VAULT_BLUEPRINT) => EntityType::InternalFungibleVault,
                (RESOURCE_PACKAGE, NON_FUNGIBLE_VAULT_BLUEPRINT) => {
                    EntityType::InternalNonFungibleVault
                }
                (ACCOUNT_PACKAGE, ACCOUNT_BLUEPRINT) => EntityType::InternalAccount,
                _ => EntityType::InternalGenericComponent,
            };

            self.api.kernel_allocate_node_id(entity_type)?
        };

        let node_init: BTreeMap<SubstateKey, IndexedScryptoValue> = fields
            .into_iter()
            .enumerate()
            .map(|(i, x)| {
                (
                    // TODO check size during package publishing time
                    SubstateKey::Tuple(i as u8),
                    IndexedScryptoValue::from_vec(x).expect("Checked by payload-schema validation"),
                )
            })
            .collect();

        let self_module_id = match (package_address, blueprint_ident) {
            (METADATA_MODULE_PACKAGE, METADATA_BLUEPRINT) => SysModuleId::Virtualized,
            _ => SysModuleId::Object,
        };

        self.api.kernel_create_node(
            node_id,
            btreemap!(
                self_module_id.into() => node_init,
                SysModuleId::TypeInfo.into() => ModuleInit::TypeInfo(
                    TypeInfoSubstate::Object(ObjectInfo {
                        blueprint: Blueprint::new(&package_address,blueprint_ident),
                        global:false,
                        outer_object
                    })
                ).to_substates(),
            ),
        )?;

        Ok(node_id.into())
    }

    fn get_blueprint_schema(
        &mut self,
        blueprint: &Blueprint,
    ) -> Result<BlueprintSchema, RuntimeError> {
        let handle = self.api.kernel_lock_substate(
            blueprint.package_address.as_node_id(),
            SysModuleId::Object.into(),
            &PackageOffset::Info.into(),
            LockFlags::read_only(),
        )?;
        let package: PackageInfoSubstate =
            self.api.kernel_read_substate(handle)?.as_typed().unwrap();
        let schema = package
            .schema
            .blueprints
            .get(blueprint.blueprint_name.as_str())
            .ok_or(RuntimeError::SystemError(SystemError::CreateObjectError(
                Box::new(CreateObjectError::BlueprintNotFound(
                    blueprint.blueprint_name.to_string(),
                )),
            )))?
            .clone();

        self.api.kernel_drop_lock(handle)?;

        Ok(schema)
    }

    fn verify_blueprint_fields(
        &mut self,
        blueprint: &Blueprint,
        fields: &Vec<Vec<u8>>,
    ) -> Result<Option<String>, RuntimeError> {
        let handle = self.api.kernel_lock_substate(
            blueprint.package_address.as_node_id(),
            SysModuleId::Object.into(),
            &PackageOffset::Info.into(),
            LockFlags::read_only(),
        )?;
        let package: PackageInfoSubstate =
            self.api.kernel_read_substate(handle)?.as_typed().unwrap();
        let schema = package
            .schema
            .blueprints
            .get(&blueprint.blueprint_name)
            .ok_or(RuntimeError::SystemError(SystemError::CreateObjectError(
                Box::new(CreateObjectError::BlueprintNotFound(
                    blueprint.blueprint_name.to_string(),
                )),
            )))?;
        if schema.substates.len() != fields.len() {
            return Err(RuntimeError::SystemError(SystemError::CreateObjectError(
                Box::new(CreateObjectError::WrongNumberOfSubstates(
                    blueprint.clone(),
                    fields.len(),
                    schema.substates.len(),
                )),
            )));
        }
        for i in 0..fields.len() {
            validate_payload_against_schema(&fields[i], &schema.schema, schema.substates[i], self)
                .map_err(|err| {
                    RuntimeError::SystemError(SystemError::CreateObjectError(Box::new(
                        CreateObjectError::InvalidSubstateWrite(err.error_message(&schema.schema)),
                    )))
                })?;
        }

        let parent_blueprint = schema.outer_blueprint.clone();

        self.api.kernel_drop_lock(handle)?;

        Ok(parent_blueprint)
    }
}

impl<'a, Y, V> ClientSubstateLockApi<RuntimeError> for SystemService<'a, Y, V>
where
    Y: KernelApi<SystemConfig<V>>,
    V: SystemCallbackObject,
{
    #[trace_resources]
    fn sys_read_substate(&mut self, lock_handle: LockHandle) -> Result<Vec<u8>, RuntimeError> {
        self.api
            .kernel_read_substate(lock_handle)
            .map(|v| v.as_slice().to_vec())
    }

    #[trace_resources]
    fn sys_write_substate(
        &mut self,
        lock_handle: LockHandle,
        buffer: Vec<u8>,
    ) -> Result<(), RuntimeError> {
        let LockInfo {
            node_id,
            module_id,
            substate_key,
            ..
        } = self.api.kernel_get_lock_info(lock_handle)?;

        let type_info = TypeInfoBlueprint::get_type(&node_id, self.api)?;
        match type_info {
            TypeInfoSubstate::KeyValueStore(store_schema) => {
                if let Err(e) = validate_payload_against_schema(
                    &buffer,
                    &store_schema.schema,
                    store_schema.value,
                    self,
                ) {
                    return Err(RuntimeError::SystemError(
                        SystemError::InvalidSubstateWrite(e.error_message(&store_schema.schema)),
                    ));
                };

                if !store_schema.can_own {
                    let indexed = IndexedScryptoValue::from_slice(&buffer)
                        .expect("Should be valid due to payload check");
                    let (_, own, _) = indexed.unpack();
                    if !own.is_empty() {
                        return Err(RuntimeError::SystemError(
                            SystemError::InvalidKeyValueStoreOwnership,
                        ));
                    }
                }
            }
            TypeInfoSubstate::Object(ObjectInfo { blueprint, .. }) => {
                match SysModuleId::from_repr(module_id.0).unwrap() {
                    SysModuleId::Object => {
                        // Load the Object SysModule schema from the Package
                        let handle = self.kernel_lock_substate(
                            blueprint.package_address.as_node_id(),
                            SysModuleId::Object.into(),
                            &PackageOffset::Info.into(),
                            LockFlags::read_only(),
                        )?;
                        let package_info: PackageInfoSubstate =
                            self.sys_read_substate_typed(handle)?;
                        self.kernel_drop_lock(handle)?;

                        let blueprint_schema = package_info
                            .schema
                            .blueprints
                            .get(&blueprint.blueprint_name)
                            .expect("Missing blueprint schema");

                        // Validate the substate against the schema
                        if let SubstateKey::Tuple(offset) = substate_key {
                            if let Some(index) = blueprint_schema.substates.get(offset as usize) {
                                if let Err(e) = validate_payload_against_schema(
                                    &buffer,
                                    &blueprint_schema.schema,
                                    *index,
                                    self,
                                ) {
                                    return Err(RuntimeError::SystemError(
                                        SystemError::InvalidSubstateWrite(
                                            e.error_message(&blueprint_schema.schema),
                                        ),
                                    ));
                                };
                            } else {
                                let schema_substate_count = blueprint_schema.substates.len();
                                return Err(RuntimeError::SystemError(
                                    SystemError::InvalidSubstateWrite(format!("Stored a substate at tuple index {offset} but schema for {blueprint:?} only has {schema_substate_count} defined")),
                                ));
                            }
                        } else {
                            // TODO - we don't have schemas for this bit yet
                        }
                    }
                    SysModuleId::TypeInfo
                    | SysModuleId::Metadata
                    | SysModuleId::Royalty
                    | SysModuleId::AccessRules
                    | SysModuleId::Virtualized => {
                        // TODO: We should validate these substates, but luckily they're not accessible from
                        // Scrypto, so safe for now.
                    }
                };
            }
            TypeInfoSubstate::Index | TypeInfoSubstate::SortedIndex => {
                // TODO: Check objects stored are storeable
            }
        }

        let substate =
            IndexedScryptoValue::from_vec(buffer).expect("Should be valid due to payload check");
        self.api.kernel_write_substate(lock_handle, substate)?;

        Ok(())
    }

    #[trace_resources]
    fn sys_drop_lock(&mut self, lock_handle: LockHandle) -> Result<(), RuntimeError> {
        let info = self.api.kernel_get_lock_info(lock_handle)?;
        if info.flags.contains(LockFlags::MUTABLE) {}

        self.api.kernel_drop_lock(lock_handle)
    }
}

impl<'a, Y, V> ClientObjectApi<RuntimeError> for SystemService<'a, Y, V>
where
    Y: KernelApi<SystemConfig<V>>,
    V: SystemCallbackObject,
{
    #[trace_resources]
    fn new_object(
        &mut self,
        blueprint_ident: &str,
        fields: Vec<Vec<u8>>,
    ) -> Result<NodeId, RuntimeError> {
        let actor = self.api.kernel_get_current_actor().unwrap();
        let package_address = actor.package_address().clone();
        let instance_context = actor.instance_context();
        self.new_object_internal(blueprint_ident, fields, package_address, instance_context)
    }

    #[trace_resources]
    fn globalize(
        &mut self,
        modules: BTreeMap<ObjectModuleId, NodeId>,
    ) -> Result<GlobalAddress, RuntimeError> {
        // FIXME check completeness of modules

        let node_id = modules
            .get(&ObjectModuleId::SELF)
            .ok_or(RuntimeError::SystemError(SystemError::MissingModule(
                ObjectModuleId::SELF,
            )))?;

        let type_info = TypeInfoBlueprint::get_type(node_id, self.api)?;
        let blueprint = match type_info {
            TypeInfoSubstate::Object(ObjectInfo {
                blueprint, global, ..
            }) if !global => blueprint,
            _ => return Err(RuntimeError::SystemError(SystemError::CannotGlobalize)),
        };

        let entity_type = match (blueprint.package_address, blueprint.blueprint_name.as_str()) {
            (ACCOUNT_PACKAGE, PACKAGE_BLUEPRINT) => EntityType::GlobalPackage,
            (RESOURCE_PACKAGE, FUNGIBLE_RESOURCE_MANAGER_BLUEPRINT) => {
                EntityType::GlobalFungibleResource
            }
            (RESOURCE_PACKAGE, NON_FUNGIBLE_RESOURCE_MANAGER_BLUEPRINT) => {
                EntityType::GlobalNonFungibleResource
            }
            (EPOCH_MANAGER_PACKAGE, EPOCH_MANAGER_BLUEPRINT) => EntityType::GlobalEpochManager,
            (EPOCH_MANAGER_PACKAGE, VALIDATOR_BLUEPRINT) => EntityType::GlobalValidator,
            (CLOCK_PACKAGE, CLOCK_BLUEPRINT) => EntityType::GlobalClock,
            (ACCESS_CONTROLLER_PACKAGE, ACCESS_CONTROLLER_BLUEPRINT) => {
                EntityType::GlobalAccessController
            }
            (ACCOUNT_PACKAGE, ACCOUNT_BLUEPRINT) => EntityType::GlobalAccount,
            (IDENTITY_PACKAGE, IDENTITY_BLUEPRINT) => EntityType::GlobalIdentity,
            _ => EntityType::GlobalGenericComponent,
        };

        let global_node_id = self.api.kernel_allocate_node_id(entity_type)?;
        let global_address = GlobalAddress::new_or_panic(global_node_id.into());

        self.globalize_with_address(modules, global_address)?;
        Ok(global_address)
    }

    #[trace_resources]
    fn globalize_with_address(
        &mut self,
        mut modules: BTreeMap<ObjectModuleId, NodeId>,
        address: GlobalAddress,
    ) -> Result<(), RuntimeError> {
        // Check module configuration
        let module_ids = modules
            .keys()
            .cloned()
            .collect::<BTreeSet<ObjectModuleId>>();
        let standard_object = btreeset!(
            ObjectModuleId::SELF,
            ObjectModuleId::Metadata,
            ObjectModuleId::Royalty,
            ObjectModuleId::AccessRules
        );
        if module_ids != standard_object {
            return Err(RuntimeError::SystemError(SystemError::InvalidModuleSet(
                Box::new(InvalidModuleSet(module_ids)),
            )));
        }

        // Drop the node
        let node_id = modules
            .remove(&ObjectModuleId::SELF)
            .ok_or(RuntimeError::SystemError(SystemError::MissingModule(
                ObjectModuleId::SELF,
            )))?;
        self.api
            .kernel_get_callback()
            .modules
            .events
            .add_replacement(
                (node_id, ObjectModuleId::SELF),
                (*address.as_node_id(), ObjectModuleId::SELF),
            );
        let mut node_substates = self.api.kernel_drop_node(&node_id)?;

        // Update the `global` flag of the type info substate.
        let type_info_module = node_substates
            .get_mut(&SysModuleId::TypeInfo.into())
            .unwrap()
            .remove(&TypeInfoOffset::TypeInfo.into())
            .unwrap();
        let mut type_info: TypeInfoSubstate = type_info_module.as_typed().unwrap();
        match type_info {
            TypeInfoSubstate::Object(ObjectInfo { ref mut global, .. }) if !*global => {
                *global = true
            }
            _ => return Err(RuntimeError::SystemError(SystemError::CannotGlobalize)),
        };
        node_substates
            .get_mut(&SysModuleId::TypeInfo.into())
            .unwrap()
            .insert(
                TypeInfoOffset::TypeInfo.into(),
                IndexedScryptoValue::from_typed(&type_info),
            );

        //  Drop the module nodes and move the substates to the designated module ID.
        for (module_id, node_id) in modules {
            match module_id {
                ObjectModuleId::SELF => panic!("Should have been removed already"),
                ObjectModuleId::AccessRules => {
                    let blueprint = self.get_object_info(&node_id)?.blueprint;
                    let expected =
                        Blueprint::new(&ACCESS_RULES_MODULE_PACKAGE, ACCESS_RULES_BLUEPRINT);
                    if !blueprint.eq(&expected) {
                        return Err(RuntimeError::SystemError(SystemError::InvalidModuleType(
                            Box::new(InvalidModuleType {
                                expected_blueprint: expected,
                                actual_blueprint: blueprint,
                            }),
                        )));
                    }

                    self.api
                        .kernel_get_callback()
                        .modules
                        .events
                        .add_replacement(
                            (node_id, ObjectModuleId::SELF),
                            (*address.as_node_id(), ObjectModuleId::AccessRules),
                        );

                    let mut access_rule_substates = self.api.kernel_drop_node(&node_id)?;
                    let access_rules = access_rule_substates
                        .remove(&SysModuleId::Object.into())
                        .unwrap();
                    node_substates.insert(SysModuleId::AccessRules.into(), access_rules);
                }
                ObjectModuleId::Metadata => {
                    let blueprint = self.get_object_info(&node_id)?.blueprint;
                    let expected = Blueprint::new(&METADATA_MODULE_PACKAGE, METADATA_BLUEPRINT);
                    if !blueprint.eq(&expected) {
                        return Err(RuntimeError::SystemError(SystemError::InvalidModuleType(
                            Box::new(InvalidModuleType {
                                expected_blueprint: expected,
                                actual_blueprint: blueprint,
                            }),
                        )));
                    }

                    self.api
                        .kernel_get_callback()
                        .modules
                        .events
                        .add_replacement(
                            (node_id, ObjectModuleId::SELF),
                            (*address.as_node_id(), ObjectModuleId::Metadata),
                        );

                    let mut metadata_substates = self.api.kernel_drop_node(&node_id)?;
                    let metadata = metadata_substates
                        .remove(&SysModuleId::Virtualized.into())
                        .unwrap();
                    node_substates.insert(SysModuleId::Metadata.into(), metadata);
                }
                ObjectModuleId::Royalty => {
                    let blueprint = self.get_object_info(&node_id)?.blueprint;
                    let expected =
                        Blueprint::new(&ROYALTY_MODULE_PACKAGE, COMPONENT_ROYALTY_BLUEPRINT);
                    if !blueprint.eq(&expected) {
                        return Err(RuntimeError::SystemError(SystemError::InvalidModuleType(
                            Box::new(InvalidModuleType {
                                expected_blueprint: expected,
                                actual_blueprint: blueprint,
                            }),
                        )));
                    }

                    self.api
                        .kernel_get_callback()
                        .modules
                        .events
                        .add_replacement(
                            (node_id, ObjectModuleId::SELF),
                            (*address.as_node_id(), ObjectModuleId::Royalty),
                        );

                    let mut royalty_substates = self.api.kernel_drop_node(&node_id)?;
                    let royalty = royalty_substates
                        .remove(&SysModuleId::Object.into())
                        .unwrap();
                    node_substates.insert(SysModuleId::Royalty.into(), royalty);
                }
            }
        }

        self.api
            .kernel_create_node(address.into(), node_substates)?;

        Ok(())
    }

    fn globalize_with_address_and_create_inner_object(
        &mut self,
        modules: BTreeMap<ObjectModuleId, NodeId>,
        address: GlobalAddress,
        inner_object_blueprint: &str,
        inner_object_fields: Vec<Vec<u8>>,
    ) -> Result<NodeId, RuntimeError> {
        let node_id = modules
            .get(&ObjectModuleId::SELF)
            .ok_or(RuntimeError::SystemError(SystemError::MissingModule(
                ObjectModuleId::SELF,
            )))?;
        let blueprint = self.get_object_info(node_id)?.blueprint;

        self.globalize_with_address(modules, address)?;

        self.new_object_internal(
            inner_object_blueprint,
            inner_object_fields,
            blueprint.package_address,
            Some(InstanceContext {
                instance: address,
                instance_blueprint: blueprint.blueprint_name,
            }),
        )
    }

    #[trace_resources]
    fn call_method(
        &mut self,
        receiver: &NodeId,
        method_name: &str,
        args: Vec<u8>,
    ) -> Result<Vec<u8>, RuntimeError> {
        self.call_module_method(receiver, ObjectModuleId::SELF, method_name, args)
    }

    #[trace_resources]
    fn call_module_method(
        &mut self,
        receiver: &NodeId,
        module_id: ObjectModuleId,
        method_name: &str,
        args: Vec<u8>,
    ) -> Result<Vec<u8>, RuntimeError> {
        let (object_info, global_address) = match module_id {
            ObjectModuleId::SELF => {
                let type_info = TypeInfoBlueprint::get_type(receiver, self.api)?;
                match type_info {
                    TypeInfoSubstate::Object(info @ ObjectInfo { global, .. }) => {
                        let global_address = if global {
                            Some(GlobalAddress::new_or_panic(receiver.clone().into()))
                        } else {
                            // See if we have a parent

                            // TODO: Cleanup, this is a rather crude way of trying to figure out
                            // TODO: whether the node reference is a child of the current parent
                            // TODO: this should be cleaned up once call_frame is refactored
                            let (visibility, on_heap) =
                                self.api.kernel_get_node_info(receiver).unwrap();
                            match (visibility, on_heap) {
                                (RefType::Normal, false) => {
                                    self.api.kernel_get_current_actor().and_then(|a| match a {
                                        Actor::Method { global_address, .. } => global_address,
                                        _ => None,
                                    })
                                }
                                _ => None,
                            }
                        };

                        (info, global_address)
                    }

                    TypeInfoSubstate::KeyValueStore(..)
                    | TypeInfoSubstate::SortedIndex
                    | TypeInfoSubstate::Index => {
                        return Err(RuntimeError::SystemError(
                            SystemError::CallMethodOnKeyValueStore,
                        ))
                    }
                }
            }
            ObjectModuleId::Metadata => {
                // TODO: Check if type has metadata
                (
                    ObjectInfo {
                        blueprint: Blueprint::new(&METADATA_MODULE_PACKAGE, METADATA_BLUEPRINT),
                        outer_object: None,
                        global: true,
                    },
                    None,
                )
            }
            ObjectModuleId::Royalty => {
                // TODO: Check if type has royalty
                (
                    ObjectInfo {
                        blueprint: Blueprint::new(
                            &ROYALTY_MODULE_PACKAGE,
                            COMPONENT_ROYALTY_BLUEPRINT,
                        ),
                        outer_object: None,
                        global: true,
                    },
                    None,
                )
            }
            ObjectModuleId::AccessRules => {
                // TODO: Check if type has access rules
                (
                    ObjectInfo {
                        blueprint: Blueprint::new(
                            &ACCESS_RULES_MODULE_PACKAGE,
                            ACCESS_RULES_BLUEPRINT,
                        ),
                        outer_object: None,
                        global: true,
                    },
                    None,
                )
            }
        };

        let identifier = MethodIdentifier(receiver.clone(), module_id, method_name.to_string());
        let payload_size = args.len() + identifier.2.len();
        let blueprint = object_info.blueprint.clone();

        // TODO: Can we load this lazily when needed?
        let instance_context = if object_info.global {
            match global_address {
                None => None,
                Some(address) => Some(InstanceContext {
                    instance: address,
                    instance_blueprint: object_info.blueprint.blueprint_name.clone(),
                }),
            }
        } else {
            match &object_info.outer_object {
                None => None,
                Some(blueprint_parent) => {
                    // TODO: do this recursively until global?
                    let parent_info = self.get_object_info(blueprint_parent.as_node_id()).unwrap();
                    Some(InstanceContext {
                        instance: blueprint_parent.clone(),
                        instance_blueprint: parent_info.blueprint.blueprint_name.clone(),
                    })
                }
            }
        };

        let invocation = KernelInvocation {
            resolved_actor: Actor::method(
                global_address,
                identifier.clone(),
                object_info,
                instance_context,
            ),
            sys_invocation: SystemInvocation {
                blueprint,
                ident: FnIdent::Application(identifier.2.clone()),
                receiver: Some(identifier),
            },
            args: IndexedScryptoValue::from_vec(args).map_err(|e| {
                RuntimeError::SystemUpstreamError(SystemUpstreamError::InputDecodeError(e))
            })?,
            payload_size,
        };

        self.api
            .kernel_invoke(Box::new(invocation))
            .map(|v| v.into())
    }

    #[trace_resources]
    fn get_object_info(&mut self, node_id: &NodeId) -> Result<ObjectInfo, RuntimeError> {
        let type_info = TypeInfoBlueprint::get_type(&node_id, self.api)?;
        let object_info = match type_info {
            TypeInfoSubstate::Object(info) => info,
            TypeInfoSubstate::KeyValueStore(..)
            | TypeInfoSubstate::SortedIndex
            | TypeInfoSubstate::Index => {
                return Err(RuntimeError::SystemError(SystemError::NotAnObject))
            }
        };

        Ok(object_info)
    }

    #[trace_resources]
    fn drop_object(&mut self, node_id: &NodeId) -> Result<Vec<Vec<u8>>, RuntimeError> {
        let info = self.get_object_info(node_id)?;
        if let Some(blueprint_parent) = info.outer_object {
            let actor = self.api.kernel_get_current_actor().unwrap();
            let instance_context = actor.instance_context();
            match instance_context {
                Some(instance_context) if instance_context.instance.eq(&blueprint_parent) => {}
                _ => {
                    return Err(RuntimeError::KernelError(
                        KernelError::InvalidDropNodeAccess(Box::new(InvalidDropNodeAccess {
                            node_id: node_id.clone(),
                            package_address: info.blueprint.package_address,
                            blueprint_name: info.blueprint.blueprint_name,
                        })),
                    ));
                }
            }
        }

        let mut node_substates = self.api.kernel_drop_node(node_id)?;
        let user_substates = node_substates.remove(&SysModuleId::Object.into()).unwrap();
        let fields = user_substates
            .into_iter()
            .map(|(_key, v)| v.into())
            .collect();

        Ok(fields)
    }
}

impl<'a, Y, V> ClientKeyValueStoreApi<RuntimeError> for SystemService<'a, Y, V>
where
    Y: KernelApi<SystemConfig<V>>,
    V: SystemCallbackObject,
{
    #[trace_resources]
    fn new_key_value_store(&mut self, schema: KeyValueStoreSchema) -> Result<NodeId, RuntimeError> {
        schema
            .schema
            .validate()
            .map_err(|e| RuntimeError::SystemError(SystemError::InvalidKeyValueStoreSchema(e)))?;

        let entity_type = EntityType::InternalKeyValueStore;
        let node_id = self.api.kernel_allocate_node_id(entity_type)?;

        self.api.kernel_create_node(
            node_id,
            btreemap!(
                SysModuleId::Virtualized.into() => btreemap!(),
                SysModuleId::TypeInfo.into() => ModuleInit::TypeInfo(
                    TypeInfoSubstate::KeyValueStore(schema)
                ).to_substates(),
            ),
        )?;

        Ok(node_id)
    }

    #[trace_resources]
    fn get_key_value_store_info(
        &mut self,
        node_id: &NodeId,
    ) -> Result<KeyValueStoreSchema, RuntimeError> {
        let type_info = TypeInfoBlueprint::get_type(node_id, self.api)?;
        let schema = match type_info {
            TypeInfoSubstate::Object { .. }
            | TypeInfoSubstate::SortedIndex
            | TypeInfoSubstate::Index => {
                return Err(RuntimeError::SystemError(SystemError::NotAKeyValueStore))
            }
            TypeInfoSubstate::KeyValueStore(schema) => schema,
        };

        Ok(schema)
    }

    fn lock_key_value_store_entry(
        &mut self,
        node_id: &NodeId,
        key: &Vec<u8>,
        flags: LockFlags,
    ) -> Result<LockHandle, RuntimeError> {
        let type_info = TypeInfoBlueprint::get_type(&node_id, self.api)?;
        if flags.contains(LockFlags::UNMODIFIED_BASE) || flags.contains(LockFlags::FORCE_WRITE) {
            return Err(RuntimeError::SystemError(SystemError::InvalidLockFlags));
        }

        let actor = self.api.kernel_get_current_actor().unwrap();

        let module_id = match type_info {
            TypeInfoSubstate::SortedIndex | TypeInfoSubstate::Index => {
                return Err(RuntimeError::SystemError(SystemError::NotAKeyValueStore))
            }
            TypeInfoSubstate::KeyValueStore(..) => SysModuleId::Virtualized,
            TypeInfoSubstate::Object(ObjectInfo { blueprint, .. }) => {
                if let Actor::Method { module_id, .. } = &actor {
                    match module_id {
                        ObjectModuleId::SELF => {
                            match (blueprint.package_address, blueprint.blueprint_name.as_str()) {
                                (METADATA_MODULE_PACKAGE, METADATA_BLUEPRINT) => {
                                    SysModuleId::Virtualized
                                }
                                _ => {
                                    return Err(RuntimeError::SystemError(
                                        SystemError::NotAKeyValueStore,
                                    ))
                                }
                            }
                        }
                        ObjectModuleId::Metadata => SysModuleId::Metadata,
                        ObjectModuleId::Royalty | ObjectModuleId::AccessRules => {
                            return Err(RuntimeError::SystemError(SystemError::NotAKeyValueStore))
                        }
                    }
                } else {
                    match (blueprint.package_address, blueprint.blueprint_name.as_str()) {
                        (METADATA_MODULE_PACKAGE, METADATA_BLUEPRINT) => SysModuleId::Virtualized,
                        _ => return Err(RuntimeError::SystemError(SystemError::NotAKeyValueStore)),
                    }
                }
            }
        };

        let substate_key = SubstateKey::Map(key.clone());

        self.api.kernel_lock_substate_with_default(
            &node_id,
            module_id.into(),
            &substate_key,
            flags,
            Some(|| IndexedScryptoValue::from_typed(&Option::<ScryptoValue>::None)),
        )
    }
}

impl<'a, Y, V> ClientIndexApi<RuntimeError> for SystemService<'a, Y, V>
where
    Y: KernelApi<SystemConfig<V>>,
    V: SystemCallbackObject,
{
    fn new_index(&mut self) -> Result<NodeId, RuntimeError> {
        let entity_type = EntityType::InternalIndex;
        let node_id = self.api.kernel_allocate_node_id(entity_type)?;

        self.api.kernel_create_node(
            node_id,
            btreemap!(
                SysModuleId::Object.into() => btreemap!(),
                SysModuleId::TypeInfo.into() => ModuleInit::TypeInfo(
                    TypeInfoSubstate::Index
                ).to_substates(),
            ),
        )?;

        Ok(node_id)
    }

    fn insert_into_index(
        &mut self,
        node_id: &NodeId,
        key: Vec<u8>,
        buffer: Vec<u8>,
    ) -> Result<(), RuntimeError> {
        let type_info = TypeInfoBlueprint::get_type(&node_id, self.api)?;
        match type_info {
            TypeInfoSubstate::Index => {}
            _ => {
                return Err(RuntimeError::SystemError(SystemError::NotAnIterableStore));
            }
        }

        let value = IndexedScryptoValue::from_vec(buffer).map_err(|e| {
            RuntimeError::SystemUpstreamError(SystemUpstreamError::InputDecodeError(e))
        })?;

        if !value.owned_node_ids().is_empty() {
            return Err(RuntimeError::SystemError(
                SystemError::CannotStoreOwnedInIterable,
            ));
        }

        let module_id = SysModuleId::Object.into();
        let substate_key = SubstateKey::Map(key);

        self.api
            .kernel_set_substate(node_id, module_id, substate_key, value)
    }

    fn remove_from_index(
        &mut self,
        node_id: &NodeId,
        key: Vec<u8>,
    ) -> Result<Option<Vec<u8>>, RuntimeError> {
        let type_info = TypeInfoBlueprint::get_type(&node_id, self.api)?;
        match type_info {
            TypeInfoSubstate::Index => {}
            _ => {
                return Err(RuntimeError::SystemError(SystemError::NotAnIterableStore));
            }
        }

        let module_id = SysModuleId::Object.into();
        let substate_key = SubstateKey::Map(key);

        let rtn = self
            .api
            .kernel_remove_substate(node_id, module_id, &substate_key)?
            .map(|v| v.into());

        Ok(rtn)
    }

    fn scan_index(&mut self, node_id: &NodeId, count: u32) -> Result<Vec<Vec<u8>>, RuntimeError> {
        let type_info = TypeInfoBlueprint::get_type(&node_id, self.api)?;
        match type_info {
            TypeInfoSubstate::Index => {}
            _ => {
                return Err(RuntimeError::SystemError(SystemError::NotAnIterableStore));
            }
        }

        let module_id = SysModuleId::Object;
        let substates = self
            .api
            .kernel_scan_substates(node_id, module_id, count)?
            .into_iter()
            .map(|value| value.into())
            .collect();

        Ok(substates)
    }

    fn take(&mut self, node_id: &NodeId, count: u32) -> Result<Vec<Vec<u8>>, RuntimeError> {
        let type_info = TypeInfoBlueprint::get_type(&node_id, self.api)?;
        match type_info {
            TypeInfoSubstate::Index => {}
            _ => {
                return Err(RuntimeError::SystemError(SystemError::NotAnIterableStore));
            }
        }

        let module_id = SysModuleId::Object;
        let substates = self
            .api
            .kernel_take_substates(node_id, module_id, count)?
            .into_iter()
            .map(|value| value.into())
            .collect();

        Ok(substates)
    }
}

impl<'a, Y, V> ClientSortedIndexApi<RuntimeError> for SystemService<'a, Y, V>
where
    Y: KernelApi<SystemConfig<V>>,
    V: SystemCallbackObject,
{
    #[trace_resources]
    fn new_sorted_index(&mut self) -> Result<NodeId, RuntimeError> {
        let entity_type = EntityType::InternalSortedIndex;
        let node_id = self.api.kernel_allocate_node_id(entity_type)?;

        self.api.kernel_create_node(
            node_id,
            btreemap!(
                SysModuleId::Object.into() => btreemap!(),
                SysModuleId::TypeInfo.into() => ModuleInit::TypeInfo(
                    TypeInfoSubstate::SortedIndex
                ).to_substates(),
            ),
        )?;

        Ok(node_id)
    }

    #[trace_resources]
    fn insert_into_sorted_index(
        &mut self,
        node_id: &NodeId,
        sorted_key: SortedKey,
        buffer: Vec<u8>,
    ) -> Result<(), RuntimeError> {
        let type_info = TypeInfoBlueprint::get_type(&node_id, self.api)?;
        match type_info {
            TypeInfoSubstate::SortedIndex => {}
            _ => {
                return Err(RuntimeError::SystemError(SystemError::NotASortedStore));
            }
        }

        let value = IndexedScryptoValue::from_vec(buffer).map_err(|e| {
            RuntimeError::SystemUpstreamError(SystemUpstreamError::InputDecodeError(e))
        })?;

        if !value.owned_node_ids().is_empty() {
            return Err(RuntimeError::SystemError(
                SystemError::CannotStoreOwnedInIterable,
            ));
        }

        let module_id = SysModuleId::Object.into();
        let substate_key = SubstateKey::Sorted((sorted_key.0, sorted_key.1));
        self.api
            .kernel_set_substate(node_id, module_id, substate_key, value)
    }

    #[trace_resources]
    fn scan_sorted_index(
        &mut self,
        node_id: &NodeId,
        count: u32,
    ) -> Result<Vec<Vec<u8>>, RuntimeError> {
        let type_info = TypeInfoBlueprint::get_type(&node_id, self.api)?;
        match type_info {
            TypeInfoSubstate::SortedIndex => {}
            _ => {
                return Err(RuntimeError::SystemError(SystemError::NotASortedStore));
            }
        }

        let substates = self
            .api
            .kernel_scan_sorted_substates(node_id, SysModuleId::Object.into(), count)?
            .into_iter()
            .map(|value| value.into())
            .collect();

        Ok(substates)
    }

    #[trace_resources]
    fn remove_from_sorted_index(
        &mut self,
        node_id: &NodeId,
        sorted_key: &SortedKey,
    ) -> Result<Option<Vec<u8>>, RuntimeError> {
        let type_info = TypeInfoBlueprint::get_type(&node_id, self.api)?;
        match type_info {
            TypeInfoSubstate::SortedIndex => {}
            _ => {
                return Err(RuntimeError::SystemError(SystemError::NotASortedStore));
            }
        }

        let module_id = SysModuleId::Object.into();
        let substate_key = SubstateKey::Sorted((sorted_key.0, sorted_key.1.clone()));

        let rtn = self
            .api
            .kernel_remove_substate(node_id, module_id, &substate_key)?
            .map(|v| v.into());

        Ok(rtn)
    }
}

impl<'a, Y, V> ClientBlueprintApi<RuntimeError> for SystemService<'a, Y, V>
where
    Y: KernelApi<SystemConfig<V>>,
    V: SystemCallbackObject,
{
    fn call_function(
        &mut self,
        package_address: PackageAddress,
        blueprint_name: &str,
        function_name: &str,
        args: Vec<u8>,
    ) -> Result<Vec<u8>, RuntimeError> {
        let identifier = FunctionIdentifier::new(
            Blueprint::new(&package_address, blueprint_name),
            function_name.to_string(),
        );
        let payload_size = args.len() + identifier.size();

        let invocation = KernelInvocation {
            resolved_actor: Actor::function(identifier.clone()),
            args: IndexedScryptoValue::from_vec(args).map_err(|e| {
                RuntimeError::SystemUpstreamError(SystemUpstreamError::InputDecodeError(e))
            })?,
            sys_invocation: SystemInvocation {
                blueprint: identifier.0,
                ident: FnIdent::Application(identifier.1),
                receiver: None,
            },
            payload_size,
        };

        self.api
            .kernel_invoke(Box::new(invocation))
            .map(|v| v.into())
    }
}

impl<'a, Y, V> ClientCostingApi<RuntimeError> for SystemService<'a, Y, V>
where
    Y: KernelApi<SystemConfig<V>>,
    V: SystemCallbackObject,
{
    #[trace_resources(log=units, log=reason)]
    fn consume_cost_units(
        &mut self,
        units: u32,
        reason: ClientCostingReason,
    ) -> Result<(), RuntimeError> {
        // No costing applied

        self.api
            .kernel_get_callback()
            .modules
            .costing
            .apply_execution_cost(
                match reason {
                    ClientCostingReason::RunWasm => CostingReason::RunWasm,
                    ClientCostingReason::RunNative => CostingReason::RunNative,
                    ClientCostingReason::RunSystem => CostingReason::RunSystem,
                },
                |_| units,
                5,
            )
    }

    #[trace_resources]
    fn credit_cost_units(
        &mut self,
        vault_id: NodeId,
        locked_fee: LiquidFungibleResource,
        contingent: bool,
    ) -> Result<LiquidFungibleResource, RuntimeError> {
        // No costing applied

        self.api
            .kernel_get_callback()
            .modules
            .costing
            .credit_cost_units(vault_id, locked_fee, contingent)
    }
}

impl<'a, Y, V> ClientActorApi<RuntimeError> for SystemService<'a, Y, V>
where
    Y: KernelApi<SystemConfig<V>>,
    V: SystemCallbackObject,
{
    #[trace_resources]
    fn lock_field(&mut self, field: u8, flags: LockFlags) -> Result<LockHandle, RuntimeError> {
        let actor = self.api.kernel_get_current_actor().unwrap();
        let (node_id, object_module_id, object_info) = match &actor {
            Actor::Function { .. } | Actor::VirtualLazyLoad { .. } => {
                return Err(RuntimeError::SystemError(SystemError::NotAMethod))
            }
            Actor::Method {
                node_id,
                module_id,
                object_info,
                ..
            } => (node_id, module_id, object_info),
        };

        // TODO: Remove
        if flags.contains(LockFlags::UNMODIFIED_BASE) || flags.contains(LockFlags::FORCE_WRITE) {
            if !(object_info.blueprint.package_address.eq(&RESOURCE_PACKAGE)
                && object_info
                    .blueprint
                    .blueprint_name
                    .eq(FUNGIBLE_VAULT_BLUEPRINT))
            {
                return Err(RuntimeError::SystemError(SystemError::InvalidLockFlags));
            }
        }

        let sys_module_id = match object_module_id {
            ObjectModuleId::SELF => {
                match (
                    object_info.blueprint.package_address,
                    object_info.blueprint.blueprint_name.as_str(),
                ) {
                    (METADATA_MODULE_PACKAGE, METADATA_BLUEPRINT) => {
                        return Err(RuntimeError::SystemError(SystemError::NotATuple));
                    }
                    _ => SysModuleId::Object,
                }
            }
            ObjectModuleId::Metadata => {
                return Err(RuntimeError::SystemError(SystemError::NotATuple));
            }
            ObjectModuleId::Royalty => SysModuleId::Royalty,
            ObjectModuleId::AccessRules => SysModuleId::AccessRules,
        };

        // TODO: Check if valid substate_key for node_id

        let substate_key = SubstateKey::Tuple(field);

        self.api
            .kernel_lock_substate(&node_id, sys_module_id.into(), &substate_key, flags)
    }

    #[trace_resources]
    fn lock_parent_field(
        &mut self,
        field: u8,
        flags: LockFlags,
    ) -> Result<LockHandle, RuntimeError> {
        let parent = self
            .get_info()?
            .outer_object
            .ok_or(RuntimeError::SystemError(SystemError::NoParent))?;

        // TODO: Check if valid substate_key for node_id
        self.api.kernel_lock_substate(
            parent.as_node_id(),
            SysModuleId::Object.into(),
            &SubstateKey::Tuple(field),
            flags,
        )
    }

    #[trace_resources]
    fn get_info(&mut self) -> Result<ObjectInfo, RuntimeError> {
        let actor = self.api.kernel_get_current_actor().unwrap();
        let object_info = match &actor {
            Actor::Function { .. } | Actor::VirtualLazyLoad { .. } => {
                return Err(RuntimeError::SystemError(SystemError::NotAMethod))
            }
            Actor::Method { object_info, .. } => object_info.clone(),
        };

        Ok(object_info)
    }

    #[trace_resources]
    fn get_global_address(&mut self) -> Result<GlobalAddress, RuntimeError> {
        let actor = self.api.kernel_get_current_actor().unwrap();
        match actor {
            Actor::Method {
                global_address: Some(address),
                ..
            } => Ok(address),
            _ => Err(RuntimeError::SystemError(
                SystemError::GlobalAddressDoesNotExist,
            )),
        }
    }

    fn get_blueprint(&mut self) -> Result<Blueprint, RuntimeError> {
        self.consume_cost_units(FIXED_LOW_FEE, ClientCostingReason::RunSystem)?;

        let actor = self.api.kernel_get_current_actor().unwrap();
        Ok(actor.blueprint().clone())
    }
}

impl<'a, Y, V> ClientAuthApi<RuntimeError> for SystemService<'a, Y, V>
where
    Y: KernelApi<SystemConfig<V>>,
    V: SystemCallbackObject,
{
    #[trace_resources]
    fn get_auth_zone(&mut self) -> Result<NodeId, RuntimeError> {
        self.consume_cost_units(FIXED_LOW_FEE, ClientCostingReason::RunSystem)?;

        let auth_zone_id = self.api.kernel_get_callback().modules.auth.last_auth_zone();

        Ok(auth_zone_id.into())
    }

    #[trace_resources]
    fn assert_access_rule(&mut self, rule: AccessRule) -> Result<(), RuntimeError> {
        self.consume_cost_units(FIXED_LOW_FEE, ClientCostingReason::RunSystem)?;

        // Decide `authorization`, `barrier_crossing_allowed`, and `tip_auth_zone_id`
        let authorization = convert_contextless(&rule);
        let barrier_crossings_required = 1;
        let barrier_crossings_allowed = 1;
        let auth_zone_id = self.api.kernel_get_callback().modules.auth.last_auth_zone();

        // Authenticate
        if !Authentication::verify_method_auth(
            barrier_crossings_required,
            barrier_crossings_allowed,
            auth_zone_id,
            &authorization,
            self,
        )? {
            return Err(RuntimeError::SystemError(
                SystemError::AssertAccessRuleFailed,
            ));
        }

        Ok(())
    }
}

impl<'a, Y, V> ClientTransactionLimitsApi<RuntimeError> for SystemService<'a, Y, V>
where
    Y: KernelApi<SystemConfig<V>>,
    V: SystemCallbackObject,
{
    #[trace_resources]
    fn update_wasm_memory_usage(&mut self, consumed_memory: usize) -> Result<(), RuntimeError> {
        // No costing applied

        let current_depth = self.api.kernel_get_current_depth();
        self.api
            .kernel_get_callback()
            .modules
            .transaction_limits
            .update_wasm_memory_usage(current_depth, consumed_memory)
    }
}

impl<'a, Y, V> ClientExecutionTraceApi<RuntimeError> for SystemService<'a, Y, V>
where
    Y: KernelApi<SystemConfig<V>>,
    V: SystemCallbackObject,
{
    #[trace_resources]
    fn update_instruction_index(&mut self, new_index: usize) -> Result<(), RuntimeError> {
        // No costing applied

        self.api
            .kernel_get_callback()
            .modules
            .execution_trace
            .update_instruction_index(new_index);
        Ok(())
    }
}

impl<'a, Y, V> ClientEventApi<RuntimeError> for SystemService<'a, Y, V>
where
    Y: KernelApi<SystemConfig<V>>,
    V: SystemCallbackObject,
{
    #[trace_resources]
    fn emit_event(&mut self, event_name: String, event_data: Vec<u8>) -> Result<(), RuntimeError> {
        // Costing event emission.
        self.consume_cost_units(FIXED_LOW_FEE, ClientCostingReason::RunSystem)?;

        let actor = self.api.kernel_get_current_actor();

        // Locking the package info substate associated with the emitter's package
        let (blueprint_schema, local_type_index) = {
            // Getting the package address and blueprint name associated with the actor
            let blueprint = match actor {
                Some(Actor::Method {
                    node_id, module_id, ..
                }) => match module_id {
                    ObjectModuleId::AccessRules => Ok(Blueprint::new(
                        &ACCESS_RULES_MODULE_PACKAGE,
                        ACCESS_RULES_BLUEPRINT,
                    )),
                    ObjectModuleId::Royalty => Ok(Blueprint::new(
                        &ROYALTY_MODULE_PACKAGE,
                        COMPONENT_ROYALTY_BLUEPRINT,
                    )),
                    ObjectModuleId::Metadata => {
                        Ok(Blueprint::new(&METADATA_MODULE_PACKAGE, METADATA_BLUEPRINT))
                    }
                    ObjectModuleId::SELF => self.get_object_info(&node_id).map(|x| x.blueprint),
                },
                Some(Actor::Function { ref blueprint, .. }) => Ok(blueprint.clone()),
                _ => Err(RuntimeError::ApplicationError(
                    ApplicationError::EventError(Box::new(EventError::InvalidActor)),
                )),
            }?;

            let blueprint_schema = self.get_blueprint_schema(&blueprint)?;

            // Translating the event name to it's local_type_index which is stored in the blueprint
            // schema
            let local_type_index =
                if let Some(index) = blueprint_schema.event_schema.get(&event_name).cloned() {
                    index
                } else {
                    return Err(RuntimeError::ApplicationError(
                        ApplicationError::EventError(Box::new(EventError::SchemaNotFoundError {
                            blueprint: blueprint.clone(),
                            event_name,
                        })),
                    ));
                };

            (blueprint_schema, local_type_index)
        };

        // Construct the event type identifier based on the current actor
        let event_type_identifier = match actor {
            Some(Actor::Method {
                node_id, module_id, ..
            }) => Ok(EventTypeIdentifier(
                Emitter::Method(node_id, module_id),
                local_type_index,
            )),
            Some(Actor::Function { ref blueprint, .. }) => Ok(EventTypeIdentifier(
                Emitter::Function(
                    blueprint.package_address.into(),
                    ObjectModuleId::SELF,
                    blueprint.blueprint_name.to_string(),
                ),
                local_type_index,
            )),
            _ => Err(RuntimeError::ApplicationError(
                ApplicationError::EventError(Box::new(EventError::InvalidActor)),
            )),
        }?;

        // Validating the event data against the event schema
        validate_payload_against_schema(
            &event_data,
            &blueprint_schema.schema,
            event_type_identifier.1,
            self,
        )
        .map_err(|err| {
            RuntimeError::ApplicationError(ApplicationError::EventError(Box::new(
                EventError::EventSchemaNotMatch(err.error_message(&blueprint_schema.schema)),
            )))
        })?;

        // Adding the event to the event store
        self.api
            .kernel_get_callback()
            .modules
            .events
            .add_event(event_type_identifier, event_data);

        Ok(())
    }
}

impl<'a, Y, V> ClientLoggerApi<RuntimeError> for SystemService<'a, Y, V>
where
    Y: KernelApi<SystemConfig<V>>,
    V: SystemCallbackObject,
{
    fn log_message(&mut self, level: Level, message: String) -> Result<(), RuntimeError> {
        self.consume_cost_units(FIXED_LOW_FEE, ClientCostingReason::RunSystem)?;

        self.api
            .kernel_get_callback()
            .modules
            .logger
            .add_log(level, message);
        Ok(())
    }
}

impl<'a, Y, V> ClientTransactionRuntimeApi<RuntimeError> for SystemService<'a, Y, V>
where
    Y: KernelApi<SystemConfig<V>>,
    V: SystemCallbackObject,
{
    #[trace_resources]
    fn get_transaction_hash(&mut self) -> Result<Hash, RuntimeError> {
        self.consume_cost_units(FIXED_LOW_FEE, ClientCostingReason::RunSystem)?;

        Ok(self
            .api
            .kernel_get_callback()
            .modules
            .transaction_runtime
            .transaction_hash())
    }

    #[trace_resources]
    fn generate_uuid(&mut self) -> Result<u128, RuntimeError> {
        self.consume_cost_units(FIXED_LOW_FEE, ClientCostingReason::RunSystem)?;

        Ok(self
            .api
            .kernel_get_callback()
            .modules
            .transaction_runtime
            .generate_uuid())
    }
}

impl<'a, Y, V> ClientApi<RuntimeError> for SystemService<'a, Y, V>
where
    Y: KernelApi<SystemConfig<V>>,
    V: SystemCallbackObject,
{
}

impl<'a, Y, V> KernelNodeApi for SystemService<'a, Y, V>
where
    Y: KernelApi<SystemConfig<V>>,
    V: SystemCallbackObject,
{
    fn kernel_drop_node(&mut self, node_id: &NodeId) -> Result<NodeSubstates, RuntimeError> {
        self.api.kernel_drop_node(node_id)
    }

    fn kernel_allocate_virtual_node_id(&mut self, node_id: NodeId) -> Result<(), RuntimeError> {
        self.api.kernel_allocate_virtual_node_id(node_id)
    }

    fn kernel_allocate_node_id(&mut self, node_type: EntityType) -> Result<NodeId, RuntimeError> {
        self.api.kernel_allocate_node_id(node_type)
    }

    fn kernel_create_node(
        &mut self,
        node_id: NodeId,
        node_substates: NodeSubstates,
    ) -> Result<(), RuntimeError> {
        self.api.kernel_create_node(node_id, node_substates)
    }
}

impl<'a, Y, V> KernelSubstateApi for SystemService<'a, Y, V>
where
    Y: KernelApi<SystemConfig<V>>,
    V: SystemCallbackObject,
{
    fn kernel_lock_substate_with_default(
        &mut self,
        node_id: &NodeId,
        module_id: ModuleId,
        substate_key: &SubstateKey,
        flags: LockFlags,
        default: Option<fn() -> IndexedScryptoValue>,
    ) -> Result<LockHandle, RuntimeError> {
        self.api
            .kernel_lock_substate_with_default(node_id, module_id, substate_key, flags, default)
    }

    fn kernel_get_lock_info(&mut self, lock_handle: LockHandle) -> Result<LockInfo, RuntimeError> {
        self.api.kernel_get_lock_info(lock_handle)
    }

    fn kernel_drop_lock(&mut self, lock_handle: LockHandle) -> Result<(), RuntimeError> {
        self.api.kernel_drop_lock(lock_handle)
    }

    fn kernel_read_substate(
        &mut self,
        lock_handle: LockHandle,
    ) -> Result<&IndexedScryptoValue, RuntimeError> {
        self.api.kernel_read_substate(lock_handle)
    }

    fn kernel_write_substate(
        &mut self,
        lock_handle: LockHandle,
        value: IndexedScryptoValue,
    ) -> Result<(), RuntimeError> {
        self.api.kernel_write_substate(lock_handle, value)
    }

    fn kernel_set_substate(
        &mut self,
        node_id: &NodeId,
        module_id: ModuleId,
        substate_key: SubstateKey,
        value: IndexedScryptoValue,
    ) -> Result<(), RuntimeError> {
        self.api
            .kernel_set_substate(node_id, module_id, substate_key, value)
    }

    fn kernel_remove_substate(
        &mut self,
        node_id: &NodeId,
        module_id: ModuleId,
        substate_key: &SubstateKey,
    ) -> Result<Option<IndexedScryptoValue>, RuntimeError> {
        self.api
            .kernel_remove_substate(node_id, module_id, substate_key)
    }

    fn kernel_scan_sorted_substates(
        &mut self,
        node_id: &NodeId,
        module_id: ModuleId,
        count: u32,
    ) -> Result<Vec<IndexedScryptoValue>, RuntimeError> {
        self.api
            .kernel_scan_sorted_substates(node_id, module_id, count)
    }

    fn kernel_scan_substates(
        &mut self,
        node_id: &NodeId,
        module_id: SysModuleId,
        count: u32,
    ) -> Result<Vec<IndexedScryptoValue>, RuntimeError> {
        self.api.kernel_scan_substates(node_id, module_id, count)
    }

    fn kernel_take_substates(
        &mut self,
        node_id: &NodeId,
        module_id: SysModuleId,
        count: u32,
    ) -> Result<Vec<IndexedScryptoValue>, RuntimeError> {
        self.api.kernel_take_substates(node_id, module_id, count)
    }
}

impl<'a, Y, V> KernelInternalApi<SystemConfig<V>> for SystemService<'a, Y, V>
where
    Y: KernelApi<SystemConfig<V>>,
    V: SystemCallbackObject,
{
    fn kernel_get_callback(&mut self) -> &mut SystemConfig<V> {
        self.api.kernel_get_callback()
    }

    fn kernel_get_current_actor(&mut self) -> Option<Actor> {
        self.api.kernel_get_current_actor()
    }

    fn kernel_get_current_depth(&self) -> usize {
        self.api.kernel_get_current_depth()
    }

    fn kernel_get_node_info(&self, node_id: &NodeId) -> Option<(RefType, bool)> {
        self.api.kernel_get_node_info(node_id)
    }

    fn kernel_read_bucket(&mut self, bucket_id: &NodeId) -> Option<BucketSnapshot> {
        self.api.kernel_read_bucket(bucket_id)
    }

    fn kernel_read_proof(&mut self, proof_id: &NodeId) -> Option<ProofSnapshot> {
        self.api.kernel_read_proof(proof_id)
    }
}
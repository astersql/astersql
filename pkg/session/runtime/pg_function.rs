// Copyright 2026 AsterSQL.
//! Function EXECUTE over the existing standard-object record and canonical object ACL engine.
use super::pg_database::{PgDatabaseFact, PgDatabaseGrant, PgRoleFact, update_object_acl};
use super::pg_schema::{PG_NAMESPACE_DDL_LOCK, namespace_error};
use super::*;
use astersql_meta_model::PostgreSQLRelationAcl;
use std::collections::BTreeMap;

pub fn pg_function_privileges(text: &str) -> SessionResult<Vec<(u8, bool)>> {
    text.split(',')
        .map(|s| {
            let s = s.trim().to_ascii_uppercase();
            let (base, option) = s
                .strip_suffix(" WITH GRANT OPTION")
                .map_or((s.as_str(), false), |s| (s, true));
            if base != "EXECUTE" {
                return Err(namespace_error(
                    "22023",
                    "unrecognized function privilege type",
                ));
            }
            Ok((1, option))
        })
        .collect()
}
fn default_acl(owner: i64) -> PostgreSQLRelationAcl {
    PostgreSQLRelationAcl {
        default_acl: true,
        grants: vec![
            PgDatabaseGrant {
                grantor: owner,
                grantee: owner,
                privileges: 1,
                grant_options: 0,
            },
            PgDatabaseGrant {
                grantor: owner,
                grantee: 0,
                privileges: 1,
                grant_options: 0,
            },
        ],
    }
}
/// The catalog and ordinary expressions consume the same committed snapshot.
pub fn pg_function_facts(
    namespaces: &[PgNamespaceFact],
    roles: &[PgRoleFact],
) -> Vec<(String, String, PgDatabaseFact)> {
    let Some(s) = namespaces.iter().find_map(|n| n.standard.as_ref()) else {
        return vec![];
    };
    let names = Arc::new(
        roles
            .iter()
            .map(|r| (r.oid, r.name.clone()))
            .collect::<BTreeMap<_, _>>(),
    );
    let mut types = PG_INTROSPECTION_FUNCTION_IDENTITIES
        .iter()
        .map(|(id, name)| {
            (
                "pg_catalog".to_string(),
                name.to_string(),
                *id,
                s.function_owners.get(id).copied(),
            )
        })
        .collect::<Vec<_>>();
    types.extend(
        pg_extension_declarations(namespaces)
            .into_iter()
            .map(|f| (f.namespace_name, f.name, f.oid, Some(f.owner))),
    );
    types
        .into_iter()
        .map(|(schema, name, id, owner)| {
            let acl = s.function_acls.get(&id).cloned().or_else(|| {
                if s.function_acl_initialized || s.extension_functions.contains_key(&id) {
                    owner.map(default_acl)
                } else {
                    None
                }
            });
            (
                schema.into(),
                name,
                PgDatabaseFact {
                    id,
                    owner: owner.and_then(|id| names.get(&id).map(|n| (id, n.clone()))),
                    can_connect: false,
                    acl: acl.as_ref().map(|a| a.grants.clone()),
                    acl_default: acl.is_some_and(|a| a.default_acl),
                    warning: None,
                    is_template: None,
                    allow_connections: None,
                    roles: names.clone(),
                },
            )
        })
        .collect()
}
pub fn pg_function_acl_text(fact: &PgDatabaseFact) -> SessionResult<Option<String>> {
    fact.acl_text_with_modes(&[(1, 'X')])
}
pub fn pg_has_function_privilege(
    types: &[(String, String, PgDatabaseFact)],
    path: &[String],
    namespaces: &[PgNamespaceFact],
    roles: &[PgRoleFact],
    actor: &str,
    values: &[Option<String>],
    oids: &[bool],
) -> SessionResult<Option<bool>> {
    if values.iter().any(Option::is_none) {
        return Ok(None);
    }
    let offset = usize::from(values.len() == 3);
    let user = if offset == 0 {
        actor
    } else {
        values[0].as_deref().unwrap()
    };
    let member = if offset == 1 && oids[0] {
        user.parse::<u32>()
            .map(i64::from)
            .map_err(|_| namespace_error("22P02", "invalid role OID"))?
    } else if user == "public" {
        0
    } else {
        roles
            .iter()
            .find(|r| r.name == user)
            .ok_or_else(|| namespace_error("42704", "role does not exist"))?
            .oid
    };
    let object = values[offset].as_deref().unwrap();
    let fact = if oids[offset] {
        let oid = object
            .parse::<u32>()
            .map(i64::from)
            .map_err(|_| namespace_error("22P02", "invalid function OID"))?;
        types.iter().find(|(_, _, f)| f.id == oid)
    } else {
        let (parts, inputs) = function_signature_parts(object)?;
        let matching =
            |f: &&(String, String, PgDatabaseFact)| function_inputs(namespaces, f.2.id) == inputs;
        let mut search = path.to_vec();
        if !search.iter().any(|s| s == "pg_catalog") {
            search.insert(0, "pg_catalog".into());
        }
        let found = match parts.as_slice() {
            [name] => search.iter().find_map(|ns| {
                types
                    .iter()
                    .filter(matching)
                    .find(|(s, n, _)| s == ns && n == name)
            }),
            [ns, name] => {
                let namespace = namespaces
                    .iter()
                    .find(|f| f.name == *ns)
                    .ok_or_else(|| namespace_error("3F000", "schema does not exist"))?;
                if !namespace.usage {
                    return Err(namespace_error("42501", "permission denied for schema"));
                }
                types
                    .iter()
                    .filter(matching)
                    .find(|(s, n, _)| s == ns && n == name)
            }
            _ => {
                return Err(namespace_error(
                    "0A000",
                    "cross-database function references are unsupported",
                ));
            }
        };
        Some(found.ok_or_else(|| namespace_error("42883", "function does not exist"))?)
    };
    let modes = pg_function_privileges(values[offset + 1].as_deref().unwrap())?;
    let Some((_, _, fact)) = fact else {
        return Ok(None);
    };
    let mut allowed = false;
    for (bit, option) in modes {
        allowed |= fact.has_privilege_mask(roles, member, bit, option, 1)?;
    }
    Ok(Some(allowed))
}
impl ConcreteSession {
    pub(super) fn pg_function_privilege_command(
        &self,
        command: &PgSchemaCommand,
    ) -> SessionResult<Option<(&'static str, String)>> {
        let PgSchemaCommand::FunctionPrivileges {
            schema,
            name,
            arguments,
            grantee,
            public,
            privileges,
            grant,
            grant_option,
            cascade,
        } = command
        else {
            unreachable!()
        };
        let _guard = PG_NAMESPACE_DDL_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let namespaces = self.pg_namespace_facts()?;
        let actor = self.pg_schema_identity();
        let roles = if actor == "root" {
            self.pg_namespace_role_identities()?
        } else {
            self.pg_role_facts()?
        };
        let types = pg_function_facts(&namespaces, &roles)
            .into_iter()
            .filter(|(_, _, f)| function_inputs(&namespaces, f.id) == *arguments)
            .collect::<Vec<_>>();
        let mut path = self.pg_effective_path()?;
        if !path.iter().any(|s| s == "pg_catalog") {
            path.insert(0, "pg_catalog".into());
        }
        let (_, _, fact) = if let Some(schema) = schema {
            types.iter().find(|(s, n, _)| s == schema && n == name)
        } else {
            path.iter()
                .find_map(|s| types.iter().find(|(ns, n, _)| ns == s && n == name))
        }
        .ok_or_else(|| namespace_error("42883", "function does not exist"))?;
        let owner = fact
            .owner
            .as_ref()
            .ok_or_else(|| namespace_error("0A000", "function owner is unavailable"))?
            .0;
        fact.validate_acl_mask(1)?;
        let actor_id = roles
            .iter()
            .find(|r| r.name == actor)
            .ok_or_else(|| namespace_error("0A000", "actor identity unavailable"))?
            .oid;
        let grantee = if *public {
            0
        } else {
            roles
                .iter()
                .find(|r| r.name == *grantee)
                .ok_or_else(|| namespace_error("42704", "role does not exist"))?
                .oid
        };
        if *public && *grant && *grant_option {
            return Err(namespace_error(
                "0LP01",
                "grant options cannot be granted to PUBLIC",
            ));
        }
        let all = privileges == "ALL";
        if !all {
            pg_function_privileges(privileges)?;
        }
        let database = self
            .domain
            .info_schema()
            .SchemaByName(&astersql_infoschema::CiString::new(self.current_database()))
            .ok_or_else(|| namespace_error("3D000", "no current database selected"))?;
        let context = kv::WithInternalSourceType(kv::Context::default(), kv::InternalTxnDDL);
        let mut warning = None;
        self.domain
            .storage_handle()
            .with_storage(|store| {
                kv::RunInNewTxn(&context, store, true, |_, txn| {
                    let key = astersql_meta::transaction_meta_string_key(b"PgStandardSchema");
                    let value = txn.Get(&context, key.clone(), &[])?;
                    let mut standard: PgStandardSchema = serde_json::from_slice(&value.Value)
                        .map_err(|e| kv::errors::New(e.to_string()))?;
                    let current = standard
                        .function_acl_overrides
                        .get(&database.id)
                        .and_then(|m| m.get(&fact.id))
                        .cloned()
                        .or_else(|| standard.function_acls.get(&fact.id).cloned())
                        .unwrap_or_else(|| default_acl(owner));
                    let mut acl = current;
                    let (notice, modified) = update_object_acl(
                        &mut acl.grants,
                        owner,
                        &actor,
                        actor_id,
                        &roles,
                        1,
                        grantee,
                        *grant,
                        *grant_option,
                        *cascade,
                        all,
                    )?;
                    warning = notice;
                    if modified {
                        acl.default_acl = false;
                        standard
                            .function_acl_overrides
                            .entry(database.id)
                            .or_default()
                            .insert(fact.id, acl);
                        txn.Set(
                            key,
                            serde_json::to_vec(&standard)
                                .map_err(|e| kv::errors::New(e.to_string()))?,
                        )?;
                    }
                    Ok(())
                })
            })
            .map_err(|e| {
                let text = e.to_string();
                if text.contains("dependent database privileges") {
                    namespace_error("2BP01", "dependent privileges exist")
                } else if text.contains("requires grant option") {
                    namespace_error("42501", "permission denied for function")
                } else if text.contains("ACL") || text.contains("membership unknown") {
                    namespace_error("0A000", "function ACL authority is unavailable")
                } else {
                    session_error("persist function ACL", e)
                }
            })?;
        Ok(warning)
    }
}

// Role deletion already checks database-scoped dependencies in this same transaction.
pub(super) fn function_role_dependency(
    txn: &mut dyn kv::Transaction,
    context: &kv::Context,
    database: i64,
    role: i64,
) -> Result<(), kv::errors::SharedError> {
    let key = astersql_meta::transaction_meta_string_key(b"PgStandardSchema");
    let bytes = match txn.Get(context, key, &[]) {
        Ok(v) => v.Value,
        Err(e) if kv::IsErrNotFound(&e) => return Ok(()),
        Err(e) => return Err(e),
    };
    let s: PgStandardSchema =
        serde_json::from_slice(&bytes).map_err(|e| kv::errors::New(e.to_string()))?;
    if s.extension_functions
        .values()
        .any(|f| f.database == database && f.owner == role)
    {
        return Err(kv::errors::New("role has function dependencies"));
    }
    let mut owners = s.function_owners;
    if let Some(overrides) = s.function_owner_overrides.get(&database) {
        owners.extend(overrides);
    }
    let mut acls = s.function_acls;
    if let Some(overrides) = s.function_acl_overrides.get(&database) {
        acls.extend(overrides.clone());
    }
    if owners.values().any(|id| *id == role)
        || acls.values().any(|a| {
            a.grants
                .iter()
                .any(|g| g.grantor == role || g.grantee == role)
        })
    {
        return Err(kv::errors::New("role has function dependencies"));
    }
    Ok(())
}
/// ALTER OWNER preserves explicit ACLs, replacing old-owner identities and merging duplicates.
pub(super) fn transfer_function_acl(
    s: &mut PgStandardSchema,
    database: i64,
    oid: i64,
    old: Option<i64>,
    new: i64,
) {
    let Some(old) = old else {
        return;
    };
    let Some(mut acl) = s
        .function_acl_overrides
        .get(&database)
        .and_then(|m| m.get(&oid))
        .or_else(|| s.function_acls.get(&oid))
        .cloned()
    else {
        return;
    };
    let mut merged = BTreeMap::<(i64, i64), (u8, u8)>::new();
    for g in acl.grants {
        let key = (
            if g.grantor == old { new } else { g.grantor },
            if g.grantee == old { new } else { g.grantee },
        );
        let entry = merged.entry(key).or_default();
        entry.0 |= g.privileges;
        entry.1 |= g.grant_options;
    }
    acl.grants = merged
        .into_iter()
        .map(
            |((grantor, grantee), (privileges, grant_options))| PgDatabaseGrant {
                grantor,
                grantee,
                privileges,
                grant_options,
            },
        )
        .collect();
    s.function_acl_overrides
        .entry(database)
        .or_default()
        .insert(oid, acl);
}

/// Resolve the complete registered input signature, including qualified/quoted identifiers.
/// These primitives each have one oid input; other overloads must never match by name alone.
pub fn function_signature_parts(signature: &str) -> SessionResult<(Vec<String>, Vec<u32>)> {
    let open = signature
        .find('(')
        .ok_or_else(|| namespace_error("22P02", "expected function input signature"))?;
    let input = signature[open + 1..]
        .strip_suffix(')')
        .ok_or_else(|| namespace_error("22P02", "invalid function signature"))?;
    let mut inputs = Vec::new();
    if !input.trim().is_empty() {
        for text in input.split(',') {
            let parts = super::pg_table::table_name_parts(text.trim())
                .map_err(|_| namespace_error("22P02", "invalid function signature"))?;
            let name = match parts.as_slice() {
                [name] => name,
                [schema, name] if schema == "pg_catalog" => name,
                _ => return Err(namespace_error("42883", "function does not exist")),
            };
            inputs.push(match name.as_str() {
                "oid" => 26,
                "integer" | "int4" | "int" => 23,
                "bigint" | "int8" => 20,
                "text" => 25,
                _ => return Err(namespace_error("42883", "function does not exist")),
            });
        }
    }
    Ok((
        super::pg_table::table_name_parts(signature[..open].trim())?,
        inputs,
    ))
}
fn function_inputs(namespaces: &[PgNamespaceFact], oid: i64) -> Vec<u32> {
    pg_extension_declarations(namespaces)
        .into_iter()
        .find(|f| f.oid == oid)
        .map_or_else(|| vec![26], |f| f.arguments)
}

/// Current declarations are read from the same standard-object snapshot as owner/ACL facts.
pub fn pg_extension_declarations(
    namespaces: &[PgNamespaceFact],
) -> Vec<astersql_expression::PgExtensionDeclaration> {
    namespaces
        .iter()
        .find_map(|n| n.standard.as_ref())
        .map(|s| {
            s.extension_functions
                .values()
                .filter_map(|f| {
                    namespaces.iter().find(|n| n.id == f.namespace).map(|n| {
                        let mut f = f.clone();
                        f.namespace_name = n.name.clone();
                        f
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

impl ConcreteSession {
    fn pg_extension_scope(&self) -> usize {
        pg_extension_scope(&self.domain)
    }
    /// Register a real extension implementation in an existing namespace. No SQL body or alias is created.
    pub fn pg_register_extension_function(
        &self,
        schema: &str,
        arguments: Vec<u32>,
        result: u32,
        implementation: &str,
        definition: Arc<astersql_extension::FunctionDef>,
    ) -> SessionResult<i64> {
        if self.sql_dialect() != ast::SqlDialect::PostgreSQL {
            return Err(namespace_error(
                "0A000",
                "PG extension registration requires PostgreSQL dialect",
            ));
        }
        let _guard = PG_NAMESPACE_DDL_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.state.borrow().transaction.is_some() {
            return Err(namespace_error(
                "0A000",
                "extension registration inside a transaction is unsupported",
            ));
        }
        let temporary_namespace = self
            .state
            .borrow()
            .pg_temporary_namespaces
            .get(&self.current_database())
            .map(|(id, _)| *id);
        let namespaces = self.pg_namespace_facts()?;
        let ns = namespaces
            .iter()
            .find(|n| {
                n.name == schema || (schema == "pg_temp" && temporary_namespace == Some(n.id))
            })
            .ok_or_else(|| namespace_error("3F000", "schema does not exist"))?;
        if !ns.can_create {
            return Err(namespace_error("42501", "permission denied for schema"));
        }
        let database = self
            .domain
            .info_schema()
            .SchemaByName(&astersql_infoschema::CiString::new(self.current_database()))
            .ok_or_else(|| namespace_error("3D000", "no current database"))?
            .id;
        let owner = self.pg_database_role_id(&self.pg_schema_identity())?;
        if definition.Name.len() > 63 || definition.Name.contains('\0') {
            return Err(namespace_error("42602", "invalid PG function name"));
        }
        if arguments.len() > 100 || result == 26 {
            return Err(namespace_error(
                "0A000",
                "unsupported PG extension signature",
            ));
        }
        if astersql_expression::formal_registry::funcs.contains_key(&definition.Name) {
            return Err(namespace_error(
                "0A000",
                "extension name conflicts with a builtin",
            ));
        }
        if PG_INTROSPECTION_FUNCTION_IDENTITIES
            .iter()
            .any(|(_, name)| *name == definition.Name)
        {
            return Err(namespace_error("42723", "function already exists"));
        }
        if definition.Name == "pg_table_is_visible" && ns.name != "pg_catalog" {
            return Err(namespace_error(
                "0A000",
                "adapter function shadowing requires a shared native signature",
            ));
        }
        let mut declaration = astersql_expression::PgExtensionDeclaration {
            oid: 0,
            database,
            namespace: ns.id,
            namespace_name: ns.name.clone(),
            name: definition.Name.clone(),
            arguments,
            result,
            owner,
            implementation: implementation.into(),
            temporary: temporary_namespace == Some(ns.id),
        };
        if declaration.name.is_empty() || implementation.is_empty() {
            return Err(namespace_error(
                "22023",
                "extension identity must not be empty",
            ));
        }
        astersql_expression::validate_pg_extension(&declaration, definition.clone())
            .map_err(|e| session_error("validate PG extension", e))?;
        if let Some(existing) = pg_extension_declarations(&namespaces)
            .into_iter()
            .find(|f| {
                f.namespace == ns.id
                    && f.name == declaration.name
                    && f.arguments == declaration.arguments
            })
        {
            if existing.implementation == declaration.implementation
                && existing.result == declaration.result
                && !astersql_expression::pg_extension_is_linked(
                    self.pg_extension_scope(),
                    existing.oid,
                )
            {
                if existing.owner != owner && self.pg_schema_identity() != "root" {
                    return Err(namespace_error("42501", "must be owner of function"));
                }
                astersql_expression::register_pg_extension(
                    self.pg_extension_scope(),
                    existing.clone(),
                    definition,
                )
                .map_err(|e| session_error("relink PG extension", e))?;
                return Ok(existing.oid);
            }
            return Err(namespace_error("42723", "function already exists"));
        }
        let context = kv::WithInternalSourceType(kv::Context::default(), kv::InternalTxnDDL);
        self.domain
            .storage_handle()
            .with_storage(|store| {
                kv::RunInNewTxn(&context, store, true, |_, txn| {
                    let id = super::pg_schema::allocate_namespace_id(txn, &context)?;
                    declaration.oid = id
                        .checked_mul(2)
                        .and_then(|id| id.checked_add(16384))
                        .filter(|id| *id < 0x4000_0000)
                        .ok_or_else(|| kv::errors::New("function OID exhausted"))?;
                    if !declaration.temporary {
                        let key = astersql_meta::transaction_meta_string_key(b"PgStandardSchema");
                        let value = txn.Get(&context, key.clone(), &[])?;
                        let mut standard: PgStandardSchema =
                            serde_json::from_slice(&value.Value)
                                .map_err(|e| kv::errors::New(e.to_string()))?;
                        if standard.extension_functions.values().any(|f| {
                            f.database == database
                                && f.namespace == ns.id
                                && f.name == declaration.name
                                && f.arguments == declaration.arguments
                        }) {
                            return Err(kv::errors::New("function already exists"));
                        }
                        standard
                            .extension_functions
                            .insert(declaration.oid, declaration.clone());
                        txn.Set(
                            key,
                            serde_json::to_vec(&standard)
                                .map_err(|e| kv::errors::New(e.to_string()))?,
                        )?;
                    }
                    Ok(())
                })
            })
            .map_err(|e| session_error("publish PG extension", e))?;
        // Publication holds the same registry/metadata DDL boundary; readers with
        // a missing implementation refuse instead of executing another object.
        astersql_expression::register_pg_extension(
            self.pg_extension_scope(),
            declaration.clone(),
            definition,
        )
        .map_err(|e| session_error("link PG extension", e))?;
        if declaration.temporary {
            self.state
                .borrow_mut()
                .pg_extension_functions
                .insert(declaration.oid, declaration.clone());
        }
        Ok(declaration.oid)
    }
    pub fn pg_unregister_extension_function(&self, oid: i64) -> SessionResult<()> {
        if self.sql_dialect() != ast::SqlDialect::PostgreSQL {
            return Err(namespace_error(
                "0A000",
                "PG extension unregistration requires PostgreSQL dialect",
            ));
        }
        if self.state.borrow().transaction.is_some() {
            return Err(namespace_error(
                "0A000",
                "extension unregistration inside a transaction is unsupported",
            ));
        }
        let _guard = PG_NAMESPACE_DDL_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let namespaces = self.pg_namespace_facts()?;
        let declaration = pg_extension_declarations(&namespaces)
            .into_iter()
            .find(|f| f.oid == oid)
            .ok_or_else(|| namespace_error("42883", "function does not exist"))?;
        let owner = self.pg_database_role_id(&self.pg_schema_identity())?;
        if owner != declaration.owner && self.pg_schema_identity() != "root" {
            return Err(namespace_error("42501", "must be owner of function"));
        }
        if declaration.temporary {
            self.state.borrow_mut().pg_extension_functions.remove(&oid);
        } else {
            let context = kv::WithInternalSourceType(kv::Context::default(), kv::InternalTxnDDL);
            self.domain
                .storage_handle()
                .with_storage(|store| {
                    kv::RunInNewTxn(&context, store, true, |_, txn| {
                        let key = astersql_meta::transaction_meta_string_key(b"PgStandardSchema");
                        let value = txn.Get(&context, key.clone(), &[])?;
                        let mut standard: PgStandardSchema =
                            serde_json::from_slice(&value.Value)
                                .map_err(|e| kv::errors::New(e.to_string()))?;
                        standard.extension_functions.remove(&oid);
                        if let Some(acls) = standard
                            .function_acl_overrides
                            .get_mut(&declaration.database)
                        {
                            acls.remove(&oid);
                        }
                        txn.Set(
                            key,
                            serde_json::to_vec(&standard)
                                .map_err(|e| kv::errors::New(e.to_string()))?,
                        )?;
                        Ok(())
                    })
                })
                .map_err(|e| session_error("unpublish PG extension", e))?;
        }
        astersql_expression::remove_pg_extension(self.pg_extension_scope(), oid);
        Ok(())
    }
    pub(super) fn pg_resolve_extension(
        &self,
        expression: &ast::ExprNode,
        source_fields: &[ConcreteResultField],
    ) -> SessionResult<Option<astersql_expression::PgExtensionDeclaration>> {
        let ast::ExprKind::Function {
            FnName,
            Schema,
            Args,
        } = &expression.Kind
        else {
            return Ok(None);
        };
        let namespaces = self.pg_namespace_facts()?;
        let functions = pg_extension_declarations(&namespaces)
            .into_iter()
            .filter(|f| f.name == FnName.O)
            .collect::<Vec<_>>();
        if functions.is_empty() {
            return Ok(None);
        }
        let mut path = self.pg_effective_path()?;
        if !path.iter().any(|s| s == "pg_catalog") {
            path.insert(0, "pg_catalog".into());
        }
        if !Schema.O.is_empty() {
            let namespace = namespaces
                .iter()
                .find(|n| {
                    n.name == Schema.O || (Schema.L == "pg_temp" && n.name.starts_with("pg_temp_"))
                })
                .ok_or_else(|| namespace_error("3F000", "schema does not exist"))?;
            if !namespace.usage {
                return Err(namespace_error("42501", "permission denied for schema"));
            }
            path = vec![namespace.name.clone()];
        } else {
            path.retain(|s| s != "pg_temp" && !s.starts_with("pg_temp_"));
        }
        let inputs = Args
            .iter()
            .map(|arg| {
                match self
                    .relational_query_expression_type_from_sources(arg, source_fields)
                    .GetType()
                {
                    astersql_parser_mysql::r#type::TypeLong => 23,
                    astersql_parser_mysql::r#type::TypeLonglong => 20,
                    _ => 25,
                }
            })
            .collect::<Vec<_>>();
        let candidates = functions
            .iter()
            .map(
                |f| astersql_expression::formal_registry::PgDeclaredCandidate {
                    schema: f.namespace_name.clone(),
                    signature: astersql_expression::formal_registry::PgFunctionCandidate {
                        arguments: f.arguments.clone(),
                        result: f.result,
                    },
                    variadic: None,
                    default_arguments: 0,
                },
            )
            .collect::<Vec<_>>();
        let selected = astersql_expression::formal_registry::select_pg_declared_candidate(
            &inputs,
            &candidates,
            &path.iter().map(String::as_str).collect::<Vec<_>>(),
            false,
            |s, t| s == 23 && matches!(t, 20 | 26),
            |t| if t == 25 { b'S' } else { b'N' },
            |_| false,
        )
        .map_err(|s| namespace_error(s, "function does not exist for these argument types"))?;
        Ok(Some(
            functions[candidates
                .iter()
                .position(|c| std::ptr::eq(c, selected))
                .unwrap()]
            .clone(),
        ))
    }
    pub(super) fn pg_evaluate_extension(
        &self,
        declaration: &astersql_expression::PgExtensionDeclaration,
        values: Vec<Option<String>>,
    ) -> SessionResult<Option<String>> {
        use astersql_expression::BuildContext;
        let namespaces = self.pg_namespace_facts()?;
        let roles = self.pg_role_facts()?;
        let functions = pg_function_facts(&namespaces, &roles);
        if pg_has_function_privilege(
            &functions,
            &self.pg_effective_path()?,
            &namespaces,
            &roles,
            &self.pg_schema_identity(),
            &[Some(declaration.oid.to_string()), Some("EXECUTE".into())],
            &[true, false],
        )? != Some(true)
        {
            return Err(namespace_error("42501", "permission denied for function"));
        }
        // PG SET ROLE changes the privilege principal, not the authenticated
        // identity exposed to callbacks. Reuse the existing INHERIT-filtered view.
        let selected = self.active_roles.borrow().first().cloned();
        let (principal, host) = selected.map_or_else(
            || {
                (
                    self.login_user.clone().unwrap_or_default(),
                    self.authenticated_host.clone().unwrap_or_default(),
                )
            },
            |role| (role.Username, role.Hostname),
        );
        let mut active = self.active_roles.borrow().clone();
        active.retain(|role| role.Username != principal || role.Hostname != host);
        active.extend(
            self.pg_declared_inherited_roles()?
                .into_iter()
                .filter(|name| *name != principal)
                .map(|name| astersql_privilege_privileges::RoleIdentity::new(name, "%")),
        );
        active.sort_by(|a, b| (&a.Username, &a.Hostname).cmp(&(&b.Username, &b.Hostname)));
        active.dedup();
        let checker = Arc::new(PgExtensionPrivilegeChecker {
            privileges: self.pg_inherited_privileges()?,
            user: principal,
            host,
            roles: active,
        });
        let mut relations = super::pg_table::PG_SYSTEM_RELATIONS
            .iter()
            .map(|(name, oid)| (i64::from(*oid), "pg_catalog".to_string(), name.to_string()))
            .collect::<Vec<_>>();
        for (schema, name, model) in self.pg_table_models()? {
            if let Some(oid) = super::pg_table::pg_table_oid(model.ID) {
                relations.push((oid, schema, name));
            }
        }
        if relations.len() > 16384 {
            return Err(namespace_error("54000", "catalog row limit exceeded"));
        }
        let mut path = self.pg_effective_path()?;
        if !path.iter().any(|s| s == "pg_catalog") {
            path.insert(0, "pg_catalog".into());
        }
        if relations.len().saturating_mul(path.len()) > 100000 {
            return Err(namespace_error("54000", "catalog work limit exceeded"));
        }
        let visibility = Arc::new(move |oid: i64| {
            let target = relations.iter().find(|r| r.0 == oid)?;
            for schema in &path {
                if let Some(first) = relations.iter().find(|r| r.1 == *schema && r.2 == target.2) {
                    return Some(first.0 == oid);
                }
            }
            Some(false)
        });
        let user = Arc::new(astersql_parser_auth::parser::auth::auth::UserIdentity {
            username: self
                .login_user
                .clone()
                .ok_or_else(|| namespace_error("0A000", "authenticated identity unavailable"))?,
            hostname: self.login_host.clone().unwrap_or_default(),
            auth_username: self.login_user.clone().unwrap_or_default(),
            auth_hostname: self.authenticated_host.clone().unwrap_or_default(),
            ..Default::default()
        });
        let roles = self
            .active_roles
            .borrow()
            .iter()
            .map(|r| {
                Arc::new(astersql_parser_auth::parser::auth::auth::RoleIdentity {
                    username: r.Username.clone(),
                    hostname: r.Hostname.clone(),
                })
            })
            .collect::<Vec<_>>();
        let eval = astersql_expression_exprstatic::NewEvalContext(vec![
            astersql_expression_exprstatic::WithSQLDialect(ast::SqlDialect::PostgreSQL),
            astersql_expression_exprstatic::WithPgTableVisibilityEvaluator(Some(visibility)),
            astersql_expression_exprstatic::WithCurrentDB(self.current_database()),
            astersql_expression_exprstatic::WithOptionalProperty(vec![
                Box::new(astersql_expression::CurrentUserPropProvider::new(
                    move || (user.clone(), roles.clone()),
                )),
                Box::new(astersql_expression::SessionVarsPropProvider::new(
                    self.session_vars.clone(),
                )),
                Box::new(astersql_expression::PrivilegeCheckerProvider::new(
                    move || checker.clone(),
                )),
            ]),
        ]);
        let context = astersql_expression_exprstatic::NewExprContext(vec![
            astersql_expression_exprstatic::WithEvalCtx(Arc::new(eval)),
        ]);
        let arguments = values
            .into_iter()
            .zip(&declaration.arguments)
            .map(|(value, oid)| {
                let mut argument = value
                    .map(|value| astersql_expression::NewStrConst(&value))
                    .unwrap_or_else(astersql_expression::NewNull);
                argument.RetType = Some(astersql_parser_types::NewFieldType(match *oid {
                    23 => astersql_parser_mysql::r#type::TypeLong,
                    20 | 26 => astersql_parser_mysql::r#type::TypeLonglong,
                    _ => astersql_parser_mysql::r#type::TypeVarString,
                }));
                Box::new(argument) as astersql_expression::ExprBox
            })
            .collect();
        let function = astersql_expression::build_pg_extension(
            self.pg_extension_scope(),
            declaration,
            &context,
            arguments,
        )
        .map_err(pg_extension_execution_error)?;
        let value = function
            .Eval(
                context.GetEvalCtx(),
                astersql_expression::chunk::Row::default(),
            )
            .map_err(pg_extension_execution_error)?;
        if value.IsNull() {
            return Ok(None);
        }
        if declaration.result == 23 {
            i32::try_from(value.GetInt64())
                .map_err(|_| namespace_error("22003", "integer out of range"))?;
        }
        Ok(Some(if declaration.result == 16 {
            if value.GetInt64() != 0 {
                "t".into()
            } else {
                "f".into()
            }
        } else if declaration.result == 25 {
            value.GetString()
        } else {
            value.GetInt64().to_string()
        }))
    }
}
struct PgExtensionPrivilegeChecker {
    privileges: astersql_privilege_privileges::MySQLPrivilege,
    user: String,
    host: String,
    roles: Vec<astersql_privilege_privileges::RoleIdentity>,
}
impl astersql_expression::PrivilegeChecker for PgExtensionPrivilegeChecker {
    fn request_verification(
        &self,
        db: &str,
        table: &str,
        column: &str,
        privilege: astersql_parser_mysql::privs::PrivilegeType,
    ) -> bool {
        self.privileges.RequestVerification(
            &self.roles,
            &self.user,
            &self.host,
            db,
            table,
            column,
            privilege.0,
        )
    }
    fn request_dynamic_verification(&self, name: &str, grantable: bool) -> bool {
        self.privileges.RequestDynamicVerification(
            &self.roles,
            &self.user,
            &self.host,
            name,
            grantable,
        )
    }
}

// Runtime scope IDs cannot alias after a Domain address is reused. The callback
// registry remains the existing extensionFuncs; these weak handles only own its lifetime.
pub(super) fn pg_extension_scope(domain: &Arc<Domain>) -> usize {
    type Scopes = HashMap<usize, (std::sync::Weak<Domain>, usize)>;
    static SCOPES: OnceLock<Mutex<Scopes>> = OnceLock::new();
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1);
    let mut scopes = SCOPES
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    scopes.retain(|_, (owner, scope)| {
        if owner.upgrade().is_some() {
            true
        } else {
            astersql_expression::remove_pg_extension_scope(*scope);
            false
        }
    });
    scopes
        .entry(Arc::as_ptr(domain) as usize)
        .or_insert_with(|| (Arc::downgrade(domain), NEXT.fetch_add(1, Ordering::Relaxed)))
        .1
}

fn pg_extension_execution_error(error: astersql_expression::errors::Error) -> SessionError {
    let message = error.to_string();
    let source = error.into_shared();
    if let Some(pg) = source.downcast_ref::<astersql_expression::PgExtensionError>() {
        namespace_error(pg.state, &pg.message)
    } else {
        SessionError::with_source(message, source)
    }
}

impl ConcreteSession {
    pub(super) fn parse_registered_with_warnings(
        &self,
        sql: &str,
        mode: astersql_parser_mysql::r#const::SQLMode,
    ) -> SessionResult<(Vec<Box<dyn ast::Node>>, Vec<SessionWarning>)> {
        let mut names = Vec::new();
        if self.sql_dialect() == ast::SqlDialect::PostgreSQL {
            if let Some(database) = self
                .domain
                .info_schema()
                .SchemaByName(&astersql_infoschema::CiString::new(self.current_database()))
            {
                if let Some(standard) = self.pg_standard_schema(false)? {
                    names.extend(
                        standard
                            .extension_functions
                            .values()
                            .filter(|f| f.database == database.id)
                            .map(|f| f.name.clone()),
                    );
                }
                names.extend(
                    self.state
                        .borrow()
                        .pg_extension_functions
                        .values()
                        .filter(|f| f.database == database.id)
                        .map(|f| f.name.clone()),
                );
            }
        }
        super::parse_with_registered_declarations(sql, mode, self.sql_dialect(), &names)
    }
    pub(super) fn parse_with_pg_extensions(
        &self,
        sql: &str,
    ) -> SessionResult<Vec<Box<dyn ast::Node>>> {
        let mode = astersql_parser_mysql::r#const::GetSQLMode(
            astersql_parser_mysql::r#const::DefaultSQLMode,
        )
        .map_err(|e| session_error("parse default sql_mode", e))?;
        self.parse_registered_with_warnings(sql, mode)
            .map(|(statements, _)| statements)
    }
}

impl ConcreteSession {
    /// Revalidate the whole input against this worker's registered declarations before any execution.
    pub fn split_sql_with_pg_extensions(&self, sql: &str) -> SessionResult<Vec<String>> {
        self.parse_with_pg_extensions(sql)?;
        Ok(super::split_statement_sql(sql))
    }
}

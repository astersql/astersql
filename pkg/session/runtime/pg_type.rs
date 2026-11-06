// Copyright 2026 AsterSQL.
//! Type USAGE over the existing standard-object record and canonical object ACL engine.
use super::pg_database::{PgDatabaseFact, PgDatabaseGrant, PgRoleFact, update_object_acl};
use super::pg_schema::{PG_NAMESPACE_DDL_LOCK, namespace_error};
use super::*;
use astersql_meta_model::PostgreSQLRelationAcl;
use std::collections::BTreeMap;

pub fn pg_type_privileges(text: &str) -> SessionResult<Vec<(u8, bool)>> {
    text.split(',')
        .map(|s| {
            let s = s.trim().to_ascii_uppercase();
            let (base, option) = s
                .strip_suffix(" WITH GRANT OPTION")
                .map_or((s.as_str(), false), |s| (s, true));
            if base != "USAGE" {
                return Err(namespace_error("22023", "unrecognized type privilege type"));
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
pub fn pg_type_facts(
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
    let mut types: Vec<_> = s
        .builtin_types
        .iter()
        .map(|(name, t)| {
            (
                "pg_catalog",
                name.clone(),
                t.oid,
                s.builtin_type_owners.get(&t.oid).copied(),
            )
        })
        .collect();
    for (name, id) in [
        ("sql_identifier", s.sql_identifier),
        ("character_data", s.character_data),
        ("yes_or_no", s.yes_or_no),
        ("cardinal_number", s.cardinal_number),
    ] {
        types.push((
            "information_schema",
            name.into(),
            16384 + id,
            s.type_owners.get(&id).copied(),
        ));
    }
    types
        .into_iter()
        .map(|(schema, name, id, owner)| {
            let acl = s.type_acls.get(&id).cloned().or_else(|| {
                if s.type_acl_initialized {
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
pub fn pg_type_acl_text(fact: &PgDatabaseFact) -> SessionResult<Option<String>> {
    fact.acl_text_with_modes(&[(1, 'U')])
}
pub fn pg_has_type_privilege(
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
            .map_err(|_| namespace_error("22P02", "invalid type OID"))?;
        types.iter().find(|(_, _, f)| f.id == oid)
    } else {
        let parts = super::pg_table::table_name_parts(object)?;
        let mut search = path.to_vec();
        if !search.iter().any(|s| s == "pg_catalog") {
            search.insert(0, "pg_catalog".into());
        }
        let found = match parts.as_slice() {
            [name] => search
                .iter()
                .find_map(|ns| types.iter().find(|(s, n, _)| s == ns && n == name)),
            [ns, name] => {
                let namespace = namespaces
                    .iter()
                    .find(|f| f.name == *ns)
                    .ok_or_else(|| namespace_error("3F000", "schema does not exist"))?;
                if !namespace.usage {
                    return Err(namespace_error("42501", "permission denied for schema"));
                }
                types.iter().find(|(s, n, _)| s == ns && n == name)
            }
            _ => {
                return Err(namespace_error(
                    "0A000",
                    "cross-database type references are unsupported",
                ));
            }
        };
        Some(found.ok_or_else(|| namespace_error("42704", "type does not exist"))?)
    };
    let modes = pg_type_privileges(values[offset + 1].as_deref().unwrap())?;
    let Some((_, _, fact)) = fact else {
        return Ok(None);
    };
    // PostgreSQL arrays use their element type's authority, while typacl itself stays NULL.
    let element = namespaces
        .iter()
        .find_map(|n| n.standard.as_ref())
        .and_then(|s| {
            s.builtin_types
                .values()
                .find(|t| t.array == fact.id && t.array != 0)
        })
        .map(|t| t.oid);
    let fact = if let Some(element) = element {
        &types
            .iter()
            .find(|(_, _, f)| f.id == element)
            .ok_or_else(|| namespace_error("0A000", "array element type is unavailable"))?
            .2
    } else {
        fact
    };
    let mut allowed = false;
    for (bit, option) in modes {
        allowed |= fact.has_privilege_mask(roles, member, bit, option, 1)?;
    }
    Ok(Some(allowed))
}
impl ConcreteSession {
    pub(super) fn pg_type_privilege_command(
        &self,
        command: &PgSchemaCommand,
    ) -> SessionResult<Option<(&'static str, String)>> {
        let PgSchemaCommand::TypePrivileges {
            domain,
            schema,
            name,
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
        let types = pg_type_facts(&namespaces, &roles);
        let mut path = self.pg_effective_path()?;
        if !path.iter().any(|s| s == "pg_catalog") {
            path.insert(0, "pg_catalog".into());
        }
        let (type_schema, _, fact) = if let Some(schema) = schema {
            types.iter().find(|(s, n, _)| s == schema && n == name)
        } else {
            path.iter()
                .find_map(|s| types.iter().find(|(ns, n, _)| ns == s && n == name))
        }
        .ok_or_else(|| namespace_error("42704", "type does not exist"))?;
        if *domain && type_schema != "information_schema" {
            return Err(namespace_error("42809", "type is not a domain"));
        }
        if namespaces
            .iter()
            .find_map(|n| n.standard.as_ref())
            .is_some_and(|s| {
                s.builtin_types
                    .values()
                    .any(|t| t.oid == fact.id && t.is_array)
            })
        {
            return Err(namespace_error(
                "0LP01",
                "cannot set privileges of array types",
            ));
        }
        let owner = fact
            .owner
            .as_ref()
            .ok_or_else(|| namespace_error("0A000", "type owner is unavailable"))?
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
            pg_type_privileges(privileges)?;
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
                        .type_acl_overrides
                        .get(&database.id)
                        .and_then(|m| m.get(&fact.id))
                        .cloned()
                        .or_else(|| standard.type_acls.get(&fact.id).cloned())
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
                            .type_acl_overrides
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
                    namespace_error("42501", "permission denied for type")
                } else if text.contains("ACL") || text.contains("membership unknown") {
                    namespace_error("0A000", "type ACL authority is unavailable")
                } else {
                    session_error("persist type ACL", e)
                }
            })?;
        Ok(warning)
    }
}

// Role deletion already checks database-scoped dependencies in this same transaction.
pub(super) fn type_role_dependency(
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
    let mut builtin = s.builtin_type_owners;
    if let Some(owners) = s.builtin_type_owner_overrides.get(&database) {
        builtin.extend(owners);
    }
    let mut domains = s.type_owners;
    if let Some(owners) = s.type_owner_overrides.get(&database) {
        domains.extend(owners);
    }
    let mut acls = s.type_acls;
    if let Some(overrides) = s.type_acl_overrides.get(&database) {
        acls.extend(overrides.clone());
    }
    if builtin
        .values()
        .chain(domains.values())
        .any(|id| *id == role)
        || acls.values().any(|a| {
            a.grants
                .iter()
                .any(|g| g.grantor == role || g.grantee == role)
        })
    {
        return Err(kv::errors::New("role has type dependencies"));
    }
    Ok(())
}
/// ALTER OWNER preserves explicit ACLs, replacing old-owner identities and merging duplicates.
pub(super) fn transfer_type_acl(
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
        .type_acl_overrides
        .get(&database)
        .and_then(|m| m.get(&oid))
        .or_else(|| s.type_acls.get(&oid))
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
    s.type_acl_overrides
        .entry(database)
        .or_default()
        .insert(oid, acl);
}

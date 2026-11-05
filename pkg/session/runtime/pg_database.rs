// Copyright 2026 AsterSQL.
//! Shared PostgreSQL database ownership. Legacy databases require explicit admin adoption.
use super::pg_schema::namespace_error;
use super::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Explicit administrator operations; reads never adopt legacy databases.
#[derive(Clone, Debug)]
pub enum PgDatabaseCommand {
    Owner {
        database: String,
        role: String,
    },
    Policy {
        database: String,
        is_template: Option<bool>,
        allow_connections: Option<bool>,
    },
    Connect {
        database: String,
        role: String,
        grant: bool,
    },
}

/// Ownership and per-actor CONNECT from shared KV, keyed by persistent native database ID.
#[derive(Clone, Debug)]
pub struct PgDatabaseFact {
    pub id: i64,
    pub owner: Option<(i64, String)>,
    pub can_connect: bool,
    pub is_template: Option<bool>,
    pub allow_connections: Option<bool>,
    pub roles: Arc<BTreeMap<i64, String>>,
}

// Missing legacy facts remain unknown until an administrator explicitly sets them.
#[derive(Clone, Default, Serialize, Deserialize)]
struct DatabasePolicy {
    is_template: Option<bool>,
    allow_connections: Option<bool>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Access {
    public: bool,
    roles: BTreeSet<i64>,
}
impl Default for Access {
    fn default() -> Self {
        Self {
            public: true,
            roles: BTreeSet::new(),
        }
    }
}

fn load<T: serde::de::DeserializeOwned + Default>(
    txn: &dyn kv::Transaction,
    ctx: &kv::Context,
    key: kv::Key,
) -> Result<T, kv::errors::SharedError> {
    match txn.Get(ctx, key, &[]) {
        Ok(value) => {
            serde_json::from_slice(&value.Value).map_err(|e| kv::errors::New(e.to_string()))
        }
        Err(e) if kv::IsErrNotFound(&e) => Ok(T::default()),
        Err(e) => Err(e),
    }
}
fn role_key() -> kv::Key {
    astersql_meta::transaction_meta_string_key(b"PgDatabaseRoleIds")
}
fn owner_key(id: i64) -> kv::Key {
    astersql_meta::transaction_meta_hash_key(b"PgDatabaseOwners", id.to_string().as_bytes())
}
fn policy_key(id: i64) -> kv::Key {
    astersql_meta::transaction_meta_hash_key(b"PgDatabasePolicy", id.to_string().as_bytes())
}
fn access_key(id: i64) -> kv::Key {
    astersql_meta::transaction_meta_hash_key(b"PgDatabaseConnect", id.to_string().as_bytes())
}
impl ConcreteSession {
    /// A database owner references an existing native role, never the reading connection.
    pub fn pg_database_role_id(&self, role: &str) -> SessionResult<i64> {
        if role != "root" {
            let privileges = runtime_privilege_handle(&self.domain).Get();
            let records = privileges
                .user
                .iter()
                .filter(|r| r.base.User == role)
                .collect::<Vec<_>>();
            if records.is_empty() {
                return Err(namespace_error(
                    "42704",
                    "database owner role does not exist",
                ));
            }
            // PG roles are host-independent; do not silently collapse distinct native accounts.
            if records.len() != 1 || records[0].base.Host != "%" {
                return Err(namespace_error(
                    "0A000",
                    "database role requires one host-independent native account",
                ));
            }
        }
        let ctx = kv::WithInternalSourceType(kv::Context::default(), kv::InternalTxnDDL);
        let mut result = 0;
        self.domain
            .storage_handle()
            .with_storage(|store| {
                kv::RunInNewTxn(&ctx, store, true, |_, txn| {
                    let mut roles: BTreeMap<String, i64> = load(txn, &ctx, role_key())?;
                    result = if let Some(id) = roles.get(role) {
                        *id
                    } else {
                        let key = astersql_meta::transaction_meta_string_key(b"NextGlobalID");
                        let previous = match txn.Get(&ctx, key.clone(), &[]) {
                            Ok(value) => String::from_utf8(value.Value)
                                .map_err(|e| kv::errors::New(e.to_string()))?
                                .parse::<i64>()
                                .map_err(|e| kv::errors::New(e.to_string()))?,
                            Err(e) if kv::IsErrNotFound(&e) => 0,
                            Err(e) => return Err(e),
                        };
                        let next = previous
                            .checked_add(1)
                            .filter(|id| *id > 0 && *id < 0x2000_0000)
                            .ok_or_else(|| kv::errors::New("PostgreSQL role identity exhausted"))?;
                        txn.Set(key, next.to_string().into_bytes())?;
                        let oid = 0x6000_0000 + next;
                        roles.insert(role.to_owned(), oid);
                        txn.Set(
                            role_key(),
                            serde_json::to_vec(&roles)
                                .map_err(|e| kv::errors::New(e.to_string()))?,
                        )?;
                        oid
                    };
                    Ok(())
                })
            })
            .map_err(|e| session_error("persist database role identity", e))?;
        Ok(result)
    }

    /// Read one ownership/ACL snapshot without allocating identities. An absent
    /// owner is an unmigrated legacy object, not an implicit bootstrap owner.
    pub fn pg_database_facts(
        &self,
        command: Option<&PgDatabaseCommand>,
    ) -> SessionResult<Vec<PgDatabaseFact>> {
        let actor = self.pg_schema_identity();
        let snapshot = self.domain.info_schema();
        let schemas = snapshot.AllSchemas();
        let target = match command {
            Some(
                PgDatabaseCommand::Owner { database, .. }
                | PgDatabaseCommand::Connect { database, .. }
                | PgDatabaseCommand::Policy { database, .. },
            ) => Some(
                schemas
                    .iter()
                    .find(|s| s.name.original == *database)
                    .ok_or_else(|| namespace_error("3D000", "database does not exist"))?
                    .id,
            ),
            None => None,
        };
        // Assigning ownership of a legacy database is always an explicit admin operation.
        if command.is_some()
            && !matches!(command, Some(PgDatabaseCommand::Policy { .. }))
            && actor != "root"
        {
            return Err(namespace_error(
                "42501",
                "database administration requires root",
            ));
        }
        let owner = match command {
            Some(PgDatabaseCommand::Owner { role, .. }) => Some(self.pg_database_role_id(role)?),
            _ => None,
        };
        let connect_role = match command {
            Some(PgDatabaseCommand::Connect { role, .. }) if role != "PUBLIC" => {
                Some(self.pg_database_role_id(role)?)
            }
            _ => None,
        };
        let ctx = kv::WithInternalSourceType(kv::Context::default(), kv::InternalTxnDDL);
        let mut facts = Vec::new();
        self.domain
            .storage_handle()
            .with_storage(|store| {
                kv::RunInNewTxn(&ctx, store, true, |_, txn| {
                    facts.clear();
                    if let Some(id) = target {
                        txn.Get(
                            &ctx,
                            astersql_meta::transaction_meta_hash_key(
                                b"DBs",
                                format!("DB:{id}").as_bytes(),
                            ),
                            &[],
                        )?;
                    }
                    if let (Some(id), Some(owner)) = (target, owner) {
                        txn.Set(
                            owner_key(id),
                            serde_json::to_vec(&owner)
                                .map_err(|e| kv::errors::New(e.to_string()))?,
                        )?;
                    }
                    if let (Some(id), Some(PgDatabaseCommand::Connect { role, grant, .. })) =
                        (target, command)
                    {
                        let mut access: Access = load(txn, &ctx, access_key(id))?;
                        if role == "PUBLIC" {
                            access.public = *grant;
                        } else if *grant {
                            access
                                .roles
                                .insert(connect_role.expect("validated role identity"));
                        } else {
                            access
                                .roles
                                .remove(&connect_role.expect("validated role identity"));
                        }
                        txn.Set(
                            access_key(id),
                            serde_json::to_vec(&access)
                                .map_err(|e| kv::errors::New(e.to_string()))?,
                        )?;
                    }
                    let roles: BTreeMap<String, i64> = load(txn, &ctx, role_key())?;
                    if let (
                        Some(id),
                        Some(PgDatabaseCommand::Policy {
                            is_template,
                            allow_connections,
                            ..
                        }),
                    ) = (target, command)
                    {
                        let owner: Option<i64> = load(txn, &ctx, owner_key(id))?;
                        if actor != "root"
                            && !roles.get(&actor).is_some_and(|oid| Some(*oid) == owner)
                        {
                            return Err(kv::errors::New("database policy requires owner"));
                        }
                        if *allow_connections == Some(false)
                            && schemas
                                .iter()
                                .any(|s| s.id == id && s.name.original == self.current_database())
                        {
                            return Err(kv::errors::New(
                                "cannot disallow connections for current database",
                            ));
                        }
                        let mut policy: DatabasePolicy = load(txn, &ctx, policy_key(id))?;
                        if is_template.is_some() {
                            policy.is_template = *is_template;
                        }
                        if allow_connections.is_some() {
                            policy.allow_connections = *allow_connections;
                        }
                        txn.Set(
                            policy_key(id),
                            serde_json::to_vec(&policy)
                                .map_err(|e| kv::errors::New(e.to_string()))?,
                        )?;
                    }
                    if roles
                        .values()
                        .any(|oid| !(0x6000_0001..0x8000_0000).contains(oid))
                    {
                        return Err(kv::errors::New("database owner identity is unavailable"));
                    }
                    let role_names = roles
                        .iter()
                        .map(|(name, id)| (*id, name.clone()))
                        .collect::<BTreeMap<_, _>>();
                    let role_names = Arc::new(role_names);
                    for schema in &schemas {
                        let policy: DatabasePolicy = load(txn, &ctx, policy_key(schema.id))?;
                        let owner: Option<i64> = load(txn, &ctx, owner_key(schema.id))?;
                        let owner = owner
                            .map(|oid| {
                                role_names
                                    .get(&oid)
                                    .map(|name| (oid, name.clone()))
                                    .ok_or_else(|| {
                                        kv::errors::New("database owner identity is unavailable")
                                    })
                            })
                            .transpose()?;
                        let access: Access = load(txn, &ctx, access_key(schema.id))?;
                        let can_connect = actor == "root"
                            || owner.as_ref().is_some_and(|(_, name)| *name == actor)
                            || access.public
                            || roles
                                .get(&actor)
                                .is_some_and(|id| access.roles.contains(id));
                        facts.push(PgDatabaseFact {
                            id: schema.id,
                            owner,
                            can_connect,
                            is_template: policy.is_template,
                            allow_connections: policy.allow_connections,
                            roles: role_names.clone(),
                        });
                    }
                    Ok(())
                })
            })
            .map_err(|e| {
                if e.to_string() == "database policy requires owner" {
                    namespace_error("42501", "must be owner of database")
                } else if e.to_string() == "cannot disallow connections for current database" {
                    namespace_error("22023", "cannot disallow connections for current database")
                } else if e.to_string() == "database owner identity is unavailable" {
                    namespace_error("0A000", "database owner identity is unavailable")
                } else {
                    session_error("read database ownership", e)
                }
            })?;
        Ok(facts)
    }

    // Native account names may be reused, so never retain an old role identity
    // across DROP USER. Prevent dangling owner references before the native drop.
    pub(super) fn pg_database_drop_role(&self, role: &str, remove: bool) -> SessionResult<()> {
        let schemas = self.domain.info_schema().AllSchemas();
        let ctx = kv::WithInternalSourceType(kv::Context::default(), kv::InternalTxnDDL);
        self.domain
            .storage_handle()
            .with_storage(|store| {
                kv::RunInNewTxn(&ctx, store, true, |_, txn| {
                    let mut roles: BTreeMap<String, i64> = load(txn, &ctx, role_key())?;
                    if let Some(oid) = roles.get(role) {
                        for schema in &schemas {
                            let owner: Option<i64> = load(txn, &ctx, owner_key(schema.id))?;
                            if owner == Some(*oid) {
                                return Err(kv::errors::New("role owns a database"));
                            }
                        }
                    }
                    if remove && roles.remove(role).is_some() {
                        txn.Set(
                            role_key(),
                            serde_json::to_vec(&roles)
                                .map_err(|e| kv::errors::New(e.to_string()))?,
                        )?;
                    }
                    Ok(())
                })
            })
            .map_err(|e| session_error("drop database role identity", e))
    }

    /// Authenticate against the existing shared account owner, including locks and expiry.
    pub fn pg_verify_native_account(
        &self,
        user: &str,
        host: &str,
        auth: &[u8],
        salt: &[u8],
    ) -> SessionResult<()> {
        if astersql_privilege_privileges::SkipWithGrant() {
            return Err(namespace_error(
                "28P01",
                "native account authentication requires grants enabled",
            ));
        }
        let mut privileges = astersql_privilege_privileges::NewUserPrivileges(
            runtime_privilege_handle(&self.domain),
        );
        privileges
            .ConnectionVerification(
                &astersql_privilege_privileges::UserIdentity {
                    Username: user.into(),
                    Hostname: host.into(),
                },
                user,
                host,
                auth,
                salt,
                &Default::default(),
            )
            .map_err(|_| namespace_error("28P01", "password authentication failed"))?;
        Ok(())
    }
}

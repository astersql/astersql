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

/// Sanitized public role facts. Unknown legacy PG attributes stay unknown.
#[derive(Clone, Debug)]
pub struct PgRoleFact {
    pub oid: i64,
    pub name: String,
    pub can_login: Option<bool>,
    pub attributes: Option<PgRoleAttributes>,
    pub valid_until_known: bool,
}

/// Explicit PG declaration defaults, not a conversion of native global privileges.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PgRoleAttributes {
    pub superuser: bool,
    pub create_db: bool,
    pub replication: bool,
    pub bypass_rls: bool,
    // Missing fields in older declarations stay unknown; only a new explicit
    // declaration may assign the PG18 defaults below.
    #[serde(default)]
    pub inherit: Option<bool>,
    #[serde(default)]
    pub create_role: Option<bool>,
    #[serde(default)]
    pub connection_limit: Option<i32>,
}

impl Default for PgRoleAttributes {
    fn default() -> Self {
        Self {
            superuser: false,
            create_db: false,
            replication: false,
            bypass_rls: false,
            // PG16+ uses this as the default for newly granted membership
            // edges, not as a runtime switch over existing native active roles.
            inherit: Some(true),
            // The PG role utility requires the administrator; no native SUPER
            // or CREATE ROLE grant can promote this declaration.
            create_role: Some(false),
            // PG declarations currently expose only the unlimited policy.
            // Native MySQL global/user quotas are a separate execution policy.
            connection_limit: Some(-1),
        }
    }
}

/// Bounded PG declarations use native accounts and the existing persistent OID map.
#[derive(Clone, Debug)]
pub enum PgRoleCommand {
    Create { name: String, can_login: bool },
    Drop { name: String },
}

#[derive(Clone, Serialize, Deserialize)]
struct RoleProperties {
    can_login: bool,
    // Present only for explicit PG declarations, never inferred from native SUPER.
    attributes: Option<PgRoleAttributes>,
}
fn role_properties_key(oid: i64) -> kv::Key {
    astersql_meta::transaction_meta_hash_key(
        b"PgDatabaseRoleProperties",
        oid.to_string().as_bytes(),
    )
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

    /// Read the authentication and effective actors without flattening user/host.
    /// The native active-role storage is also used by namespace/ACL evaluation.
    pub fn pg_session_identity(&self) -> SessionResult<(String, String)> {
        let login = self
            .login_user
            .clone()
            .ok_or_else(|| namespace_error("28000", "session has no authenticated role"))?;
        let active = self.active_roles.borrow();
        if active.len() > 1 || active.iter().any(|role| role.Hostname != "%") {
            return Err(namespace_error(
                "0A000",
                "ambiguous PostgreSQL effective role",
            ));
        }
        if self.authenticated_host.as_deref() != Some("%") {
            return Err(namespace_error(
                "0A000",
                "PostgreSQL role host mapping is unavailable",
            ));
        }
        let actor = active
            .first()
            .map_or_else(|| login.clone(), |role| role.Username.clone());
        Ok((login, actor))
    }

    /// Select exactly one actor in the existing shared session role storage.
    /// Authorization uses the authenticated account even after a prior SET ROLE.
    pub fn pg_set_role(&self, role: Option<&str>) -> SessionResult<()> {
        let (login, _) = self.pg_session_identity()?;
        let Some(name) = role else {
            self.active_roles.borrow_mut().clear();
            return Ok(());
        };
        // This read validates stable PG identities and rejects Host collisions.
        if !self.pg_role_facts()?.iter().any(|fact| fact.name == name) {
            return Err(namespace_error("22023", "role does not exist"));
        }
        if name == login {
            self.active_roles.borrow_mut().clear();
            return Ok(());
        }
        let target = astersql_privilege_privileges::RoleIdentity::new(name, "%");
        let privileges = runtime_privilege_handle(&self.domain).Get();
        if login != "root" && !privileges.FindRole(&login, "%", &target) {
            return Err(namespace_error("42501", "permission denied to set role"));
        }
        // Do not mutate on any failure; native GRANT/REVOKE is consulted anew.
        *self.active_roles.borrow_mut() = vec![target];
        Ok(())
    }

    pub fn pg_role_administrator(&self) -> bool {
        self.login_user.as_deref() == Some("root") && self.active_roles.borrow().is_empty()
    }

    /// Invoked only after the driver explicitly admits the development bootstrap identity.
    pub fn pg_register_bootstrap_role(&self) -> SessionResult<()> {
        self.pg_register_native_role("root", "%", true, false)
    }

    /// Record a native declaration, independently of temporary account locking.
    /// Ambiguous host mappings remain unsupported, not collapsed into one PG role.
    pub(super) fn pg_register_native_role(
        &self,
        name: &str,
        host: &str,
        can_login: bool,
        postgres_defaults: bool,
    ) -> SessionResult<()> {
        if host != "%" {
            return Ok(());
        }
        if name != "root"
            && runtime_privilege_handle(&self.domain)
                .Get()
                .user
                .iter()
                .filter(|r| r.base.User == name)
                .count()
                != 1
        {
            return Ok(());
        }
        let oid = self.pg_database_role_id(name)?;
        let ctx = kv::WithInternalSourceType(kv::Context::default(), kv::InternalTxnDDL);
        self.domain
            .storage_handle()
            .with_storage(|store| {
                kv::RunInNewTxn(&ctx, store, true, |_, txn| {
                    let existing: Option<RoleProperties> =
                        load(txn, &ctx, role_properties_key(oid))?;
                    // Bootstrap authentication must not overwrite an explicit declaration.
                    if existing.is_none() || postgres_defaults {
                        txn.Set(
                            role_properties_key(oid),
                            serde_json::to_vec(&RoleProperties {
                                can_login,
                                attributes: if postgres_defaults {
                                    Some(PgRoleAttributes::default())
                                } else {
                                    None
                                },
                            })
                            .map_err(|e| kv::errors::New(e.to_string()))?,
                        )?;
                    }
                    Ok(())
                })
            })
            .map_err(|e| session_error("persist PostgreSQL role properties", e))
    }

    /// Public metadata never executes mysql.user SQL or exposes authentication records.
    /// No identity allocation occurs on this read path.
    pub fn pg_role_facts(&self) -> SessionResult<Vec<PgRoleFact>> {
        let accounts = runtime_privilege_handle(&self.domain).Get();
        let ctx = kv::WithInternalSourceType(kv::Context::default(), kv::InternalTxnDDL);
        let mut facts = Vec::new();
        self.domain
            .storage_handle()
            .with_storage(|store| {
                kv::RunInNewTxn(&ctx, store, false, |_, txn| {
                    facts.clear();
                    let roles: BTreeMap<String, i64> = load(txn, &ctx, role_key())?;
                    let mut names = BTreeSet::new();
                    for account in &accounts.user {
                        if account.base.Host != "%" || !names.insert(account.base.User.clone()) {
                            return Err(kv::errors::New(
                                "PostgreSQL role host mapping is unavailable",
                            ));
                        }
                    }
                    // The explicit development bootstrap identity can exist without a native row.
                    if roles.contains_key("root") {
                        names.insert("root".into());
                    }
                    for name in names {
                        if name.len() > 63 {
                            return Err(kv::errors::New(
                                "PostgreSQL role name mapping is unavailable",
                            ));
                        }
                        let oid = *roles.get(&name).ok_or_else(|| {
                            kv::errors::New("PostgreSQL role identity is unavailable")
                        })?;
                        if !(0x6000_0001..0x8000_0000).contains(&oid) {
                            return Err(kv::errors::New("invalid PostgreSQL role identity"));
                        }
                        let props: Option<RoleProperties> =
                            load(txn, &ctx, role_properties_key(oid))?;
                        let expiry_changed = accounts
                            .user
                            .iter()
                            .find(|r| r.base.User == name)
                            .is_some_and(|r| r.PasswordExpired || r.PasswordLifeTime > 0);
                        facts.push(PgRoleFact {
                            oid,
                            name,
                            can_login: props.as_ref().map(|p| p.can_login),
                            valid_until_known: props
                                .as_ref()
                                .is_some_and(|p| p.attributes.is_some())
                                && !expiry_changed,
                            attributes: props.and_then(|p| p.attributes),
                        });
                    }
                    Ok(())
                })
            })
            .map_err(|e| {
                if matches!(
                    e.to_string().as_str(),
                    "PostgreSQL role host mapping is unavailable"
                        | "PostgreSQL role identity is unavailable"
                        | "invalid PostgreSQL role identity"
                        | "PostgreSQL role declaration is unavailable"
                        | "PostgreSQL role name mapping is unavailable"
                ) {
                    namespace_error("0A000", "complete PostgreSQL role facts are unavailable")
                } else {
                    session_error("read PostgreSQL role facts", e)
                }
            })?;
        Ok(facts)
    }

    /// NOCREATEDB is a real execution policy, not merely a displayed attribute.
    pub(super) fn pg_check_database_creation(&self) -> SessionResult<()> {
        let actor = self.pg_schema_identity();
        if actor == "root" {
            return Ok(());
        }
        let ctx = kv::Context::default();
        let mut allowed = None;
        self.domain
            .storage_handle()
            .with_storage(|store| {
                kv::RunInNewTxn(&ctx, store, false, |_, txn| {
                    let roles: BTreeMap<String, i64> = load(txn, &ctx, role_key())?;
                    let oid = roles
                        .get(&actor)
                        .ok_or_else(|| kv::errors::New("role identity unavailable"))?;
                    let props: Option<RoleProperties> = load(txn, &ctx, role_properties_key(*oid))?;
                    allowed = props.and_then(|p| p.attributes).map(|a| a.create_db);
                    Ok(())
                })
            })
            .map_err(|e| session_error("read PostgreSQL database creation policy", e))?;
        match allowed {
            Some(true) => Ok(()),
            Some(false) => Err(namespace_error(
                "42501",
                "permission denied to create database",
            )),
            None => Err(namespace_error(
                "0A000",
                "PostgreSQL database creation policy is unavailable",
            )),
        }
    }

    pub fn pg_role_command(&self, command: &PgRoleCommand) -> SessionResult<()> {
        if self.state.borrow().transaction.is_some() {
            return Err(namespace_error(
                "0A000",
                "role administration inside a transaction is unsupported",
            ));
        }
        if self.login_user.as_deref().unwrap_or("root") != "root"
            || !self.active_roles.borrow().is_empty()
        {
            return Err(namespace_error(
                "42501",
                "role administration requires an administrator",
            ));
        }
        let (name, can_login) = match command {
            PgRoleCommand::Create { name, can_login } => (name, Some(*can_login)),
            PgRoleCommand::Drop { name } => (name, None),
        };
        if name.is_empty() {
            return Err(namespace_error("42602", "role name must not be empty"));
        }
        if name.len() > 63 {
            return Err(namespace_error(
                "0A000",
                "role names exceeding 63 bytes are unsupported",
            ));
        }
        if name == "root" {
            return Err(namespace_error(
                "0A000",
                "bootstrap role administration is unsupported",
            ));
        }
        let quoted = name.replace('\\', "\\\\").replace('\'', "''");
        if let Some(can_login) = can_login {
            if runtime_privilege_handle(&self.domain)
                .Get()
                .user
                .iter()
                .any(|r| r.base.User == *name)
            {
                return Err(namespace_error("42710", "role already exists"));
            }
            let native = ConcreteSession::new(self.domain.clone());
            native.execute(&format!(
                "CREATE {} '{}'@'%'{}",
                if can_login { "USER" } else { "ROLE" },
                quoted,
                if can_login {
                    " IDENTIFIED WITH mysql_native_password BY ''"
                } else {
                    ""
                }
            ))?;
            self.pg_register_native_role(name, "%", can_login, true)
        } else {
            let records = runtime_privilege_handle(&self.domain).Get();
            let accounts = records
                .user
                .iter()
                .filter(|r| r.base.User == *name)
                .collect::<Vec<_>>();
            if accounts.is_empty() {
                return Err(namespace_error("42704", "role does not exist"));
            }
            if accounts.len() != 1 || accounts[0].base.Host != "%" {
                return Err(namespace_error("0A000", "ambiguous role host mapping"));
            }
            ConcreteSession::new(self.domain.clone())
                .execute(&format!("DROP USER '{}'@'%'", quoted))?;
            Ok(())
        }
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
                            let tables = self
                                .domain
                                .info_schema()
                                .SchemaTableInfos(&schema.name)
                                .map_err(|e| kv::errors::New(e.to_string()))?;
                            if tables.iter().any(|t| {
                                t.model_meta
                                    .as_ref()
                                    .is_some_and(|m| m.PostgreSQLOwner == Some(*oid))
                            }) {
                                return Err(kv::errors::New("role owns a relation"));
                            }
                            let owner: Option<i64> = load(txn, &ctx, owner_key(schema.id))?;
                            if owner == Some(*oid) {
                                return Err(kv::errors::New("role owns a database"));
                            }
                        }
                    }
                    if remove && let Some(oid) = roles.remove(role) {
                        txn.Delete(role_properties_key(oid))?;
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
    ) -> SessionResult<astersql_privilege_privileges::VerificationInfo> {
        if astersql_privilege_privileges::SkipWithGrant() {
            return Err(namespace_error(
                "28P01",
                "native account authentication requires grants enabled",
            ));
        }
        let mut privileges = astersql_privilege_privileges::NewUserPrivileges(
            runtime_privilege_handle(&self.domain),
        );
        let verified = privileges
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
        if self.sql_dialect() == ast::SqlDialect::PostgreSQL {
            let ctx = kv::Context::default();
            let permitted = self.domain.storage_handle().with_storage(|store| {
                kv::RunInNewTxn(&ctx, store, false, |_, txn| {
                    let roles: BTreeMap<String, i64> = load(txn, &ctx, role_key())?;
                    if let Some(oid) = roles.get(&verified.authenticated_user) {
                        let props: Option<RoleProperties> =
                            load(txn, &ctx, role_properties_key(*oid))?;
                        if props.is_some_and(|p| !p.can_login) {
                            return Err(kv::errors::New("role cannot login"));
                        }
                    }
                    Ok(())
                })
            });
            if permitted.is_err() {
                return Err(namespace_error("28P01", "password authentication failed"));
            }
        }
        Ok(verified)
    }
}

// Copyright 2026 AsterSQL.
//! Database-scoped PostgreSQL namespace identities, relation binding and USAGE/CREATE ACLs.
//! These metadata-only operations never create a native MySQL database.
use super::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

// Match the runtime's process-local DDL boundary while checking namespace
// dependencies. Durable allocation itself remains a retried KV transaction.
pub(super) static PG_NAMESPACE_DDL_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Copy, Debug)]
pub enum PgTablePrivilege {
    Select,
    Insert,
    Update,
    References,
    Delete,
    Trigger,
}
impl PgTablePrivilege {
    pub fn sql(self) -> &'static str {
        match self {
            Self::Select => "SELECT",
            Self::Insert => "INSERT",
            Self::Update => "UPDATE",
            Self::References => "REFERENCES",
            Self::Delete => "DELETE",
            Self::Trigger => "TRIGGER",
        }
    }
}

#[derive(Clone, Debug)]
pub enum PgSchemaCommand {
    FunctionOwner {
        schema: Option<String>,
        name: String,
        owner: String,
    },
    Create(String),
    Drop(String),
    Rename {
        schema: String,
        name: String,
    },
    CreateOwned {
        schema: String,
        owner: String,
    },
    Owner {
        schema: String,
        owner: String,
    },
    TypeOwner {
        domain: bool,
        schema: Option<String>,
        name: String,
        owner: String,
    },
    RelationOwner {
        schema: Option<String>,
        table: String,
        owner: String,
    },
    RelationPrivilege {
        privilege: PgTablePrivilege,
        schema: String,
        table: String,
        grantee: String,
        column: Option<String>,
        grant: bool,
    },
    Privileges {
        schema: String,
        grantee: String,
        public: bool,
        privileges: String,
        grant: bool,
        grant_option: bool,
        cascade: bool,
    },
    RevokeAll {
        schema: String,
        grantee: String,
    },
    Usage {
        /// Select CREATE rather than USAGE for this bounded schema grant.
        create: bool,
        schema: String,
        grantee: String,
        grant: bool,
    },
}

#[derive(Clone, Serialize, Deserialize)]
struct Namespace {
    #[serde(default)]
    id: i64,
    owner: String,
    #[serde(default)]
    owner_id: Option<i64>,
    usage: BTreeSet<String>,
    #[serde(default)]
    create: BTreeSet<String>,
    #[serde(default)]
    explicit_acl: Option<bool>,
    #[serde(default)]
    grants: Option<Vec<super::pg_database::PgDatabaseGrant>>,
    #[serde(default = "owner_privilege_default")]
    owner_usage: bool,
    #[serde(default = "owner_privilege_default")]
    owner_create: bool,
}

// Older persisted namespaces gave their owner both default privileges.
fn owner_privilege_default() -> bool {
    true
}

impl ConcreteSession {
    /// Keep connection-local path separate from the native current database.
    pub fn pg_search_path(&self, path: Vec<String>) {
        self.state.borrow_mut().pg_search_path = path;
    }

    // Authentication establishes the login identity. SET ROLE verifies shared
    // membership before replacing this actor; ACL evaluation also expands its closure.
    pub(super) fn pg_schema_identity(&self) -> String {
        self.active_roles
            .borrow()
            .first()
            .map(|r| r.Username.clone())
            .unwrap_or_else(|| self.login_user.clone().unwrap_or_else(|| "root".into()))
    }

    fn pg_namespaces(
        &self,
        command: Option<&PgSchemaCommand>,
    ) -> SessionResult<(BTreeMap<String, Namespace>, Option<(&'static str, String)>)> {
        let database = self
            .domain
            .info_schema()
            .SchemaByName(&astersql_infoschema::CiString::new(self.current_database()))
            .ok_or_else(|| SessionError::new("PostgreSQL database is unavailable"))?;
        let actor = self.pg_schema_identity();
        if matches!(
            command,
            Some(PgSchemaCommand::Create(_) | PgSchemaCommand::CreateOwned { .. })
        ) {
            self.pg_require_database_privilege(4)?;
        }
        let command_owner_id = match command {
            Some(PgSchemaCommand::Create(_)) => Some(self.pg_database_role_id(&actor)?),
            Some(
                PgSchemaCommand::CreateOwned { owner, .. } | PgSchemaCommand::Owner { owner, .. },
            ) => Some(self.pg_database_role_id(owner)?),
            _ => None,
        };
        let grant_roles = if matches!(command, Some(PgSchemaCommand::Privileges { .. })) {
            Some(if actor == "root" {
                self.pg_namespace_role_identities()?
            } else {
                self.pg_role_facts()?
            })
        } else {
            None
        };
        let mut command_warning = None;
        let context = kv::WithInternalSourceType(kv::Context::default(), kv::InternalTxnDDL);
        let standard = self.pg_standard_schema(false)?;
        let mut result = BTreeMap::new();
        self.domain
            .storage_handle()
            .with_storage(|store| {
                kv::RunInNewTxn(&context, store, true, |_, transaction| {
                    let key = astersql_meta::transaction_meta_hash_key(
                        b"PgNamespaces",
                        database.id.to_string().as_bytes(),
                    );
                    let mut namespaces: BTreeMap<String, Namespace> =
                        match transaction.Get(&context, key.clone(), &[]) {
                            Ok(value) => serde_json::from_slice(&value.Value)
                                .map_err(|e| kv::errors::New(e.to_string()))?,
                            Err(error) if kv::IsErrNotFound(&error) => BTreeMap::from([
                                (
                                    "public".into(),
                                    Namespace {
                                        explicit_acl: Some(false),
                                        grants: None,
                                        owner_usage: true,
                                        owner_create: true,
                                        id: 0,
                                        owner: "root".into(),
                                        owner_id: None,
                                        usage: BTreeSet::from(["PUBLIC".into()]),
                                        create: BTreeSet::new(),
                                    },
                                ),
                                (
                                    "pg_catalog".into(),
                                    Namespace {
                                        explicit_acl: Some(false),
                                        grants: None,
                                        owner_usage: true,
                                        owner_create: true,
                                        id: 0,
                                        owner: "root".into(),
                                        owner_id: None,
                                        usage: BTreeSet::from(["PUBLIC".into()]),
                                        create: BTreeSet::new(),
                                    },
                                ),
                            ]),
                            Err(error) => return Err(error),
                        };
                    if let Some(standard) = &standard {
                        namespaces
                            .entry("information_schema".into())
                            .or_insert_with(|| Namespace {
                                id: standard.namespace,
                                owner: "root".into(),
                                owner_id: None,
                                usage: BTreeSet::from(["PUBLIC".into()]),
                                create: BTreeSet::new(),
                                explicit_acl: Some(false),
                                grants: None,
                                owner_usage: true,
                                owner_create: true,
                            });
                    }
                    if let Some(standard) = &standard
                        && namespaces
                            .get("information_schema")
                            .is_none_or(|ns| ns.id != standard.namespace)
                    {
                        return Err(kv::errors::New("standard schema identity is unavailable"));
                    }
                    for (name, namespace) in &mut namespaces {
                        if namespace.id == 0
                            && !matches!(
                                name.as_str(),
                                "public" | "pg_catalog" | "information_schema"
                            )
                        {
                            if command.is_none() {
                                return Err(kv::errors::New(
                                    "namespace identity requires explicit migration",
                                ));
                            }
                            namespace.id = allocate_namespace_id(transaction, &context)?;
                        }
                    }
                    if let Some(command) = command {
                        match command {
                            PgSchemaCommand::RelationPrivilege { .. }
                            | PgSchemaCommand::RelationOwner { .. }
                            | PgSchemaCommand::TypeOwner { .. }
                            | PgSchemaCommand::FunctionOwner { .. } => {
                                unreachable!("handled before namespace transaction")
                            }
                            PgSchemaCommand::Create(name) => {
                                if namespaces.contains_key(name) {
                                    return Err(kv::errors::New("schema already exists"));
                                }
                                namespaces.insert(
                                    name.clone(),
                                    Namespace {
                                        explicit_acl: Some(false),
                                        grants: command_owner_id.map(default_namespace_acl),
                                        owner_usage: true,
                                        owner_create: true,
                                        id: allocate_namespace_id(transaction, &context)?,
                                        owner: actor.clone(),
                                        owner_id: command_owner_id,
                                        usage: BTreeSet::new(),
                                        create: BTreeSet::new(),
                                    },
                                );
                            }
                            PgSchemaCommand::CreateOwned { schema, owner } => {
                                if actor != "root" && owner != &actor {
                                    return Err(kv::errors::New(
                                        "permission denied to create schema",
                                    ));
                                }
                                if namespaces.contains_key(schema) {
                                    return Err(kv::errors::New("schema already exists"));
                                }
                                namespaces.insert(
                                    schema.clone(),
                                    Namespace {
                                        id: allocate_namespace_id(transaction, &context)?,
                                        owner: owner.clone(),
                                        owner_id: command_owner_id,
                                        usage: BTreeSet::new(),
                                        create: BTreeSet::new(),
                                        explicit_acl: Some(false),
                                        grants: command_owner_id.map(default_namespace_acl),
                                        owner_usage: true,
                                        owner_create: true,
                                    },
                                );
                            }
                            PgSchemaCommand::Owner { schema, owner } => {
                                let existing = namespaces
                                    .get_mut(schema)
                                    .ok_or_else(|| kv::errors::New("schema does not exist"))?;
                                if actor != "root" {
                                    return Err(kv::errors::New("must be owner of schema"));
                                }
                                if let (Some(old), Some(new), Some(grants)) = (
                                    existing.owner_id,
                                    command_owner_id,
                                    existing.grants.as_mut(),
                                ) {
                                    let mut merged = BTreeMap::<(i64, i64), (u8, u8)>::new();
                                    for e in grants.iter() {
                                        let key = (
                                            if e.grantor == old { new } else { e.grantor },
                                            if e.grantee == old { new } else { e.grantee },
                                        );
                                        let v = merged.entry(key).or_default();
                                        v.0 |= e.privileges;
                                        v.1 |= e.grant_options;
                                    }
                                    *grants = merged
                                        .into_iter()
                                        .map(|((grantor, grantee), (privileges, grant_options))| {
                                            super::pg_database::PgDatabaseGrant {
                                                grantor,
                                                grantee,
                                                privileges,
                                                grant_options,
                                            }
                                        })
                                        .collect();
                                }
                                existing.owner = owner.clone();
                                existing.owner_id = command_owner_id;
                            }
                            PgSchemaCommand::Privileges {
                                schema,
                                grantee,
                                public,
                                privileges,
                                grant,
                                grant_option,
                                cascade,
                            } => {
                                let ns = namespaces
                                    .get_mut(schema)
                                    .ok_or_else(|| kv::errors::New("schema does not exist"))?;
                                let roles = grant_roles.as_ref().unwrap();
                                let fact = namespace_acl_fact(ns, roles);
                                fact.validate_acl()
                                    .map_err(|e| kv::errors::New(e.to_string()))?;
                                let owner = fact
                                    .owner
                                    .as_ref()
                                    .ok_or_else(|| {
                                        kv::errors::New("schema ACL requires explicit migration")
                                    })?
                                    .0;
                                let actor_id = roles
                                    .iter()
                                    .find(|r| r.name == actor)
                                    .ok_or_else(|| {
                                        kv::errors::New("schema ACL role is unavailable")
                                    })?
                                    .oid;
                                let grantee = if *public {
                                    0
                                } else {
                                    roles
                                        .iter()
                                        .find(|r| r.name == *grantee)
                                        .ok_or_else(|| {
                                            kv::errors::New("schema grantee role does not exist")
                                        })?
                                        .oid
                                };
                                if *grant && *grant_option && grantee == 0 {
                                    return Err(kv::errors::New(
                                        "grant options cannot be granted to PUBLIC",
                                    ));
                                }
                                let bits = if privileges == "ALL" {
                                    5
                                } else {
                                    pg_schema_privileges(privileges)
                                        .map_err(|e| kv::errors::New(e.to_string()))?
                                        .iter()
                                        .fold(0, |b, (v, _)| b | v)
                                };
                                let (warning, modified) = super::pg_database::update_object_acl(
                                    ns.grants.as_mut().unwrap(),
                                    owner,
                                    &actor,
                                    actor_id,
                                    roles,
                                    bits,
                                    grantee,
                                    *grant,
                                    *grant_option,
                                    *cascade,
                                    privileges == "ALL",
                                )?;
                                command_warning = warning;
                                if modified {
                                    ns.explicit_acl = Some(true);
                                }
                            }
                            PgSchemaCommand::RevokeAll { .. } | PgSchemaCommand::Usage { .. } => {
                                unreachable!("normalized before namespace transaction")
                            }
                            PgSchemaCommand::Rename { schema, name } => {
                                if matches!(
                                    schema.as_str(),
                                    "public" | "pg_catalog" | "information_schema"
                                ) || name.starts_with("pg_")
                                {
                                    return Err(kv::errors::New(
                                        "renaming built-in schema is unsupported",
                                    ));
                                }
                                if namespaces.contains_key(name) {
                                    return Err(kv::errors::New("schema already exists"));
                                }
                                let existing = namespaces
                                    .get(schema)
                                    .ok_or_else(|| kv::errors::New("schema does not exist"))?;
                                if actor != "root" && actor != existing.owner {
                                    return Err(kv::errors::New("must be owner of schema"));
                                }
                                let existing = namespaces.remove(schema).unwrap();
                                namespaces.insert(name.clone(), existing);
                            }
                            PgSchemaCommand::Drop(name) => {
                                if matches!(
                                    name.as_str(),
                                    "public" | "pg_catalog" | "information_schema"
                                ) {
                                    return Err(kv::errors::New(
                                        "dropping built-in schema is unsupported",
                                    ));
                                }
                                let schema = namespaces
                                    .get(name)
                                    .ok_or_else(|| kv::errors::New("schema does not exist"))?;
                                if actor != "root" && actor != schema.owner {
                                    return Err(kv::errors::New("must be owner of schema"));
                                }
                                if self
                                    .domain
                                    .info_schema()
                                    .SchemaTableInfos(&database.name)
                                    .map_err(|e| kv::errors::New(e.to_string()))?
                                    .iter()
                                    .any(|table| {
                                        table.model_meta.as_ref().is_some_and(|table| {
                                            table
                                                .PostgreSQLNamespace
                                                .as_ref()
                                                .is_some_and(|(id, _)| *id == schema.id)
                                        })
                                    })
                                {
                                    return Err(kv::errors::New(
                                        "cannot drop schema because it is not empty",
                                    ));
                                }
                                namespaces.remove(name);
                            }
                        }
                        // Remove the empty extension record after the last owned schema
                        // is dropped, retaining modified built-in ACLs when present.
                        let defaults = namespaces.len() == (if standard.is_some() { 3 } else { 2 })
                            && namespaces.iter().all(|(name, ns)| {
                                matches!(
                                    name.as_str(),
                                    "public" | "pg_catalog" | "information_schema"
                                ) && ns.explicit_acl == Some(false)
                                    && ns.owner == "root"
                                    // Explicit ownership must survive default-record cleanup.
                                    && ns.owner_id.is_none()
                                    && ns.usage == BTreeSet::from(["PUBLIC".into()])
                            });
                        if defaults {
                            transaction.Delete(key.clone())?;
                        } else {
                            transaction.Set(
                                key.clone(),
                                serde_json::to_vec(&namespaces)
                                    .map_err(|e| kv::errors::New(e.to_string()))?,
                            )?;
                        }
                    }
                    result = namespaces;
                    Ok(())
                })
            })
            .map_err(|e| match e.to_string().as_str() {
                "standard schema identity is unavailable" => {
                    namespace_error("0A000", "standard schema identity is unavailable")
                }
                "namespace identity requires explicit migration" => namespace_error(
                    "0A000",
                    "namespace identity requires explicit administrator migration",
                ),
                "dependent database privileges exist" => {
                    namespace_error("2BP01", "dependent schema privileges exist")
                }
                "database privilege requires grant option" => {
                    namespace_error("42501", "permission denied for schema")
                }
                "grant options cannot be granted to PUBLIC" => {
                    namespace_error("0LP01", "grant options cannot be granted to PUBLIC")
                }
                "schema grantee role does not exist" => {
                    namespace_error("42704", "schema grantee role does not exist")
                }
                message
                    if message.contains("ACL")
                        || message.contains("membership unknown")
                        || message.contains("facts are unavailable") =>
                {
                    namespace_error("0A000", message)
                }
                "schema already exists" => namespace_error("42P06", "schema already exists"),
                "schema does not exist" => namespace_error("3F000", "schema does not exist"),
                "cannot drop schema because it is not empty" => {
                    namespace_error("2BP01", "cannot drop schema because it is not empty")
                }
                "must be owner of schema" | "permission denied to create schema" => {
                    namespace_error("42501", "permission denied for schema")
                }
                "renaming built-in schema is unsupported"
                | "dropping built-in schema is unsupported" => {
                    namespace_error("0A000", "built-in schema operation is unsupported")
                }
                _ => session_error("PostgreSQL schema metadata", e),
            })?;
        Ok((result, command_warning))
    }

    pub fn pg_schema_command(&self, command: &PgSchemaCommand) -> SessionResult<()> {
        if matches!(
            command,
            PgSchemaCommand::Usage { .. }
                | PgSchemaCommand::RevokeAll { .. }
                | PgSchemaCommand::Privileges { .. }
        ) {
            return self.pg_schema_privilege_command(command).map(|_| ());
        }
        if let PgSchemaCommand::FunctionOwner {
            schema,
            name,
            owner,
        } = command
        {
            return self.pg_function_owner(schema.as_deref(), name, owner);
        }
        if let PgSchemaCommand::TypeOwner {
            schema,
            name,
            owner,
            domain,
        } = command
        {
            return self.pg_type_owner(schema.as_deref(), name, owner, *domain);
        }
        if let PgSchemaCommand::RelationOwner {
            schema,
            table,
            owner,
        } = command
        {
            let _guard = PG_NAMESPACE_DDL_LOCK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // Legacy relations can only be adopted explicitly by the administrator.
            if self.pg_schema_identity() != "root" {
                return Err(namespace_error(
                    "42501",
                    "permission denied to change relation owner",
                ));
            }
            let owner = self.pg_database_role_id(owner)?;
            let physical = self.pg_bind_relation(schema.as_deref(), table, false)?;
            let database = self.current_database();
            let model = self
                .domain
                .table_by_name(&database, &physical)
                .map_err(|e| session_error("resolve relation owner", e))?;
            self.domain
                .ddl_set_postgresql_owner(&database, model.ID, owner)
                .map_err(|e| session_error("persist relation owner", e))?;
            return Ok(());
        }
        if let PgSchemaCommand::RelationPrivilege {
            privilege,
            schema,
            table,
            grantee,
            column,
            grant,
        } = command
        {
            if self.pg_schema_identity() != "root" {
                return Err(namespace_error(
                    "42501",
                    "permission denied to grant table access",
                ));
            }
            self.pg_database_role_id(grantee)?;
            let physical = self.pg_bind_relation(Some(schema), table, false)?;
            // Privilege table writes retain their native dialect; the PG actor
            // and logical namespace were authorized before this internal bridge.
            let native =
                ConcreteSession::new_with_dialect(self.domain.clone(), ast::SqlDialect::MySQL);
            let ident = |name: &str| format!("`{}`", name.replace('`', "``"));
            native.execute(&format!(
                "{} {}{} ON {}.{} {} '{}'@'%'",
                if *grant { "GRANT" } else { "REVOKE" },
                privilege.sql(),
                column
                    .as_ref()
                    .map_or(String::new(), |c| format!("({})", ident(c))),
                ident(&self.current_database()),
                ident(&physical),
                if *grant { "TO" } else { "FROM" },
                grantee.replace('\\', "\\\\").replace('\'', "''")
            ))?;
            return Ok(());
        }
        let _guard = PG_NAMESPACE_DDL_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.sql_dialect() != ast::SqlDialect::PostgreSQL {
            return Err(SessionError::new(
                "PostgreSQL schema metadata requires PostgreSQL dialect",
            ));
        }
        if matches!(command, PgSchemaCommand::Owner { .. }) && self.pg_schema_identity() != "root" {
            return Err(namespace_error("42501", "permission denied for schema"));
        }
        self.pg_namespaces(Some(command)).map(|_| ())
    }

    pub fn pg_schema_privilege_command(
        &self,
        command: &PgSchemaCommand,
    ) -> SessionResult<Option<(&'static str, String)>> {
        let _guard = PG_NAMESPACE_DDL_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.sql_dialect() != ast::SqlDialect::PostgreSQL {
            return Err(namespace_error(
                "0A000",
                "PostgreSQL ACL requires PostgreSQL dialect",
            ));
        }
        let converted;
        let command = match command {
            PgSchemaCommand::Usage {
                create,
                schema,
                grantee,
                grant,
            } => {
                converted = PgSchemaCommand::Privileges {
                    schema: schema.clone(),
                    grantee: grantee.clone(),
                    public: grantee == "PUBLIC",
                    privileges: if *create { "CREATE" } else { "USAGE" }.into(),
                    grant: *grant,
                    grant_option: false,
                    cascade: false,
                };
                &converted
            }
            PgSchemaCommand::RevokeAll { schema, grantee } => {
                converted = PgSchemaCommand::Privileges {
                    schema: schema.clone(),
                    grantee: grantee.clone(),
                    public: grantee == "PUBLIC",
                    privileges: "ALL".into(),
                    grant: false,
                    grant_option: false,
                    cascade: false,
                };
                &converted
            }
            PgSchemaCommand::Privileges { .. } => command,
            _ => {
                return Err(namespace_error(
                    "0A000",
                    "expected schema privilege command",
                ));
            }
        };
        self.pg_namespaces(Some(command))
            .map(|(_, warning)| warning)
    }

    /// Read fresh committed metadata for each statement, including after GRANT/REVOKE.
    pub fn pg_effective_path(&self) -> SessionResult<Vec<String>> {
        let namespaces = self.pg_namespace_facts()?;
        let actor = self.pg_schema_identity();
        let mut result = Vec::new();
        for entry in &self.state.borrow().pg_search_path {
            let name = if entry == "$user" { &actor } else { entry };
            if namespaces.iter().any(|ns| ns.name == *name && ns.usage) && !result.contains(name) {
                result.push(name.clone());
            }
        }
        Ok(result)
    }
}

/// Authoritative namespace facts; unavailable USAGE is distinct from absence.
#[derive(Clone, Debug)]
pub struct PgNamespaceFact {
    pub id: i64,
    pub name: String,
    pub usage: bool,
    /// Effective CREATE from the owner, enabled roles, PUBLIC, or bootstrap admin.
    pub can_create: bool,
    pub owner: Option<i64>,
    pub acl: Option<String>,
    pub privilege_fact: Option<super::pg_database::PgDatabaseFact>,
    /// A default SQL NULL is only valid when the persisted ACL state is known.
    pub acl_available: bool,
    /// Ownership visibility is independent of the owner's revoked ACL bits.
    pub owner_usable: bool,
    pub standard: Option<PgStandardSchema>,
}

fn allocate_namespace_id(
    transaction: &mut dyn kv::Transaction,
    context: &kv::Context,
) -> Result<i64, kv::errors::SharedError> {
    let key = astersql_meta::transaction_meta_string_key(b"NextGlobalID");
    let previous = match transaction.Get(context, key.clone(), &[]) {
        Ok(value) => String::from_utf8(value.Value)
            .map_err(|e| kv::errors::New(e.to_string()))?
            .parse::<i64>()
            .map_err(|e| kv::errors::New(e.to_string()))?,
        Err(error) if kv::IsErrNotFound(&error) => 0,
        Err(error) => return Err(error),
    };
    let id = previous
        .checked_add(1)
        .filter(|id| *id <= (0x4000_0000 - 16384 - 1) / 2)
        .ok_or_else(|| kv::errors::New("PostgreSQL namespace identity exhausted"))?;
    transaction.Set(key, id.to_string().into_bytes())?;
    Ok(id)
}

pub(super) fn namespace_error(state: &'static str, message: &str) -> SessionError {
    let error = super::function_diagnostics::PgFunctionError {
        state,
        message: message.into(),
        detail: None,
        hint: None,
        position: None,
    };
    SessionError::with_source(message, astersql_errors::SharedError::new(error))
}

impl ConcreteSession {
    fn pg_enabled_namespace_roles(&self) -> SessionResult<BTreeSet<String>> {
        let actor = self.pg_schema_identity();
        let mut enabled = BTreeSet::from([actor.clone()]);
        if let (Some(user), Some(host)) = (
            self.login_user.as_deref(),
            self.authenticated_host.as_deref(),
        ) {
            let privileges = self.pg_inherited_privileges()?;
            enabled.extend(
                privileges
                    .FindAllUserEffectiveRoles(user, host, &self.active_roles.borrow())
                    .iter()
                    .map(|r| r.Username.clone()),
            );
        }
        enabled.extend(self.pg_declared_inherited_roles()?);
        Ok(enabled)
    }
    fn pg_namespace_role_identities(&self) -> SessionResult<Vec<super::pg_database::PgRoleFact>> {
        let names = self
            .pg_database_facts(None)?
            .first()
            .map(|f| f.roles.clone())
            .unwrap_or_default();
        Ok(names
            .iter()
            .map(|(oid, name)| super::pg_database::PgRoleFact {
                oid: *oid,
                name: name.clone(),
                can_login: None,
                attributes: None,
                valid_until_known: false,
                memberships: Vec::new(),
            })
            .collect())
    }
    pub fn pg_namespace_facts(&self) -> SessionResult<Vec<PgNamespaceFact>> {
        let actor = self.pg_schema_identity();
        let database = self
            .domain
            .info_schema()
            .SchemaByName(&astersql_infoschema::CiString::new(self.current_database()))
            .ok_or_else(|| namespace_error("3D000", "no current database selected"))?;
        // Root's established bootstrap authority needs identity names for ACL
        // validation, not declarations for unrelated historical native accounts.
        let role_facts = if actor == "root" {
            self.pg_namespace_role_identities()?
        } else {
            self.pg_role_facts()?
        };
        let roles = role_facts
            .iter()
            .map(|f| (f.oid, f.name.clone()))
            .collect::<BTreeMap<_, _>>();
        let enabled = self.pg_enabled_namespace_roles()?;
        let mut standard = self.pg_standard_schema(false)?;
        if let Some(standard) = &mut standard {
            if let Some(owners) = standard.builtin_type_owner_overrides.get(&database.id) {
                standard.builtin_type_owners.extend(owners);
            }
            if let Some(owners) = standard.type_owner_overrides.get(&database.id) {
                standard.type_owners.extend(owners);
            }
        }
        let mut facts: Vec<_> = self
            .pg_namespaces(None)?
            .0
            .into_iter()
            .map(|(name, namespace)| {
                let fact = namespace_acl_fact(&namespace, &role_facts);
                let available = fact.validate_acl().is_ok() && fact.owner.is_some();
                let actor_id = role_facts
                    .iter()
                    .find(|r| r.name == actor)
                    .map_or(-1, |r| r.oid);
                let effective = |bit| -> SessionResult<bool> {
                    if actor == "root" {
                        Ok(true)
                    } else {
                        fact.has_privilege(&role_facts, actor_id, bit, false)
                    }
                };
                Ok(PgNamespaceFact {
                    standard: (name == "information_schema")
                        .then(|| standard.clone())
                        .flatten(),
                    owner_usable: actor == "root" || enabled.contains(&namespace.owner),
                    owner: namespace
                        .owner_id
                        .filter(|id| roles.get(id) == Some(&namespace.owner)),
                    acl_available: available,
                    acl: if available {
                        fact.acl_text_with_modes(&[(1, 'U'), (4, 'C')])?
                    } else {
                        None
                    },
                    privilege_fact: Some(fact.clone()),
                    id: if name == "public" {
                        database.id
                    } else {
                        namespace.id
                    },
                    can_create: if available {
                        effective(4)?
                    } else if namespace.grants.is_some() {
                        false
                    } else {
                        actor == "root"
                            || (enabled.contains(&namespace.owner) && namespace.owner_create)
                            || namespace.create.contains("PUBLIC")
                            || namespace.create.iter().any(|r| enabled.contains(r))
                    },
                    usage: if available {
                        effective(1)?
                    } else if namespace.grants.is_some() {
                        false
                    } else {
                        actor == "root"
                            || (enabled.contains(&namespace.owner) && namespace.owner_usage)
                            || namespace.usage.contains("PUBLIC")
                            || namespace.usage.iter().any(|r| enabled.contains(r))
                    },
                    name,
                })
            })
            .collect::<SessionResult<Vec<_>>>()?;
        if let Some((id, owner)) = self
            .state
            .borrow()
            .pg_temporary_namespaces
            .get(&self.current_database())
            .copied()
        {
            facts.push(PgNamespaceFact {
                id,
                name: format!("pg_temp_{id}"),
                usage: true,
                can_create: true,
                owner: Some(owner),
                acl: None,
                privilege_fact: Some(super::pg_database::PgDatabaseFact {
                    id,
                    owner: roles.get(&owner).map(|n| (owner, n.clone())),
                    can_connect: true,
                    acl: Some(default_namespace_acl(owner)),
                    acl_default: true,
                    warning: None,
                    is_template: None,
                    allow_connections: None,
                    roles: Arc::new(roles.clone()),
                }),
                acl_available: true,
                owner_usable: true,
                standard: None,
            });
        }
        Ok(facts)
    }

    /// Resolve logical PG identity to the existing native table storage key.
    /// Creation uses the first effective explicit namespace, never a fallback database.
    pub fn pg_bind_relation(
        &self,
        schema: Option<&str>,
        name: &str,
        create: bool,
    ) -> SessionResult<String> {
        if !create
            && schema.is_none()
            && self
                .local_temporary_table(&self.current_database(), name)
                .is_some()
        {
            return Ok(name.to_owned());
        }
        let facts = self.pg_namespace_facts()?;
        let path = if let Some(schema) = schema {
            let fact = facts.iter().find(|f| f.name == schema).ok_or_else(|| {
                if create {
                    namespace_error("3F000", "schema does not exist")
                } else {
                    namespace_error("42P01", "relation does not exist")
                }
            })?;
            if !fact.usage {
                return Err(namespace_error("42501", "permission denied for schema"));
            }
            vec![schema.to_owned()]
        } else {
            let actor = self.pg_schema_identity();
            self.state
                .borrow()
                .pg_search_path
                .iter()
                .filter_map(|entry| {
                    let name = if entry == "$user" { &actor } else { entry };
                    facts
                        .iter()
                        .find(|fact| fact.name == *name && fact.usage)
                        .map(|fact| fact.name.clone())
                })
                .collect()
        };
        if create && path.is_empty() {
            return Err(namespace_error(
                "3F000",
                "no schema has been selected to create in",
            ));
        }
        let snapshot = self.domain.info_schema();
        let database = astersql_infoschema::CiString::new(self.current_database());
        for schema in path {
            let fact = facts
                .iter()
                .find(|f| f.name == schema)
                .ok_or_else(|| namespace_error("3F000", "schema no longer exists"))?;
            if schema == "pg_catalog" {
                if create {
                    return Err(namespace_error(
                        "42501",
                        "permission denied for schema pg_catalog",
                    ));
                }
                continue;
            }
            let physical = if schema == "public" {
                name.to_owned()
            } else {
                format!("__astersql_pg_{}_{}", fact.id, name)
            };
            if schema == "public" && name.starts_with("__astersql_pg_") {
                return Err(namespace_error("42501", "reserved PostgreSQL storage name"));
            }
            if create {
                if !fact.can_create {
                    return Err(namespace_error(
                        "42501",
                        "permission denied to create in schema",
                    ));
                }
                return Ok(physical);
            }
            if let Ok(table) = snapshot.ModelTableInfoByExactName(&database, &physical) {
                let matches = if schema == "public" {
                    table.PostgreSQLNamespace.is_none()
                } else {
                    table.PostgreSQLNamespace.as_ref() == Some(&(fact.id, name.to_owned()))
                };
                if matches {
                    return Ok(physical);
                }
            }
        }
        Err(namespace_error("42P01", "relation does not exist"))
    }

    pub(super) fn pg_relation_namespace(
        &self,
        physical: &str,
    ) -> SessionResult<Option<(i64, String)>> {
        let Some(tail) = physical.strip_prefix("__astersql_pg_") else {
            return Ok(None);
        };
        let Some((id, name)) = tail.split_once('_') else {
            return Err(namespace_error("42602", "invalid PostgreSQL storage name"));
        };
        let id = id
            .parse::<i64>()
            .map_err(|_| namespace_error("42602", "invalid PostgreSQL storage identity"))?;
        if !self
            .pg_namespace_facts()?
            .iter()
            .any(|f| f.id == id && f.name != "public" && f.name != "pg_catalog" && f.usage)
        {
            return Err(namespace_error("42501", "permission denied for schema"));
        }
        Ok(Some((id, name.into())))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct BuiltinTypeIdentity {
    oid: i64,
    array: i64,
    is_array: bool,
}

/// Durable identities for the bounded standard schema. OIDs are derived by the
/// server from these globally allocated IDs, never copied from a reference build.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PgStandardSchema {
    // Declarations register identities on upgrade, never unknown historical owners.
    #[serde(default)]
    builtin_types: BTreeMap<String, BuiltinTypeIdentity>,
    #[serde(default)]
    pub builtin_type_owners: BTreeMap<i64, i64>,
    #[serde(default)]
    pub builtin_type_owner_overrides: BTreeMap<i64, BTreeMap<i64, i64>>,
    /// Per-function ownership registered in the initial standard-object transaction.
    /// A missing historical field stays unknown; startup never backfills it.
    #[serde(default)]
    pub function_owners: BTreeMap<i64, i64>,
    /// Explicit database-local ownership of the registered adapter primitives.
    /// Historical missing entries carry no ownership assertion.
    #[serde(default)]
    pub function_owner_overrides: BTreeMap<i64, BTreeMap<i64, i64>>,
    /// Explicit per-type ownership; absent historical entries remain unknown.
    #[serde(default)]
    pub type_owners: BTreeMap<i64, i64>,
    /// ALTER OWNER is database-local even when bootstrap type identities are shared.
    #[serde(default)]
    pub type_owner_overrides: BTreeMap<i64, BTreeMap<i64, i64>>,
    /// Owner of the standard view, independent of later schema ownership changes.
    pub owner: i64,
    pub namespace: i64,
    pub sql_identifier: i64,
    pub character_data: i64,
    pub schemata: i64,
    #[serde(default)]
    pub tables: i64,
    #[serde(default)]
    pub yes_or_no: i64,
    #[serde(default)]
    pub views: i64,
    #[serde(default)]
    pub columns: i64,
    #[serde(default)]
    pub cardinal_number: i64,
    #[serde(default)]
    pub table_constraints: i64,
    #[serde(default)]
    pub key_column_usage: i64,
    #[serde(default)]
    pub referential_constraints: i64,
    #[serde(default)]
    pub constraint_column_usage: i64,
}
impl ConcreteSession {
    pub fn pg_initialize_standard_schema(&self) -> SessionResult<()> {
        self.pg_standard_schema(true)?;
        Ok(())
    }
    /// Register the server's existing builtin declarations during explicit startup.
    /// Only a newly created standard record receives the bootstrap ownership fact.
    pub fn pg_initialize_standard_schema_with_types(
        &self,
        types: &[(i64, &str, i64, bool)],
    ) -> SessionResult<()> {
        self.pg_standard_schema_with_types(true, types)?;
        Ok(())
    }
    fn pg_standard_schema(&self, initialize: bool) -> SessionResult<Option<PgStandardSchema>> {
        self.pg_standard_schema_with_types(initialize, &[])
    }
    fn pg_standard_schema_with_types(
        &self,
        initialize: bool,
        types: &[(i64, &str, i64, bool)],
    ) -> SessionResult<Option<PgStandardSchema>> {
        let owner = if initialize {
            Some(self.pg_database_role_id("root")?)
        } else {
            None
        };
        let ctx = kv::WithInternalSourceType(kv::Context::default(), kv::InternalTxnDDL);
        let mut result = None;
        self.domain
            .storage_handle()
            .with_storage(|store| {
                kv::RunInNewTxn(&ctx, store, true, |_, txn| {
                    let key = astersql_meta::transaction_meta_string_key(b"PgStandardSchema");
                    let mut created = false;
                    result = match txn.Get(&ctx, key.clone(), &[]) {
                        Ok(value) => Some(
                            serde_json::from_slice(&value.Value)
                                .map_err(|e| kv::errors::New(e.to_string()))?,
                        ),
                        Err(e) if kv::IsErrNotFound(&e) && initialize => {
                            created = true;
                            let standard = PgStandardSchema {
                                builtin_types: BTreeMap::new(),
                                builtin_type_owners: BTreeMap::new(),
                                builtin_type_owner_overrides: BTreeMap::new(),
                                owner: owner.expect("initialization has an owner"),
                                function_owners: PG_INTROSPECTION_FUNCTION_IDENTITIES
                                    .iter()
                                    .map(|(id, _)| {
                                        (*id, owner.expect("initialization has an owner"))
                                    })
                                    .collect(),
                                function_owner_overrides: BTreeMap::new(),
                                type_owners: BTreeMap::new(),
                                type_owner_overrides: BTreeMap::new(),
                                namespace: allocate_namespace_id(txn, &ctx)?,
                                sql_identifier: allocate_namespace_id(txn, &ctx)?,
                                character_data: allocate_namespace_id(txn, &ctx)?,
                                schemata: allocate_namespace_id(txn, &ctx)?,
                                tables: allocate_namespace_id(txn, &ctx)?,
                                yes_or_no: allocate_namespace_id(txn, &ctx)?,
                                views: allocate_namespace_id(txn, &ctx)?,
                                columns: allocate_namespace_id(txn, &ctx)?,
                                cardinal_number: allocate_namespace_id(txn, &ctx)?,
                                table_constraints: allocate_namespace_id(txn, &ctx)?,
                                key_column_usage: allocate_namespace_id(txn, &ctx)?,
                                referential_constraints: allocate_namespace_id(txn, &ctx)?,
                                constraint_column_usage: allocate_namespace_id(txn, &ctx)?,
                            };
                            let mut standard = standard;
                            for id in [
                                standard.sql_identifier,
                                standard.character_data,
                                standard.yes_or_no,
                                standard.cardinal_number,
                            ] {
                                standard
                                    .type_owners
                                    .insert(id, owner.expect("initialization has an owner"));
                            }
                            txn.Set(
                                key.clone(),
                                serde_json::to_vec(&standard)
                                    .map_err(|e| kv::errors::New(e.to_string()))?,
                            )?;
                            Some(standard)
                        }
                        Err(e) if kv::IsErrNotFound(&e) => None,
                        Err(e) => return Err(e),
                    };
                    // Upgrade only at explicit startup, never during catalog reads.
                    if initialize && let Some(standard) = &mut result {
                        if standard.tables == 0 && standard.yes_or_no == 0 {
                            standard.tables = allocate_namespace_id(txn, &ctx)?;
                            standard.yes_or_no = allocate_namespace_id(txn, &ctx)?;
                            standard.type_owners.insert(
                                standard.yes_or_no,
                                owner.expect("initialization has an owner"),
                            );
                            txn.Set(
                                key.clone(),
                                serde_json::to_vec(standard)
                                    .map_err(|e| kv::errors::New(e.to_string()))?,
                            )?;
                        }
                        if standard.views == 0 {
                            standard.views = allocate_namespace_id(txn, &ctx)?;
                            txn.Set(
                                key,
                                serde_json::to_vec(standard)
                                    .map_err(|e| kv::errors::New(e.to_string()))?,
                            )?;
                        }
                        if standard.columns == 0 && standard.cardinal_number == 0 {
                            standard.columns = allocate_namespace_id(txn, &ctx)?;
                            standard.cardinal_number = allocate_namespace_id(txn, &ctx)?;
                            standard.type_owners.insert(
                                standard.cardinal_number,
                                owner.expect("initialization has an owner"),
                            );
                            txn.Set(
                                astersql_meta::transaction_meta_string_key(b"PgStandardSchema"),
                                serde_json::to_vec(standard)
                                    .map_err(|e| kv::errors::New(e.to_string()))?,
                            )?;
                        }
                        if standard.table_constraints == 0 {
                            standard.table_constraints = allocate_namespace_id(txn, &ctx)?;
                            txn.Set(
                                astersql_meta::transaction_meta_string_key(b"PgStandardSchema"),
                                serde_json::to_vec(standard)
                                    .map_err(|e| kv::errors::New(e.to_string()))?,
                            )?;
                        }
                        if standard.key_column_usage == 0 {
                            standard.key_column_usage = allocate_namespace_id(txn, &ctx)?;
                            txn.Set(
                                astersql_meta::transaction_meta_string_key(b"PgStandardSchema"),
                                serde_json::to_vec(standard)
                                    .map_err(|e| kv::errors::New(e.to_string()))?,
                            )?;
                        }
                        if standard.referential_constraints == 0 {
                            standard.referential_constraints = allocate_namespace_id(txn, &ctx)?;
                            txn.Set(
                                astersql_meta::transaction_meta_string_key(b"PgStandardSchema"),
                                serde_json::to_vec(standard)
                                    .map_err(|e| kv::errors::New(e.to_string()))?,
                            )?;
                        }
                        if standard.constraint_column_usage == 0 {
                            standard.constraint_column_usage = allocate_namespace_id(txn, &ctx)?;
                            txn.Set(
                                astersql_meta::transaction_meta_string_key(b"PgStandardSchema"),
                                serde_json::to_vec(standard)
                                    .map_err(|e| kv::errors::New(e.to_string()))?,
                            )?;
                        }
                    }
                    if initialize && let Some(standard) = &mut result {
                        let mut changed = false;
                        for &(oid, name, array, is_array) in types {
                            let identity = BuiltinTypeIdentity {
                                oid,
                                array,
                                is_array,
                            };
                            if let Some(previous) = standard.builtin_types.get(name) {
                                if previous != &identity {
                                    return Err(kv::errors::New("builtin type identity changed"));
                                }
                            } else {
                                standard.builtin_types.insert(name.into(), identity);
                                if created {
                                    standard
                                        .builtin_type_owners
                                        .insert(oid, owner.expect("initialization has an owner"));
                                }
                                changed = true;
                            }
                        }
                        if changed {
                            txn.Set(
                                astersql_meta::transaction_meta_string_key(b"PgStandardSchema"),
                                serde_json::to_vec(standard)
                                    .map_err(|e| kv::errors::New(e.to_string()))?,
                            )?;
                        }
                    }
                    Ok(())
                })
            })
            .map_err(|e| session_error("PostgreSQL standard schema metadata", e))?;
        if let Some(standard) = &result {
            let ids = [
                standard.namespace,
                standard.sql_identifier,
                standard.character_data,
                standard.schemata,
                standard.tables,
                standard.yes_or_no,
                standard.views,
                standard.columns,
                standard.cardinal_number,
                standard.table_constraints,
                standard.key_column_usage,
                standard.referential_constraints,
                standard.constraint_column_usage,
            ];
            let unique: BTreeSet<_> = ids.into_iter().collect();
            if unique.len() != 13
                || !(0x6000_0001..0x8000_0000).contains(&standard.owner)
                || ids
                    .iter()
                    .any(|id| *id <= 0 || *id > (0x4000_0000 - 16384 - 1) / 2)
            {
                return Err(namespace_error(
                    "0A000",
                    "standard schema identity is unavailable",
                ));
            }
        }
        Ok(result)
    }
}

impl ConcreteSession {
    /// Ownership uses the authenticated actor's enabled, inherited roles. A
    /// missing owner is unavailable evidence, never a confirmed non-owner NULL.
    pub fn pg_standard_view_owner(
        &self,
        table: &astersql_meta_model::TableInfo,
    ) -> SessionResult<bool> {
        let roles = self
            .pg_database_facts(None)?
            .first()
            .map(|f| (*f.roles).clone())
            .unwrap_or_default();
        let owner = table
            .PostgreSQLOwner
            .and_then(|id| roles.get(&id))
            .ok_or_else(|| namespace_error("0A000", "view owner identity is unavailable"))?;
        if self.pg_schema_identity() == "root" {
            return Ok(true);
        }
        let (Some(login), Some(host)) = (
            self.login_user.as_deref(),
            self.authenticated_host.as_deref(),
        ) else {
            return Err(namespace_error(
                "0A000",
                "authenticated view owner source is unavailable",
            ));
        };
        let selected = self.active_roles.borrow().first().cloned();
        let (user, host) = selected.as_ref().map_or((login, host), |r| {
            (r.Username.as_str(), r.Hostname.as_str())
        });
        let privileges = self.pg_inherited_privileges()?;
        let active = privileges.getAllRoles(user, host);
        let inherited = privileges.FindAllRole(&active);
        Ok(owner == user || inherited.iter().any(|r| &r.Username == owner))
    }
    /// Shared ACLs and effective roles; namespace USAGE and path do not filter
    /// standard table listings. The caller supplies its execution-snapshot model.
    pub fn pg_standard_table_visible(
        &self,
        database: &str,
        table: &astersql_meta_model::TableInfo,
    ) -> SessionResult<bool> {
        let actor = self.pg_schema_identity();
        let roles = self
            .pg_database_facts(None)?
            .first()
            .map(|f| (*f.roles).clone())
            .unwrap_or_default();
        if actor == "root" {
            return Ok(true);
        }
        let (Some(login), Some(login_host)) = (
            self.login_user.as_deref(),
            self.authenticated_host.as_deref(),
        ) else {
            return Err(namespace_error(
                "0A000",
                "authenticated relation ACL source is unavailable",
            ));
        };
        let selected = self.active_roles.borrow().first().cloned();
        let (user, host) = selected.as_ref().map_or((login, login_host), |r| {
            (r.Username.as_str(), r.Hostname.as_str())
        });
        let privileges = self.pg_inherited_privileges()?;
        // PostgreSQL enables inherited memberships on login; native MySQL
        // default-role activation is a separate contract and stays unchanged.
        let active = privileges.getAllRoles(user, host);
        let inherited = privileges.FindAllRole(&active);
        let owner = table.PostgreSQLOwner.and_then(|id| roles.get(&id));
        if owner.is_some_and(|name| name == user || inherited.iter().any(|r| &r.Username == name)) {
            return Ok(true);
        }
        let mask = astersql_parser_mysql::privs::AllPrivMask.0;
        Ok(
            privileges.RequestVerification(&active, user, host, database, &table.Name.O, "", mask)
                || table
                    .Columns
                    .iter()
                    .filter(|c| c.State == astersql_meta_model::StatePublic && !c.Hidden)
                    .any(|c| {
                        privileges.RequestVerification(
                            &active,
                            user,
                            host,
                            database,
                            &table.Name.O,
                            &c.Name.O,
                            mask,
                        )
                    }),
        )
    }
    /// Standard columns admit only ownership or SELECT/INSERT/UPDATE/REFERENCES on this column.
    pub fn pg_standard_column_visible(
        &self,
        database: &str,
        table: &astersql_meta_model::TableInfo,
        column: &str,
    ) -> SessionResult<bool> {
        self.pg_standard_column_access(database, table, column, false)
    }
    /// TC admits ownership or writing privileges, never SELECT alone.
    pub fn pg_standard_constraint_visible(
        &self,
        database: &str,
        table: &astersql_meta_model::TableInfo,
    ) -> SessionResult<bool> {
        if self.pg_standard_view_owner(table)? {
            return Ok(true);
        }
        for column in table
            .Columns
            .iter()
            .filter(|c| c.State == astersql_meta_model::StatePublic && !c.Hidden)
        {
            if self.pg_standard_column_access(database, table, &column.Name.O, true)? {
                return Ok(true);
            }
        }
        // Empty tables can still carry table-level DELETE/TRIGGER grants.
        self.pg_standard_column_access(database, table, "", true)
    }
    fn pg_standard_column_access(
        &self,
        database: &str,
        table: &astersql_meta_model::TableInfo,
        column: &str,
        write_only: bool,
    ) -> SessionResult<bool> {
        let actor = self.pg_schema_identity();
        let roles = self
            .pg_database_facts(None)?
            .first()
            .map(|f| (*f.roles).clone())
            .unwrap_or_default();
        if actor == "root" {
            return Ok(true);
        }
        let (Some(login), Some(login_host)) = (
            self.login_user.as_deref(),
            self.authenticated_host.as_deref(),
        ) else {
            return Err(namespace_error(
                "0A000",
                "authenticated relation ACL source is unavailable",
            ));
        };
        let selected = self.active_roles.borrow().first().cloned();
        let (user, host) = selected.as_ref().map_or((login, login_host), |r| {
            (r.Username.as_str(), r.Hostname.as_str())
        });
        let privileges = self.pg_inherited_privileges()?;
        // PostgreSQL enables inherited memberships on login; native MySQL
        // default-role activation is a separate contract and stays unchanged.
        let active = privileges.getAllRoles(user, host);
        let inherited = privileges.FindAllRole(&active);
        let owner = table.PostgreSQLOwner.and_then(|id| roles.get(&id));
        if owner.is_some_and(|name| name == user || inherited.iter().any(|r| &r.Username == name)) {
            return Ok(true);
        }
        use astersql_privilege_privileges::{InsertPriv, ReferencesPriv, SelectPriv, UpdatePriv};
        let mask = if write_only {
            InsertPriv | UpdatePriv | ReferencesPriv
        } else {
            SelectPriv | InsertPriv | UpdatePriv | ReferencesPriv
        };
        let table_mask = if write_only {
            mask | astersql_privilege_privileges::DeletePriv
                | astersql_privilege_privileges::TriggerPriv
        } else {
            mask
        };
        // tables_priv.ColumnPriv summarizes grants on any column. It cannot
        // establish permission on this particular column; consult its exact key.
        let table_grant = privileges.RequestVerification(
            &active,
            user,
            host,
            database,
            &table.Name.O,
            "",
            table_mask,
        );
        let identities =
            std::iter::once(astersql_privilege_privileges::RoleIdentity::new(user, host))
                .chain(privileges.FindAllUserEffectiveRoles(user, host, &active));
        Ok(table_grant
            || identities.into_iter().any(|role| {
                privileges
                    .MatchColumns(
                        &role.Username,
                        &role.Hostname,
                        database,
                        &table.Name.O,
                        column,
                    )
                    .is_some_and(|record| record.ColumnPriv & mask != 0)
            }))
    }
}

impl ConcreteSession {
    /// Local table identities share the allocator, but never publish a relation
    /// into Domain. Only this connection retains the namespace and table models.
    pub(super) fn pg_allocate_temporary_identity(&self) -> SessionResult<(i64, i64)> {
        self.pg_require_database_privilege(2)?;
        let database = self.current_database();
        let existing = self
            .state
            .borrow()
            .pg_temporary_namespaces
            .get(&database)
            .copied();
        let owner = match existing {
            Some((_, owner)) => owner,
            None => self.pg_database_role_id(&self.pg_schema_identity())?,
        };
        let ctx = kv::WithInternalSourceType(kv::Context::default(), kv::InternalTxnDDL);
        let mut ids = (0, 0);
        self.domain
            .storage_handle()
            .with_storage(|store| {
                kv::RunInNewTxn(&ctx, store, true, |_, txn| {
                    ids.0 = match existing {
                        Some((id, _)) => id,
                        None => allocate_namespace_id(txn, &ctx)?,
                    };
                    ids.1 = allocate_namespace_id(txn, &ctx)?;
                    Ok(())
                })
            })
            .map_err(|e| session_error("allocate PG temporary identities", e))?;
        self.state
            .borrow_mut()
            .pg_temporary_namespaces
            .insert(database, (ids.0, owner));
        Ok(ids)
    }
    pub fn pg_local_temporary_models(&self) -> Vec<astersql_meta_model::TableInfo> {
        let database = self.current_database();
        self.state
            .borrow()
            .local_temporary_tables
            .iter()
            .filter(|((db, _), _)| db == &database)
            .map(|(_, model)| model.clone())
            .collect()
    }
}

impl ConcreteSession {
    // Update only registered standard type identities in the existing metadata record.
    // This is explicit migration for historical entries, never a read-time fallback.
    fn pg_type_owner(
        &self,
        schema: Option<&str>,
        name: &str,
        owner: &str,
        domain: bool,
    ) -> SessionResult<()> {
        let _guard = PG_NAMESPACE_DDL_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.pg_schema_identity() != "root" {
            return Err(namespace_error(
                "42501",
                "permission denied to change type owner",
            ));
        }
        let mut path = self.pg_effective_path()?;
        if !path.iter().any(|s| s == "pg_catalog") {
            path.insert(0, "pg_catalog".into());
        }
        let namespaces = schema.map_or(path, |s| vec![s.into()]);
        let standard = self
            .pg_standard_schema(false)?
            .ok_or_else(|| namespace_error("0A000", "type identities are unavailable"))?;
        let target = namespaces
            .iter()
            .find_map(|schema| match schema.as_str() {
                "information_schema" => match name {
                    "sql_identifier" => Some((standard.sql_identifier, 0, false, false)),
                    "character_data" => Some((standard.character_data, 0, false, false)),
                    "yes_or_no" => Some((standard.yes_or_no, 0, false, false)),
                    "cardinal_number" => Some((standard.cardinal_number, 0, false, false)),
                    _ => None,
                },
                "pg_catalog" => standard
                    .builtin_types
                    .get(name)
                    .map(|t| (t.oid, t.array, t.is_array, true)),
                _ => None,
            })
            .ok_or_else(|| namespace_error("42704", "type does not exist"))?;
        let (id, array, is_array, builtin) = target;
        if is_array {
            return Err(namespace_error(
                "42809",
                "cannot alter array type; alter its element type instead",
            ));
        }
        if domain && builtin {
            return Err(namespace_error("42809", "type is not a domain"));
        }
        let owner = self.pg_database_role_id(owner)?;
        let database = self
            .domain
            .info_schema()
            .SchemaByName(&astersql_infoschema::CiString::new(self.current_database()))
            .ok_or_else(|| namespace_error("3D000", "no current database selected"))?;
        let context = kv::WithInternalSourceType(kv::Context::default(), kv::InternalTxnDDL);
        self.domain
            .storage_handle()
            .with_storage(|store| {
                kv::RunInNewTxn(&context, store, true, |_, txn| {
                    let key = astersql_meta::transaction_meta_string_key(b"PgStandardSchema");
                    let value = txn.Get(&context, key.clone(), &[])?;
                    let mut standard: PgStandardSchema = serde_json::from_slice(&value.Value)
                        .map_err(|e| kv::errors::New(e.to_string()))?;
                    let overrides = if builtin {
                        &mut standard.builtin_type_owner_overrides
                    } else {
                        &mut standard.type_owner_overrides
                    };
                    let owners = overrides.entry(database.id).or_default();
                    owners.insert(id, owner);
                    if array != 0 {
                        owners.insert(array, owner);
                    }
                    txn.Set(
                        key,
                        serde_json::to_vec(&standard)
                            .map_err(|e| kv::errors::New(e.to_string()))?,
                    )
                })
            })
            .map_err(|e| session_error("persist PostgreSQL type owner", e))
    }
}

/// Identities of the three registered PG adapter primitives, not user routines.
/// Catalog declarations and explicit ownership updates share this same registry.
pub const PG_INTROSPECTION_FUNCTION_IDENTITIES: &[(i64, &str)] = &[
    (16000, "pg_get_function_arguments"),
    (16001, "pg_get_function_result"),
    (16002, "pg_get_function_sqlbody"),
];

impl ConcreteSession {
    // Reuse the existing standard-object KV record and administrative DDL boundary.
    // This explicitly registers/transfers a fact; catalog reads never migrate it.
    fn pg_function_owner(
        &self,
        schema: Option<&str>,
        name: &str,
        owner: &str,
    ) -> SessionResult<()> {
        let _guard = PG_NAMESPACE_DDL_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.pg_schema_identity() != "root" {
            return Err(namespace_error(
                "42501",
                "permission denied to change function owner",
            ));
        }
        let id = PG_INTROSPECTION_FUNCTION_IDENTITIES
            .iter()
            .find(|(_, registered)| *registered == name)
            .filter(|_| schema.is_none_or(|schema| schema == "pg_catalog"))
            .map(|(id, _)| *id)
            .ok_or_else(|| namespace_error("42883", "function does not exist"))?;
        let owner = self.pg_database_role_id(owner)?;
        let database = self
            .domain
            .info_schema()
            .SchemaByName(&astersql_infoschema::CiString::new(self.current_database()))
            .ok_or_else(|| namespace_error("3D000", "no current database selected"))?;
        let context = kv::WithInternalSourceType(kv::Context::default(), kv::InternalTxnDDL);
        self.domain
            .storage_handle()
            .with_storage(|store| {
                kv::RunInNewTxn(&context, store, true, |_, txn| {
                    let key = astersql_meta::transaction_meta_string_key(b"PgStandardSchema");
                    let value = txn.Get(&context, key.clone(), &[])?;
                    let mut standard: PgStandardSchema = serde_json::from_slice(&value.Value)
                        .map_err(|e| kv::errors::New(e.to_string()))?;
                    standard
                        .function_owner_overrides
                        .entry(database.id)
                        .or_default()
                        .insert(id, owner);
                    txn.Set(
                        key,
                        serde_json::to_vec(&standard)
                            .map_err(|e| kv::errors::New(e.to_string()))?,
                    )
                })
            })
            .map_err(|e| session_error("persist PostgreSQL function owner", e))
    }
}

fn default_namespace_acl(owner: i64) -> Vec<super::pg_database::PgDatabaseGrant> {
    vec![super::pg_database::PgDatabaseGrant {
        grantor: owner,
        grantee: owner,
        privileges: 5,
        grant_options: 0,
    }]
}
fn namespace_acl_fact(
    ns: &Namespace,
    roles: &[super::pg_database::PgRoleFact],
) -> super::pg_database::PgDatabaseFact {
    let names = roles
        .iter()
        .map(|r| (r.oid, r.name.clone()))
        .collect::<BTreeMap<_, _>>();
    super::pg_database::PgDatabaseFact {
        id: ns.id,
        owner: ns
            .owner_id
            .filter(|id| names.get(id) == Some(&ns.owner))
            .map(|id| (id, ns.owner.clone())),
        can_connect: false,
        acl: ns
            .explicit_acl
            .and(ns.grants.clone())
            .filter(|entries| entries.iter().all(|e| e.privileges & !5 == 0)),
        acl_default: ns.explicit_acl == Some(false),
        warning: None,
        is_template: None,
        allow_connections: None,
        roles: Arc::new(names),
    }
}
pub fn pg_schema_privileges(text: &str) -> SessionResult<Vec<(u8, bool)>> {
    text.split(',')
        .map(|item| {
            let item = item.trim().to_ascii_uppercase();
            let (base, option) = item
                .strip_suffix(" WITH GRANT OPTION")
                .map_or((item.as_str(), false), |base| (base, true));
            let bit = match base {
                "USAGE" => 1,
                "CREATE" => 4,
                _ => {
                    return Err(namespace_error(
                        "22023",
                        "unrecognized schema privilege type",
                    ));
                }
            };
            Ok((bit, option))
        })
        .collect()
}
/// Namespace query and DDL consumers share the persisted ACL and membership graph.
pub fn pg_has_schema_privilege(
    namespaces: &[PgNamespaceFact],
    roles: &[super::pg_database::PgRoleFact],
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
            .map_err(|_| namespace_error("22P02", "invalid schema OID"))?;
        namespaces.iter().find(|f| pg_namespace_oid(f) == Some(oid))
    } else {
        Some(
            namespaces
                .iter()
                .find(|f| f.name == object)
                .ok_or_else(|| namespace_error("3F000", "schema does not exist"))?,
        )
    };
    let modes = pg_schema_privileges(values[offset + 1].as_deref().unwrap())?;
    let Some(fact) = fact else { return Ok(None) };
    let fact = fact
        .privilege_fact
        .as_ref()
        .ok_or_else(|| namespace_error("0A000", "schema ACL requires explicit migration"))?;
    let mut allowed = false;
    for (bit, option) in modes {
        allowed |= fact.has_privilege(roles, member, bit, option)?;
    }
    Ok(Some(allowed))
}
fn pg_namespace_oid(fact: &PgNamespaceFact) -> Option<i64> {
    if fact.name == "pg_catalog" {
        return Some(11);
    }
    if fact.id == 0 {
        return None;
    }
    let signed = i64::try_from(fact.id.unsigned_abs())
        .ok()?
        .checked_mul(2)?
        .checked_sub(i64::from(fact.id < 0))?;
    signed.checked_add(16384).filter(|oid| *oid < 0x4000_0000)
}

pub(super) fn namespace_role_dependency(
    txn: &dyn kv::Transaction,
    ctx: &kv::Context,
    database: i64,
    oid: i64,
    role: &str,
) -> Result<(), kv::errors::SharedError> {
    let key =
        astersql_meta::transaction_meta_hash_key(b"PgNamespaces", database.to_string().as_bytes());
    let namespaces: BTreeMap<String, Namespace> = match txn.Get(ctx, key, &[]) {
        Ok(v) => serde_json::from_slice(&v.Value).map_err(|e| kv::errors::New(e.to_string()))?,
        Err(e) if kv::IsErrNotFound(&e) => BTreeMap::new(),
        Err(e) => return Err(e),
    };
    if namespaces.values().any(|ns| {
        ns.owner_id == Some(oid)
            || ns.usage.contains(role)
            || ns.create.contains(role)
            || ns
                .grants
                .as_ref()
                .is_some_and(|a| a.iter().any(|e| e.grantor == oid || e.grantee == oid))
    }) {
        return Err(kv::errors::New("role has schema dependencies"));
    }
    Ok(())
}

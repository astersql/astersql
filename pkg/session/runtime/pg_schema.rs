// Copyright 2026 AsterSQL.
//! Database-scoped PostgreSQL namespace identities, relation binding and USAGE ACLs.
//! These metadata-only operations never create a native MySQL database.
use super::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

// Match the runtime's process-local DDL boundary while checking namespace
// dependencies. Durable allocation itself remains a retried KV transaction.
pub(super) static PG_NAMESPACE_DDL_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Debug)]
pub enum PgSchemaCommand {
    Create(String),
    Drop(String),
    Usage {
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
    usage: BTreeSet<String>,
}

impl ConcreteSession {
    /// Keep connection-local path separate from the native current database.
    pub fn pg_search_path(&self, path: Vec<String>) {
        self.state.borrow_mut().pg_search_path = path;
    }

    // Bootstrap PG authentication currently admits root only. SET ROLE already
    // verifies membership in the shared role owner before replacing this actor.
    fn pg_schema_identity(&self) -> String {
        self.active_roles
            .borrow()
            .first()
            .map(|r| r.Username.clone())
            .unwrap_or_else(|| self.login_user.clone().unwrap_or_else(|| "root".into()))
    }

    fn pg_namespaces(
        &self,
        command: Option<&PgSchemaCommand>,
    ) -> SessionResult<BTreeMap<String, Namespace>> {
        let database = self
            .domain
            .info_schema()
            .SchemaByName(&astersql_infoschema::CiString::new(self.current_database()))
            .ok_or_else(|| SessionError::new("PostgreSQL database is unavailable"))?;
        let actor = self.pg_schema_identity();
        let context = kv::WithInternalSourceType(kv::Context::default(), kv::InternalTxnDDL);
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
                                        id: 0,
                                        owner: "root".into(),
                                        usage: BTreeSet::from(["PUBLIC".into()]),
                                    },
                                ),
                                (
                                    "pg_catalog".into(),
                                    Namespace {
                                        id: 0,
                                        owner: "root".into(),
                                        usage: BTreeSet::from(["PUBLIC".into()]),
                                    },
                                ),
                            ]),
                            Err(error) => return Err(error),
                        };
                    let mut migrated = false;
                    for (name, namespace) in &mut namespaces {
                        if namespace.id == 0 && !matches!(name.as_str(), "public" | "pg_catalog") {
                            namespace.id = allocate_namespace_id(transaction, &context)?;
                            migrated = true;
                        }
                    }
                    if let Some(command) = command {
                        match command {
                            PgSchemaCommand::Create(name) => {
                                if actor != "root" {
                                    return Err(kv::errors::New(
                                        "permission denied to create schema",
                                    ));
                                }
                                if namespaces.contains_key(name) {
                                    return Err(kv::errors::New("schema already exists"));
                                }
                                namespaces.insert(
                                    name.clone(),
                                    Namespace {
                                        id: allocate_namespace_id(transaction, &context)?,
                                        owner: actor.clone(),
                                        usage: BTreeSet::new(),
                                    },
                                );
                            }
                            PgSchemaCommand::Drop(name) => {
                                if matches!(name.as_str(), "public" | "pg_catalog") {
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
                            PgSchemaCommand::Usage {
                                schema,
                                grantee,
                                grant,
                            } => {
                                let schema = namespaces
                                    .get_mut(schema)
                                    .ok_or_else(|| kv::errors::New("schema does not exist"))?;
                                if actor != "root" && actor != schema.owner {
                                    return Err(kv::errors::New("must be owner of schema"));
                                }
                                if *grant {
                                    schema.usage.insert(grantee.clone());
                                } else {
                                    schema.usage.remove(grantee);
                                }
                            }
                        }
                        // Remove the empty extension record after the last owned schema
                        // is dropped, retaining modified built-in ACLs when present.
                        let defaults = namespaces.len() == 2
                            && namespaces.iter().all(|(name, ns)| {
                                matches!(name.as_str(), "public" | "pg_catalog")
                                    && ns.owner == "root"
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
                    if migrated && command.is_none() {
                        transaction.Set(
                            key,
                            serde_json::to_vec(&namespaces)
                                .map_err(|e| kv::errors::New(e.to_string()))?,
                        )?;
                    }
                    result = namespaces;
                    Ok(())
                })
            })
            .map_err(|e| session_error("PostgreSQL schema metadata", e))?;
        Ok(result)
    }

    pub fn pg_schema_command(&self, command: &PgSchemaCommand) -> SessionResult<()> {
        let _guard = PG_NAMESPACE_DDL_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.sql_dialect() != ast::SqlDialect::PostgreSQL {
            return Err(SessionError::new(
                "PostgreSQL schema metadata requires PostgreSQL dialect",
            ));
        }
        if let PgSchemaCommand::Usage { grantee, .. } = command {
            if grantee != "PUBLIC"
                && !runtime_privilege_handle(&self.domain)
                    .Get()
                    .user
                    .iter()
                    .any(|u| u.base.User == *grantee)
            {
                return Err(SessionError::new("schema grantee role does not exist"));
            }
        }
        self.pg_namespaces(Some(command))?;
        Ok(())
    }

    /// Read fresh committed metadata for each statement, including after GRANT/REVOKE.
    pub fn pg_effective_path(&self) -> SessionResult<Vec<String>> {
        let namespaces = self.pg_namespaces(None)?;
        let actor = self.pg_schema_identity();
        let mut result = Vec::new();
        for entry in &self.state.borrow().pg_search_path {
            let name = if entry == "$user" { &actor } else { entry };
            if let Some(schema) = namespaces.get(name)
                && (actor == "root"
                    || actor == schema.owner
                    || schema.usage.contains("PUBLIC")
                    || schema.usage.contains(&actor))
                && !result.contains(name)
            {
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
    /// Creation is currently limited to the namespace owner or bootstrap admin.
    pub can_create: bool,
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

fn namespace_error(state: &'static str, message: &str) -> SessionError {
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
    pub fn pg_namespace_facts(&self) -> SessionResult<Vec<PgNamespaceFact>> {
        let actor = self.pg_schema_identity();
        let database = self
            .domain
            .info_schema()
            .SchemaByName(&astersql_infoschema::CiString::new(self.current_database()))
            .ok_or_else(|| namespace_error("3D000", "no current database selected"))?;
        Ok(self
            .pg_namespaces(None)?
            .into_iter()
            .map(|(name, namespace)| PgNamespaceFact {
                id: if name == "public" {
                    database.id
                } else {
                    namespace.id
                },
                can_create: actor == "root" || actor == namespace.owner,
                usage: actor == "root"
                    || actor == namespace.owner
                    || namespace.usage.contains("PUBLIC")
                    || namespace.usage.contains(&actor),
                name,
            })
            .collect())
    }

    /// Resolve logical PG identity to the existing native table storage key.
    /// Creation uses the first effective explicit namespace, never a fallback database.
    pub fn pg_bind_relation(
        &self,
        schema: Option<&str>,
        name: &str,
        create: bool,
    ) -> SessionResult<String> {
        let facts = self.pg_namespace_facts()?;
        let path = if let Some(schema) = schema {
            let fact = facts
                .iter()
                .find(|f| f.name == schema)
                .ok_or_else(|| namespace_error("3F000", "schema does not exist"))?;
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

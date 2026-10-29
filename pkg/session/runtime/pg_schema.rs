// Copyright 2026 AsterSQL.
//! Database-scoped PostgreSQL empty namespaces and USAGE ACLs.
//! These metadata-only operations never create a native MySQL database.
use super::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

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
                                        owner: "root".into(),
                                        usage: BTreeSet::from(["PUBLIC".into()]),
                                    },
                                ),
                                (
                                    "pg_catalog".into(),
                                    Namespace {
                                        owner: "root".into(),
                                        usage: BTreeSet::from(["PUBLIC".into()]),
                                    },
                                ),
                            ]),
                            Err(error) => return Err(error),
                        };
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
                            transaction.Delete(key)?;
                        } else {
                            transaction.Set(
                                key,
                                serde_json::to_vec(&namespaces)
                                    .map_err(|e| kv::errors::New(e.to_string()))?,
                            )?;
                        }
                    }
                    result = namespaces;
                    Ok(())
                })
            })
            .map_err(|e| session_error("PostgreSQL schema metadata", e))?;
        Ok(result)
    }

    pub fn pg_schema_command(&self, command: &PgSchemaCommand) -> SessionResult<()> {
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

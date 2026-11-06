// Copyright 2026 AsterSQL.
//! Bounded PostgreSQL sequence operations over shared metadata and canonical KV.
use super::pg_schema::namespace_error;
use super::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Sequence operations accepted by the bounded PostgreSQL protocol adapter.
/// Names are decoded identifiers; CREATE options are validated integer options.
#[derive(Clone, Debug)]
pub enum PgSequenceCommand {
    /// One bounded identity declaration change; allocation state is preserved.
    AlterIdentity {
        schema: Option<String>,
        name: String,
        column: String,
        option: String,
        value: i64,
    },
    Create {
        schema: Option<String>,
        name: String,
        options: String,
        declared_type: astersql_meta_model::PostgreSQLSequenceType,
        owned_by: Option<(Option<String>, String, String)>,
    },
    OwnedBy {
        schema: Option<String>,
        name: String,
        owner: Option<(Option<String>, String, String)>,
    },
    Rename {
        schema: Option<String>,
        name: String,
        new_name: String,
    },
    Drop {
        schema: Option<String>,
        name: String,
    },
    Grant {
        schema: Option<String>,
        name: String,
        privileges: Vec<String>,
        grantee: String,
        grant: bool,
    },
    Set {
        schema: Option<String>,
        name: Option<String>,
        value: Option<i64>,
        is_called: Option<bool>,
    },
    Value {
        schema: Option<String>,
        name: Option<String>,
        next: bool,
    },
}
#[derive(Default, Serialize, Deserialize)]
struct SequenceAcl {
    owner: String,
    grants: BTreeMap<String, BTreeSet<String>>,
}
impl ConcreteSession {
    fn sequence_acl(
        &self,
        id: i64,
        owner: Option<&str>,
        change: Option<(&[String], &str, bool)>,
    ) -> SessionResult<SequenceAcl> {
        let context = kv::WithInternalSourceType(kv::Context::default(), kv::InternalTxnDDL);
        let actor = self.pg_schema_identity();
        let mut result = SequenceAcl::default();
        self.domain
            .storage_handle()
            .with_storage(|store| {
                kv::RunInNewTxn(&context, store, true, |_, transaction| {
                    let key = astersql_meta::transaction_meta_hash_key(
                        b"PgSequenceAcl",
                        id.to_string().as_bytes(),
                    );
                    let mut acl: SequenceAcl = match transaction.Get(&context, key.clone(), &[]) {
                        Ok(value) => serde_json::from_slice(&value.Value)
                            .map_err(|e| kv::errors::New(e.to_string()))?,
                        Err(error) if kv::IsErrNotFound(&error) => SequenceAcl {
                            owner: owner.unwrap_or("root").into(),
                            ..Default::default()
                        },
                        Err(error) => return Err(error),
                    };
                    if let Some((privileges, grantee, grant)) = change {
                        if actor != "root" && actor != acl.owner {
                            return Err(kv::errors::New("permission denied for sequence ACL"));
                        }
                        let grants = acl.grants.entry(grantee.into()).or_default();
                        for privilege in privileges {
                            if grant {
                                grants.insert(privilege.clone());
                            } else {
                                grants.remove(privilege);
                            }
                        }
                    }
                    if owner.is_some() || change.is_some() {
                        transaction.Set(
                            key,
                            serde_json::to_vec(&acl).map_err(|e| kv::errors::New(e.to_string()))?,
                        )?;
                    }
                    result = acl;
                    Ok(())
                })
            })
            .map_err(|error| {
                if error
                    .to_string()
                    .contains("permission denied for sequence ACL")
                {
                    namespace_error("42501", "must be owner of sequence")
                } else {
                    session_error("sequence ACL metadata", error)
                }
            })?;
        Ok(result)
    }

    fn pg_sequence_owner(
        &self,
        owner: Option<&(Option<String>, String, String)>,
    ) -> SessionResult<Option<(i64, i64)>> {
        let Some((schema, name, column)) = owner else {
            return Ok(None);
        };
        let physical = self.pg_bind_relation(schema.as_deref(), name, false)?;
        let target = self
            .domain
            .info_schema()
            .ModelTableInfoByExactName(
                &astersql_infoschema::CiString::new(&self.current_database()),
                &physical,
            )
            .map_err(|_| namespace_error("42P01", "OWNED BY relation does not exist"))?;
        let column = target
            .Columns
            .iter()
            .find(|c| c.Name.O == *column && !c.Hidden)
            .ok_or_else(|| namespace_error("42703", "OWNED BY column does not exist"))?;
        Ok(Some((target.ID, column.ID)))
    }

    /// Execute against the shared object and ACL, updating currval only on success.
    pub fn pg_sequence(&self, command: &PgSequenceCommand) -> SessionResult<Option<i64>> {
        if self.sql_dialect() != ast::SqlDialect::PostgreSQL {
            return Err(namespace_error(
                "0A000",
                "sequence operation requires PostgreSQL dialect",
            ));
        }
        if let PgSequenceCommand::AlterIdentity {
            schema,
            name,
            column,
            option,
            value,
        } = command
        {
            let physical = self.pg_bind_relation(schema.as_deref(), name, false)?;
            let database = self.current_database();
            let table = self
                .domain
                .info_schema()
                .ModelTableInfoByExactName(
                    &astersql_infoschema::CiString::new(&database),
                    &physical,
                )
                .map_err(|_| namespace_error("42P01", "relation does not exist"))?;
            let actor = self.pg_schema_identity();
            if actor != "root" && table.PostgreSQLOwner != Some(self.pg_database_role_id(&actor)?) {
                return Err(namespace_error("42501", "must be owner of table"));
            }
            let column = table
                .Columns
                .iter()
                .find(|c| c.Name.O == *column && !c.Hidden)
                .ok_or_else(|| namespace_error("42703", "column does not exist"))?;
            if column.PostgreSQLIdentity.is_none() {
                return Err(namespace_error("55000", "column is not an identity column"));
            }
            self.domain
                .ddl_alter_postgresql_identity(&database, table.ID, column.ID, option, *value)
                .map_err(|e| {
                    namespace_error(
                        if e.to_string().contains("unsupported") {
                            "0A000"
                        } else {
                            "22023"
                        },
                        "invalid identity sequence configuration",
                    )
                })?;
            return Ok(None);
        }
        let (schema, name) = match command {
            PgSequenceCommand::AlterIdentity { .. } => unreachable!("handled above"),
            PgSequenceCommand::Value { name: None, .. }
            | PgSequenceCommand::Set { name: None, .. }
            | PgSequenceCommand::Set { value: None, .. }
            | PgSequenceCommand::Set {
                is_called: None, ..
            } => return Ok(None),
            PgSequenceCommand::Value {
                schema,
                name: Some(name),
                ..
            }
            | PgSequenceCommand::Set {
                schema,
                name: Some(name),
                ..
            }
            | PgSequenceCommand::Create { schema, name, .. }
            | PgSequenceCommand::OwnedBy { schema, name, .. }
            | PgSequenceCommand::Rename { schema, name, .. }
            | PgSequenceCommand::Drop { schema, name }
            | PgSequenceCommand::Grant { schema, name, .. } => (schema.as_deref(), name.as_str()),
        };
        let create = matches!(command, PgSequenceCommand::Create { .. });
        let physical = self.pg_bind_relation(schema, name, create)?;
        let database = self.current_database();
        let quoted = format!(
            "\"{}\".\"{}\"",
            database.replace('"', "\"\""),
            physical.replace('"', "\"\"")
        );
        if let PgSequenceCommand::Create {
            options,
            declared_type,
            owned_by,
            ..
        } = command
        {
            let mut parser = astersql_parser::Parser::with_dialect(ast::SqlDialect::PostgreSQL);
            let statement = parser
                .ParseOneStmt(&format!("CREATE SEQUENCE {quoted} {options}"), "", "")
                .map_err(|e| namespace_error("42601", &e.to_string()))?;
            let statement = statement
                .as_any()
                .downcast_ref::<ast::CreateSequenceStmt>()
                .ok_or_else(|| namespace_error("42601", "invalid CREATE SEQUENCE"))?;
            let owner = self.pg_sequence_owner(owned_by.as_ref())?;
            self.execute_create_sequence_owned(statement, Some(*declared_type), owner)
                .map_err(|error| {
                    if error.to_string().contains("must have same owner")
                        || error.to_string().contains("must be in same schema")
                    {
                        namespace_error("55000", &error.to_string())
                    } else {
                        error
                    }
                })?;
        }
        let table = self
            .domain
            .info_schema()
            .ModelTableInfoByExactName(&astersql_infoschema::CiString::new(&database), &physical)
            .map_err(|_| namespace_error("42P01", "sequence does not exist"))?;
        if table.Sequence.is_none() {
            return Err(namespace_error("42809", "object is not a sequence"));
        }
        let actor = self.pg_schema_identity();
        let acl = self.sequence_acl(table.ID, create.then_some(actor.as_str()), None)?;
        if create {
            return Ok(None);
        }
        match command {
            PgSequenceCommand::OwnedBy { owner, .. } => {
                if actor != "root" && actor != acl.owner {
                    return Err(namespace_error("42501", "must be owner of sequence"));
                }
                let owner = self.pg_sequence_owner(owner.as_ref())?;
                self.domain
                    .ddl_set_sequence_owner(&database, table.ID, owner)
                    .map_err(|e| {
                        namespace_error(
                            if e.to_string().contains("internal identity") {
                                "0A000"
                            } else {
                                "55000"
                            },
                            &e.to_string(),
                        )
                    })?;
                Ok(None)
            }
            PgSequenceCommand::Rename { new_name, .. } => {
                if actor != "root" && actor != acl.owner {
                    return Err(namespace_error("42501", "must be owner of sequence"));
                }
                let target = self.pg_bind_relation(schema, new_name, true)?;
                self.domain
                    .ddl_rename_tables(vec![(database.clone(), physical, database, target)])
                    .map_err(|e| session_error("rename sequence", e))?;
                Ok(None)
            }
            PgSequenceCommand::Drop { .. } => {
                if actor != "root" && actor != acl.owner {
                    return Err(namespace_error("42501", "must be owner of sequence"));
                }
                // Drop the already checked physical identity, preserving exact names.
                self.domain
                    .ddl_drop_table_ids(&[table.ID])
                    .map_err(|error| {
                        if error.to_string().contains("internal identity") {
                            namespace_error(
                                "2BP01",
                                "cannot drop internal identity sequence without its table",
                            )
                        } else {
                            session_error("drop sequence metadata", error)
                        }
                    })?;
                Ok(None)
            }
            PgSequenceCommand::Grant {
                privileges,
                grantee,
                grant,
                ..
            } => {
                if grantee != "PUBLIC"
                    && !runtime_privilege_handle(&self.domain)
                        .Get()
                        .user
                        .iter()
                        .any(|u| u.base.User == *grantee)
                {
                    return Err(namespace_error(
                        "42704",
                        "sequence grantee role does not exist",
                    ));
                }
                self.sequence_acl(table.ID, None, Some((privileges, grantee, *grant)))?;
                Ok(None)
            }
            PgSequenceCommand::Set {
                value: Some(value),
                is_called,
                ..
            } => {
                let has_update = [actor.as_str(), "PUBLIC"].iter().any(|role| {
                    acl.grants
                        .get(*role)
                        .is_some_and(|privileges| privileges.contains("update"))
                });
                if actor != "root" && actor != acl.owner && !has_update {
                    return Err(namespace_error("42501", "permission denied for sequence"));
                }
                let seq = table.Sequence.as_ref().unwrap();
                if *value < seq.MinValue || *value > seq.MaxValue {
                    return Err(namespace_error(
                        "22003",
                        "setval value is outside sequence bounds",
                    ));
                }
                let value = self
                    .domain
                    .sequence_set_value_with_state(&table, *value, *is_called == Some(true))
                    .map_err(|error| session_error("set sequence value", error))?;
                // Other connections keep their preallocated values after setval.
                self.state.borrow_mut().pg_sequence_cache.remove(&table.ID);
                if *is_called == Some(true) {
                    self.state
                        .borrow_mut()
                        .pg_sequence_values
                        .insert(table.ID, value);
                }
                Ok(Some(value))
            }
            PgSequenceCommand::Set { value: None, .. } => unreachable!(),
            PgSequenceCommand::Value { next, .. } => {
                let has = |privilege: &str| {
                    [actor.as_str(), "PUBLIC"].iter().any(|role| {
                        acl.grants
                            .get(*role)
                            .is_some_and(|privileges| privileges.contains(privilege))
                    })
                };
                if actor != "root"
                    && actor != acl.owner
                    && !has("usage")
                    && !has(if *next { "update" } else { "select" })
                {
                    return Err(namespace_error("42501", "permission denied for sequence"));
                }
                if *next {
                    let seq = table.Sequence.as_ref().unwrap();
                    let cached = self.state.borrow_mut().pg_sequence_cache.remove(&table.ID);
                    let (value, last) = if let Some(range) = cached {
                        range
                    } else {
                        self.domain
                            .sequence_reserve_range(
                                &table,
                                if seq.Cache { seq.CacheValue } else { 1 },
                            )
                            .map_err(|error| match error {
                                astersql_meta_autoid::AutoIdError::AutoIncrementReadFailed(_) => {
                                    namespace_error("2200H", "sequence has reached its limit")
                                }
                                _ => session_error("allocate sequence value", error),
                            })?
                    };
                    if value != last {
                        // The reserved endpoint proves this addition remains within bigint.
                        let next = (i128::from(value) + i128::from(seq.Increment)) as i64;
                        self.state
                            .borrow_mut()
                            .pg_sequence_cache
                            .insert(table.ID, (next, last));
                    }
                    self.state
                        .borrow_mut()
                        .pg_sequence_values
                        .insert(table.ID, value);
                    Ok(Some(value))
                } else {
                    self.state
                        .borrow()
                        .pg_sequence_values
                        .get(&table.ID)
                        .copied()
                        .map(Some)
                        .ok_or_else(|| {
                            namespace_error(
                                "55000",
                                "currval of sequence is not yet defined in this session",
                            )
                        })
                }
            }
            PgSequenceCommand::Create { .. } | PgSequenceCommand::AlterIdentity { .. } => {
                unreachable!()
            }
        }
    }
}

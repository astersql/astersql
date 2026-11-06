// Copyright 2026 AsterSQL.
//! Table ACL consumers over canonical relation metadata and the shared role graph.
use super::pg_database::{PgDatabaseFact, PgRoleFact, update_object_acl};
use super::pg_schema::{PG_NAMESPACE_DDL_LOCK, namespace_error};
use super::*;
use astersql_meta_model::{PostgreSQLAclGrant, PostgreSQLRelationAcl, TableInfo};
use std::collections::BTreeMap;

// PG18 information_schema deliberately excludes MAINTAIN from table listings,
// and SELECT from writable constraint visibility. Columns admit arwx only.
pub(super) const TABLE_LISTING_PRIVILEGES: u8 = 127;
pub(super) const CONSTRAINT_VISIBLE_TABLE_PRIVILEGES: u8 = 125;
pub(super) const COLUMN_VISIBLE_TABLE_PRIVILEGES: u8 = 39;

pub(super) fn default_table_acl(owner: i64) -> PostgreSQLRelationAcl {
    PostgreSQLRelationAcl {
        default_acl: true,
        grants: vec![PostgreSQLAclGrant {
            grantor: owner,
            grantee: owner,
            privileges: u8::MAX,
            grant_options: 0,
        }],
    }
}
pub fn pg_table_privileges(text: &str) -> SessionResult<Vec<(u8, bool)>> {
    text.split(',')
        .map(|item| {
            let item = item.trim().to_ascii_uppercase();
            let (base, option) = item
                .strip_suffix(" WITH GRANT OPTION")
                .map_or((item.as_str(), false), |s| (s, true));
            let bit = match base {
                "INSERT" => 1,
                "SELECT" => 2,
                "UPDATE" => 4,
                "DELETE" => 8,
                "TRUNCATE" => 16,
                "REFERENCES" => 32,
                "TRIGGER" => 64,
                "MAINTAIN" => 128,
                _ => {
                    return Err(namespace_error(
                        "22023",
                        "unrecognized table privilege type",
                    ));
                }
            };
            Ok((bit, option))
        })
        .collect()
}
pub fn pg_table_acl_fact(table: &TableInfo, roles: &[PgRoleFact]) -> PgDatabaseFact {
    let names: BTreeMap<_, _> = roles.iter().map(|r| (r.oid, r.name.clone())).collect();
    PgDatabaseFact {
        id: table.ID,
        owner: table
            .PostgreSQLOwner
            .and_then(|id| names.get(&id).map(|name| (id, name.clone()))),
        can_connect: false,
        acl: table
            .PostgreSQLAcl
            .as_ref()
            .filter(|a| {
                !a.default_acl
                    || table
                        .PostgreSQLOwner
                        .is_some_and(|owner| **a == default_table_acl(owner))
            })
            .map(|a| a.grants.clone()),
        acl_default: table.PostgreSQLAcl.as_ref().is_some_and(|a| a.default_acl),
        warning: None,
        is_template: None,
        allow_connections: None,
        roles: Arc::new(names),
    }
}
pub fn pg_table_acl_text(table: &TableInfo, roles: &[PgRoleFact]) -> SessionResult<Option<String>> {
    let fact = pg_table_acl_fact(table, roles);
    if fact.owner.is_none() {
        return Err(namespace_error("0A000", "table owner is unavailable"));
    }
    fact.acl_text_with_modes(&[
        (1, 'a'),
        (2, 'r'),
        (4, 'w'),
        (8, 'd'),
        (16, 'D'),
        (32, 'x'),
        (64, 't'),
        (128, 'm'),
    ])
}
/// Same disjoint checked identity mapping as pg_class, including local temporary IDs.
pub fn pg_table_oid(id: i64) -> Option<i64> {
    if id == 0 {
        return None;
    }
    let oid = u128::from(id.unsigned_abs()) * 2 - u128::from(id < 0) + 0x4000_0000;
    (oid < 0x8000_0000).then_some(oid as i64)
}

/// Resolve an SQL relation name without reading data or requiring table privileges.
pub fn pg_has_table_privilege(
    tables: &[(String, String, TableInfo)],
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
    let table = if oids[offset] {
        let oid = object
            .parse::<u32>()
            .map(i64::from)
            .map_err(|_| namespace_error("22P02", "invalid relation OID"))?;
        tables
            .iter()
            .find(|(_, _, t)| pg_table_oid(t.ID) == Some(oid))
    } else {
        let parts = table_name_parts(object)?;
        let found = match parts.as_slice() {
            [name] => path.iter().find_map(|ns| {
                tables
                    .iter()
                    .find(|(schema, n, _)| schema == ns && n == name)
            }),
            [ns, name] => {
                // Lookup checks the caller's namespace USAGE, independently of
                // the role whose table ACL is queried (PG RangeVarGetRelid).
                let namespace = namespaces
                    .iter()
                    .find(|f| f.name == *ns)
                    .ok_or_else(|| namespace_error("3F000", "schema does not exist"))?;
                if !namespace.usage {
                    return Err(namespace_error("42501", "permission denied for schema"));
                }
                tables
                    .iter()
                    .find(|(schema, n, _)| schema == ns && n == name)
            }
            _ => {
                return Err(namespace_error(
                    "0A000",
                    "cross-database relation references are unsupported",
                ));
            }
        };
        Some(found.ok_or_else(|| namespace_error("42P01", "relation does not exist"))?)
    };
    let modes = pg_table_privileges(values[offset + 1].as_deref().unwrap())?;
    let Some((_, _, table)) = table else {
        return Ok(None);
    };
    if table.Sequence.is_some() {
        return Err(namespace_error(
            "0A000",
            "sequence table privilege facts are unavailable",
        ));
    }
    let fact = pg_table_acl_fact(table, roles);
    let mut allowed = false;
    for (bit, option) in modes {
        allowed |= fact.has_privilege_mask(roles, member, bit, option, u8::MAX)?;
    }
    Ok(Some(allowed))
}

pub fn pg_column_acl_fact(
    table: &TableInfo,
    column: i64,
    roles: &[PgRoleFact],
) -> SessionResult<PgDatabaseFact> {
    let acls = table
        .PostgreSQLColumnAcls
        .as_ref()
        .ok_or_else(|| namespace_error("0A000", "column ACL requires explicit migration"))?;
    let mut fact = pg_table_acl_fact(table, roles);
    if fact.owner.is_none() {
        return Err(namespace_error("0A000", "column ACL owner is unavailable"));
    }
    fact.acl = Some(
        acls.get(&column)
            .map_or_else(Vec::new, |a| a.grants.clone()),
    );
    fact.acl_default = !acls.contains_key(&column);
    if acls.get(&column).is_some_and(|a| a.default_acl) {
        return Err(namespace_error(
            "0A000",
            "explicit column ACL cannot be default",
        ));
    }
    fact.validate_acl_mask(COLUMN_VISIBLE_TABLE_PRIVILEGES)?;
    Ok(fact)
}
pub fn pg_column_acl_text(
    table: &TableInfo,
    column: i64,
    roles: &[PgRoleFact],
) -> SessionResult<Option<String>> {
    pg_column_acl_fact(table, column, roles)?.acl_text_with_modes(&[
        (1, 'a'),
        (2, 'r'),
        (4, 'w'),
        (32, 'x'),
    ])
}
pub fn pg_has_column_privilege(
    tables: &[(String, String, TableInfo)],
    path: &[String],
    namespaces: &[PgNamespaceFact],
    roles: &[PgRoleFact],
    actor: &str,
    values: &[Option<String>],
    oids: &[bool],
    any: bool,
) -> SessionResult<Option<bool>> {
    if values.iter().any(Option::is_none) {
        return Ok(None);
    }
    let offset = usize::from(values.len() == if any { 3 } else { 4 });
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
    let table = if oids[offset] {
        let oid = object
            .parse::<u32>()
            .map(i64::from)
            .map_err(|_| namespace_error("22P02", "invalid relation OID"))?;
        tables
            .iter()
            .find(|(_, _, t)| pg_table_oid(t.ID) == Some(oid))
    } else {
        let parts = table_name_parts(object)?;
        let found = match parts.as_slice() {
            [name] => path.iter().find_map(|ns| {
                tables
                    .iter()
                    .find(|(schema, n, _)| schema == ns && n == name)
            }),
            [ns, name] => {
                // Lookup checks the caller's namespace USAGE, independently of
                // the role whose table ACL is queried (PG RangeVarGetRelid).
                let namespace = namespaces
                    .iter()
                    .find(|f| f.name == *ns)
                    .ok_or_else(|| namespace_error("3F000", "schema does not exist"))?;
                if !namespace.usage {
                    return Err(namespace_error("42501", "permission denied for schema"));
                }
                tables
                    .iter()
                    .find(|(schema, n, _)| schema == ns && n == name)
            }
            _ => {
                return Err(namespace_error(
                    "0A000",
                    "cross-database relation references are unsupported",
                ));
            }
        };
        Some(found.ok_or_else(|| namespace_error("42P01", "relation does not exist"))?)
    };
    let modes = pg_table_privileges(values.last().unwrap().as_deref().unwrap())?;
    if modes
        .iter()
        .any(|(bit, _)| bit & COLUMN_VISIBLE_TABLE_PRIVILEGES == 0)
    {
        return Err(namespace_error(
            "22023",
            "unrecognized column privilege type",
        ));
    }
    let Some((_, _, table)) = table else {
        return Ok(None);
    };
    let columns = if any {
        table
            .Columns
            .iter()
            .filter(|c| c.State == astersql_meta_model::StatePublic && !c.Hidden)
            .map(|c| c.ID)
            .collect::<Vec<_>>()
    } else {
        let column = values[offset + 1].as_deref().unwrap();
        let slots = table
            .postgresql_attribute_slots()
            .map_err(|_| namespace_error("0A000", "column identity unavailable"))?;
        let id = if oids[offset + 1] {
            column
                .parse::<i16>()
                .ok()
                .and_then(|n| usize::try_from(i32::from(n) - 1).ok())
                .and_then(|i| slots.get(i))
                .filter(|(_, dropped)| !dropped)
                .map(|(id, _)| *id)
        } else {
            Some(
                table
                    .Columns
                    .iter()
                    .find(|c| {
                        c.Name.O == column
                            && c.State == astersql_meta_model::StatePublic
                            && !c.Hidden
                    })
                    .ok_or_else(|| namespace_error("42703", "column does not exist"))?
                    .ID,
            )
        };
        let Some(id) = id else {
            return Ok(None);
        };
        vec![id]
    };
    let table_fact = pg_table_acl_fact(table, roles);
    let mut result = false;
    for &(bit, option) in &modes {
        let table_allowed = table_fact.has_privilege_mask(roles, member, bit, option, u8::MAX)?;
        if table_allowed {
            result = true;
            continue;
        }
        for &column in &columns {
            result |= pg_column_acl_fact(table, column, roles)?.has_privilege_mask(
                roles,
                member,
                bit,
                option,
                COLUMN_VISIBLE_TABLE_PRIVILEGES,
            )?;
        }
    }
    Ok(Some(result))
}

fn table_name_parts(input: &str) -> SessionResult<Vec<String>> {
    let mut chars = input.trim().chars().peekable();
    let mut parts = Vec::new();
    loop {
        let mut part = String::new();
        if chars.peek() == Some(&'"') {
            chars.next();
            loop {
                match chars.next() {
                    Some('"') if chars.peek() == Some(&'"') => {
                        chars.next();
                        part.push('"');
                    }
                    Some('"') => break,
                    Some(c) => part.push(c),
                    None => return Err(namespace_error("42602", "invalid name syntax")),
                }
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c == '.' || c.is_whitespace() {
                    break;
                }
                if !(c.is_alphanumeric() || c == '_' || c == '$') {
                    return Err(namespace_error("42602", "invalid name syntax"));
                }
                part.extend(c.to_lowercase());
                chars.next();
            }
        }
        if part.is_empty() {
            return Err(namespace_error("42602", "invalid name syntax"));
        }
        parts.push(part);
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        match chars.next() {
            None => return Ok(parts),
            Some('.') if parts.len() < 3 => {
                while chars.peek().is_some_and(|c| c.is_whitespace()) {
                    chars.next();
                }
            }
            _ => return Err(namespace_error("42602", "invalid name syntax")),
        }
    }
}
pub(super) fn pg_select_columns(
    statement: &ast::SelectStmt,
    table_source: &ast::TableSource,
    model: &TableInfo,
) -> Vec<String> {
    struct References {
        names: Vec<String>,
        table: String,
        alias: String,
        all: bool,
    }
    impl ast::Visitor for References {
        fn enter(&mut self, node: &dyn ast::Node) -> bool {
            if let Some(field) = node.as_any().downcast_ref::<ast::SelectField>() {
                if let Some(wildcard) = &field.WildCard {
                    if wildcard.Table.O.is_empty()
                        || wildcard.Table.O == self.table
                        || wildcard.Table.O == self.alias
                    {
                        self.all = true;
                    }
                }
            }
            false
        }
        fn leave(&mut self, _: &dyn ast::Node) -> bool {
            true
        }
        fn enter_column_name(&mut self, c: &ast::ColumnName) -> bool {
            if c.Table.O.is_empty() || c.Table.O == self.table || c.Table.O == self.alias {
                self.names.push(c.Name.O.clone());
            }
            false
        }
    }
    let mut references = References {
        names: Vec::new(),
        table: table_source.Source.Name.O.clone(),
        alias: table_source.AsName.O.clone(),
        all: statement.Fields.Fields.iter().any(|f| {
            f.WildCard.as_ref().is_some_and(|w| {
                w.Table.O.is_empty()
                    || w.Table.O == table_source.Source.Name.O
                    || w.Table.O == table_source.AsName.O
            })
        }),
    };
    ast::Node::accept(statement, &mut references);
    if references.all {
        references.names = model
            .Columns
            .iter()
            .filter(|c| c.State == astersql_meta_model::StatePublic && !c.Hidden)
            .map(|c| c.Name.O.clone())
            .collect();
    }
    references.names
}

impl ConcreteSession {
    pub(super) fn pg_table_models(&self) -> SessionResult<Vec<(String, String, TableInfo)>> {
        let snapshot = self.domain.info_schema();
        let database = self.current_database();
        let schema = snapshot
            .SchemaByName(&astersql_infoschema::CiString::new(&database))
            .ok_or_else(|| namespace_error("3D000", "database does not exist"))?;
        let mut tables = snapshot
            .SchemaTableInfos(&schema.name)
            .map_err(|e| session_error("read relation ACL models", e))?
            .into_iter()
            .filter_map(|t| t.model_meta.as_ref().map(|m| (**m).clone()))
            .collect::<Vec<_>>();
        tables.extend(self.pg_local_temporary_models());
        let namespaces = self.pg_namespace_facts()?;
        Ok(tables
            .into_iter()
            .map(|m| {
                let (ns, name) = m.PostgreSQLNamespace.as_ref().map_or_else(
                    || ("public".to_owned(), m.Name.O.clone()),
                    |(id, name)| {
                        (
                            namespaces
                                .iter()
                                .find(|f| f.id == *id)
                                .map(|f| f.name.clone())
                                .unwrap_or_default(),
                            name.clone(),
                        )
                    },
                );
                (ns, name, m)
            })
            .collect())
    }
    pub(super) fn pg_table_privilege_command(
        &self,
        command: &PgSchemaCommand,
    ) -> SessionResult<Option<(&'static str, String)>> {
        let PgSchemaCommand::TablePrivileges {
            columns,
            schema,
            table,
            grantee,
            public,
            privileges,
            grant,
            grant_option,
            cascade,
        } = command
        else {
            return Err(namespace_error("0A000", "expected table ACL command"));
        };
        let _guard = PG_NAMESPACE_DDL_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let actor = self.pg_schema_identity();
        let roles = if actor == "root" {
            self.pg_namespace_role_identities()?
        } else {
            self.pg_role_facts()?
        };
        let actor_id = roles
            .iter()
            .find(|r| r.name == actor)
            .ok_or_else(|| namespace_error("42704", "role does not exist"))?
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
        if *grant && *grant_option && grantee == 0 {
            return Err(namespace_error(
                "0LP01",
                "grant options cannot be granted to PUBLIC",
            ));
        }
        let bits = if privileges == "ALL" {
            u8::MAX
        } else {
            pg_table_privileges(privileges)?
                .iter()
                .fold(0, |bits, (bit, _)| bits | bit)
        };
        if !columns.is_empty()
            && privileges != "ALL"
            && bits & !COLUMN_VISIBLE_TABLE_PRIVILEGES != 0
        {
            return Err(namespace_error("22023", "invalid column privilege"));
        }
        let bits = if !columns.is_empty() && privileges == "ALL" {
            COLUMN_VISIBLE_TABLE_PRIVILEGES
        } else {
            bits
        };
        let physical = self.pg_bind_relation(schema.as_deref(), table, false)?;
        let database = self.current_database();
        let model = self
            .domain
            .table_by_name(&database, &physical)
            .map_err(|e| session_error("resolve table ACL", e))?;
        if model.Sequence.is_some() {
            return Err(namespace_error(
                "0A000",
                "sequence table ACL is unavailable",
            ));
        }
        let mut warning = None;
        self.domain
            .ddl_update_postgresql_acl(&database, model.ID, |model| {
                let fact = pg_table_acl_fact(model, &roles);
                fact.validate_acl_mask(u8::MAX)
                    .map_err(|e| kv::errors::New(e.to_string()))?;
                let owner = fact
                    .owner
                    .ok_or_else(|| kv::errors::New("table ACL owner is unavailable"))?
                    .0;
                let ids = columns
                    .iter()
                    .map(|name| {
                        model
                            .Columns
                            .iter()
                            .find(|c| {
                                c.Name.O == *name
                                    && c.State == astersql_meta_model::StatePublic
                                    && !c.Hidden
                            })
                            .map(|c| c.ID)
                            .ok_or_else(|| kv::errors::New("column does not exist"))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                // Validate all targets before changing anything; the catalog transaction publishes atomically.
                for &id in &ids {
                    pg_column_acl_fact(model, id, &roles)
                        .map_err(|e| kv::errors::New(e.to_string()))?;
                }
                if ids.is_empty() {
                    let acl = model
                        .PostgreSQLAcl
                        .as_mut()
                        .ok_or_else(|| kv::errors::New("table ACL requires migration"))?;
                    let (notice, modified) = update_object_acl(
                        &mut acl.grants,
                        owner,
                        &actor,
                        actor_id,
                        &roles,
                        bits,
                        grantee,
                        *grant,
                        *grant_option,
                        *cascade,
                        privileges == "ALL",
                    )?;
                    warning = notice;
                    if modified {
                        acl.default_acl = false;
                        if !*grant {
                            let authority = acl.grants.clone();
                            if let Some(columns) = model.PostgreSQLColumnAcls.as_mut() {
                                for acl in columns.values_mut() {
                                    super::pg_database::update_object_acl_with_authority(
                                        &mut acl.grants,
                                        owner,
                                        &actor,
                                        owner,
                                        &roles,
                                        0,
                                        0,
                                        false,
                                        false,
                                        *cascade,
                                        true,
                                        &authority,
                                    )?;
                                }
                            }
                        }
                    }
                } else {
                    let table_grants = model.PostgreSQLAcl.as_ref().unwrap().grants.clone();
                    for id in ids {
                        let map = model
                            .PostgreSQLColumnAcls
                            .as_mut()
                            .ok_or_else(|| kv::errors::New("column ACL requires migration"))?;
                        let mut grants =
                            map.get(&id).map_or_else(Vec::new, |acl| acl.grants.clone());
                        let (notice, modified) =
                            super::pg_database::update_object_acl_with_authority(
                                &mut grants,
                                owner,
                                &actor,
                                actor_id,
                                &roles,
                                bits,
                                grantee,
                                *grant,
                                *grant_option,
                                *cascade,
                                privileges == "ALL",
                                &table_grants,
                            )?;
                        warning = notice;
                        if modified {
                            if grants.is_empty() {
                                map.remove(&id);
                            } else {
                                map.insert(
                                    id,
                                    PostgreSQLRelationAcl {
                                        default_acl: false,
                                        grants,
                                    },
                                );
                            }
                        }
                    }
                }
                Ok(())
            })
            .map_err(|e| {
                let message = e.to_string();
                let state = if message.contains("column does not exist") {
                    "42703"
                } else if message.contains("dependent database privileges") {
                    "2BP01"
                } else if message.contains("requires grant option") {
                    "42501"
                } else if message.contains("ACL") || message.contains("membership unknown") {
                    "0A000"
                } else {
                    "XX000"
                };
                namespace_error(state, &message)
            })?;
        Ok(warning)
    }
    /// Some means authoritative PG ACL; None retains the historical native path.
    pub(super) fn pg_table_access(
        &self,
        database: &str,
        name: &str,
        bit: u8,
    ) -> SessionResult<Option<bool>> {
        if self.sql_dialect() != ast::SqlDialect::PostgreSQL {
            return Ok(None);
        }
        let model = if let Some(local) = self.local_temporary_table(database, name) {
            Arc::new(local)
        } else {
            self.domain
                .table_by_name(database, name)
                .map_err(|e| session_error("resolve table access", e))?
        };
        self.pg_table_model_access(&model, bit)
    }
    pub(super) fn pg_table_model_access(
        &self,
        model: &TableInfo,
        bit: u8,
    ) -> SessionResult<Option<bool>> {
        if model.PostgreSQLAcl.is_none() {
            return Ok(None);
        }
        let actor = self.pg_schema_identity();
        let roles = if actor == "root" {
            self.pg_namespace_role_identities()?
        } else {
            self.pg_role_facts()?
        };
        let member = roles
            .iter()
            .find(|r| r.name == actor)
            .ok_or_else(|| namespace_error("42704", "role does not exist"))?
            .oid;
        pg_table_acl_fact(&model, &roles)
            .has_privilege_mask(&roles, member, bit, false, u8::MAX)
            .map(Some)
    }
    pub(super) fn pg_require_dml_read_access(
        &self,
        refs: Option<&ast::TableRefsClause>,
        returning: &[ast::SelectField],
        expressions: &[ast::ExprNode],
    ) -> SessionResult<()> {
        let mut fields = returning.to_vec();
        fields.extend(expressions.iter().cloned().map(|expr| ast::SelectField {
            Expr: Some(expr),
            ..Default::default()
        }));
        let select = ast::SelectStmt {
            From: refs.cloned(),
            Fields: ast::FieldList { Fields: fields },
            ..Default::default()
        };
        struct Reads(bool);
        impl ast::Visitor for Reads {
            fn enter(&mut self, _: &dyn ast::Node) -> bool {
                false
            }
            fn leave(&mut self, _: &dyn ast::Node) -> bool {
                true
            }
            fn enter_column_name(&mut self, _: &ast::ColumnName) -> bool {
                self.0 = true;
                false
            }
        }
        let mut reads = Reads(select.Fields.Fields.iter().any(|f| f.WildCard.is_some()));
        ast::Node::accept(&select, &mut reads);
        if !reads.0 {
            return Ok(());
        }
        self.pg_require_select_columns(&select)
    }
    pub(super) fn pg_require_select_columns(&self, select: &ast::SelectStmt) -> SessionResult<()> {
        if self.sql_dialect() != ast::SqlDialect::PostgreSQL {
            return Ok(());
        }
        let mut sources = Vec::new();
        if let Some(from) = &select.From {
            if let Some(left) = from.TableRefs.Left.as_deref() {
                collect_physical_table_sources(left, &mut sources);
            }
            if let Some(right) = from.TableRefs.Right.as_deref() {
                collect_physical_table_sources(right, &mut sources);
            }
        }
        for source in sources {
            let database = if source.Source.Schema.O.is_empty() {
                self.current_database()
            } else {
                source.Source.Schema.O.clone()
            };
            let Some((_, model)) = self.mdl_stats_table(&database, relation_name(&source.Source))
            else {
                continue;
            };
            if model.PostgreSQLAcl.is_none() {
                continue;
            }
            if self.pg_table_model_access(&model, 2)? == Some(true) {
                continue;
            }
            let columns = pg_select_columns(select, source, &model);
            if !self.pg_column_model_access(&model, &columns, 2)? {
                return Err(namespace_error("42501", "permission denied for table"));
            }
        }
        Ok(())
    }
    /// Check every referenced column, preserving the distinction between a table
    /// grant and column grants. Empty references (COUNT(*)/SELECT 1) need any column.
    pub(super) fn pg_column_model_access(
        &self,
        model: &TableInfo,
        columns: &[String],
        bit: u8,
    ) -> SessionResult<bool> {
        let roles = self.pg_role_facts()?;
        let actor = self.pg_schema_identity();
        let member = roles
            .iter()
            .find(|r| r.name == actor)
            .ok_or_else(|| namespace_error("42704", "role does not exist"))?
            .oid;
        let any = columns.is_empty();
        let columns = if columns.is_empty() {
            model
                .Columns
                .iter()
                .filter(|c| c.State == astersql_meta_model::StatePublic && !c.Hidden)
                .map(|c| c.ID)
                .collect::<Vec<_>>()
        } else {
            columns
                .iter()
                .filter_map(|name| {
                    model
                        .Columns
                        .iter()
                        .find(|c| {
                            c.Name.O == *name
                                && c.State == astersql_meta_model::StatePublic
                                && !c.Hidden
                        })
                        .map(|c| c.ID)
                })
                .collect()
        };
        let mut allowed = !any;
        for id in columns {
            let value = pg_column_acl_fact(model, id, &roles)?.has_privilege_mask(
                &roles,
                member,
                bit,
                false,
                COLUMN_VISIBLE_TABLE_PRIVILEGES,
            )?;
            if any {
                allowed |= value;
            } else {
                allowed &= value;
            }
        }
        Ok(allowed)
    }
    pub(super) fn pg_require_write_access(
        &self,
        refs: Option<&ast::TableRefsClause>,
        bit: u8,
    ) -> SessionResult<()> {
        self.pg_require_column_write_access(refs, bit, None)
    }
    pub(super) fn pg_require_column_write_access(
        &self,
        refs: Option<&ast::TableRefsClause>,
        bit: u8,
        columns: Option<&[String]>,
    ) -> SessionResult<()> {
        if self.sql_dialect() != ast::SqlDialect::PostgreSQL {
            return Ok(());
        }
        let mut sources = Vec::new();
        if let Some(refs) = refs {
            if let Some(left) = refs.TableRefs.Left.as_deref() {
                collect_physical_table_sources(left, &mut sources);
            }
            if let Some(right) = refs.TableRefs.Right.as_deref() {
                collect_physical_table_sources(right, &mut sources);
            }
        }
        for source in sources {
            let database = if source.Source.Schema.L.is_empty() {
                self.current_database()
            } else {
                source.Source.Schema.O.clone()
            };
            if self.pg_table_access(&database, relation_name(&source.Source), bit)? == Some(false) {
                if let Some(columns) = columns {
                    let model = self
                        .domain
                        .table_by_name(&database, relation_name(&source.Source))
                        .map_err(|e| session_error("resolve column write access", e))?;
                    let all = if columns.is_empty() {
                        model
                            .Columns
                            .iter()
                            .filter(|c| c.State == astersql_meta_model::StatePublic && !c.Hidden)
                            .map(|c| c.Name.O.clone())
                            .collect::<Vec<_>>()
                    } else {
                        columns.to_vec()
                    };
                    if self.pg_column_model_access(&model, &all, bit)? {
                        continue;
                    }
                }
                return Err(namespace_error("42501", "permission denied for table"));
            }
        }
        Ok(())
    }
}

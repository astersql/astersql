// Copyright 2026 AsterSQL.
//! PG object identity, computed from persistent native IDs without allocation.
use crate::conn::{ConnError, ConnResult};
use astersql_infoschema::InfoSchema;

// Reserved synthetic catalog type codes; created and consumed only by PG modules.
pub(crate) const OID_TYPE: u8 = crate::pg_result::CatalogColumnType::Oid as u8;
pub(crate) const REGCLASS_TYPE: u8 = crate::pg_result::CatalogColumnType::Regclass as u8;
const NATIVE_BASE: u32 = 16384;
const TABLE_BASE: u32 = 0x4000_0000;
const INDEX_BASE: u32 = 0x8000_0000;

pub(crate) const SYSTEM_RELATIONS: &[(&str, u32)] = &[
    ("pg_type", 1247),
    ("pg_attribute", 1249),
    ("pg_proc", 1255),
    ("pg_operator", 2617),
    ("pg_aggregate", 2600),
    ("pg_class", 1259),
    ("pg_constraint", 2606),
    ("pg_attrdef", 2604),
    ("pg_index", 2610),
    ("pg_language", 2612),
    ("pg_namespace", 2615),
    ("pg_description", 2609),
    ("pg_depend", 2608),
    ("pg_database", 1262),
    ("pg_authid", 1260),
    ("pg_auth_members", 1261),
    ("pg_tablespace", 1213),
    ("pg_shdescription", 2396),
    ("pg_sequence", 2224),
    ("pg_inherits", 2611),
    ("pg_opclass", 2616),
    ("pg_opfamily", 2753),
    ("pg_amop", 2602),
    ("pg_amproc", 2603),
    ("pg_collation", 3456),
    ("pg_policy", 3256),
    ("pg_trigger", 2620),
    ("pg_rewrite", 2618),
    ("pg_foreign_table", 3118),
    ("pg_foreign_server", 1417),
];
fn range_error() -> ConnError {
    ConnError::Session("PG object ID exceeds the supported OID range".into())
}
fn signed_id(id: i64) -> ConnResult<u64> {
    if id == 0 {
        return Err(range_error());
    }
    let n = u128::from(id.unsigned_abs()) * 2 - u128::from(id < 0);
    u64::try_from(n).map_err(|_| range_error())
}
fn bounded(value: u128, base: u32, end: u32) -> ConnResult<u32> {
    let oid = value + u128::from(base);
    if oid >= u128::from(end) {
        return Err(range_error());
    }
    u32::try_from(oid).map_err(|_| range_error())
}
/// Stable across connections and restarts when the persistent native ID is retained.
/// Separate disjoint ranges prevent schema/table/local-index identity collisions.
pub(crate) fn namespace_oid(id: i64) -> ConnResult<u32> {
    bounded(u128::from(signed_id(id)?), NATIVE_BASE, TABLE_BASE)
}
pub(crate) fn table_oid(id: i64) -> ConnResult<u32> {
    bounded(u128::from(signed_id(id)?), TABLE_BASE, INDEX_BASE)
}
/// Native index IDs are table-local. Cantor pairing preserves both identifiers;
/// checked range rejection prevents truncation, hashing, or order-based allocation.
pub(crate) fn index_oid(table: i64, index: i64) -> ConnResult<u32> {
    let a = u128::from(signed_id(table)?);
    let b = u128::from(signed_id(index)?);
    let sum = a + b;
    if sum > u128::from(u32::MAX) {
        return Err(range_error());
    }
    bounded(sum * (sum + 1) / 2 + b, INDEX_BASE, u32::MAX)
}

/// Parse PG relation input identifiers, retaining quoted case and escaped quotes.
pub(crate) fn name_parts(input: &str) -> ConnResult<Vec<String>> {
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
                    None => return Err(diagnostic("42602", "invalid name syntax".into())),
                }
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c == '.' || c.is_whitespace() {
                    break;
                }
                if !(c.is_alphanumeric() || c == '_' || c == '$') {
                    return Err(diagnostic("42602", "invalid name syntax".into()));
                }
                part.extend(c.to_lowercase());
                chars.next();
            }
        }
        if part.is_empty() {
            return Err(diagnostic("42602", "invalid name syntax".into()));
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
            _ => return Err(diagnostic("42602", "invalid name syntax".into())),
        }
    }
}
pub(crate) fn resolve(input: &str, database: &str, snapshot: &dyn InfoSchema) -> ConnResult<u32> {
    resolve_with_path(input, database, snapshot, false)
}
pub(crate) fn resolve_with_path(
    input: &str,
    database: &str,
    snapshot: &dyn InfoSchema,
    public_first: bool,
) -> ConnResult<u32> {
    let path = if public_first {
        vec!["public".into(), "pg_catalog".into()]
    } else {
        vec!["pg_catalog".into(), "public".into()]
    };
    resolve_in_path(input, database, snapshot, &path, &[])
}

fn diagnostic(state: &'static str, message: String) -> ConnError {
    ConnError::PgFunction(astersql_session::runtime::PgFunctionError {
        state,
        message,
        detail: None,
        hint: None,
        position: None,
    })
}

/// Resolve the same published relation identities exposed by pg_class. Namespace
/// identities and USAGE come from the authenticated session, never name prefixes.
pub(crate) fn resolve_in_path(
    input: &str,
    database: &str,
    snapshot: &dyn InfoSchema,
    path: &[String],
    facts: &[astersql_session::runtime::PgNamespaceFact],
) -> ConnResult<u32> {
    if !input.trim().is_empty() && input.trim().bytes().all(|b| b.is_ascii_digit()) {
        return input.trim().parse::<u32>().map_err(|_| range_error());
    }
    let parts = name_parts(input)?;
    let (schema, name) = match parts.as_slice() {
        [name] => (None, name),
        [schema, name] => (Some(schema.as_str()), name),
        _ => {
            return Err(diagnostic(
                "0A000",
                "cross-database regclass input is unsupported".into(),
            ));
        }
    };
    let public_oid = snapshot
        .AllSchemas()
        .iter()
        .find(|s| s.name.lower == database.to_lowercase())
        .map(|s| namespace_oid(s.id))
        .transpose()?;
    let namespace = |name: &str| -> ConnResult<Option<u32>> {
        if let Some(fact) = facts.iter().find(|f| f.name == name) {
            if !fact.usage {
                return Err(diagnostic(
                    "42501",
                    format!("permission denied for schema {name}"),
                ));
            }
            return if name == "pg_catalog" {
                Ok(Some(11))
            } else {
                namespace_oid(fact.id).map(Some)
            };
        }
        if name == "pg_catalog" {
            Ok(Some(11))
        } else if name == "public" {
            Ok(public_oid)
        } else {
            Ok(None)
        }
    };
    let mut search = schema.map_or_else(|| path.to_vec(), |s| vec![s.to_owned()]);
    if schema.is_none() && !search.iter().any(|s| s == "pg_catalog") {
        search.insert(0, "pg_catalog".into());
    }
    let rows = crate::pg_catalog::class_rows(snapshot, database)?;
    for schema in search {
        let Some(ns) = namespace(&schema)? else {
            continue;
        };
        let mut found = None;
        for row in &rows {
            if row[1] == crate::conn::Value::Text(name.clone())
                && row[2] == crate::conn::Value::Signed(i64::from(ns))
            {
                let crate::conn::Value::Signed(oid) = row[0] else {
                    unreachable!()
                };
                if found.replace(oid as u32).is_some() {
                    return Err(diagnostic(
                        "42725",
                        "ambiguous PG index relation name".into(),
                    ));
                }
            }
        }
        if let Some(oid) = found {
            return Ok(oid);
        }
    }
    Err(diagnostic(
        "42P01",
        format!("relation {input} does not exist"),
    ))
}

pub(crate) fn display(oid: u32, database: &str, snapshot: &dyn InfoSchema) -> ConnResult<String> {
    display_in_path(
        oid,
        database,
        snapshot,
        &["pg_catalog".into(), "public".into()],
        &[],
    )
}

pub(crate) fn display_in_path(
    oid: u32,
    database: &str,
    snapshot: &dyn InfoSchema,
    path: &[String],
    facts: &[astersql_session::runtime::PgNamespaceFact],
) -> ConnResult<String> {
    for row in crate::pg_catalog::class_rows(snapshot, database)? {
        if row[0] != crate::conn::Value::Signed(i64::from(oid)) {
            continue;
        }
        let crate::conn::Value::Text(name) = &row[1] else {
            unreachable!()
        };
        let bare = quoted(name);
        if resolve_in_path(&bare, database, snapshot, path, facts).ok() == Some(oid) {
            return Ok(bare);
        }
        let crate::conn::Value::Signed(ns) = row[2] else {
            unreachable!()
        };
        let schema = if ns == 11 {
            "pg_catalog"
        } else {
            facts
                .iter()
                .find(|f| namespace_oid(f.id).ok().map(i64::from) == Some(ns))
                .map_or("public", |f| f.name.as_str())
        };
        let qualified = format!("{}.{}", quoted(schema), bare);
        // Legacy native metadata can contain several table-local indexes with
        // the same PG relation name. Qualification cannot disambiguate those.
        if let Err(error) = resolve_in_path(&qualified, database, snapshot, path, facts)
            && crate::pg_conn::sqlstate(&error) == "42725"
        {
            return Err(error);
        }
        return Ok(qualified);
    }
    Ok(oid.to_string())
}
pub(crate) fn quoted(name: &str) -> String {
    if name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '$')
    {
        name.into()
    } else {
        format!("\"{}\"", name.replace('"', "\"\""))
    }
}

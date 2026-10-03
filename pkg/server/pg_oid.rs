// Copyright 2026 AsterSQL.
//! PG object identity, computed from persistent native IDs without allocation.
use crate::conn::{ConnError, ConnResult};
use astersql_infoschema::{CiString, InfoSchema};

// Reserved synthetic catalog type codes; created and consumed only by PG modules.
pub(crate) const OID_TYPE: u8 = 240;
pub(crate) const REGCLASS_TYPE: u8 = 241;
const NATIVE_BASE: u32 = 16384;
const TABLE_BASE: u32 = 0x4000_0000;
const INDEX_BASE: u32 = 0x8000_0000;

pub(crate) const SYSTEM_RELATIONS: &[(&str, u32)] = &[
    ("pg_type", 1247),
    ("pg_attribute", 1249),
    ("pg_proc", 1255),
    ("pg_class", 1259),
    ("pg_constraint", 2606),
    ("pg_attrdef", 2604),
    ("pg_index", 2610),
    ("pg_language", 2612),
    ("pg_namespace", 2615),
    ("pg_description", 2609),
    ("pg_depend", 2608),
    ("pg_database", 1262),
    ("pg_tablespace", 1213),
    ("pg_shdescription", 2396),
    ("pg_sequence", 2224),
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
fn name_parts(input: &str) -> ConnResult<Vec<String>> {
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
                    None => return Err(ConnError::Session("invalid PG regclass name".into())),
                }
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c == '.' || c.is_whitespace() {
                    break;
                }
                if !(c.is_alphanumeric() || c == '_' || c == '$') {
                    return Err(ConnError::Session("invalid PG regclass name".into()));
                }
                part.extend(c.to_lowercase());
                chars.next();
            }
        }
        if part.is_empty() {
            return Err(ConnError::Session("invalid PG regclass name".into()));
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
            _ => return Err(ConnError::Session("invalid PG regclass name".into())),
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
    if !input.trim().is_empty() && input.trim().bytes().all(|b| b.is_ascii_digit()) {
        return input.trim().parse::<u32>().map_err(|_| range_error());
    }
    let parts = name_parts(input)?;
    let (schema, name) = match parts.as_slice() {
        [name] => (None, name),
        [schema, name] => (Some(schema.as_str()), name),
        _ => return Err(ConnError::UnsupportedCommand(0)),
    };
    if schema.is_none() && public_first && !database.is_empty() {
        let tables = snapshot
            .SchemaTableInfos(&CiString::new(database))
            .map_err(|e| ConnError::Session(e.to_string()))?;
        if let Some(table) = tables.iter().find(|t| t.name.original == *name) {
            return table_oid(table.id);
        }
        let mut found = None;
        for table in &tables {
            for index in &crate::pg_catalog::catalog_indexes(
                table
                    .model_meta
                    .as_ref()
                    .ok_or(ConnError::UnsupportedCommand(0))?,
            )? {
                if index.Name.O == *name {
                    let oid = index_oid(table.id, index.ID)?;
                    if found.replace(oid).is_some() {
                        return Err(ConnError::Session(
                            "ambiguous PG index relation name".into(),
                        ));
                    }
                }
            }
        }
        if let Some(oid) = found {
            return Ok(oid);
        }
    }
    if schema.is_none() || schema == Some("pg_catalog") {
        if let Some((_, oid)) = SYSTEM_RELATIONS.iter().find(|(n, _)| *n == name) {
            return Ok(*oid);
        }
        if schema == Some("pg_catalog") {
            return Err(missing(input));
        }
    }
    if schema.is_some_and(|s| s != "public") {
        return Err(ConnError::UnsupportedCommand(0));
    }
    if database.is_empty() {
        return Err(missing(input));
    }
    let tables = snapshot
        .SchemaTableInfos(&CiString::new(database))
        .map_err(|e| ConnError::Session(e.to_string()))?;
    // Exact comparison after PG folding preserves quoted identifiers rather than
    // silently adopting the native case-insensitive resolver's behavior.
    if let Some(table) = tables.iter().find(|t| t.name.original == *name) {
        return table_oid(table.id);
    }
    let mut found = None;
    for table in tables {
        for index in &crate::pg_catalog::catalog_indexes(
            table
                .model_meta
                .as_ref()
                .ok_or(ConnError::UnsupportedCommand(0))?,
        )? {
            if index.Name.O == *name {
                let oid = index_oid(table.id, index.ID)?;
                if found.replace(oid).is_some() {
                    return Err(ConnError::Session(
                        "ambiguous PG index relation name".into(),
                    ));
                }
            }
        }
    }
    found.ok_or_else(|| missing(input))
}
fn missing(input: &str) -> ConnError {
    ConnError::Session(format!("Table '{input}' doesn't exist"))
}

pub(crate) fn display(oid: u32, database: &str, snapshot: &dyn InfoSchema) -> ConnResult<String> {
    if let Some((name, _)) = SYSTEM_RELATIONS.iter().find(|(_, id)| *id == oid) {
        return Ok((*name).into());
    }
    let tables = snapshot
        .SchemaTableInfos(&CiString::new(database))
        .map_err(|e| ConnError::Session(e.to_string()))?;
    for table in tables {
        if table_oid(table.id)? == oid {
            return Ok(native_name(&table.name.original));
        }
        for index in &crate::pg_catalog::catalog_indexes(
            table
                .model_meta
                .as_ref()
                .ok_or(ConnError::UnsupportedCommand(0))?,
        )? {
            if index_oid(table.id, index.ID)? == oid {
                return Ok(native_name(&index.Name.O));
            }
        }
    }
    Ok(oid.to_string())
}
fn native_name(name: &str) -> String {
    if SYSTEM_RELATIONS.iter().any(|(n, _)| *n == name) {
        format!("public.{}", quoted(name))
    } else {
        quoted(name)
    }
}
fn quoted(name: &str) -> String {
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

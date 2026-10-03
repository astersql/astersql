// Copyright 2026 AsterSQL.
//! PostgreSQL simple-query adaptation and engine-derived text result encoding.
use crate::conn::{ColumnInfo, NativeType, QueryResult, Value};
use crate::pg_conn::write_message;
use astersql_parser_ast as ast;
use std::io;
use std::net::TcpStream;

/// Recognize the PostgreSQL startup-time probe before the canonical SQL parser.
/// Its epoch value is fixed at PG service startup, shared by every connection.
pub(crate) fn adapt_session_query(
    sql: &str,
    startup_epoch_micros: u128,
) -> Result<std::borrow::Cow<'_, str>, (&'static str, String)> {
    let Some(startup) = startup_time_query(sql, startup_epoch_micros) else {
        return adapt_query(sql);
    };
    adapt_query(&startup).map(|query| std::borrow::Cow::Owned(query.into_owned()))
}

fn startup_time_query(sql: &str, startup_epoch_micros: u128) -> Option<String> {
    // Match SQL tokens rather than deleting whitespace or searching substrings:
    // quoted literals and lookalike identifiers must never become functions.
    fn token<'a>(remaining: &mut &'a str, expected: &str) -> Option<()> {
        let input = remaining.trim_start();
        let head = input.get(..expected.len())?;
        let matches = if expected.starts_with('\'') {
            head == expected
        } else {
            head.eq_ignore_ascii_case(expected)
        };
        if !matches {
            return None;
        }
        let tail = &input[expected.len()..];
        if expected
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric)
            && tail
                .as_bytes()
                .first()
                .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        {
            return None;
        }
        *remaining = tail;
        Some(())
    }
    let mut remaining = sql;
    for expected in [
        "select",
        "round",
        "(",
        "extract",
        "(",
        "epoch",
        "from",
        "pg_postmaster_start_time",
        "(",
        ")",
        "at",
        "time",
        "zone",
        "'UTC'",
        ")",
        ")",
    ] {
        token(&mut remaining, expected)?;
    }
    // Keep aliases and any trailing clauses for ordinary parser validation;
    // multi-statement requests are still rejected by the PG command gate.
    let suffix = remaining.trim_start();
    let suffix = if suffix.is_empty() || suffix == ";" {
        " AS round"
    } else {
        remaining
    };
    let seconds = startup_epoch_micros / 1_000_000;
    let micros = startup_epoch_micros % 1_000_000;
    // PostgreSQL ROUND(numeric) rounds a positive half away from zero. Keep
    // integer arithmetic here and a decimal literal so Describe and Execute
    // both derive numeric metadata without a shared-engine function fallback.
    let rounded = seconds + u128::from(micros >= 500_000);
    Some(format!("SELECT {rounded}.0{suffix}"))
}

/// Adapt PostgreSQL catalog identity projections at the PG boundary only.
/// AST field offsets keep strings, quoted/qualified columns and aliases intact;
/// DATABASE() obtains the current database from the canonical session at execution.
pub(crate) fn adapt_query(sql: &str) -> Result<std::borrow::Cow<'_, str>, (&'static str, String)> {
    if !sql.to_ascii_lowercase().contains("current_catalog") {
        return Ok(std::borrow::Cow::Borrowed(sql));
    }
    let (statements, _) = astersql_parser::New()
        .ParseSQL(sql, &[])
        .map_err(|error| ("42601", error.to_string()))?;
    struct CatalogProjection<'a> {
        sql: &'a str,
        replacements: std::collections::BTreeMap<usize, &'static str>,
    }
    impl ast::Visitor for CatalogProjection<'_> {
        fn enter(&mut self, node: &dyn ast::Node) -> bool {
            if let Some(select) = node.as_any().downcast_ref::<ast::SelectStmt>() {
                for field in &select.Fields.Fields {
                    let Some(ast::ExprNode {
                        Kind: ast::ExprKind::Column(column),
                        ..
                    }) = &field.Expr
                    else {
                        continue;
                    };
                    if column.Schema.L.is_empty()
                        && column.Table.L.is_empty()
                        && column.Name.L == "current_catalog"
                        && self
                            .sql
                            .get(field.Offset..field.Offset + "current_catalog".len())
                            .is_some_and(|text| text.eq_ignore_ascii_case("current_catalog"))
                    {
                        self.replacements.insert(
                            field.Offset,
                            if field.AsName.L.is_empty() {
                                "DATABASE() AS current_catalog"
                            } else {
                                "DATABASE()"
                            },
                        );
                    }
                }
            }
            false
        }
        fn leave(&mut self, _: &dyn ast::Node) -> bool {
            true
        }
    }
    let mut visitor = CatalogProjection {
        sql,
        replacements: Default::default(),
    };
    for statement in &statements {
        statement.accept(&mut visitor);
    }
    if visitor.replacements.is_empty() {
        return Ok(std::borrow::Cow::Borrowed(sql));
    }
    let mut output = String::with_capacity(sql.len());
    let mut previous = 0;
    for (offset, replacement) in visitor.replacements {
        output.push_str(&sql[previous..offset]);
        output.push_str(replacement);
        previous = offset + "current_catalog".len();
    }
    output.push_str(&sql[previous..]);
    Ok(std::borrow::Cow::Owned(output))
}

/// Parse before execution: a batched engine error cannot preserve prior results.
/// No SQL rewriting or multi-statement partial-success claims are permitted.
pub(crate) fn command(sql: &str) -> Result<Option<&'static str>, (&'static str, String)> {
    let (statements, _) = astersql_parser::New()
        .ParseSQL(sql, &[])
        .map_err(|e| ("42601", e.to_string()))?;
    if statements.is_empty() {
        return Ok(None);
    }
    if statements.len() != 1 {
        return Err((
            "0A000",
            "multiple statements in one Query are not supported".into(),
        ));
    }
    let node = statements[0].as_any();
    let tag = if node.is::<ast::SelectStmt>() || node.is::<ast::SetOprStmt>() {
        "SELECT"
    } else if let Some(insert) = node.downcast_ref::<ast::InsertStmt>() {
        if insert.IsReplace {
            return Err(("0A000", "REPLACE is not a PostgreSQL command".into()));
        }
        "INSERT"
    } else if node.is::<ast::UpdateStmt>() {
        "UPDATE"
    } else if node.is::<ast::DeleteStmt>() {
        "DELETE"
    } else if node.is::<ast::CreateTableStmt>() {
        "CREATE TABLE"
    } else if let Some(drop) = node.downcast_ref::<ast::DropTableStmt>() {
        if drop.IsView {
            "DROP VIEW"
        } else {
            "DROP TABLE"
        }
    } else if node.is::<ast::CreateDatabaseStmt>() {
        "CREATE DATABASE"
    } else if node.is::<ast::DropDatabaseStmt>() {
        "DROP DATABASE"
    } else if node.is::<ast::AlterTableStmt>() {
        "ALTER TABLE"
    } else if node.is::<ast::TruncateTableStmt>() {
        "TRUNCATE TABLE"
    } else if node.is::<ast::BeginStmt>() {
        "BEGIN"
    } else if node.is::<ast::CommitStmt>() {
        "COMMIT"
    } else if node.is::<ast::RollbackStmt>() {
        "ROLLBACK"
    } else if node.is::<ast::SetStmt>() {
        "SET"
    } else {
        return Err((
            "0A000",
            "unsupported PostgreSQL simple-query command".into(),
        ));
    };
    Ok(Some(tag))
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn count(n: usize) -> io::Result<i16> {
    i16::try_from(n).map_err(|_| invalid("too many PostgreSQL result fields"))
}
fn length(n: usize) -> io::Result<i32> {
    i32::try_from(n).map_err(|_| invalid("PostgreSQL result field exceeds maximum length"))
}
// Codes 230..237 are unused by native MySQL types, including JSON (245)
// and DECIMAL (246). They belong exclusively to PG catalog providers. Shared engine
// NativeType inference and MySQL column/protocol mappings never use them.
#[derive(Clone, Copy)]
#[repr(u8)]
pub(crate) enum CatalogColumnType {
    InternalChar = 230,
    Oid = 231,
    Regclass = 232,
    Int2Array = 233,
    Int4Array = 234,
    OidArray = 235,
    TextArray = 236,
    Int2Vector = 237,
    Int8Array = 238,
    OidVector = 239,
}
impl CatalogColumnType {
    fn from_code(code: u8) -> Option<Self> {
        Some(match code {
            230 => Self::InternalChar,
            231 => Self::Oid,
            232 => Self::Regclass,
            233 => Self::Int2Array,
            234 => Self::Int4Array,
            235 => Self::OidArray,
            236 => Self::TextArray,
            237 => Self::Int2Vector,
            238 => Self::Int8Array,
            239 => Self::OidVector,
            _ => return None,
        })
    }
    fn wire_type(self) -> (u32, i16) {
        match self {
            Self::InternalChar => (18, 1),
            Self::Oid => (26, 4),
            Self::Regclass => (2205, 4),
            Self::Int2Array => (1005, -1),
            Self::Int4Array => (1007, -1),
            Self::OidArray => (1028, -1),
            Self::TextArray => (1009, -1),
            Self::Int2Vector => (22, -1),
            Self::Int8Array => (1016, -1),
            Self::OidVector => (30, -1),
        }
    }
}
/// PostgreSQL metadata derived only from the engine type, never cell contents.
fn pg_type(column: &ColumnInfo, native: Option<&NativeType>) -> io::Result<(u32, i16)> {
    let code = native.map_or(column.column_type, |t| t.code);
    let flags = native.map_or(column.flags as usize, |t| t.flags);
    if flags & astersql_parser_mysql::r#type::IsBooleanFlag != 0 {
        return Ok((16, 1));
    }
    if let Some(kind) = CatalogColumnType::from_code(code) {
        return Ok(kind.wire_type());
    }
    let unsigned = flags & astersql_parser_mysql::r#type::UnsignedFlag != 0;
    Ok(match code {
        1 => (21, 2), // tinyint fits int2, including unsigned
        2 if unsigned => (23, 4),
        2 => (21, 2),
        3 if unsigned => (20, 8),
        3 | 9 => (23, 4),
        8 if unsigned => (1700, -1), // preserve the full uint64 range
        8 => (20, 8),
        0 | 246 => (1700, -1),
        4 => (700, 4),
        5 => (701, 8),
        6 => (25, -1),       // NULL has no stronger engine type
        7 | 12 => (1114, 8), // native wall-clock timestamp, no invented timezone
        10 | 14 => (1082, 4),
        11 => (1083, 8),
        13 => (21, 2),
        15 | 253 | 254 | 249..=252 if column.charset == 63 => (17, -1),
        15 | 253 | 254 | 249..=252 => (25, -1),
        _ => return Err(invalid("unsupported engine result type")),
    })
}
fn value_text(value: &Value, oid: u32) -> io::Result<Option<Vec<u8>>> {
    let text = match value {
        Value::Null => return Ok(None),
        Value::Text(s) => s.clone(),
        Value::Signed(n) => n.to_string(),
        Value::Unsigned(n) => n.to_string(),
        Value::Float(n) if n.is_nan() => "NaN".into(),
        Value::Float(n) if *n == f64::INFINITY => "Infinity".into(),
        Value::Float(n) if *n == f64::NEG_INFINITY => "-Infinity".into(),
        Value::Float(n) => n.to_string(),
        Value::Bytes(bytes) if oid == 17 => {
            return Ok(Some(bytea(bytes)));
        }
        Value::Bytes(_) => return Err(invalid("binary value without binary engine type")),
    };
    if oid == 17 {
        return Ok(Some(bytea(text.as_bytes())));
    }
    let text = match oid {
        16 => match text.as_str() {
            "1" | "true" => "t".to_owned(),
            "0" | "false" => "f".to_owned(),
            _ => return Err(invalid("invalid engine boolean value")),
        },
        // MySQL accepts zero dates and extended/negative TIME durations;
        // PostgreSQL date/time input does not. Reject rather than changing values.
        1082 | 1114 => {
            let date = text.split(' ').next().unwrap_or_default();
            let parts: Vec<_> = date.split('-').collect();
            if parts.len() != 3
                || parts[0].parse::<u32>().unwrap_or(0) == 0
                || !(1..=12).contains(&parts[1].parse::<u32>().unwrap_or(0))
                || !(1..=31).contains(&parts[2].parse::<u32>().unwrap_or(0))
            {
                return Err(invalid("engine zero or invalid date is unsupported"));
            }
            text
        }
        1083 => {
            let hour = text.split(':').next().unwrap_or_default().parse::<u32>();
            if !hour.is_ok_and(|hour| hour < 24) {
                return Err(invalid(
                    "engine TIME duration is not a PostgreSQL time of day",
                ));
            }
            text
        }
        _ => text,
    };
    if text.contains('\0') {
        return Err(invalid("PostgreSQL text cannot contain NUL"));
    }
    Ok(Some(text.into_bytes()))
}
fn bytea(bytes: &[u8]) -> Vec<u8> {
    let mut result = String::from("\\x");
    for byte in bytes {
        use std::fmt::Write;
        write!(&mut result, "{byte:02x}").unwrap();
    }
    result.into_bytes()
}

/// Encode a complete result before writing any metadata so unsupported values
/// cannot leave the client in a partially described result stream.
pub(crate) fn encode(result: &QueryResult, command: &str) -> io::Result<Vec<(u8, Vec<u8>)>> {
    let mut messages = Vec::new();
    if !result.columns.is_empty() {
        let mut body = count(result.columns.len())?.to_be_bytes().to_vec();
        if result.native_types.len() != result.columns.len() {
            return Err(invalid("engine type metadata count mismatch"));
        }
        let types = result
            .columns
            .iter()
            .enumerate()
            .map(|(i, c)| pg_type(c, result.native_types.get(i)))
            .collect::<io::Result<Vec<_>>>()?;
        for (column, (oid, size)) in result.columns.iter().zip(&types) {
            if column.name.contains('\0') {
                return Err(invalid("result name contains NUL"));
            }
            body.extend_from_slice(column.name.as_bytes());
            body.push(0);
            body.extend_from_slice(&0u32.to_be_bytes()); // unknown source table OID
            body.extend_from_slice(&0i16.to_be_bytes()); // unknown attribute number
            body.extend_from_slice(&oid.to_be_bytes());
            body.extend_from_slice(&size.to_be_bytes());
            body.extend_from_slice(&(-1i32).to_be_bytes());
            body.extend_from_slice(&0i16.to_be_bytes()); // text format
        }
        messages.push((b'T', body));
        for row in &result.rows {
            if row.len() != result.columns.len() {
                return Err(invalid("result column count mismatch"));
            }
            let mut body = count(row.len())?.to_be_bytes().to_vec();
            for (value, (oid, _)) in row.iter().zip(&types) {
                let text = value_text(value, *oid)?;
                if let Some(text) = text {
                    if text.contains(&0) {
                        return Err(invalid("PostgreSQL text cannot contain NUL"));
                    }
                    body.extend_from_slice(&length(text.len())?.to_be_bytes());
                    body.extend_from_slice(&text);
                } else {
                    body.extend_from_slice(&(-1i32).to_be_bytes());
                }
            }
            messages.push((b'D', body));
        }
    }
    let completion = match command {
        "SELECT" => format!("SELECT {}", result.rows.len()),
        "INSERT" => format!("INSERT 0 {}", result.state.affected_rows),
        "UPDATE" | "DELETE" => format!("{command} {}", result.state.affected_rows),
        _ => command.into(),
    };
    messages.push((b'C', [completion.as_bytes(), b"\0"].concat()));
    Ok(messages)
}
pub(crate) fn write_result(
    socket: &mut TcpStream,
    result: &QueryResult,
    command: &str,
) -> io::Result<()> {
    for (tag, body) in encode(result, command)? {
        write_message(socket, tag, &body)?;
    }
    Ok(())
}

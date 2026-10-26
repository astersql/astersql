// Copyright 2026 AsterSQL.
//! PostgreSQL query adaptation and engine-derived text/binary result encoding.
use crate::conn::{ColumnInfo, NativeType, QueryResult, Value};
use crate::pg_conn::write_message;
use astersql_parser_ast as ast;
use std::io;
use std::net::TcpStream;

fn parser_error(message: String) -> (&'static str, String) {
    let state = if message == "PostgreSQL float precision must be between 1 and 53" {
        "22023"
    } else {
        "42601"
    };
    (state, message)
}

/// Preserve TIME declaration diagnostics from the real parser at Query/Parse,
/// before adapters or binding parse the statement again. Other syntax failures
/// keep their existing owner and error-position handling.
pub(crate) fn time_precision_diagnostics(
    sql: &str,
) -> Result<Vec<(u8, Vec<u8>)>, (&'static str, String)> {
    let warnings = match astersql_parser::Parser::with_dialect(ast::SqlDialect::PostgreSQL)
        .ParseSQL(sql, &[])
    {
        Ok((_, warnings)) => warnings,
        Err(error) if error.to_string() == "PostgreSQL TIME precision must not be negative" => {
            return Err(("22023", error.to_string()));
        }
        Err(_) => return Ok(Vec::new()),
    };
    Ok(warnings
        .into_iter()
        .filter_map(|warning| {
            let message = warning.to_string();
            if !message.starts_with("PostgreSQL TIME precision reduced to 6") {
                return None;
            }
            let mut body = b"SWARNING\0VWARNING\0C22023\0M".to_vec();
            body.extend_from_slice(message.as_bytes());
            body.extend_from_slice(&[0, 0]);
            Some((b'N', body))
        })
        .collect())
}

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
    let (statements, _) = astersql_parser::Parser::with_dialect(ast::SqlDialect::PostgreSQL)
        .ParseSQL(sql, &[])
        .map_err(|error| parser_error(error.to_string()))?;
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
    command_for_phase(sql, false).map(|plan| plan.command)
}

/// Analysis plan for PostgreSQL extended-protocol Prepare.
///
/// A conflict clause remains attached to the prepared statement for the later
/// execution gate. `analysis_sql` exists only so the shared engine can produce
/// ordinary read-only parameter and result metadata for the INSERT itself.
pub(crate) struct PrepareCommand {
    pub command: Option<&'static str>,
    pub returning: bool,
    pub postgresql_conflict: Option<ast::PostgreSQLConflictClause>,
    pub analysis_sql: Option<String>,
}

/// Preparing RETURNING binds metadata without permitting DML execution.
pub(crate) fn prepare_command(sql: &str) -> Result<PrepareCommand, (&'static str, String)> {
    command_for_phase(sql, true)
}

fn command_for_phase(sql: &str, prepare: bool) -> Result<PrepareCommand, (&'static str, String)> {
    let (statements, _) = astersql_parser::Parser::with_dialect(ast::SqlDialect::PostgreSQL)
        .ParseSQL(sql, &[])
        .map_err(|error| parser_error(error.to_string()))?;
    if statements.is_empty() {
        return Ok(PrepareCommand {
            command: None,
            returning: false,
            postgresql_conflict: None,
            analysis_sql: None,
        });
    }
    if statements.len() != 1 {
        return Err((
            "0A000",
            "multiple statements in one Query are not supported".into(),
        ));
    }
    let node = statements[0].as_any();
    let mut postgresql_conflict = None;
    let tag = if node.is::<ast::SelectStmt>() || node.is::<ast::SetOprStmt>() {
        "SELECT"
    } else if let Some(insert) = node.downcast_ref::<ast::InsertStmt>() {
        if insert.IsReplace {
            return Err(("0A000", "REPLACE is not a PostgreSQL command".into()));
        }
        postgresql_conflict = insert.PostgreSQLConflict.clone();
        if postgresql_conflict.is_some() && !prepare {
            return Err((
                "0A000",
                "PostgreSQL ON CONFLICT is unsupported by the shared executor".into(),
            ));
        }
        if !insert.Returning.is_empty() && !prepare {
            return Err((
                "0A000",
                "DML RETURNING is unsupported by the PostgreSQL listener".into(),
            ));
        }
        "INSERT"
    } else if let Some(update) = node.downcast_ref::<ast::UpdateStmt>() {
        if update.From.is_some() {
            return Err((
                "0A000",
                "PostgreSQL UPDATE FROM is unsupported by the shared executor".into(),
            ));
        }
        if !update.Returning.is_empty() {
            if !prepare {
                return Err((
                    "0A000",
                    "DML RETURNING is unsupported by the PostgreSQL listener".into(),
                ));
            }
        }
        "UPDATE"
    } else if let Some(delete) = node.downcast_ref::<ast::DeleteStmt>() {
        if delete.Using.is_some() {
            return Err((
                "0A000",
                "PostgreSQL DELETE USING is unsupported by the shared executor".into(),
            ));
        }
        if !delete.Returning.is_empty() && !prepare {
            return Err((
                "0A000",
                "DML RETURNING is unsupported by the PostgreSQL listener".into(),
            ));
        }
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
    let returning = node
        .downcast_ref::<ast::InsertStmt>()
        .is_some_and(|insert| !insert.Returning.is_empty())
        || node
            .downcast_ref::<ast::UpdateStmt>()
            .is_some_and(|update| !update.Returning.is_empty())
        || node
            .downcast_ref::<ast::DeleteStmt>()
            .is_some_and(|delete| !delete.Returning.is_empty());
    let analysis_sql = if let Some(conflict) = &postgresql_conflict {
        let (start, end) = conflict.SourceSpan;
        if start >= end || sql.get(start..end) != Some(conflict.OriginalText.as_str()) {
            return Err((
                "0A000",
                "invalid PostgreSQL conflict-clause source span".into(),
            ));
        }
        let before = sql.get(..start).ok_or_else(|| {
            (
                "0A000",
                "invalid PostgreSQL conflict-clause source span".into(),
            )
        })?;
        let after = sql.get(end..).ok_or_else(|| {
            (
                "0A000",
                "invalid PostgreSQL conflict-clause source span".into(),
            )
        })?;
        Some(format!("{before}{after}"))
    } else {
        None
    };
    Ok(PrepareCommand {
        command: Some(tag),
        returning,
        postgresql_conflict,
        analysis_sql,
    })
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
// Codes 230..242 are unused by native MySQL types, including JSON (245)
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
    Char = 240,
    AclArray = 241,
    Name = 242,
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
            240 => Self::Char,
            241 => Self::AclArray,
            242 => Self::Name,
            _ => return None,
        })
    }
    fn wire_type(self) -> (u32, i16) {
        match self {
            Self::InternalChar => (18, 1),
            Self::Name => (19, 64),
            Self::Char => (1042, -1),
            Self::AclArray => (1034, -1),
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
    if let Some(oid) = native.and_then(|native| native.postgresql_text_oid) {
        return Ok((oid, -1));
    }
    if native.is_some_and(|native| native.postgresql_record_fields.is_some()) {
        return Ok((2249, -1));
    }
    if let Some(element) = native.and_then(|native| native.postgresql_array_element) {
        return Ok((element.array_oid(), -1));
    }
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
// PostgreSQL excludes decimal spellings exactly on a binary rounding midpoint.
// Compare powers of two/five after factoring small integer coefficients; no
// arbitrary precision allocation is needed, even at the subnormal endpoints.
fn float_midpoint(scientific: &str, n: f64, single: bool) -> bool {
    if n == 0.0 {
        return false;
    }
    let (mantissa, exponent) = scientific.trim_start_matches('-').split_once('e').unwrap();
    let digits = mantissa.replace('.', "");
    let decimal = exponent.parse::<i32>().unwrap() - (digits.len() as i32 - 1);
    let coefficient = digits.parse::<u64>().unwrap();
    let (bits, width, bias, mask) = if single {
        ((n as f32).to_bits() as u64, 23, 127, 0xff)
    } else {
        (n.to_bits(), 52, 1023, 0x7ff)
    };
    let fraction = bits & ((1u64 << width) - 1);
    let exponent = (bits >> width) & mask;
    let significand = fraction + if exponent == 0 { 0 } else { 1u64 << width };
    let binary = exponent.max(1) as i32 - bias - width - 2;
    fn factors(mut coefficient: u64, mut two: i32, mut five: i32) -> (u64, i32, i32) {
        while coefficient % 2 == 0 {
            coefficient /= 2;
            two += 1;
        }
        while coefficient % 5 == 0 {
            coefficient /= 5;
            five += 1;
        }
        (coefficient, two, five)
    }
    let decimal = factors(coefficient, decimal, decimal);
    let lower = 4 * significand - 1 - u64::from(fraction != 0 || exponent <= 1);
    decimal == factors(lower, binary, 0) || decimal == factors(4 * significand + 2, binary, 0)
}

fn float_text(n: f64, single: bool) -> String {
    let n = if single { f64::from(n as f32) } else { n };
    let mut scientific = if single {
        format!("{:e}", n as f32)
    } else {
        format!("{n:e}")
    };
    if float_midpoint(&scientific, n, single) {
        let digits = scientific
            .split('e')
            .next()
            .unwrap()
            .bytes()
            .filter(u8::is_ascii_digit)
            .count();
        for precision in digits..=16 {
            scientific = format!("{n:.precision$e}");
            if !float_midpoint(&scientific, n, single) {
                break;
            }
        }
    }
    let (mantissa, exponent) = scientific.split_once('e').unwrap();
    let exponent: i32 = exponent.parse().unwrap();
    if exponent < -4 || exponent >= if single { 6 } else { 15 } {
        return format!("{mantissa}e{exponent:+03}");
    }
    let negative = mantissa.starts_with('-');
    let digits = mantissa.trim_start_matches('-').replace('.', "");
    let point = exponent + 1;
    let plain = if point <= 0 {
        format!("0.{}{digits}", "0".repeat((-point) as usize))
    } else if point as usize >= digits.len() {
        format!("{digits}{}", "0".repeat(point as usize - digits.len()))
    } else {
        format!(
            "{}.{}",
            &digits[..point as usize],
            &digits[point as usize..]
        )
    };
    if negative { format!("-{plain}") } else { plain }
}

fn value_text(value: &Value, oid: u32) -> io::Result<Option<Vec<u8>>> {
    let text = match value {
        Value::Null => return Ok(None),
        Value::Text(s) if oid == 700 || oid == 701 => {
            let value = if oid == 700 {
                let wide = s
                    .parse::<f64>()
                    .map_err(|_| invalid("invalid engine float4 value"))?;
                let narrow = s
                    .parse::<f32>()
                    .map_err(|_| invalid("invalid engine float4 value"))?;
                if wide.is_finite() && (!narrow.is_finite() || (wide != 0.0 && narrow == 0.0)) {
                    return Err(invalid("engine float4 value out of range"));
                }
                f64::from(narrow)
            } else {
                s.parse::<f64>()
                    .map_err(|_| invalid("invalid engine float8 value"))?
            };
            return value_text(&Value::Float(value), oid);
        }
        Value::Text(s) => s.clone(),
        Value::Signed(n) => n.to_string(),
        Value::Unsigned(n) => n.to_string(),
        Value::Float(n) if n.is_nan() => "NaN".into(),
        Value::Float(n) if *n == f64::INFINITY => "Infinity".into(),
        Value::Float(n) if *n == f64::NEG_INFINITY => "-Infinity".into(),
        Value::Float(n) if oid == 700 || oid == 701 => {
            // Format at the declared width; widening binary32 before formatting
            // exposes digits that are not part of the real value's text identity.
            if oid == 700 {
                let narrow = *n as f32;
                if n.is_finite() && (!narrow.is_finite() || (*n != 0.0 && narrow == 0.0)) {
                    return Err(invalid("engine float4 value out of range"));
                }
            }
            float_text(*n, oid == 700)
        }
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
            "1" | "true" | "t" => "t".to_owned(),
            "0" | "false" | "f" => "f".to_owned(),
            _ => return Err(invalid("invalid engine boolean value")),
        },
        // MySQL accepts zero dates and extended/negative TIME durations;
        // PostgreSQL date/time input does not. Reject rather than changing values.
        1082 => text
            .parse::<astersql_types::pg_temporal::PgDate>()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
            .to_string(),
        1114 => {
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
        1083 => text
            .parse::<astersql_types::pg_temporal::PgTime>()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
            .to_string(),
        _ => text,
    };
    if text.contains('\0') {
        return Err(invalid("PostgreSQL text cannot contain NUL"));
    }
    Ok(Some(text.into_bytes()))
}
/// Parse the canonical record text carrier using its independent typed layout.
/// Quoted empty fields are values; unquoted empty fields are SQL NULL.
fn record_binary(
    text: &str,
    fields: &[astersql_session::runtime::PostgreSQLRecordField],
) -> io::Result<Vec<u8>> {
    let chars = text.chars().collect::<Vec<_>>();
    if chars.first() != Some(&'(') || chars.last() != Some(&')') {
        return Err(invalid("invalid record value"));
    }
    let mut at = 1;
    let mut values = Vec::new();
    loop {
        let quoted = chars.get(at) == Some(&'"');
        let mut value = String::new();
        if quoted {
            at += 1;
            loop {
                match chars.get(at).copied() {
                    Some('"') if chars.get(at + 1) == Some(&'"') => {
                        value.push('"');
                        at += 2;
                    }
                    Some('"') => {
                        at += 1;
                        break;
                    }
                    Some('\\') => {
                        at += 1;
                        value.push(
                            *chars
                                .get(at)
                                .ok_or_else(|| invalid("invalid record escape"))?,
                        );
                        at += 1;
                    }
                    Some(ch) => {
                        value.push(ch);
                        at += 1;
                    }
                    None => return Err(invalid("unclosed record field")),
                }
            }
        } else {
            while let Some(ch) = chars.get(at).copied() {
                if ch == ',' || ch == ')' {
                    break;
                }
                value.push(ch);
                at += 1;
            }
        }
        values.push(if !quoted && value.is_empty() {
            None
        } else {
            Some(value)
        });
        match chars.get(at) {
            Some(',') => at += 1,
            Some(')') if at + 1 == chars.len() => break,
            _ => return Err(invalid("invalid record field delimiter")),
        }
    }
    if values.len() != fields.len() {
        return Err(invalid("record field count mismatch"));
    }
    let mut result = length(fields.len())?.to_be_bytes().to_vec();
    for (field, value) in fields.iter().zip(values) {
        result.extend_from_slice(&field.oid.to_be_bytes());
        if let Some(value) = value {
            let bytes = if field.oid == 2249 {
                record_binary(
                    &value,
                    field
                        .fields
                        .as_deref()
                        .ok_or_else(|| invalid("missing nested record layout"))?,
                )?
            } else {
                value_binary(&Value::Text(value), field.oid)?
                    .ok_or_else(|| invalid("nonnull record field lost"))?
            };
            result.extend_from_slice(&length(bytes.len())?.to_be_bytes());
            result.extend_from_slice(&bytes);
        } else {
            result.extend_from_slice(&(-1i32).to_be_bytes());
        }
    }
    Ok(result)
}

fn value_binary(value: &Value, oid: u32) -> io::Result<Option<Vec<u8>>> {
    if matches!(value, Value::Null) {
        return Ok(None);
    }
    if oid == 17 {
        return Ok(Some(match value {
            Value::Bytes(bytes) => bytes.clone(),
            Value::Text(s) => s.as_bytes().to_vec(),
            _ => return Err(invalid("invalid bytea value")),
        }));
    }
    let text = value_text(value, oid)?.unwrap();
    let text = std::str::from_utf8(&text).map_err(|_| invalid("invalid binary value"))?;
    let bad = |_| invalid("invalid binary integer value");
    let bytes = match oid {
        16 => vec![u8::from(text == "t")],
        18 => {
            if text.len() != 1 {
                return Err(invalid("invalid internal char"));
            }
            text.as_bytes().to_vec()
        }
        21 => text.parse::<i16>().map_err(bad)?.to_be_bytes().to_vec(),
        23 => text.parse::<i32>().map_err(bad)?.to_be_bytes().to_vec(),
        20 => text.parse::<i64>().map_err(bad)?.to_be_bytes().to_vec(),
        26 | 2205 => text.parse::<u32>().map_err(bad)?.to_be_bytes().to_vec(),
        700 => text
            .parse::<f32>()
            .map_err(|_| invalid("invalid float4"))?
            .to_be_bytes()
            .to_vec(),
        701 => text
            .parse::<f64>()
            .map_err(|_| invalid("invalid float8"))?
            .to_be_bytes()
            .to_vec(),
        19 | 25 | 705 | 1042 | 1043 => text.as_bytes().to_vec(),
        1700 => binary_numeric(text)?,
        22 | 30 | 1000 | 1005 | 1007 | 1009 | 1016 | 1028 | 1231 => binary_array(text, oid)?,
        1082 => text
            .parse::<astersql_types::pg_temporal::PgDate>()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
            .to_binary()
            .to_vec(),
        1083 => text
            .parse::<astersql_types::pg_temporal::PgTime>()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
            .to_binary()
            .to_vec(),
        1114 => {
            use chrono::Timelike;
            let epoch = chrono::NaiveDate::from_ymd_opt(2000, 1, 1).unwrap();
            let invalid_time = |_| invalid("invalid binary temporal value");
            let datetime = chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S%.f")
                .map_err(invalid_time)?;
            if datetime.nanosecond() >= 1_000_000_000 || datetime.nanosecond() % 1000 != 0 {
                return Err(invalid("timestamp exceeds engine microsecond precision"));
            }
            datetime
                .signed_duration_since(epoch.and_hms_opt(0, 0, 0).unwrap())
                .num_microseconds()
                .ok_or_else(|| invalid("timestamp out of range"))?
                .to_be_bytes()
                .to_vec()
        }
        _ => return Err(invalid("unsupported binary result type")),
    };
    Ok(Some(bytes))
}

#[derive(Debug)]
enum ArrayTextValue {
    Array(Vec<ArrayTextValue>),
    Item(Option<String>),
}

fn parse_array_text(chars: &[char], position: &mut usize) -> io::Result<ArrayTextValue> {
    if chars.get(*position) != Some(&'{') {
        return Err(invalid("invalid PostgreSQL array text"));
    }
    *position += 1;
    let mut values = Vec::new();
    if chars.get(*position) == Some(&'}') {
        *position += 1;
        return Ok(ArrayTextValue::Array(values));
    }
    loop {
        let value = if chars.get(*position) == Some(&'{') {
            parse_array_text(chars, position)?
        } else {
            let quoted = chars.get(*position) == Some(&'"');
            if quoted {
                *position += 1;
            }
            let mut item = String::new();
            let mut escaped = false;
            let mut closed = !quoted;
            while let Some(character) = chars.get(*position).copied() {
                match character {
                    '\\' => {
                        escaped = true;
                        *position += 1;
                        item.push(
                            *chars
                                .get(*position)
                                .ok_or_else(|| invalid("invalid array escape"))?,
                        );
                        *position += 1;
                    }
                    '"' if quoted => {
                        *position += 1;
                        closed = true;
                        break;
                    }
                    ',' | '}' if !quoted => break,
                    '{' | '}' if quoted => {
                        item.push(character);
                        *position += 1;
                    }
                    _ => {
                        item.push(character);
                        *position += 1;
                    }
                }
            }
            if !closed || (!quoted && item.is_empty()) {
                return Err(invalid("invalid PostgreSQL array item"));
            }
            ArrayTextValue::Item(if !quoted && !escaped && item == "NULL" {
                None
            } else {
                Some(item)
            })
        };
        values.push(value);
        match chars.get(*position) {
            Some(',') => {
                *position += 1;
                if chars.get(*position) == Some(&'}') {
                    return Err(invalid("empty PostgreSQL array item"));
                }
            }
            Some('}') => {
                *position += 1;
                break;
            }
            _ => return Err(invalid("invalid PostgreSQL array delimiter")),
        }
    }
    Ok(ArrayTextValue::Array(values))
}

fn array_shape(value: &ArrayTextValue) -> io::Result<Vec<usize>> {
    let ArrayTextValue::Array(values) = value else {
        return Err(invalid("invalid PostgreSQL array root"));
    };
    if values.is_empty() {
        return Ok(vec![0]);
    }
    let nested = matches!(values.first(), Some(ArrayTextValue::Array(_)));
    if values
        .iter()
        .any(|value| matches!(value, ArrayTextValue::Array(_)) != nested)
    {
        return Err(invalid("inconsistent PostgreSQL array dimensions"));
    }
    let mut shape = vec![values.len()];
    if nested {
        let child = array_shape(&values[0])?;
        for value in &values[1..] {
            if array_shape(value)? != child {
                return Err(invalid("inconsistent PostgreSQL array dimensions"));
            }
        }
        shape.extend(child);
    }
    Ok(shape)
}

fn flatten_array(value: &ArrayTextValue, output: &mut Vec<Option<String>>) {
    if let ArrayTextValue::Array(values) = value {
        for value in values {
            match value {
                ArrayTextValue::Array(_) => flatten_array(value, output),
                ArrayTextValue::Item(item) => output.push(item.clone()),
            }
        }
    }
}

/// Encode PostgreSQL's base-10000 binary numeric representation.
fn binary_numeric(text: &str) -> io::Result<Vec<u8>> {
    let (negative, value) = match text.strip_prefix('-') {
        Some(value) => (true, value),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };
    let (integer, fraction) = value.split_once('.').unwrap_or((value, ""));
    if (integer.is_empty() && fraction.is_empty())
        || !integer
            .chars()
            .chain(fraction.chars())
            .all(|character| character.is_ascii_digit())
    {
        return Err(invalid("invalid binary numeric value"));
    }
    let scale = i16::try_from(fraction.len()).map_err(|_| invalid("numeric scale out of range"))?;
    let padded_integer = format!(
        "{:0>width$}",
        if integer.is_empty() { "0" } else { integer },
        width = ((integer.len().max(1) + 3) / 4) * 4
    );
    let mut digits = padded_integer
        .as_bytes()
        .chunks(4)
        .map(|chunk| std::str::from_utf8(chunk).unwrap().parse::<u16>().unwrap())
        .collect::<Vec<_>>();
    let integer_groups = digits.len();
    if !fraction.is_empty() {
        let padded_fraction = format!("{fraction:0<width$}", width = (fraction.len() + 3) / 4 * 4);
        digits.extend(
            padded_fraction
                .as_bytes()
                .chunks(4)
                .map(|chunk| std::str::from_utf8(chunk).unwrap().parse::<u16>().unwrap()),
        );
    }
    let mut weight =
        i16::try_from(integer_groups).map_err(|_| invalid("numeric weight out of range"))? - 1;
    while digits.first() == Some(&0) {
        digits.remove(0);
        weight -= 1;
    }
    while digits.last() == Some(&0) {
        digits.pop();
    }
    let count =
        i16::try_from(digits.len()).map_err(|_| invalid("numeric digit count out of range"))?;
    let mut output = count.to_be_bytes().to_vec();
    output.extend_from_slice(&weight.to_be_bytes());
    output.extend_from_slice(
        &(if digits.is_empty() {
            0u16
        } else if negative {
            0x4000u16
        } else {
            0u16
        })
        .to_be_bytes(),
    );
    output.extend_from_slice(&scale.to_be_bytes());
    for digit in digits {
        output.extend_from_slice(&digit.to_be_bytes());
    }
    Ok(output)
}

/// Catalog and expression arrays use the same recursive PostgreSQL binary layout.
fn binary_array(text: &str, oid: u32) -> io::Result<Vec<u8>> {
    let vector = matches!(oid, 22 | 30);
    let element_oid: u32 = match oid {
        22 | 1005 => 21,
        30 | 1028 => 26,
        1007 => 23,
        1016 => 20,
        1009 => 25,
        1000 => 16,
        1231 => 1700,
        _ => unreachable!(),
    };
    let (values, dimensions, lower_bounds) = if vector {
        let values = text
            .split_ascii_whitespace()
            .map(|s| Some(s.to_owned()))
            .collect::<Vec<_>>();
        let dims =
            vec![i32::try_from(values.len()).map_err(|_| invalid("array dimension too large"))?];
        (values, dims, vec![0i32])
    } else {
        let chars = text.chars().collect::<Vec<_>>();
        let mut position = 0;
        let parsed = parse_array_text(&chars, &mut position)?;
        if position != chars.len() {
            return Err(invalid("trailing PostgreSQL array text"));
        }
        let shape = if matches!(&parsed,ArrayTextValue::Array(values) if values.is_empty()) {
            vec![]
        } else {
            array_shape(&parsed)?
        };
        let mut values = Vec::new();
        flatten_array(&parsed, &mut values);
        let dimensions = shape
            .iter()
            .map(|value| i32::try_from(*value).map_err(|_| invalid("array dimension too large")))
            .collect::<io::Result<Vec<_>>>()?;
        (values, dimensions, vec![1i32; shape.len()])
    };
    let dimension_count = if vector || !values.is_empty() {
        dimensions.len()
    } else {
        0
    };
    let mut output = i32::try_from(dimension_count)
        .map_err(|_| invalid("array rank too large"))?
        .to_be_bytes()
        .to_vec();
    output.extend_from_slice(&i32::from(values.iter().any(Option::is_none)).to_be_bytes());
    output.extend_from_slice(&element_oid.to_be_bytes());
    for (dimension, lower_bound) in dimensions.into_iter().zip(lower_bounds) {
        output.extend_from_slice(&dimension.to_be_bytes());
        output.extend_from_slice(&lower_bound.to_be_bytes());
    }
    for value in values {
        let bytes = match value {
            Some(text) => value_binary(&Value::Text(text), element_oid)?.unwrap(),
            None => {
                output.extend_from_slice(&(-1i32).to_be_bytes());
                continue;
            }
        };
        output.extend_from_slice(&length(bytes.len())?.to_be_bytes());
        output.extend_from_slice(&bytes);
    }
    Ok(output)
}
/// Legacy bytea text encoding for the session bytea_output workflow.
/// Printable ASCII is literal; backslash doubles and other bytes use octal.
#[allow(dead_code)] // Session setting integration belongs to its own workflow.
pub(crate) fn bytea_escape(bytes: &[u8]) -> Vec<u8> {
    let mut result = Vec::new();
    for &byte in bytes {
        match byte {
            b'\\' => result.extend_from_slice(b"\\\\"),
            0x20..=0x7e => result.push(byte),
            _ => result.extend_from_slice(&[
                b'\\',
                b'0' + (byte >> 6),
                b'0' + ((byte >> 3) & 7),
                b'0' + (byte & 7),
            ]),
        }
    }
    result
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
    encode_formats(result, command, &[])
}

/// Formats are local to a bound portal; simple Query and statement Describe use text.
pub(crate) fn encode_formats(
    result: &QueryResult,
    command: &str,
    formats: &[usize],
) -> io::Result<Vec<(u8, Vec<u8>)>> {
    if !formats.is_empty()
        && (formats.len() != result.columns.len() || formats.iter().any(|f| *f > 1))
    {
        return Err(invalid("invalid result formats"));
    }
    if !result.column_origins.is_empty() && result.column_origins.len() != result.columns.len() {
        return Err(invalid("engine column origin count mismatch"));
    }
    if !result.catalog_column_origins.is_empty()
        && result.catalog_column_origins.len() != result.columns.len()
    {
        return Err(invalid("catalog column origin count mismatch"));
    }
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
            .map(|(i, c)| {
                let native = result.native_types.get(i);
                if matches!(command, "INSERT" | "UPDATE" | "DELETE")
                    && c.charset != 63
                    && native.is_some_and(|tp| tp.code == 15)
                {
                    Ok((1043, -1))
                } else {
                    pg_type(c, native)
                }
            })
            .collect::<io::Result<Vec<_>>>()?;
        for (index, (column, (oid, size))) in result.columns.iter().zip(&types).enumerate() {
            if column.name.contains('\0') {
                return Err(invalid("result name contains NUL"));
            }
            body.extend_from_slice(column.name.as_bytes());
            body.push(0);
            let catalog_origin = result.catalog_column_origins.get(index).copied().flatten();
            let native_origin = result.column_origins.get(index).copied().flatten();
            if catalog_origin.is_some() && native_origin.is_some() {
                return Err(invalid("conflicting native and catalog column origins"));
            }
            let (table_oid, attribute) = if let Some((oid, attribute)) = catalog_origin {
                if attribute <= 0
                    || !crate::pg_oid::SYSTEM_RELATIONS
                        .iter()
                        .any(|(_, registered)| *registered == oid)
                {
                    return Err(invalid("unregistered catalog column origin"));
                }
                (oid, attribute)
            } else {
                match native_origin {
                    Some(origin) if origin.attribute_number > 0 => (
                        crate::pg_oid::table_oid(origin.table_id)
                            .map_err(|error| invalid(&error.to_string()))?,
                        origin.attribute_number,
                    ),
                    Some(_) => return Err(invalid("invalid physical column attribute number")),
                    None => (0, 0), // Unknown or non-physical expression; never infer from a label.
                }
            };
            body.extend_from_slice(&table_oid.to_be_bytes());
            body.extend_from_slice(&attribute.to_be_bytes());
            body.extend_from_slice(&oid.to_be_bytes());
            body.extend_from_slice(&size.to_be_bytes());
            let typmod = if matches!(*oid, 1042 | 1043) {
                result
                    .native_types
                    .get(index)
                    .filter(|tp| tp.length >= 0)
                    .and_then(|tp| i32::try_from(tp.length).ok())
                    .and_then(|length| length.checked_add(4))
                    .unwrap_or(-1)
            } else if *oid == 1083 {
                result
                    .native_types
                    .get(index)
                    .map(|tp| tp.decimal as i32)
                    .filter(|precision| (0..=6).contains(precision))
                    .unwrap_or(-1)
            } else if *oid == 1700 {
                result
                    .native_types
                    .get(index)
                    .filter(|tp| {
                        matches!(tp.code, 0 | 246)
                            && (1..=1000).contains(&tp.length)
                            && (-1000..=1000).contains(&tp.decimal)
                    })
                    .map(|tp| (((tp.length as i32) << 16) | ((tp.decimal as i32) & 0x7ff)) + 4)
                    .unwrap_or(-1)
            } else {
                -1
            };
            body.extend_from_slice(&typmod.to_be_bytes());
            body.extend_from_slice(
                &(formats.get(index).copied().unwrap_or(0) as i16).to_be_bytes(),
            );
        }
        messages.push((b'T', body));
        for row in &result.rows {
            if row.len() != result.columns.len() {
                return Err(invalid("result column count mismatch"));
            }
            let mut body = count(row.len())?.to_be_bytes().to_vec();
            for (index, (value, (oid, _))) in row.iter().zip(&types).enumerate() {
                let binary = formats.get(index) == Some(&1);
                let text = if binary {
                    if *oid == 2249 {
                        match value_text(value, *oid)? {
                            None => None,
                            Some(text) => Some(record_binary(
                                std::str::from_utf8(&text)
                                    .map_err(|_| invalid("invalid record UTF8"))?,
                                result
                                    .native_types
                                    .get(index)
                                    .and_then(|tp| tp.postgresql_record_fields.as_deref())
                                    .ok_or_else(|| invalid("missing record field layout"))?,
                            )?),
                        }
                    } else {
                        value_binary(value, *oid)?
                    }
                } else {
                    value_text(value, *oid)?
                };
                if let Some(text) = text {
                    if !binary && text.contains(&0) {
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

/// Preserve calendar diagnostics carried by the I/O encoder without changing
/// unrelated unsupported-codec errors.
pub(crate) fn encoding_sqlstate(error: &io::Error) -> &'static str {
    error
        .get_ref()
        .and_then(|source| source.downcast_ref::<astersql_types::pg_temporal::PgTemporalError>())
        .map_or("0A000", |error| error.sqlstate())
}

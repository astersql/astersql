// Copyright 2026 AsterSQL.
//! PostgreSQL parameters, portal result formats and extended-query connection state.
use crate::conn::{CancellationToken, PreparedMetadata, QueryResult, TiDBContext};
use crate::conn_stmt::BinaryParam;
use crate::pg_conn::{sqlstate, write_error, write_message};
use std::collections::HashMap;
use std::io;
use std::net::TcpStream;
use std::sync::Arc;

type Error = (&'static str, String);
type Result<T> = std::result::Result<T, Error>;
fn error(state: &'static str, message: &str) -> Error {
    (state, message.into())
}
fn engine(e: crate::conn::ConnError) -> Error {
    (sqlstate(&e), e.to_string())
}
struct Reader<'a> {
    bytes: &'a [u8],
}
impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if n > self.bytes.len() {
            return Err(error("08P01", "truncated extended message"));
        }
        let (head, tail) = self.bytes.split_at(n);
        self.bytes = tail;
        Ok(head)
    }
    fn string(&mut self) -> Result<String> {
        let n = self
            .bytes
            .iter()
            .position(|b| *b == 0)
            .ok_or_else(|| error("08P01", "unterminated string"))?;
        let value = std::str::from_utf8(self.take(n)?)
            .map_err(|_| error("08P01", "invalid UTF-8"))?
            .to_owned();
        self.take(1)?;
        Ok(value)
    }
    fn count(&mut self) -> Result<usize> {
        let n = i16::from_be_bytes(self.take(2)?.try_into().unwrap());
        usize::try_from(n).map_err(|_| error("08P01", "negative count"))
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn end(&self) -> Result<()> {
        if self.bytes.is_empty() {
            Ok(())
        } else {
            Err(error("08P01", "trailing message bytes"))
        }
    }
}

/// Replace only indexed markers outside engine strings/comments. The mapping
/// records each occurrence, so repeated and out-of-order parameters remain bound.
pub(crate) fn markers(sql: &str) -> Result<(String, Vec<usize>)> {
    let bytes = sql.as_bytes();
    let mut output = Vec::new();
    let mut map = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        if matches!(bytes[i], b'\'' | b'"' | b'`') {
            let quote = bytes[i];
            i += 1;
            let mut closed = false;
            while i < bytes.len() {
                if bytes[i] == b'\\' {
                    i = (i + 2).min(bytes.len());
                    continue;
                }
                if bytes[i] == quote {
                    i += 1;
                    if bytes.get(i) == Some(&quote) {
                        i += 1;
                    } else {
                        closed = true;
                        break;
                    }
                } else {
                    i += 1;
                }
            }
            if !closed {
                return Err(error("42601", "unterminated quoted SQL"));
            }
        } else if bytes[i..].starts_with(b"/*") {
            i += 2;
            // Engine executable comments could expose hidden parameter markers.
            if matches!(bytes.get(i), Some(b'!') | Some(b'+')) {
                return Err(error(
                    "0A000",
                    "executable and hint comments are unsupported in prepared SQL",
                ));
            }
            while i < bytes.len() && !bytes[i..].starts_with(b"*/") {
                i += 1;
            }
            if i == bytes.len() {
                return Err(error("42601", "unterminated SQL comment"));
            }
            i += 2;
        } else if bytes[i..].starts_with(b"--") || bytes[i] == b'#' {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
        } else if bytes[i] == b'$' {
            i += 1;
            let number = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if i == number {
                return Err(error("0A000", "dollar quoting is unsupported"));
            }
            if start > 0 && (bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_')
                || bytes
                    .get(i)
                    .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_')
            {
                return Err(error("42601", "parameter marker is part of an identifier"));
            }
            let index = sql[number..i]
                .parse::<usize>()
                .map_err(|_| error("42P02", "parameter index overflow"))?;
            if !(1..=32767).contains(&index) {
                return Err(error("42P02", "parameter index out of range"));
            }
            map.push(index - 1);
            output.push(b'?');
            continue;
        } else if bytes[i] == b'?' {
            return Err(error("42601", "use indexed PostgreSQL parameter markers"));
        } else {
            i += 1;
        }
        output.extend_from_slice(&bytes[start..i]);
    }
    Ok((String::from_utf8(output).unwrap(), map))
}

/// Source AST identities, ordered by their original byte positions. Explicit
/// casts can convert an already inferred source; direct assignment contexts
/// must agree when multiple initially unknown nodes share the same PG index.
pub(crate) fn indexed_parameter_contract(sql: &str) -> Result<Vec<(usize, usize, bool)>> {
    use astersql_parser_ast::{ExprKind, ExprNode, Node, SqlDialect, Visitor};
    #[derive(Default)]
    struct Parameters {
        markers: Vec<(usize, usize)>,
        assignments: std::collections::HashSet<usize>,
    }
    impl Visitor for Parameters {
        fn enter(&mut self, node: &dyn Node) -> bool {
            if let Some(expr) = node.as_any().downcast_ref::<ExprNode>() {
                if let (Some(index), ExprKind::ParamMarker { Offset }) =
                    (expr.PostgreSQLParameterIndex(), &expr.Kind)
                {
                    self.markers.push((usize::from(index) - 1, *Offset));
                }
            }
            let assignments = if let Some(insert) = node
                .as_any()
                .downcast_ref::<astersql_parser_ast::InsertStmt>()
            {
                insert.Lists.iter().flatten().collect::<Vec<_>>()
            } else if let Some(update) = node
                .as_any()
                .downcast_ref::<astersql_parser_ast::UpdateStmt>()
            {
                update
                    .List
                    .iter()
                    .map(|assignment| &assignment.Expr)
                    .collect()
            } else {
                Vec::new()
            };
            for mut source in assignments {
                while let ExprKind::Parentheses(inner) = &source.Kind {
                    source = inner;
                }
                if let ExprKind::ParamMarker { Offset } = source.Kind {
                    self.assignments.insert(Offset);
                }
            }
            false
        }
        fn leave(&mut self, _: &dyn Node) -> bool {
            true
        }
    }
    let (statements, _) = astersql_parser::Parser::with_dialect(SqlDialect::PostgreSQL)
        .ParseSQL(sql, &[])
        .map_err(|_| error("42601", "invalid PostgreSQL parameter source"))?;
    let mut visitor = Parameters::default();
    for statement in statements {
        statement.accept(&mut visitor);
    }
    visitor.markers.sort_by_key(|(_, offset)| *offset);
    Ok(visitor
        .markers
        .into_iter()
        .map(|(index, offset)| (index, offset, visitor.assignments.contains(&offset)))
        .collect())
}

fn question_mark_positions(sql: &str) -> Result<Vec<usize>> {
    let bytes = sql.as_bytes();
    let mut positions = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if matches!(bytes[i], b'\'' | b'"' | b'`') {
            let quote = bytes[i];
            i += 1;
            let mut closed = false;
            while i < bytes.len() {
                if bytes[i] == b'\\' {
                    i = (i + 2).min(bytes.len());
                    continue;
                }
                if bytes[i] == quote {
                    i += 1;
                    if bytes.get(i) == Some(&quote) {
                        i += 1;
                    } else {
                        closed = true;
                        break;
                    }
                } else {
                    i += 1;
                }
            }
            if !closed {
                return Err(error("42601", "unterminated quoted SQL"));
            }
        } else if bytes[i..].starts_with(b"/*") {
            i += 2;
            while i < bytes.len() && !bytes[i..].starts_with(b"*/") {
                i += 1;
            }
            if i == bytes.len() {
                return Err(error("42601", "unterminated SQL comment"));
            }
            i += 2;
        } else if bytes[i..].starts_with(b"--") || bytes[i] == b'#' {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
        } else if bytes[i] == b'?' {
            positions.push(i);
            i += 1;
        } else {
            i += 1;
        }
    }
    Ok(positions)
}

fn analysis_parameter_mapping(
    sql: &str,
    mapping: &[usize],
    omitted_span: Option<(usize, usize)>,
) -> Result<Vec<usize>> {
    let Some((start, end)) = omitted_span else {
        return Ok(mapping.to_vec());
    };
    let positions = question_mark_positions(sql)?;
    if start >= end || positions.len() != mapping.len() {
        return Err(error(
            "0A000",
            "invalid PostgreSQL conflict parameter mapping",
        ));
    }
    let prefix_count = positions.partition_point(|position| *position < start);
    let after_clause = positions.partition_point(|position| *position < end);
    let mut analysis_mapping = Vec::with_capacity(mapping.len() - (after_clause - prefix_count));
    analysis_mapping.extend_from_slice(&mapping[..prefix_count]);
    analysis_mapping.extend_from_slice(&mapping[after_clause..]);
    Ok(analysis_mapping)
}

pub(crate) fn parameter(oid: u32, value: Option<&[u8]>) -> Result<BinaryParam> {
    let Some(value) = value else {
        return Ok(BinaryParam {
            is_null: true,
            tp: if oid == 16 { 1 } else { 6 },
            ..BinaryParam::default()
        });
    };
    let text =
        std::str::from_utf8(value).map_err(|_| error("22021", "text parameter is not UTF-8"))?;
    if text.contains('\0') {
        return Err(error("22021", "text parameter contains NUL"));
    }
    let invalid = || error("22P02", "invalid text parameter value");
    let (tp, value) = match oid {
        16 => (
            1,
            vec![match text
                .trim_matches(|c| matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0b' | '\x0c'))
                .to_ascii_lowercase()
                .as_str()
            {
                "t" | "tr" | "tru" | "true" | "y" | "ye" | "yes" | "on" | "1" => 1,
                "f" | "fa" | "fal" | "fals" | "false" | "n" | "no" | "of" | "off" | "0" => 0,
                _ => return Err(invalid()),
            }],
        ),
        21 => (
            2,
            astersql_expression::pg_cast_int2(Some(text))
                .map_err(|e| error(e.sqlstate(), &e.to_string()))?
                .expect("non-null parameter")
                .to_le_bytes()
                .to_vec(),
        ),
        23 => (
            3,
            astersql_expression::pg_cast_int4(Some(text))
                .map_err(|e| error(e.sqlstate(), &e.to_string()))?
                .expect("non-null parameter")
                .to_le_bytes()
                .to_vec(),
        ),
        20 => (
            8,
            astersql_expression::pg_cast_int8(Some(text))
                .map_err(|e| error(e.sqlstate(), &e.to_string()))?
                .expect("non-null parameter")
                .to_le_bytes()
                .to_vec(),
        ),
        700 => (
            4,
            astersql_expression::pg_cast_float4(Some(text))
                .map_err(|e| error(e.sqlstate(), &e.to_string()))?
                .expect("non-null parameter")
                .to_le_bytes()
                .to_vec(),
        ),
        701 => (
            5,
            astersql_expression::pg_cast_float8(Some(text))
                .map_err(|e| error(e.sqlstate(), &e.to_string()))?
                .expect("non-null parameter")
                .to_le_bytes()
                .to_vec(),
        ),
        25 | 1043 | 1042 => (253, value.to_vec()),
        1700 => {
            let numeric = text
                .parse::<astersql_types::pg_numeric::PgNumeric>()
                .map_err(|e| error(e.sqlstate(), &e.to_string()))?;
            // Text transport avoids the MySQL prepared DECIMAL capacity. The
            // prepared context retains OID1700 and inserts the numeric cast.
            (253, numeric.to_string().into_bytes())
        }
        17 => {
            let mut bytes = Vec::new();
            if let Some(hex) = text.strip_prefix("\\x") {
                let invalid_hex = || error("22023", "invalid hexadecimal bytea input");
                let mut digits = hex.bytes();
                while let Some(high) = digits.next() {
                    // PostgreSQL permits whitespace only between complete bytes.
                    if matches!(high, b' ' | b'\t' | b'\n' | b'\r') {
                        continue;
                    }
                    let high = (high as char).to_digit(16).ok_or_else(invalid_hex)?;
                    let low = digits.next().ok_or_else(invalid_hex)?;
                    let low = (low as char).to_digit(16).ok_or_else(invalid_hex)?;
                    bytes.push((high * 16 + low) as u8);
                }
            } else {
                let mut input = value.iter().copied();
                while let Some(byte) = input.next() {
                    if byte != b'\\' {
                        bytes.push(byte);
                        continue;
                    }
                    let first = input.next().ok_or_else(invalid)?;
                    if first == b'\\' {
                        bytes.push(b'\\');
                        continue;
                    }
                    // Exactly three octal digits must fit in one byte (000–377).
                    let second = input.next().ok_or_else(invalid)?;
                    let third = input.next().ok_or_else(invalid)?;
                    if !(b'0'..=b'3').contains(&first)
                        || !(b'0'..=b'7').contains(&second)
                        || !(b'0'..=b'7').contains(&third)
                    {
                        return Err(invalid());
                    }
                    bytes.push((first - b'0') * 64 + (second - b'0') * 8 + (third - b'0'));
                }
            }
            (252, bytes)
        }
        1082 => (
            253,
            text.parse::<astersql_types::pg_temporal::PgDate>()
                .map_err(|e| error(e.sqlstate(), &e.to_string()))?
                .to_string()
                .into_bytes(),
        ),
        1083 | 1114 => temporal(oid, text)?,
        _ => return Err(error("0A000", "unsupported text parameter OID")),
    };
    Ok(BinaryParam {
        tp,
        value,
        ..BinaryParam::default()
    })
}
/// Convert PG network order to the shared engine's existing parameter encoding.
fn binary_parameter(oid: u32, value: Option<&[u8]>) -> Result<BinaryParam> {
    let Some(value) = value else {
        return parameter(oid, None);
    };
    let invalid = || error("22P03", "invalid binary parameter");
    let (tp, bytes) = match oid {
        16 => match value {
            [0] | [1] => (1, value.to_vec()),
            _ => return Err(invalid()),
        },
        21 => (
            2,
            i16::from_be_bytes(value.try_into().map_err(|_| invalid())?)
                .to_le_bytes()
                .to_vec(),
        ),
        23 => (
            3,
            i32::from_be_bytes(value.try_into().map_err(|_| invalid())?)
                .to_le_bytes()
                .to_vec(),
        ),
        20 => (
            8,
            i64::from_be_bytes(value.try_into().map_err(|_| invalid())?)
                .to_le_bytes()
                .to_vec(),
        ),
        700 => (
            4,
            f32::from_be_bytes(value.try_into().map_err(|_| invalid())?)
                .to_le_bytes()
                .to_vec(),
        ),
        701 => (
            5,
            f64::from_be_bytes(value.try_into().map_err(|_| invalid())?)
                .to_le_bytes()
                .to_vec(),
        ),
        1082 => {
            let date = astersql_types::pg_temporal::PgDate::from_binary(value)
                .map_err(|e| error(e.sqlstate(), &e.to_string()))?;
            return parameter(1082, Some(date.to_string().as_bytes()));
        }
        17 => (252, value.to_vec()),
        25 | 1042 | 1043 => return parameter(oid, Some(value)),
        _ => return Err(error("0A000", "unsupported binary parameter OID")),
    };
    Ok(BinaryParam {
        tp,
        value: bytes,
        ..BinaryParam::default()
    })
}

fn catalog_parameter(oid: u32, value: Option<&[u8]>) -> Result<crate::pg_catalog_query::Expr> {
    use crate::pg_catalog_query::Expr;
    let Some(value) = value else {
        return Ok(Expr::Null);
    };
    if oid == 26 {
        let text =
            std::str::from_utf8(value).map_err(|_| error("22021", "OID parameter is not UTF-8"))?;
        if text.contains('\0') {
            return Err(error("22021", "OID parameter contains NUL"));
        }
        let number = text.trim().parse::<i128>().map_err(|cause| {
            if matches!(
                cause.kind(),
                std::num::IntErrorKind::PosOverflow | std::num::IntErrorKind::NegOverflow
            ) {
                error("22003", "OID parameter out of range")
            } else {
                error("22P02", "invalid OID parameter")
            }
        })?;
        let number =
            u32::try_from(number).map_err(|_| error("22003", "OID parameter out of range"))?;
        return Ok(Expr::Integer(i64::from(number)));
    }
    let parsed = parameter(oid, Some(value))?;
    Ok(match oid {
        16 => Expr::Boolean(parsed.value[0] != 0),
        21 => Expr::Integer(i64::from(i16::from_le_bytes(
            parsed.value.try_into().unwrap(),
        ))),
        23 => Expr::Integer(i64::from(i32::from_le_bytes(
            parsed.value.try_into().unwrap(),
        ))),
        20 => Expr::Integer(i64::from_le_bytes(parsed.value.try_into().unwrap())),
        25 | 1042 | 1043 => Expr::Text(String::from_utf8(parsed.value).unwrap()),
        _ => return Err(error("0A000", "unsupported catalog parameter OID")),
    })
}

fn catalog_binary_parameter(
    oid: u32,
    value: Option<&[u8]>,
) -> Result<crate::pg_catalog_query::Expr> {
    use crate::pg_catalog_query::Expr;
    let Some(value) = value else {
        return Ok(Expr::Null);
    };
    let invalid = || error("22P03", "invalid binary catalog parameter");
    // JDBC can bind a schema identifier as int4/int8 and cast it to oid in
    // SQL, or bind an oid directly. OIDs preserve the full unsigned range.
    Ok(match oid {
        26 => Expr::Integer(i64::from(u32::from_be_bytes(
            value.try_into().map_err(|_| invalid())?,
        ))),
        20 => Expr::Integer(i64::from_be_bytes(value.try_into().map_err(|_| invalid())?)),
        23 => Expr::Integer(i64::from(i32::from_be_bytes(
            value.try_into().map_err(|_| invalid())?,
        ))),
        21 => Expr::Integer(i64::from(i16::from_be_bytes(
            value.try_into().map_err(|_| invalid())?,
        ))),
        16 => match value {
            [0] => Expr::Boolean(false),
            [1] => Expr::Boolean(true),
            _ => return Err(invalid()),
        },
        25 | 1042 | 1043 => return catalog_parameter(oid, Some(value)),
        _ => return Err(error("0A000", "unsupported binary catalog parameter OID")),
    })
}

fn temporal(oid: u32, text: &str) -> Result<(u8, Vec<u8>)> {
    use chrono::{Datelike, Timelike};
    let invalid = || error("22007", "invalid date or time parameter");
    if oid == 1083 {
        let time = chrono::NaiveTime::parse_from_str(text, "%H:%M:%S%.f").map_err(|_| invalid())?;
        if time.nanosecond() >= 1_000_000_000 || time.nanosecond() % 1000 != 0 {
            return Err(error("0A000", "time exceeds engine microsecond precision"));
        }
        let mut bytes = vec![0];
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&[time.hour() as u8, time.minute() as u8, time.second() as u8]);
        bytes.extend_from_slice(&(time.nanosecond() / 1000).to_le_bytes());
        return Ok((11, bytes));
    }
    let datetime = chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S%.f")
        .map_err(|_| invalid())?;
    let date = datetime.date();
    let time = datetime.time();
    if !(1..=9999).contains(&date.year()) {
        return Err(error("0A000", "date outside engine range"));
    }
    let mut bytes = (date.year() as u16).to_le_bytes().to_vec();
    bytes.extend_from_slice(&[date.month() as u8, date.day() as u8]);
    if time.nanosecond() >= 1_000_000_000 || time.nanosecond() % 1000 != 0 {
        return Err(error(
            "0A000",
            "timestamp exceeds engine microsecond precision",
        ));
    }
    bytes.extend_from_slice(&[time.hour() as u8, time.minute() as u8, time.second() as u8]);
    bytes.extend_from_slice(&(time.nanosecond() / 1000).to_le_bytes());
    Ok((12, bytes))
}
struct Statement {
    original_sql: String,
    path: Vec<String>,
    catalog: Option<crate::pg_catalog::CatalogQuery>,
    session_query: Option<crate::pg_session::SessionQuery>,
    metadata: PreparedMetadata,
    oids: Vec<u32>,
    mapping: Vec<usize>,
    command: Option<&'static str>,
    returning: bool,
    postgresql_conflict: Option<astersql_parser_ast::PostgreSQLConflictClause>,
}
struct Portal {
    integer_parameters: bool,
    catalog: Option<crate::pg_catalog::CatalogQuery>,
    session_query: Option<crate::pg_session::SessionQuery>,
    statement_name: String,
    columns: Vec<crate::conn::ColumnInfo>,
    native_types: Vec<crate::conn::NativeType>,
    column_origins: Vec<Option<astersql_session::runtime::ConcreteColumnOrigin>>,
    statement: u32,
    args: Vec<BinaryParam>,
    description: Option<Vec<u8>>,
    formats: Vec<usize>,
    command: Option<&'static str>,
    returning: bool,
    postgresql_conflict: Option<astersql_parser_ast::PostgreSQLConflictClause>,
    messages: Option<Vec<(u8, Vec<u8>)>>,
    offset: usize,
}
#[derive(Default)]
pub(crate) struct Extended {
    startup_epoch_micros: u128,
    pub(crate) session: crate::pg_session::PgSession,
    statements: HashMap<String, Statement>,
    portals: HashMap<String, Portal>,
    failed: bool,
    // A bound portal pins its engine plan across Statement reanalysis.
    retired_statements: Vec<u32>,
}
impl Extended {
    pub(crate) fn new(startup_epoch_micros: u128) -> Self {
        Self {
            startup_epoch_micros,
            ..Self::default()
        }
    }

    pub(crate) fn reset_unnamed(&mut self, context: &Arc<dyn TiDBContext>) {
        if let Some(statement) = self.statements.remove("") {
            if statement.catalog.is_none() && statement.session_query.is_none() {
                let _ = context.close_prepared_statement(statement.metadata.statement_id);
            }
            self.portals.retain(|_, p| !p.statement_name.is_empty());
        }
        self.portals.remove("");
        self.collect_retired(context);
    }
    pub(crate) fn handle(
        &mut self,
        tag: u8,
        body: &[u8],
        socket: &mut TcpStream,
        context: &Arc<dyn TiDBContext>,
        execute: impl FnOnce(
            u32,
            &[BinaryParam],
            Option<crate::pg_catalog::CatalogQuery>,
        ) -> io::Result<crate::conn::ConnResult<QueryResult>>,
    ) -> io::Result<bool> {
        if self.failed && tag != b'S' {
            return Ok(true);
        }
        if !matches!(tag, b'P' | b'B' | b'D' | b'E' | b'C' | b'S' | b'H') {
            if tag == b'Q' {
                return Ok(false);
            }
            self.failed = true;
            write_error(socket, "ERROR", "0A000", "unsupported frontend command")?;
            return Ok(true);
        }
        if tag == b'S' {
            if !body.is_empty() {
                self.failed = true;
                write_error(socket, "ERROR", "08P01", "Sync must be empty")?;
                return Ok(true);
            }
            self.failed = false;
            if !context.in_transaction() {
                self.portals.clear();
            }
            self.collect_retired(context);
            write_message(
                socket,
                b'Z',
                &[self.session.transaction_status(context.as_ref())],
            )?;
            return Ok(true);
        }
        if self.failed {
            return Ok(true);
        }
        self.session.relation_error_position = None;
        // PostgreSQL sends ParameterDescription before revalidating the result
        // descriptor. Its parameter contract survives a later analysis error.
        if tag == b'D' {
            let mut reader = Reader { bytes: body };
            if reader.take(1).ok() == Some(b"S".as_slice()) {
                if let Ok(name) = reader.string() {
                    if reader.end().is_ok() {
                        if let Some(statement) = self.statements.get(&name) {
                            let mut parameters =
                                (statement.oids.len() as i16).to_be_bytes().to_vec();
                            for oid in &statement.oids {
                                parameters.extend_from_slice(&oid.to_be_bytes());
                            }
                            write_message(socket, b't', &parameters)?;
                        }
                    }
                }
            }
        }
        match self.process(tag, body, context, execute) {
            Ok(messages) => {
                for (tag, body) in messages {
                    write_message(socket, tag, &body)?;
                }
            }
            Err((state, message)) => {
                self.failed = true;
                self.session.record_error(context.as_ref());
                let original_sql = if tag == b'P' {
                    let mut reader = Reader { bytes: body };
                    reader.string().and_then(|_| reader.string()).ok()
                } else if tag == b'B' || tag == b'D' {
                    let mut reader = Reader { bytes: body };
                    let name = if tag == b'B' {
                        reader.string().and_then(|_| reader.string()).ok()
                    } else if reader.take(1).ok() == Some(b"S".as_slice()) {
                        reader.string().ok()
                    } else {
                        None
                    };
                    name.and_then(|name| self.statements.get(&name).map(|s| s.original_sql.clone()))
                } else {
                    None
                };
                if let Some(sql) = original_sql {
                    crate::pg_conn::write_sql_error(
                        socket,
                        state,
                        &message,
                        &sql,
                        self.session.relation_error_position,
                    )?;
                } else {
                    write_error(socket, "ERROR", state, &message)?;
                }
            }
        }
        self.collect_retired(context);
        Ok(true)
    }
    fn prepare(
        &mut self,
        sql: String,
        mut oids: Vec<u32>,
        context: &Arc<dyn TiDBContext>,
    ) -> Result<Statement> {
        let count = oids.len();
        let supplied_oids = oids.clone();
        let mut parameter_contract = Vec::new();
        let original_sql = sql.clone();
        let path = self
            .session
            .effective_explicit_path(context.as_ref())
            .map_err(engine)?;
        let session_query = crate::pg_session::SessionQuery::parse(&sql)?;
        let catalog = if session_query.is_some() {
            None
        } else {
            crate::pg_catalog::CatalogQuery::parse_session_with_types(
                &sql,
                context.as_ref(),
                &self.session,
                &oids,
            )?
        };
        let (sql, mapping) = if catalog.is_some() || session_query.is_some() {
            (sql, Vec::new())
        } else {
            let sql = crate::pg_name::adapt(&sql, context.as_ref(), &mut self.session)?;
            let sql = crate::pg_sql::adapt_with_context(&sql, context.as_ref())?;
            let (engine_sql, mapping) = markers(&sql)?;
            if !mapping.is_empty() {
                parameter_contract = indexed_parameter_contract(&original_sql)?;
            }
            let identities = parameter_contract
                .iter()
                .map(|(index, _, _)| *index)
                .collect::<Vec<_>>();
            if mapping != identities {
                return Err(error(
                    "XX000",
                    "PostgreSQL parameter source identity mismatch",
                ));
            }
            (engine_sql, identities)
        };
        let sql = if catalog.is_some() || session_query.is_some() {
            std::borrow::Cow::Borrowed(sql.as_str())
        } else {
            crate::pg_result::adapt_session_query(&sql, self.startup_epoch_micros)?
        };
        let n = catalog.as_ref().map_or_else(
            || mapping.iter().max().map_or(0, |n| n + 1),
            |query| query.parameter_oids().len(),
        );
        if let Some(query) = &catalog {
            oids = query.parameter_oids().to_vec();
        }
        if count > n {
            return Err(error("08P01", "too many parameter OIDs"));
        }
        oids.resize(n, 0);
        for oid in &oids {
            if catalog.is_none()
                && !matches!(
                    *oid,
                    0 | 16
                        | 17
                        | 20
                        | 21
                        | 23
                        | 25
                        | 700
                        | 701
                        | 1042
                        | 1043
                        | 1082
                        | 1083
                        | 1114
                        | 1700
                )
            {
                return Err(error("0A000", "unsupported text parameter OID"));
            }
        }
        let (command, returning, postgresql_conflict, analysis_sql) =
            if let Some(query) = &session_query {
                (Some(query.command()), false, None, None)
            } else if catalog.is_some() {
                (Some("SELECT"), false, None, None)
            } else {
                let plan = crate::pg_result::prepare_command(&sql)?;
                (
                    plan.command,
                    plan.returning,
                    plan.postgresql_conflict,
                    plan.analysis_sql,
                )
            };
        let analysis_mapping = analysis_parameter_mapping(
            &sql,
            &mapping,
            postgresql_conflict
                .as_ref()
                .map(|conflict| conflict.SourceSpan),
        )?;
        let analysis_occurrences = analysis_parameter_mapping(
            &sql,
            &(0..mapping.len()).collect::<Vec<_>>(),
            postgresql_conflict
                .as_ref()
                .map(|conflict| conflict.SourceSpan),
        )?;
        if command.is_none() {
            return Err(error("0A000", "empty prepared statements are unsupported"));
        }
        self.session
            .check_transaction_command(command, context.as_ref())?;
        let metadata = if let Some(query) = &session_query {
            query.metadata()
        } else if let Some(catalog) = &catalog {
            catalog.metadata()
        } else {
            let analysis_sql = analysis_sql.as_deref().unwrap_or(&sql);
            context
                .prepare_statement_with_parameter_types(
                    analysis_sql,
                    &analysis_mapping
                        .iter()
                        .map(|index| oids[*index])
                        .collect::<Vec<_>>(),
                    &CancellationToken::new(),
                )
                .map_err(|error| {
                    if let crate::conn::ConnError::PgArray(array) = &error {
                        self.session.relation_error_position = array.position.and_then(|offset| {
                            analysis_sql
                                .get(..offset)
                                .map(|prefix| prefix.chars().count() + 1)
                        });
                    }
                    engine(error)
                })?
        };
        if catalog.is_none() && metadata.parameter_count != analysis_mapping.len() {
            if catalog.is_none() && session_query.is_none() {
                let _ = context.close_prepared_statement(metadata.statement_id);
            }
            return Err(error("0A000", "engine parameter count mismatch"));
        }
        // Inference comes from parsed engine expressions, in occurrence
        // order. Fold those constraints into PostgreSQL parameter order;
        // the first direct cast fixes an unspecified source type.
        for ((index, occurrence), oid) in analysis_mapping
            .iter()
            .zip(&analysis_occurrences)
            .zip(&metadata.parameter_cast_oids)
        {
            if oids[*index] == 0 {
                oids[*index] = *oid;
            } else if *oid != 0
                && oids[*index] != *oid
                && supplied_oids.get(*index).copied().unwrap_or(0) == 0
                && let Some((_, offset, true)) = parameter_contract.get(*occurrence)
            {
                let _ = context.close_prepared_statement(metadata.statement_id);
                self.session.relation_error_position = original_sql
                    .get(..*offset)
                    .map(|prefix| prefix.chars().count() + 1);
                return Err(error("42P08", "inconsistent types deduced for parameter"));
            }
        }
        if oids.contains(&0) {
            if catalog.is_none() && session_query.is_none() {
                let _ = context.close_prepared_statement(metadata.statement_id);
            }
            return Err(error("42P18", "could not determine data type of parameter"));
        }
        Ok(Statement {
            original_sql,
            path,
            session_query,
            catalog,
            metadata,
            oids,
            mapping,
            command,
            returning,
            postgresql_conflict,
        })
    }
    fn collect_retired(&mut self, context: &Arc<dyn TiDBContext>) {
        self.retired_statements.retain(|id| {
            if self.portals.values().any(|portal| portal.statement == *id) {
                true
            } else {
                let _ = context.close_prepared_statement(*id);
                false
            }
        });
    }

    fn reanalyze(&mut self, name: &str, context: &Arc<dyn TiDBContext>) -> Result<()> {
        let statement = self
            .statements
            .get(name)
            .ok_or_else(|| error("26000", "unknown prepared statement"))?;
        let path = self
            .session
            .effective_explicit_path(context.as_ref())
            .map_err(engine)?;
        if statement.path == path {
            return Ok(());
        }
        let original_sql = statement.original_sql.clone();
        let oids = statement.oids.clone();
        let previous_shape =
            result_shape(&statement.metadata, statement.command.unwrap_or("SELECT"))?;
        let replacement = self.prepare(original_sql, oids, context)?;
        if result_shape(
            &replacement.metadata,
            replacement.command.unwrap_or("SELECT"),
        )? != previous_shape
        {
            if replacement.catalog.is_none() && replacement.session_query.is_none() {
                let _ = context.close_prepared_statement(replacement.metadata.statement_id);
            }
            return Err(error("0A000", "cached plan must not change result type"));
        }
        let previous = self
            .statements
            .insert(name.to_owned(), replacement)
            .unwrap();
        if previous.catalog.is_none() && previous.session_query.is_none() {
            self.retired_statements.push(previous.metadata.statement_id);
        }
        self.collect_retired(context);
        Ok(())
    }

    fn process(
        &mut self,
        tag: u8,
        body: &[u8],
        context: &Arc<dyn TiDBContext>,
        execute: impl FnOnce(
            u32,
            &[BinaryParam],
            Option<crate::pg_catalog::CatalogQuery>,
        ) -> io::Result<crate::conn::ConnResult<QueryResult>>,
    ) -> Result<Vec<(u8, Vec<u8>)>> {
        let mut reader = Reader { bytes: body };
        match tag {
            b'P' => {
                let name = reader.string()?;
                let sql = reader.string()?;
                let count = reader.count()?;
                let mut oids = Vec::new();
                for _ in 0..count {
                    oids.push(reader.u32()?);
                }
                reader.end()?;
                if !name.is_empty() && self.statements.contains_key(&name) {
                    return Err(error("42P05", "prepared statement already exists"));
                }
                let statement = self.prepare(sql, oids, context)?;
                if let Some(previous) = self.statements.remove(&name) {
                    if previous.catalog.is_none() && previous.session_query.is_none() {
                        context
                            .close_prepared_statement(previous.metadata.statement_id)
                            .map_err(engine)?;
                    }
                    self.portals.retain(|_, p| p.statement_name != name);
                }
                if name.is_empty() {
                    self.portals.remove("");
                }
                self.statements.insert(name, statement);
                Ok(vec![(b'1', vec![])])
            }
            b'B' => {
                let name = reader.string()?;
                let statement_name = reader.string()?;
                if !name.is_empty() && self.portals.contains_key(&name) {
                    return Err(error("42P03", "portal already exists"));
                }
                self.reanalyze(&statement_name, context)?;
                let statement = self
                    .statements
                    .get(&statement_name)
                    .ok_or_else(|| error("26000", "unknown prepared statement"))?;
                let formats = reader.count()?;
                let mut parameter_formats = Vec::with_capacity(formats);
                for _ in 0..formats {
                    let format = reader.count()?;
                    if format > 1 {
                        return Err(error("08P01", "invalid parameter format code"));
                    }
                    parameter_formats.push(format);
                }
                let count = reader.count()?;
                if count != statement.oids.len()
                    || !(formats == 0 || formats == 1 || formats == count)
                {
                    return Err(error("08P01", "parameter count mismatch"));
                }
                let mut args = Vec::new();
                let mut catalog_values = Vec::new();
                let date_context = if statement.catalog.is_none() && statement.oids.contains(&1082)
                {
                    Some(context.pg_date_context(None).map_err(engine)?)
                } else {
                    None
                };
                for (index, oid) in statement.oids.iter().enumerate() {
                    let format = match parameter_formats.as_slice() {
                        [] => 0,
                        [format] => *format,
                        formats => formats[index],
                    };
                    let length = i32::from_be_bytes(reader.take(4)?.try_into().unwrap());
                    let value = if length == -1 {
                        None
                    } else {
                        Some(
                            reader.take(
                                usize::try_from(length)
                                    .map_err(|_| error("08P01", "invalid parameter length"))?,
                            )?,
                        )
                    };
                    if statement.catalog.is_some() {
                        catalog_values.push(if format == 1 {
                            catalog_binary_parameter(*oid, value)?
                        } else {
                            catalog_parameter(*oid, value)?
                        });
                    } else {
                        args.push(if format == 1 {
                            binary_parameter(*oid, value)?
                        } else {
                            if *oid == 1082 && value.is_some() {
                                let text = std::str::from_utf8(value.unwrap())
                                    .map_err(|_| error("22021", "invalid UTF-8 date parameter"))?;
                                if text.contains('\0') {
                                    return Err(error("22021", "NUL in date parameter"));
                                }
                                let date = astersql_types::pg_temporal::PgDate::parse_with_context(
                                    text,
                                    date_context.unwrap(),
                                )
                                .map_err(|e| error(e.sqlstate(), &e.to_string()))?;
                                parameter(*oid, Some(date.to_string().as_bytes()))?
                            } else {
                                parameter(*oid, value)?
                            }
                        });
                    }
                }
                let formats = reader.count()?;
                if !(formats == 0 || formats == 1 || formats == statement.metadata.columns.len()) {
                    return Err(error("08P01", "result format count mismatch"));
                }
                let mut result_formats = Vec::with_capacity(formats);
                for _ in 0..formats {
                    let format = reader.count()?;
                    if format > 1 {
                        return Err(error("08P01", "invalid result format code"));
                    }
                    result_formats.push(format);
                }
                let result_formats = match result_formats.as_slice() {
                    [] => vec![0; statement.metadata.columns.len()],
                    [format] => vec![*format; statement.metadata.columns.len()],
                    _ => result_formats,
                };
                reader.end()?;
                let description = description_formats(
                    &statement.metadata,
                    &result_formats,
                    statement.command.unwrap_or("SELECT"),
                )?;
                let mut catalog = statement.catalog.clone();
                if let Some(query) = &mut catalog {
                    query.bind_values(catalog_values)?;
                }
                self.portals.insert(
                    name,
                    Portal {
                        integer_parameters: !statement.mapping.is_empty()
                            && statement
                                .metadata
                                .parameter_cast_oids
                                .iter()
                                .all(|oid| *oid != 0),
                        catalog,
                        session_query: statement.session_query.clone(),
                        statement_name,
                        columns: statement.metadata.columns.clone(),
                        native_types: statement.metadata.native_types.clone(),
                        column_origins: statement.metadata.column_origins.clone(),
                        statement: statement.metadata.statement_id,
                        args: statement.mapping.iter().map(|i| args[*i].clone()).collect(),
                        description,
                        formats: result_formats,
                        command: statement.command,
                        returning: statement.returning,
                        postgresql_conflict: statement.postgresql_conflict.clone(),
                        messages: None,
                        offset: 0,
                    },
                );
                Ok(vec![(b'2', vec![])])
            }
            b'D' => {
                let kind = reader.take(1)?[0];
                let name = reader.string()?;
                reader.end()?;
                match kind {
                    b'S' => {
                        self.reanalyze(&name, context)?;
                        let statement = self
                            .statements
                            .get(&name)
                            .ok_or_else(|| error("26000", "unknown prepared statement"))?;
                        let mut messages = Vec::new();
                        messages.push(
                            description(
                                &statement.metadata,
                                statement.command.unwrap_or("SELECT"),
                            )?
                            .map_or((b'n', vec![]), |body| (b'T', body)),
                        );
                        Ok(messages)
                    }
                    b'P' => {
                        let portal = self
                            .portals
                            .get(&name)
                            .ok_or_else(|| error("34000", "unknown portal"))?;
                        Ok(vec![
                            portal
                                .description
                                .clone()
                                .map_or((b'n', vec![]), |body| (b'T', body)),
                        ])
                    }
                    _ => Err(error("08P01", "invalid Describe target")),
                }
            }
            b'E' => {
                let name = reader.string()?;
                let limit = reader.u32()?;
                reader.end()?;
                if limit > i32::MAX as u32 {
                    return Err(error("08P01", "negative row limit"));
                }
                let portal = self
                    .portals
                    .get_mut(&name)
                    .ok_or_else(|| error("34000", "unknown portal"))?;
                if portal.postgresql_conflict.is_some() {
                    return Err(error(
                        "0A000",
                        "PostgreSQL ON CONFLICT DO NOTHING is unsupported by the shared executor",
                    ));
                }
                if portal.returning {
                    return Err(error(
                        "0A000",
                        "DML RETURNING is unsupported by the PostgreSQL listener",
                    ));
                }
                if portal.messages.is_none() {
                    let Some(command) = portal.command else {
                        return Ok(vec![(b'I', vec![])]);
                    };
                    let execution = if let Some(query) = &portal.session_query {
                        self.session.execute(query, context.as_ref())
                    } else {
                        let catalog = if let Some(mut query) = portal.catalog.clone() {
                            query.current_schema = self
                                .session
                                .effective_schema(context.as_ref())
                                .map_err(engine)?;
                            Some(query)
                        } else {
                            None
                        };
                        execute(portal.statement, &portal.args, catalog)
                            .map_err(|e| error("XX000", &e.to_string()))?
                    };
                    let mut result = match execution {
                        Ok(result) => result,
                        Err(e) => {
                            context.finish_protocol_response(std::time::Duration::ZERO);
                            return Err(engine(e));
                        }
                    };
                    self.session.record_success(Some(command), context.as_ref());
                    // Constant evaluation may omit record-set fields. The original
                    // AST types remain fixed for parameter-free SQL.
                    if portal.args.is_empty()
                        && result.native_types.is_empty()
                        && result.columns.len() == portal.native_types.len()
                    {
                        result.native_types = portal.native_types.clone();
                        result.column_origins = portal.column_origins.clone();
                        result.columns = portal.columns.clone();
                    }
                    // Binding reparses literals in the shared kernel. Keep the
                    // prepared labels while still checking actual result types.
                    if portal.integer_parameters && result.columns.len() == portal.columns.len() {
                        for (column, prepared) in result.columns.iter_mut().zip(&portal.columns) {
                            column.name.clone_from(&prepared.name);
                        }
                    }
                    let encoded =
                        crate::pg_result::encode_formats(&result, command, &portal.formats)
                            .map_err(|e| {
                                error(crate::pg_result::encoding_sqlstate(&e), &e.to_string())
                            });
                    if let Some(lifecycle) = &result.response_lifecycle {
                        lifecycle.finish();
                    }
                    context.finish_protocol_response(std::time::Duration::ZERO);
                    let mut messages = encoded?;
                    if messages.first().is_some_and(|m| m.0 == b'T') {
                        let metadata = messages.remove(0).1;
                        if portal.description.as_ref() != Some(&metadata) {
                            return Err(error(
                                "0A000",
                                "engine prepared and executed result metadata disagree",
                            ));
                        }
                    }
                    portal.messages = Some(messages);
                }
                let messages = portal.messages.as_ref().unwrap();
                let start = portal.offset;
                let mut rows = 0;
                while portal.offset < messages.len() {
                    if messages[portal.offset].0 == b'D' {
                        if limit != 0 && rows == limit {
                            break;
                        }
                        rows += 1;
                    }
                    portal.offset += 1;
                }
                let mut response = messages[start..portal.offset].to_vec();
                if portal.offset < messages.len() {
                    response.push((b's', vec![]));
                } else if response.is_empty() {
                    if let Some(completion) = messages.last() {
                        response.push(completion.clone());
                    }
                }
                Ok(response)
            }
            b'C' => {
                let kind = reader.take(1)?[0];
                let name = reader.string()?;
                reader.end()?;
                match kind {
                    b'S' => {
                        // Clients also close names whose Parse failed. PG Close
                        // succeeds for absent objects, allowing the next pipeline
                        // to proceed after Sync instead of starting another error.
                        if let Some(statement) = self.statements.remove(&name) {
                            if statement.catalog.is_none() && statement.session_query.is_none() {
                                context
                                    .close_prepared_statement(statement.metadata.statement_id)
                                    .map_err(engine)?;
                            }
                        }
                        self.portals.retain(|_, p| p.statement_name != name);
                    }
                    b'P' => {
                        self.portals.remove(&name);
                    }
                    _ => return Err(error("08P01", "invalid Close target")),
                }
                Ok(vec![(b'3', vec![])])
            }
            b'H' => {
                reader.end()?;
                Ok(vec![])
            }
            _ => unreachable!(),
        }
    }
}
// PostgreSQL's fixed result contract compares names and types, not origin IDs:
// another relation with the same row type is a valid reanalysis target.
fn result_shape(metadata: &PreparedMetadata, command: &str) -> Result<Option<Vec<u8>>> {
    let mut shape = metadata.clone();
    shape.column_origins.clear();
    shape.catalog_column_origins.clear();
    description(&shape, command)
}
fn description(metadata: &PreparedMetadata, command: &str) -> Result<Option<Vec<u8>>> {
    description_formats(metadata, &[], command)
}
fn description_formats(
    metadata: &PreparedMetadata,
    formats: &[usize],
    command: &str,
) -> Result<Option<Vec<u8>>> {
    if metadata.columns.is_empty() {
        return Ok(None);
    }
    let result = QueryResult {
        columns: metadata.columns.clone(),
        native_types: metadata.native_types.clone(),
        column_origins: metadata.column_origins.clone(),
        catalog_column_origins: metadata.catalog_column_origins.clone(),
        ..QueryResult::default()
    };
    let messages = crate::pg_result::encode_formats(&result, command, formats)
        .map_err(|e| error("0A000", &e.to_string()))?;
    Ok(Some(messages[0].1.clone()))
}

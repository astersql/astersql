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
    let (source_sql, row_insertions) = crate::pg_sql::values_parameter_source(sql)?;
    let (statements, _) = astersql_parser::Parser::with_dialect(SqlDialect::PostgreSQL)
        .ParseSQL(&source_sql, &[])
        .map_err(|_| error("42601", "invalid PostgreSQL parameter source"))?;
    let mut visitor = Parameters::default();
    for statement in statements {
        statement.accept(&mut visitor);
    }
    visitor.markers.sort_by_key(|(_, offset)| *offset);
    Ok(visitor
        .markers
        .into_iter()
        .map(|(index, offset)| {
            let added = row_insertions
                .iter()
                .enumerate()
                .filter(|(n, start)| **start + n * 4 < offset)
                .count()
                * 4;
            (index, offset - added, visitor.assignments.contains(&offset))
        })
        .collect())
}

/// Direct temporal parameter casts can reject constants during Bind planning.
/// Collect from the real AST; column metadata cannot identify which parameter
/// a projection consumes, and conditional expressions must stay lazy.
fn direct_temporal_parameter_modifiers(
    sql: &str,
    time_target: bool,
) -> Result<Vec<(usize, isize, isize)>> {
    use astersql_parser_ast::{ExprKind, ExprNode, Node, SelectStmt, SqlDialect, Visitor};
    #[derive(Default)]
    struct Modifiers {
        casts: Vec<(usize, isize, isize)>,
        time_target: bool,
    }
    impl Visitor for Modifiers {
        fn enter(&mut self, node: &dyn Node) -> bool {
            if let Some(select) = node.as_any().downcast_ref::<SelectStmt>() {
                for field in &select.Fields.Fields {
                    let Some(mut expression) = field.Expr.as_ref() else {
                        continue;
                    };
                    while let ExprKind::Parentheses(inner) = &expression.Kind {
                        expression = inner;
                    }
                    if let ExprKind::Cast { Expr, Tp, .. } = &expression.Kind
                        && (if self.time_target {
                            Tp.GetType() == astersql_parser_mysql::r#type::TypeDuration
                                && Tp.PostgreSQLArrayElement().is_none()
                        } else {
                            (Tp.GetType() == astersql_parser_mysql::r#type::TypePgInterval
                                || Tp.PostgreSQLArrayElement()
                                    == Some(
                                        astersql_session::runtime::PostgreSQLArrayElement::Interval,
                                    ))
                                && (Tp.GetFlen() != -1 || Tp.GetDecimal() != -1)
                        })
                    {
                        let mut source: &ExprNode = Expr;
                        while let ExprKind::Parentheses(inner) = &source.Kind {
                            source = inner;
                        }
                        if let Some(index) = source.PostgreSQLParameterIndex()
                            && matches!(source.Kind, ExprKind::ParamMarker { .. })
                        {
                            self.casts.push((
                                usize::from(index) - 1,
                                Tp.GetFlen(),
                                Tp.GetDecimal(),
                            ));
                        }
                    }
                }
                // Do not eagerly inspect subqueries in conditional projections.
                return true;
            }
            false
        }
        fn leave(&mut self, _: &dyn Node) -> bool {
            true
        }
    }
    let (statements, _) = astersql_parser::Parser::with_dialect(SqlDialect::PostgreSQL)
        .ParseSQL(sql, &[])
        .map_err(|_| error("42601", "invalid interval parameter source"))?;
    let mut visitor = Modifiers {
        time_target,
        ..Default::default()
    };
    for statement in statements {
        statement.accept(&mut visitor);
    }
    Ok(visitor.casts)
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

// Private BinaryParam tags; these are never accepted as PostgreSQL wire OIDs or MySQL types.
pub(crate) const PG_FLOAT4_PARAMETER: u8 = 0xf0;
pub(crate) const PG_FLOAT4_ARRAY_PARAMETER: u8 = 0xf1;
pub(crate) const PG_FLOAT8_PARAMETER: u8 = 0xf2;
pub(crate) const PG_FLOAT8_ARRAY_PARAMETER: u8 = 0xf3;

pub(crate) fn parameter(oid: u32, value: Option<&[u8]>) -> Result<BinaryParam> {
    let Some(value) = value else {
        return Ok(BinaryParam {
            is_null: true,
            tp: if oid == 16 { 1 } else { 6 },
            ..BinaryParam::default()
        });
    };
    if oid == 1021 {
        let array = array_text(value, oid)?;
        return Ok(BinaryParam {
            tp: PG_FLOAT4_ARRAY_PARAMETER,
            value: array
                .to_binary()
                .map_err(|e| error(e.sqlstate().unwrap_or("XX000"), &e.to_string()))?,
            ..BinaryParam::default()
        });
    }
    if oid == 1022 {
        let array = array_text(value, oid)?;
        return Ok(BinaryParam {
            tp: PG_FLOAT8_ARRAY_PARAMETER,
            value: array
                .to_binary()
                .map_err(|e| error(e.sqlstate().unwrap_or("XX000"), &e.to_string()))?,
            ..BinaryParam::default()
        });
    }
    let text =
        std::str::from_utf8(value).map_err(|_| error("22021", "text parameter is not UTF-8"))?;
    if text.contains('\0') {
        return Err(error("22021", "text parameter contains NUL"));
    }
    let invalid = || error("22P02", "invalid text parameter value");
    let (tp, value) = match oid {
        1000 | 1001 | 1005 | 1007 | 1009 | 1014 | 1015 | 1182 | 1183 | 1115 | 1185 | 1187
        | 2951 | 199 | 3807 | 1016 | 1021 | 1022 | 1231 => (
            253,
            array_text(value, oid)?
                .to_text(b',')
                .map_err(|e| error(e.sqlstate().unwrap_or("XX000"), &e.to_string()))?
                .into_bytes(),
        ),
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
            PG_FLOAT4_PARAMETER,
            astersql_expression::pg_cast_float4(Some(text))
                .map_err(|e| error(e.sqlstate(), &e.to_string()))?
                .expect("non-null parameter")
                .to_le_bytes()
                .to_vec(),
        ),
        701 => (
            PG_FLOAT8_PARAMETER,
            astersql_expression::pg_cast_float8(Some(text))
                .map_err(|e| error(e.sqlstate(), &e.to_string()))?
                .expect("non-null parameter")
                .to_le_bytes()
                .to_vec(),
        ),
        25 | 1043 | 1042 => (253, value.to_vec()),
        114 => (
            253,
            astersql_types::pg_json::PgJson::from_bytes(value)
                .map_err(|e| error(e.sqlstate(), &e.to_string()))?
                .as_bytes()
                .to_vec(),
        ),
        3802 => (
            253,
            astersql_types::pg_jsonb::PgJsonb::from_bytes(value)
                .map_err(|e| error(e.sqlstate(), &e.to_string()))?
                .as_bytes()
                .to_vec(),
        ),
        2950 => (
            253,
            text.parse::<astersql_types::pg_uuid::PgUuid>()
                .map_err(|e| error(e.sqlstate(), &e.to_string()))?
                .to_string()
                .into_bytes(),
        ),
        1700 => {
            let numeric = text
                .parse::<astersql_types::pg_numeric::PgNumeric>()
                .map_err(|e| error(e.sqlstate(), &e.to_string()))?;
            // Text transport avoids the MySQL prepared DECIMAL capacity. The
            // prepared context retains OID1700 and inserts the numeric cast.
            (253, numeric.to_string().into_bytes())
        }
        17 => (
            252,
            astersql_types::pg_array::bytea_text_input(value).map_err(|e| error(e.0, e.1))?,
        ),
        1082 => (
            253,
            text.parse::<astersql_types::pg_temporal::PgDate>()
                .map_err(|e| error(e.sqlstate(), &e.to_string()))?
                .to_string()
                .into_bytes(),
        ),
        1083 => (
            253,
            text.parse::<astersql_types::pg_temporal::PgTime>()
                .map_err(|e| error(e.sqlstate(), &e.to_string()))?
                .to_string()
                .into_bytes(),
        ),
        1186 => (
            253,
            text.parse::<astersql_types::pg_temporal::PgInterval>()
                .map_err(|e| error(e.sqlstate(), &e.to_string()))?
                .to_string()
                .into_bytes(),
        ),
        1184 => (
            253,
            text.parse::<astersql_types::pg_temporal::PgTimestampTz>()
                .map_err(|e| error(e.sqlstate(), &e.to_string()))?
                .to_string()
                .into_bytes(),
        ),
        1114 => (
            253,
            text.parse::<astersql_types::pg_temporal::PgTimestamp>()
                .map_err(|e| error(e.sqlstate(), &e.to_string()))?
                .to_string()
                .into_bytes(),
        ),
        _ => return Err(error("0A000", "unsupported text parameter OID")),
    };
    Ok(BinaryParam {
        tp,
        value,
        ..BinaryParam::default()
    })
}
/// Convert PG network order to the shared engine's existing parameter encoding.
pub(crate) fn binary_parameter(oid: u32, value: Option<&[u8]>) -> Result<BinaryParam> {
    let Some(value) = value else {
        return parameter(oid, None);
    };
    let invalid = || error("22P03", "invalid binary parameter");
    let (tp, bytes) = match oid {
        16 => match value {
            [] => {
                return Err(error(
                    "08P01",
                    "insufficient data left in boolean parameter",
                ));
            }
            [byte] => (1, vec![u8::from(*byte != 0)]),
            _ => return Err(invalid()),
        },
        21 => {
            if value.len() < 2 {
                return Err(error("08P01", "insufficient data left in int2 parameter"));
            }
            (
                2,
                i16::from_be_bytes(value.try_into().map_err(|_| invalid())?)
                    .to_le_bytes()
                    .to_vec(),
            )
        }
        23 => (
            3,
            i32::from_be_bytes(value.try_into().map_err(|_| invalid())?)
                .to_le_bytes()
                .to_vec(),
        ),
        20 => {
            if value.len() < 8 {
                return Err(error("08P01", "insufficient data left in int8 parameter"));
            }
            (
                8,
                i64::from_be_bytes(value.try_into().map_err(|_| invalid())?)
                    .to_le_bytes()
                    .to_vec(),
            )
        }
        700 => {
            let bits = astersql_types::pg_array::float4_binary_input(value)
                .map_err(|(state, message)| error(state, message))?;
            (PG_FLOAT4_PARAMETER, bits.to_le_bytes().to_vec())
        }
        701 => {
            let bits = astersql_types::pg_array::float8_binary_input(value)
                .map_err(|(state, message)| error(state, message))?;
            (PG_FLOAT8_PARAMETER, bits.to_le_bytes().to_vec())
        }
        1082 => {
            let date = astersql_types::pg_temporal::PgDate::from_binary(value)
                .map_err(|e| error(e.sqlstate(), &e.to_string()))?;
            return parameter(1082, Some(date.to_string().as_bytes()));
        }
        1083 => {
            let time = astersql_types::pg_temporal::PgTime::from_binary(value)
                .map_err(|e| error(e.sqlstate(), &e.to_string()))?;
            return parameter(1083, Some(time.to_string().as_bytes()));
        }
        1186 => {
            let interval = astersql_types::pg_temporal::PgInterval::from_binary(value)
                .map_err(|e| error(e.sqlstate(), &e.to_string()))?;
            return parameter(1186, Some(interval.to_string().as_bytes()));
        }
        1184 => {
            let timestamp = astersql_types::pg_temporal::PgTimestampTz::from_binary(value)
                .map_err(|e| error(e.sqlstate(), &e.to_string()))?;
            return parameter(1184, Some(timestamp.to_string().as_bytes()));
        }
        1114 => {
            let timestamp = astersql_types::pg_temporal::PgTimestamp::from_binary(value)
                .map_err(|e| error(e.sqlstate(), &e.to_string()))?;
            return parameter(1114, Some(timestamp.to_string().as_bytes()));
        }
        2950 => {
            // uuid_recv reads exactly 16 bytes, then Bind rejects leftovers.
            // Preserve textual byte order; UUIDs are not host-endian integers.
            if value.len() < 16 {
                return Err(error("08P01", "insufficient data left in UUID parameter"));
            }
            let bytes: [u8; 16] = value.try_into().map_err(|_| invalid())?;
            let uuid = astersql_types::pg_uuid::PgUuid::from_bytes(bytes);
            // The prepared statement's OID context supplies UUID identity through
            // the existing text argument bridge, including for typed NULLs.
            return parameter(2950, Some(uuid.to_string().as_bytes()));
        }
        1000 | 1001 | 1005 | 1007 | 1009 | 1014 | 1015 | 1182 | 1183 | 1115 | 1185 | 1187
        | 2951 | 199 | 3807 | 1016 | 1021 | 1022 | 1231 => {
            use astersql_types::pg_array::{PgArray, PgArrayBinaryError, PgArrayElement};
            let array = PgArray::from_binary_with(
                value,
                match oid {
                    1021 => 700,
                    1022 => 701,
                    1231 => 1700,
                    1001 => 17,
                    1005 => 21,
                    1182 => 1082,
                    1183 => 1083,
                    1115 => 1114,
                    1185 => 1184,
                    1187 => 1186,
                    2951 => 2950,
                    199 => 114,
                    3807 => 3802,
                    1016 => 20,
                    1000 => 16,
                    1009 => 25,
                    1015 => 1043,
                    1014 => 1042,
                    _ => 23,
                },
                -1,
                |payload, _| -> Result<PgArrayElement> {
                    if oid == 1021 {
                        let scalar = binary_parameter(700, Some(payload))?;
                        *payload = &[];
                        return Ok(PgArrayElement::Float4(u32::from_le_bytes(
                            scalar.value.try_into().unwrap(),
                        )));
                    }
                    if oid == 1022 {
                        let scalar = binary_parameter(701, Some(payload))?;
                        *payload = &[];
                        return Ok(PgArrayElement::Float8(u64::from_le_bytes(
                            scalar.value.try_into().unwrap(),
                        )));
                    }
                    if oid == 1231 {
                        let scalar = binary_parameter(1700, Some(payload))?;
                        *payload = &[];
                        return Ok(PgArrayElement::Numeric(
                            std::str::from_utf8(&scalar.value).unwrap().parse().unwrap(),
                        ));
                    }
                    if oid == 1001 {
                        let scalar = binary_parameter(17, Some(payload))?;
                        *payload = &[];
                        return Ok(PgArrayElement::Bytea(scalar.value));
                    }
                    if oid == 1183 {
                        let value = astersql_types::pg_temporal::PgTime::from_binary(payload)
                            .map_err(|e| (e.sqlstate(), e.to_string()))?;
                        *payload = &[];
                        return Ok(PgArrayElement::Time(value));
                    }
                    if oid == 1115 {
                        let value = astersql_types::pg_temporal::PgTimestamp::from_binary(payload)
                            .map_err(|e| (e.sqlstate(), e.to_string()))?;
                        *payload = &[];
                        return Ok(PgArrayElement::Timestamp(value));
                    }
                    if oid == 3807 {
                        let scalar = binary_parameter(3802, Some(payload))?;
                        *payload = &[];
                        return Ok(PgArrayElement::Jsonb(
                            astersql_types::pg_jsonb::PgJsonb::from_bytes(&scalar.value)
                                .map_err(|e| (e.sqlstate(), e.to_string()))?,
                        ));
                    }
                    if oid == 199 {
                        let scalar = binary_parameter(114, Some(payload))?;
                        *payload = &[];
                        return Ok(PgArrayElement::Json(
                            astersql_types::pg_json::PgJson::from_bytes(&scalar.value)
                                .map_err(|e| (e.sqlstate(), e.to_string()))?,
                        ));
                    }
                    if oid == 2951 {
                        let scalar = binary_parameter(2950, Some(payload))?;
                        *payload = &[];
                        return Ok(PgArrayElement::Uuid(
                            std::str::from_utf8(&scalar.value).unwrap().parse().unwrap(),
                        ));
                    }
                    if oid == 1187 {
                        let value = astersql_types::pg_temporal::PgInterval::from_binary(payload)
                            .map_err(|e| (e.sqlstate(), e.to_string()))?;
                        *payload = &[];
                        return Ok(PgArrayElement::Interval(value));
                    }
                    if oid == 1185 {
                        let value =
                            astersql_types::pg_temporal::PgTimestampTz::from_binary(payload)
                                .map_err(|e| (e.sqlstate(), e.to_string()))?;
                        *payload = &[];
                        return Ok(PgArrayElement::Timestamptz(value));
                    }
                    if oid == 1182 {
                        let date = astersql_types::pg_temporal::PgDate::from_binary(payload)
                            .map_err(|e| error(e.sqlstate(), &e.to_string()))?;
                        *payload = &[];
                        return Ok(PgArrayElement::Date(date));
                    }
                    if oid == 1016 {
                        let scalar = binary_parameter(20, Some(payload))?;
                        *payload = &[];
                        return Ok(PgArrayElement::Int8(i64::from_le_bytes(
                            scalar.value.try_into().unwrap(),
                        )));
                    }
                    if oid == 1005 {
                        let scalar = binary_parameter(21, Some(payload))?;
                        *payload = &[];
                        return Ok(PgArrayElement::Int2(i16::from_le_bytes(
                            scalar.value.try_into().unwrap(),
                        )));
                    }
                    if oid == 1000 {
                        let scalar = binary_parameter(16, Some(payload))?;
                        *payload = &[];
                        return Ok(PgArrayElement::Bool(scalar.value[0] != 0));
                    }
                    if matches!(oid, 1009 | 1014 | 1015) {
                        let scalar = binary_parameter(
                            match oid {
                                1014 => 1042,
                                1015 => 1043,
                                _ => 25,
                            },
                            Some(payload),
                        )?;
                        *payload = &[];
                        return Ok(PgArrayElement::Text(
                            String::from_utf8(scalar.value).unwrap(),
                        ));
                    }
                    let bytes = payload.get(..4).ok_or_else(|| {
                        error("08P01", "insufficient data left in integer array element")
                    })?;
                    let value = i32::from_be_bytes(bytes.try_into().unwrap());
                    *payload = &payload[4..];
                    Ok(PgArrayElement::Int4(value))
                },
            )
            .map_err(|failure| match &failure {
                PgArrayBinaryError::Element(e) => e.clone(),
                _ => error(failure.sqlstate().unwrap_or("XX000"), &failure.to_string()),
            })?;
            if oid == 1021 {
                return Ok(BinaryParam {
                    tp: PG_FLOAT4_ARRAY_PARAMETER,
                    value: array
                        .to_binary()
                        .map_err(|e| error(e.sqlstate().unwrap_or("XX000"), &e.to_string()))?,
                    ..BinaryParam::default()
                });
            }
            if oid == 1022 {
                return Ok(BinaryParam {
                    tp: PG_FLOAT8_ARRAY_PARAMETER,
                    value: array
                        .to_binary()
                        .map_err(|e| error(e.sqlstate().unwrap_or("XX000"), &e.to_string()))?,
                    ..BinaryParam::default()
                });
            }
            let text = array
                .to_text(b',')
                .map_err(|e| error(e.sqlstate().unwrap_or("XX000"), &e.to_string()))?;
            return Ok(BinaryParam {
                tp: 253,
                value: text.into_bytes(),
                ..BinaryParam::default()
            });
        }
        1700 => {
            let numeric = astersql_types::pg_numeric::PgNumeric::from_binary(value)
                .map_err(|e| error(e.sqlstate(), &e.to_string()))?;
            return parameter(1700, Some(numeric.to_string().as_bytes()));
        }
        17 => (252, value.to_vec()),
        3802 => {
            let Some((&version, document)) = value.split_first() else {
                return Err(error("08P01", "insufficient data left in JSONB parameter"));
            };
            if version != 1 {
                // PostgreSQL jsonb_recv reports unknown versions as internal errors.
                return Err(error(
                    "XX000",
                    &format!("unsupported jsonb version number {version}"),
                ));
            }
            return parameter(oid, Some(document));
        }
        // json_recv consumes the same JSON text as json_in, with no version byte.
        // The prepared OID context supplies JSON identity through the text bridge.
        114 | 25 | 1042 | 1043 => return parameter(oid, Some(value)),
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

struct Statement {
    original_sql: String,
    path: Vec<String>,
    schema_version: Option<i64>,
    catalog: Option<crate::pg_catalog::CatalogQuery>,
    session_query: Option<crate::pg_session::SessionQuery>,
    metadata: PreparedMetadata,
    oids: Vec<u32>,
    interval_modifiers: Vec<(usize, isize, isize)>,
    interval_time_parameters: Vec<(usize, isize, isize)>,
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

    /// SQL DEALLOCATE names the same connection-owned objects as protocol Close.
    pub(crate) fn deallocate_query(
        &mut self,
        sql: &str,
        context: &Arc<dyn TiDBContext>,
    ) -> Option<Result<()>> {
        use crate::pg_catalog_query::{Token, lex};
        let mut tokens = lex(sql).ok()?;
        if tokens.last() == Some(&Token::Symbol(';')) {
            tokens.pop();
        }
        if !matches!(tokens.first(), Some(Token::Word(word)) if word == "deallocate") {
            return None;
        }
        let mut index = 1;
        if matches!(tokens.get(index), Some(Token::Word(word)) if word == "prepare") {
            index += 1;
        }
        if tokens.len() != index + 1 {
            return None;
        }
        let name = match &tokens[index] {
            // ALL is a separate SQL form, outside this named-object bridge.
            Token::Word(name) if name == "all" => return None,
            Token::Word(name) | Token::Quoted(name) => name,
            _ => return None,
        };
        Some((|| {
            self.session
                .check_transaction_command(Some("DEALLOCATE"), context.as_ref())?;
            let statement = self
                .statements
                .remove(name)
                .ok_or_else(|| error("26000", "prepared statement does not exist"))?;
            if statement.catalog.is_none() && statement.session_query.is_none() {
                if self
                    .portals
                    .values()
                    .any(|portal| portal.statement == statement.metadata.statement_id)
                {
                    // Bound portals already pin the analyzed plan, as on reanalysis.
                    self.retired_statements
                        .push(statement.metadata.statement_id);
                } else {
                    context
                        .close_prepared_statement(statement.metadata.statement_id)
                        .map_err(engine)?;
                }
            }
            Ok(())
        })())
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
        self.session.function_error_hint = None;
        self.session.error_detail = None;
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
                if let Some(detail) = self.session.error_detail {
                    crate::pg_conn::write_error_detail(socket, state, &message, detail)?;
                } else if let Some(hint) = self.session.function_error_hint {
                    crate::pg_conn::write_function_error(
                        socket,
                        state,
                        &message,
                        self.session.relation_error_position,
                        Some(hint),
                    )?;
                } else if let Some(sql) = original_sql {
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
            // Preserve SQL syntax diagnostics before checking unused OIDs. For
            // example TIMESTAMP(-1) fails grammar, not the Parse frame shape.
            // Reuse command analysis on the original PG SQL, before adaptation.
            crate::pg_result::prepare_command(&original_sql)?;
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
                        | 114
                        | 700
                        | 701
                        | 1042
                        | 1043
                        | 1082
                        | 1083
                        | 1114
                        | 1184
                        | 1186
                        | 1700
                        | 2950
                        | 3802
                        | 1005
                        | 1001
                        | 1021
                        | 1022
                        | 1231
                        | 1182
                        | 1183
                        | 1115
                        | 1185
                        | 1187
                        | 2951
                        | 199
                        | 3807
                        | 1016
                        | 1000
                        | 1007
                        | 1009
                        | 1014
                        | 1015
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
                    self.session.error_detail = crate::pg_conn::temporal_error_detail(&error);
                    if let crate::conn::ConnError::PgFunction(function) = &error {
                        self.session.function_error_hint = function.hint;
                        self.session.relation_error_position =
                            function.position.and_then(|offset| {
                                analysis_sql
                                    .get(..offset)
                                    .map(|prefix| prefix.chars().count() + 1)
                            });
                    }
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
        let mut constraints = analysis_mapping
            .iter()
            .zip(&analysis_occurrences)
            .zip(&metadata.parameter_cast_oids)
            .collect::<Vec<_>>();
        // PostgreSQL transforms explicit expressions before coercing DML
        // targets. Establish cast source identities first, then require every
        // direct assignment of the same indexed parameter to agree.
        constraints.sort_by_key(|((_, occurrence), _)| {
            parameter_contract
                .get(**occurrence)
                .is_some_and(|(_, _, assignment)| *assignment)
        });
        for ((index, occurrence), oid) in constraints {
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
        let interval_modifiers =
            if catalog.is_none() && oids.iter().any(|oid| matches!(oid, 1186 | 1187)) {
                direct_temporal_parameter_modifiers(&original_sql, false)?
            } else {
                Vec::new()
            };
        let interval_time_parameters = if catalog.is_none() && oids.contains(&1186) {
            direct_temporal_parameter_modifiers(&original_sql, true)?
        } else {
            Vec::new()
        };
        Ok(Statement {
            interval_modifiers,
            interval_time_parameters,
            original_sql,
            path,
            schema_version: context
                .schema_snapshot()
                .map(|schema| schema.SchemaMetaVersion()),
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
        // DDL can invalidate a fixed result even when name resolution is unchanged.
        // Reuse the canonical catalog generation rather than deriving types from rows.
        let schema_version = context
            .schema_snapshot()
            .map(|schema| schema.SchemaMetaVersion());
        if statement.path == path && statement.schema_version == schema_version {
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
                let mut diagnostics = crate::pg_result::time_precision_diagnostics(&sql)?;
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
                diagnostics.push((b'1', vec![]));
                Ok(diagnostics)
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
                let date_context = if statement.catalog.is_none()
                    && statement
                        .oids
                        .iter()
                        .any(|oid| matches!(oid, 1082 | 1182 | 1114 | 1115 | 1185 | 1184))
                {
                    Some(context.pg_date_context(None).map_err(engine)?)
                } else {
                    None
                };
                let mut raw_parameters = Vec::with_capacity(count);
                for index in 0..count {
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
                    raw_parameters.push((format, value));
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
                // Frame errors take precedence over type-specific payload errors.
                for (oid, (format, value)) in statement.oids.iter().zip(raw_parameters) {
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
                            if *oid == 1186 && value.is_some() {
                                let text = std::str::from_utf8(value.unwrap()).map_err(|_| {
                                    error("22021", "invalid UTF-8 interval parameter")
                                })?;
                                if text.contains('\0') {
                                    return Err(error("22021", "NUL in interval parameter"));
                                }
                                let v = astersql_types::pg_temporal::PgInterval::parse_with_style(
                                    text,
                                    context.pg_interval_style(None).map_err(engine)?,
                                )
                                .map_err(|e| error(e.sqlstate(), &e.to_string()))?;
                                parameter(*oid, Some(v.to_string().as_bytes()))?
                            } else if *oid == 1184 && value.is_some() {
                                let text = std::str::from_utf8(value.unwrap()).map_err(|_| {
                                    error("22021", "invalid UTF-8 temporal parameter")
                                })?;
                                if text.contains('\0') {
                                    return Err(error("22021", "NUL in temporal parameter"));
                                }
                                let canonical =
                                    context.pg_timestamptz_input(text).map_err(engine)?;
                                parameter(*oid, Some(canonical.as_bytes()))?
                            } else if *oid == 1187 && value.is_some() {
                                use astersql_types::pg_array::{
                                    PgArray, PgArrayElement, PgArrayTextError,
                                };
                                let style = context.pg_interval_style(None).map_err(engine)?;
                                let array = PgArray::from_text_with(
                                    value.unwrap(),
                                    1186,
                                    -1,
                                    b',',
                                    |text, _| {
                                        astersql_types::pg_temporal::PgInterval::parse_with_style(
                                            text, style,
                                        )
                                        .map(PgArrayElement::Interval)
                                        .map_err(|e| error(e.sqlstate(), &e.to_string()))
                                    },
                                )
                                .map_err(|e| match e {
                                    PgArrayTextError::Element(e) => e,
                                    e => error(e.sqlstate().unwrap_or("XX000"), &e.to_string()),
                                })?;
                                let canonical = array.to_text(b',').map_err(|e| {
                                    error(e.sqlstate().unwrap_or("XX000"), &e.to_string())
                                })?;
                                parameter(*oid, Some(canonical.as_bytes()))?
                            } else if *oid == 1185 && value.is_some() {
                                use astersql_types::pg_array::{
                                    PgArray, PgArrayElement, PgArrayTextError,
                                };
                                let array = PgArray::from_text_with(
                                    value.unwrap(),
                                    1184,
                                    -1,
                                    b',',
                                    |text, _| {
                                        let canonical =
                                            context.pg_timestamptz_input(text).map_err(engine)?;
                                        canonical
                                            .parse::<astersql_types::pg_temporal::PgTimestampTz>()
                                            .map(PgArrayElement::Timestamptz)
                                            .map_err(|e| error(e.sqlstate(), &e.to_string()))
                                    },
                                )
                                .map_err(|e| match e {
                                    PgArrayTextError::Element(e) => e,
                                    e => error(e.sqlstate().unwrap_or("XX000"), &e.to_string()),
                                })?;
                                let canonical = array.to_text(b',').map_err(|e| {
                                    error(e.sqlstate().unwrap_or("XX000"), &e.to_string())
                                })?;
                                parameter(*oid, Some(canonical.as_bytes()))?
                            } else if matches!(*oid, 1182 | 1115 | 1185) && value.is_some() {
                                let canonical =
                                    array_text_with_context(value.unwrap(), *oid, date_context)?
                                        .to_text(b',')
                                        .map_err(|e| {
                                            error(e.sqlstate().unwrap_or("XX000"), &e.to_string())
                                        })?;
                                parameter(*oid, Some(canonical.as_bytes()))?
                            } else if matches!(*oid, 1082 | 1114) && value.is_some() {
                                let text = std::str::from_utf8(value.unwrap()).map_err(|_| {
                                    error("22021", "invalid UTF-8 temporal parameter")
                                })?;
                                if text.contains('\0') {
                                    return Err(error("22021", "NUL in temporal parameter"));
                                }
                                let date = if *oid == 1082 {
                                    astersql_types::pg_temporal::PgDate::parse_with_context(
                                        text,
                                        date_context.unwrap(),
                                    )
                                    .map(|v| v.to_string())
                                } else {
                                    astersql_types::pg_temporal::PgTimestamp::parse_with_context(
                                        text,
                                        date_context.unwrap(),
                                    )
                                    .map(|v| v.to_string())
                                }
                                .map_err(|e| error(e.sqlstate(), &e.to_string()))?;
                                parameter(*oid, Some(date.as_bytes()))?
                            } else {
                                parameter(*oid, value)?
                            }
                        });
                    }
                }
                for &(index, fields, precision) in &statement.interval_modifiers {
                    if statement.oids.get(index) == Some(&1187)
                        && let Some(argument) = args.get(index).filter(|arg| !arg.is_null)
                    {
                        array_text(&argument.value, 1187)?
                            .elements()
                            .iter()
                            .flatten()
                            .try_for_each(|v| {
                                let astersql_types::pg_array::PgArrayElement::Interval(v) = v
                                else {
                                    unreachable!()
                                };
                                v.apply_typmod(fields, precision)
                                    .map(|_| ())
                                    .map_err(|e| error(e.sqlstate(), &e.to_string()))
                            })?;
                    }
                    if statement.oids.get(index) == Some(&1186)
                        && let Some(argument) = args.get(index).filter(|arg| !arg.is_null)
                    {
                        // The parameter codec already validated text/binary and
                        // froze IntervalStyle. Validate its cast before BindComplete;
                        // Execute still consumes the original value and the same cast.
                        std::str::from_utf8(&argument.value)
                            .map_err(|_| error("22021", "invalid interval parameter"))?
                            .parse::<astersql_types::pg_temporal::PgInterval>()
                            .and_then(|value| value.apply_typmod(fields, precision))
                            .map_err(|e| error(e.sqlstate(), &e.to_string()))?;
                    }
                }
                for &(index, _, _) in &statement.interval_time_parameters {
                    if statement.oids.get(index) == Some(&1186)
                        && let Some(argument) = args.get(index).filter(|arg| !arg.is_null)
                    {
                        // Only direct interval parameters cast to time are inspected.
                        // Keep the original parameter for Execute and skip lazy branches.
                        let interval = std::str::from_utf8(&argument.value)
                            .map_err(|_| error("22021", "invalid interval parameter"))?
                            .parse::<astersql_types::pg_temporal::PgInterval>()
                            .map_err(|e| error(e.sqlstate(), &e.to_string()))?;
                        if matches!(
                            interval.components(),
                            (i32::MAX, i32::MAX, i64::MAX) | (i32::MIN, i32::MIN, i64::MIN)
                        ) {
                            return Err(error("22008", "cannot convert infinite interval to time"));
                        }
                    }
                }
                let mut execution_metadata = statement.metadata.clone();
                if !execution_metadata.execution_native_types.is_empty() {
                    execution_metadata.native_types =
                        execution_metadata.execution_native_types.clone();
                }
                let description = description_formats(
                    &execution_metadata,
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
                        native_types: execution_metadata.native_types.clone(),
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
                            self.session.error_detail = crate::pg_conn::temporal_error_detail(&e);
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
                    let encoded = crate::pg_result::encode_formats_interval(
                        &result,
                        command,
                        &portal.formats,
                        crate::pg_result::interval_style(&result, context.as_ref())
                            .map_err(engine)?,
                    )
                    .map_err(|e| error(crate::pg_result::encoding_sqlstate(&e), &e.to_string()));
                    if let Some(lifecycle) = &result.response_lifecycle {
                        lifecycle.finish();
                    }
                    context.finish_protocol_response(std::time::Duration::ZERO);
                    let mut messages = encoded?;
                    if let Some(query) = &portal.session_query {
                        let mut notices = query.notices(context.as_ref());
                        notices.append(&mut messages);
                        messages = notices;
                    }
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

/// Canonical int4 codec shared by ordinary Bind and result encoders.
/// Text is the existing engine transport; it includes all dimensions and bounds.
pub(crate) fn int4_array_text(bytes: &[u8]) -> Result<astersql_types::pg_array::PgArray> {
    array_text(bytes, 1007)
}

pub(crate) fn array_text(bytes: &[u8], oid: u32) -> Result<astersql_types::pg_array::PgArray> {
    array_text_with_context(bytes, oid, None)
}
fn array_text_with_context(
    bytes: &[u8],
    oid: u32,
    context: Option<astersql_types::pg_temporal::PgDateContext>,
) -> Result<astersql_types::pg_array::PgArray> {
    use astersql_types::pg_array::{PgArray, PgArrayElement, PgArrayTextError};
    PgArray::from_text_with(
        bytes,
        match oid {
            1021 => 700,
            1022 => 701,
            1231 => 1700,
            1001 => 17,
            1005 => 21,
            1182 => 1082,
            1183 => 1083,
            1115 => 1114,
            1185 => 1184,
            1187 => 1186,
            2951 => 2950,
            199 => 114,
            3807 => 3802,
            1016 => 20,
            1000 => 16,
            1009 => 25,
            1015 => 1043,
            1014 => 1042,
            _ => 23,
        },
        -1,
        b',',
        |text, _| {
            if oid == 1021 {
                let scalar = parameter(700, Some(text.as_bytes()))?;
                Ok(PgArrayElement::Float4(u32::from_le_bytes(
                    scalar.value.try_into().unwrap(),
                )))
            } else if oid == 1022 {
                let scalar = parameter(701, Some(text.as_bytes()))?;
                Ok(PgArrayElement::Float8(u64::from_le_bytes(
                    scalar.value.try_into().unwrap(),
                )))
            } else if oid == 1231 {
                let scalar = parameter(1700, Some(text.as_bytes()))?;
                Ok(PgArrayElement::Numeric(
                    std::str::from_utf8(&scalar.value).unwrap().parse().unwrap(),
                ))
            } else if oid == 1001 {
                Ok(PgArrayElement::Bytea(
                    parameter(17, Some(text.as_bytes()))?.value,
                ))
            } else if oid == 3807 {
                let scalar = parameter(3802, Some(text.as_bytes()))?;
                astersql_types::pg_jsonb::PgJsonb::from_bytes(&scalar.value)
                    .map(PgArrayElement::Jsonb)
                    .map_err(|e| (e.sqlstate(), e.to_string()))
            } else if oid == 199 {
                let scalar = parameter(114, Some(text.as_bytes()))?;
                astersql_types::pg_json::PgJson::from_bytes(&scalar.value)
                    .map(PgArrayElement::Json)
                    .map_err(|e| (e.sqlstate(), e.to_string()))
            } else if oid == 2951 {
                text.parse::<astersql_types::pg_uuid::PgUuid>()
                    .map(PgArrayElement::Uuid)
                    .map_err(|e| (e.sqlstate(), e.to_string()))
            } else if oid == 1187 {
                text.parse::<astersql_types::pg_temporal::PgInterval>()
                    .map(PgArrayElement::Interval)
                    .map_err(|e| (e.sqlstate(), e.to_string()))
            } else if oid == 1185 {
                text.parse::<astersql_types::pg_temporal::PgTimestampTz>()
                    .map(PgArrayElement::Timestamptz)
                    .map_err(|e| (e.sqlstate(), e.to_string()))
            } else if oid == 1115 {
                context
                    .map_or_else(
                        || text.parse::<astersql_types::pg_temporal::PgTimestamp>(),
                        |context| {
                            astersql_types::pg_temporal::PgTimestamp::parse_with_context(
                                text, context,
                            )
                        },
                    )
                    .map(PgArrayElement::Timestamp)
                    .map_err(|e| (e.sqlstate(), e.to_string()))
            } else if oid == 1183 {
                text.parse::<astersql_types::pg_temporal::PgTime>()
                    .map(PgArrayElement::Time)
                    .map_err(|e| (e.sqlstate(), e.to_string()))
            } else if oid == 1182 {
                context
                    .map_or_else(
                        || text.parse::<astersql_types::pg_temporal::PgDate>(),
                        |context| {
                            astersql_types::pg_temporal::PgDate::parse_with_context(text, context)
                        },
                    )
                    .map(PgArrayElement::Date)
                    .map_err(|e| error(e.sqlstate(), &e.to_string()))
            } else if oid == 1016 {
                let scalar = parameter(20, Some(text.as_bytes()))?;
                Ok(PgArrayElement::Int8(i64::from_le_bytes(
                    scalar.value.try_into().unwrap(),
                )))
            } else if oid == 1005 {
                let scalar = parameter(21, Some(text.as_bytes()))?;
                Ok(PgArrayElement::Int2(i16::from_le_bytes(
                    scalar.value.try_into().unwrap(),
                )))
            } else if oid == 1000 {
                let scalar = parameter(16, Some(text.as_bytes()))?;
                Ok(PgArrayElement::Bool(scalar.value[0] != 0))
            } else if matches!(oid, 1009 | 1014 | 1015) {
                let scalar = parameter(
                    match oid {
                        1014 => 1042,
                        1015 => 1043,
                        _ => 25,
                    },
                    Some(text.as_bytes()),
                )?;
                Ok(PgArrayElement::Text(
                    String::from_utf8(scalar.value).unwrap(),
                ))
            } else {
                astersql_expression::pg_cast_int4(Some(text))
                    .map(|v| PgArrayElement::Int4(v.unwrap()))
                    .map_err(|e| error(e.sqlstate(), &e.to_string()))
            }
        },
    )
    .map_err(|failure| match &failure {
        PgArrayTextError::Element(e) => e.clone(),
        _ => error(failure.sqlstate().unwrap_or("XX000"), &failure.to_string()),
    })
}

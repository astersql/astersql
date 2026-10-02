// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::*;
thread_local! {
    static BUILD_DIVISION_PRECISION: std::cell::Cell<Option<i32>> = const { std::cell::Cell::new(None) };
}
/// Carry the definition's decimal context through the synchronous relational
/// executor. Each pooled session runs on its own thread; Drop restores nested
/// contexts even when SQL evaluation exits with an error.
pub(super) struct BuildDivisionPrecision(Option<i32>);
impl BuildDivisionPrecision {
    pub(super) fn enter(precision: i32) -> SessionResult<Self> {
        if !(0..=30).contains(&precision) {
            return Err(SessionError::new("invalid definition division precision"));
        }
        Ok(Self(
            BUILD_DIVISION_PRECISION.with(|slot| slot.replace(Some(precision))),
        ))
    }
}
impl Drop for BuildDivisionPrecision {
    fn drop(&mut self) {
        BUILD_DIVISION_PRECISION.with(|slot| slot.set(self.0));
    }
}

#[cfg(test)]
#[path = "relational_value_test.rs"]
mod tests;

/// MySQL three-valued truth conversion used by relational predicates.
pub(super) fn relational_truth(value: Option<&str>) -> Option<bool> {
    value.map(|value| {
        if value.eq_ignore_ascii_case("true") {
            return true;
        }
        if value.eq_ignore_ascii_case("false") {
            return false;
        }
        if value.trim_start().starts_with('[') {
            if let Ok(vector) = astersql_types::vector::ParseVectorFloat32(value) {
                return vector.Len() != 0;
            }
        }
        value.parse::<f64>().unwrap_or(0.0) != 0.0
    })
}

pub(super) fn relational_row_items(expression: &ast::ExprNode) -> Option<&[ast::ExprNode]> {
    match &expression.Kind {
        ast::ExprKind::Row(items) => Some(items),
        ast::ExprKind::Parentheses(inner) | ast::ExprKind::Collate { Expr: inner, .. } => {
            relational_row_items(inner)
        }
        _ => None,
    }
}

pub(crate) fn relational_compare(left: &str, right: &str) -> std::cmp::Ordering {
    // Vector parsing builds an error stack for ordinary scalar text.  Only a
    // bracketed literal can be a vector, so avoid that costly failed parse for
    // every scalar predicate comparison.
    if left.trim_start().starts_with('[') && right.trim_start().starts_with('[') {
        if let (Ok(left), Ok(right)) = (
            astersql_types::vector::ParseVectorFloat32(left),
            astersql_types::vector::ParseVectorFloat32(right),
        ) {
            return left.Compare(&right).cmp(&0);
        }
    }
    match (
        relational_integer_value(left),
        relational_integer_value(right),
    ) {
        (Some(left), Some(right)) => left.cmp(&right),
        _ => match (left.parse::<f64>(), right.parse::<f64>()) {
            (Ok(left), Ok(right)) => left
                .partial_cmp(&right)
                .unwrap_or(std::cmp::Ordering::Equal),
            (Ok(left), Err(_)) => left
                .partial_cmp(&relational_numeric_prefix(right).unwrap_or(0.0))
                .unwrap_or(std::cmp::Ordering::Equal),
            (Err(_), Ok(right)) => relational_numeric_prefix(left)
                .unwrap_or(0.0)
                .partial_cmp(&right)
                .unwrap_or(std::cmp::Ordering::Equal),
            _ => left.cmp(right),
        },
    }
}

/// MySQL compares a string operand with a numeric operand in the numeric
/// context selected by the expression types. For non-decimal mixed operands,
/// TiDB uses DOUBLE, so integers beyond 2^53 intentionally lose precision.
pub(super) fn relational_expression_compare(
    left_expression: &ast::ExprNode,
    right_expression: &ast::ExprNode,
    left: &str,
    right: &str,
    row: &HashMap<String, Option<String>>,
) -> std::cmp::Ordering {
    // A VECTOR compared with its textual literal uses VECTOR semantics after
    // implicit conversion, rather than the generic string/numeric coercion.
    // This also preserves TiDB's dimension-first ordering.
    if left.trim_start().starts_with('[')
        && right.trim_start().starts_with('[')
        && let (Ok(left), Ok(right)) = (
            astersql_types::vector::ParseVectorFloat32(left),
            astersql_types::vector::ParseVectorFloat32(right),
        )
    {
        return left.Compare(&right).cmp(&0);
    }
    let left_is_string = relational_expression_is_string(left_expression, row);
    let right_is_string = relational_expression_is_string(right_expression, row);
    if left_is_string != right_is_string {
        return relational_numeric_prefix(left)
            .unwrap_or(0.0)
            .partial_cmp(&relational_numeric_prefix(right).unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal);
    }
    relational_compare(left, right)
}

fn relational_expression_column<'a>(
    expression: &ast::ExprNode,
    table: &'a astersql_meta_model::TableInfo,
) -> Option<&'a astersql_meta_model::ColumnInfo> {
    let ast::ExprKind::Column(name) = &expression.Kind else {
        return None;
    };
    let qualified = (!name.Table.L.is_empty()).then(|| format!("{}.{}", name.Table.L, name.Name.L));
    table
        .Columns
        .iter()
        .find(|column| {
            qualified
                .as_ref()
                .is_some_and(|qualified| column.Name.L == *qualified)
        })
        .or_else(|| {
            table
                .Columns
                .iter()
                .find(|column| column.Name.L == name.Name.L)
        })
}

fn relational_collated_string_column(column: &astersql_meta_model::ColumnInfo) -> bool {
    astersql_types::field::IsString(column.GetType())
        || matches!(
            column.GetType(),
            astersql_parser_mysql::r#type::TypeEnum | astersql_parser_mysql::r#type::TypeSet
        )
}

/// Evaluate predicates with the table column's collation.  The generic row
/// evaluator has no schema argument and therefore deliberately falls back to
/// binary comparison; table scans must retain MySQL's non-binary string
/// semantics for comparisons, IN and LIKE.
pub(super) fn relational_table_expression_value(
    expression: &ast::ExprNode,
    row: &HashMap<String, Option<String>>,
    table: &astersql_meta_model::TableInfo,
) -> SessionResult<Option<String>> {
    match &expression.Kind {
        ast::ExprKind::Parentheses(inner) => relational_table_expression_value(inner, row, table),
        ast::ExprKind::Binary { Op, L, R } if Op.eq_ignore_ascii_case("and") => {
            let left =
                relational_truth(relational_table_expression_value(L, row, table)?.as_deref());
            let right =
                relational_truth(relational_table_expression_value(R, row, table)?.as_deref());
            Ok(relational_boolean_value(match (left, right) {
                (Some(false), _) | (_, Some(false)) => Some(false),
                (Some(true), Some(true)) => Some(true),
                _ => None,
            }))
        }
        ast::ExprKind::Binary { Op, L, R } if Op.eq_ignore_ascii_case("or") => {
            let left =
                relational_truth(relational_table_expression_value(L, row, table)?.as_deref());
            let right =
                relational_truth(relational_table_expression_value(R, row, table)?.as_deref());
            Ok(relational_boolean_value(match (left, right) {
                (Some(true), _) | (_, Some(true)) => Some(true),
                (Some(false), Some(false)) => Some(false),
                _ => None,
            }))
        }
        ast::ExprKind::Binary { Op, L, R }
            if matches!(
                Op.as_str(),
                "=" | "==" | "!=" | "<>" | ">" | ">=" | "<" | "<="
            ) =>
        {
            let left_column = relational_expression_column(L, table);
            let right_column = relational_expression_column(R, table);
            let Some(column) = left_column.or(right_column) else {
                return relational_expression_value(expression, row);
            };
            let left_is_string = left_column.is_some_and(relational_collated_string_column)
                || left_column.is_none() && relational_expression_is_string(L, row);
            let right_is_string = right_column.is_some_and(relational_collated_string_column)
                || right_column.is_none() && relational_expression_is_string(R, row);
            let has_temporal_column = left_column.into_iter().chain(right_column).any(|column| {
                matches!(
                    column.GetType(),
                    astersql_parser_mysql::r#type::TypeDate
                        | astersql_parser_mysql::r#type::TypeDatetime
                        | astersql_parser_mysql::r#type::TypeTimestamp
                )
            });
            let temporal_mixed_with_numeric = left_column.is_some_and(|column| {
                matches!(
                    column.GetType(),
                    astersql_parser_mysql::r#type::TypeDate
                        | astersql_parser_mysql::r#type::TypeDatetime
                        | astersql_parser_mysql::r#type::TypeTimestamp
                )
            }) && right_column.is_none()
                && !right_is_string
                || right_column.is_some_and(|column| {
                    matches!(
                        column.GetType(),
                        astersql_parser_mysql::r#type::TypeDate
                            | astersql_parser_mysql::r#type::TypeDatetime
                            | astersql_parser_mysql::r#type::TypeTimestamp
                    )
                }) && left_column.is_none()
                    && !left_is_string;
            if left_is_string != right_is_string && !has_temporal_column
                || temporal_mixed_with_numeric
            {
                let left = relational_table_expression_value(L, row, table)?;
                let right = relational_table_expression_value(R, row, table)?;
                let compared = left.as_deref().zip(right.as_deref()).map(|(left, right)| {
                    let ordering = relational_expression_compare(L, R, left, right, row);
                    match Op.as_str() {
                        "=" | "==" => ordering.is_eq(),
                        "!=" | "<>" => !ordering.is_eq(),
                        ">" => ordering.is_gt(),
                        ">=" => ordering.is_ge(),
                        "<" => ordering.is_lt(),
                        "<=" => ordering.is_le(),
                        _ => unreachable!("comparison operator checked above"),
                    }
                });
                return Ok(relational_boolean_value(compared));
            }
            let string_column = relational_collated_string_column(column);
            let temporal_column = matches!(
                column.GetType(),
                astersql_parser_mysql::r#type::TypeDate
                    | astersql_parser_mysql::r#type::TypeDatetime
                    | astersql_parser_mysql::r#type::TypeTimestamp
            );
            if !string_column && !temporal_column {
                return relational_expression_value(expression, row);
            }
            let left = relational_expression_value(L, row)?;
            let right = relational_expression_value(R, row)?;
            let compared = left
                .as_deref()
                .zip(right.as_deref())
                .map(|(left, right)| -> SessionResult<bool> {
                    let left = if is_binary_string_column(column) {
                        display_runtime_value(left.to_owned())
                    } else {
                        left.to_owned()
                    };
                    let right = if is_binary_string_column(column) {
                        display_runtime_value(right.to_owned())
                    } else {
                        right.to_owned()
                    };
                    let ordering = if string_column {
                        astersql_util_collate::GetCollator(column.GetCollate())
                            .Compare(&left, &right)
                            .cmp(&0)
                    } else {
                        let converted_left = runtime_value_to_datum(
                            Some(&left),
                            column,
                            astersql_types::DefaultStmtFlags,
                        )
                        .and_then(|value| {
                            value.ToString().map_err(|error| {
                                session_error("format temporal left operand", error)
                            })
                        });
                        let converted_right = runtime_value_to_datum(
                            Some(&right),
                            column,
                            astersql_types::DefaultStmtFlags,
                        )
                        .and_then(|value| {
                            value.ToString().map_err(|error| {
                                session_error("format temporal right operand", error)
                            })
                        });
                        match (converted_left, converted_right) {
                            (Ok(left), Ok(right)) => relational_compare(&left, &right),
                            _ => relational_expression_compare(L, R, &left, &right, row),
                        }
                    };
                    match Op.as_str() {
                        "=" | "==" => Ok(ordering.is_eq()),
                        "!=" | "<>" => Ok(!ordering.is_eq()),
                        ">" => Ok(ordering.is_gt()),
                        ">=" => Ok(ordering.is_ge()),
                        "<" => Ok(ordering.is_lt()),
                        "<=" => Ok(ordering.is_le()),
                        _ => unreachable!("comparison operator checked above"),
                    }
                })
                .transpose()?;
            Ok(relational_boolean_value(compared))
        }
        ast::ExprKind::InList {
            Expr, List, Not, ..
        } if relational_expression_column(Expr, table)
            .is_some_and(relational_collated_string_column) =>
        {
            let column = relational_expression_column(Expr, table).expect("string column checked");
            let Some(value) = relational_expression_value(Expr, row)? else {
                return Ok(None);
            };
            let collator = astersql_util_collate::GetCollator(column.GetCollate());
            let mut saw_null = false;
            for candidate in List {
                let Some(candidate) = relational_expression_value(candidate, row)? else {
                    saw_null = true;
                    continue;
                };
                if collator.Compare(&value, &candidate) == 0 {
                    return Ok(relational_boolean_value(Some(!*Not)));
                }
            }
            Ok(if saw_null {
                None
            } else {
                relational_boolean_value(Some(*Not))
            })
        }
        ast::ExprKind::Like {
            Expr,
            Pattern,
            Not,
            Escape,
            ..
        } if relational_expression_column(Expr, table)
            .is_some_and(relational_collated_string_column) =>
        {
            let column = relational_expression_column(Expr, table).expect("string column checked");
            let value = relational_expression_value(Expr, row)?;
            let pattern = relational_expression_value(Pattern, row)?;
            let Some((value, pattern)) = value.as_deref().zip(pattern.as_deref()) else {
                return Ok(None);
            };
            let mut wildcard = astersql_util_collate::GetCollator(column.GetCollate()).Pattern();
            wildcard.Compile(pattern, Escape.as_bytes().first().copied().unwrap_or(b'\\'));
            let matched = wildcard.DoMatch(value);
            Ok(relational_boolean_value(Some(if *Not {
                !matched
            } else {
                matched
            })))
        }
        _ => relational_expression_value(expression, row),
    }
}

/// Parse the numeric spellings retained by the relational row codec.
///
/// In particular, `BIT` values are represented as `0x…` so their bytes can
/// round-trip losslessly. MySQL comparisons still treat that representation as
/// an unsigned integer, including `BIT(64)` values above `i64::MAX`.
fn relational_integer_value(value: &str) -> Option<i128> {
    let value = value.trim();
    if let Some(hex) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        return u128::from_str_radix(hex, 16)
            .ok()
            .and_then(|value| i128::try_from(value).ok());
    }
    // BIT columns are decoded as their raw big-endian bytes (for example
    // BIT(3) value 6 is "\x06"), matching the MySQL protocol result.  Treat
    // that representation numerically when it participates in predicates.
    if !value.is_empty()
        && value.chars().any(char::is_control)
        && value.chars().all(|character| u32::from(character) <= 0xff)
    {
        return value.chars().try_fold(0_i128, |result, character| {
            result
                .checked_mul(256)?
                .checked_add(i128::from(u32::from(character)))
        });
    }
    value.parse::<i128>().ok()
}

pub(super) fn relational_numeric_prefix(value: &str) -> Option<f64> {
    let value = value.trim_start();
    let mut end = 0;
    let mut saw_digit = false;
    let mut saw_dot = false;
    for (index, character) in value.char_indices() {
        let accepted = if index == 0 && matches!(character, '+' | '-') {
            true
        } else if character.is_ascii_digit() {
            saw_digit = true;
            true
        } else if character == '.' && !saw_dot {
            saw_dot = true;
            true
        } else {
            false
        };
        if !accepted {
            break;
        }
        end = index + character.len_utf8();
    }
    saw_digit
        .then(|| value[..end].parse::<f64>().ok())
        .flatten()
}

pub(super) fn parse_runtime_datetime(value: &str) -> Option<chrono::NaiveDateTime> {
    let value = value.trim_matches(['\'', '"']);
    for format in [
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%d %H:%M",
        "%Y-%m-%d",
        "%Y%m%d%H%M%S%.f",
        "%Y%m%d",
    ] {
        if let Ok(datetime) = chrono::NaiveDateTime::parse_from_str(value, format) {
            return Some(datetime);
        }
        if let Ok(date) = chrono::NaiveDate::parse_from_str(value, format) {
            return date.and_hms_opt(0, 0, 0);
        }
    }
    None
}

pub(super) fn runtime_time_components(value: &str) -> Option<(i64, i64, i64, u32)> {
    let value = value.trim_matches(['\'', '"']);
    let time = value.rsplit_once(' ').map_or(value, |(_, time)| time);
    let negative = time.starts_with('-');
    let time = time.trim_start_matches(['+', '-']);
    let mut parts = time.split(':');
    let hours = parts.next()?.parse::<i64>().ok()?;
    let minutes = parts.next()?.parse::<i64>().ok()?;
    let seconds = parts.next()?;
    let (seconds, micros) = seconds.split_once('.').map_or((seconds, ""), |parts| parts);
    let seconds = seconds.parse::<i64>().ok()?;
    if minutes > 59 || seconds > 59 {
        return None;
    }
    let micros = format!("{micros:0<6}")
        .chars()
        .take(6)
        .collect::<String>()
        .parse::<u32>()
        .unwrap_or_default();
    Some((
        if negative { -hours } else { hours },
        minutes,
        seconds,
        micros,
    ))
}

pub(super) fn format_mysql_json(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(format_mysql_json)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        serde_json::Value::Object(values) => format!(
            "{{{}}}",
            values
                .iter()
                .map(|(key, value)| format!(
                    "{}: {}",
                    serde_json::to_string(key).expect("serialize JSON object key"),
                    format_mysql_json(value)
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        value => serde_json::to_string(value).expect("serialize JSON scalar"),
    }
}

pub(super) fn relational_boolean_value(value: Option<bool>) -> Option<String> {
    value.map(|value| if value { "1" } else { "0" }.to_owned())
}

pub(super) fn relational_like(pattern: &str, value: &str) -> bool {
    relational_like_with_escape(pattern, value, '\\')
}

fn relational_like_with_escape(pattern: &str, value: &str, escape: char) -> bool {
    enum Token {
        AnySequence,
        AnyCharacter,
        Literal(char),
    }

    // TiDB's wildcard matcher consumes one character for `_`, not one UTF-8
    // byte. Keep the lightweight case-insensitive fallback, but run its state
    // machine over Unicode scalar values so multibyte text has SQL LIKE
    // semantics too.
    let pattern = pattern.to_lowercase();
    let mut pattern = pattern.chars().peekable();
    let mut tokens = Vec::new();
    while let Some(character) = pattern.next() {
        if character == escape {
            tokens.push(Token::Literal(pattern.next().unwrap_or(character)));
        } else {
            tokens.push(match character {
                '%' => Token::AnySequence,
                '_' => Token::AnyCharacter,
                literal => Token::Literal(literal),
            });
        }
    }
    let value = value.to_lowercase().chars().collect::<Vec<_>>();
    let mut current = vec![false; value.len() + 1];
    current[0] = true;
    for token in tokens {
        let mut next = vec![false; value.len() + 1];
        match token {
            Token::AnySequence => {
                next[0] = current[0];
                for index in 1..=value.len() {
                    next[index] = current[index] || next[index - 1];
                }
            }
            Token::AnyCharacter => {
                for index in 1..=value.len() {
                    next[index] = current[index - 1];
                }
            }
            Token::Literal(literal) => {
                for index in 1..=value.len() {
                    next[index] = current[index - 1] && value[index - 1] == literal;
                }
            }
        }
        current = next;
    }
    current[value.len()]
}

pub(super) fn relational_numeric_value(
    expression: &ast::ExprNode,
    row: &HashMap<String, Option<String>>,
) -> SessionResult<Option<i128>> {
    relational_expression_value(expression, row)?
        .map(|value| match value.parse::<i128>() {
            Ok(value) => Ok(value),
            Err(_) if !value.chars().any(|character| character.is_ascii_digit()) => Ok(0),
            Err(error) => Err(session_error("parse relational numeric expression", error)),
        })
        .transpose()
}

pub(super) fn relational_float_string(value: f64) -> Option<String> {
    if value.is_nan() {
        None
    } else if value == f64::INFINITY {
        Some("+Inf".to_owned())
    } else if value == f64::NEG_INFINITY {
        Some("-Inf".to_owned())
    } else {
        Some(value.to_string())
    }
}

pub(super) fn relational_on_duplicate_value(
    expression: &ast::ExprNode,
    current: &HashMap<String, Option<String>>,
    incoming: &HashMap<String, Option<String>>,
) -> SessionResult<Option<String>> {
    // An INSERT ... SELECT assignment can refer to projected source columns
    // directly (for example `c2 = a2`). Keep target-column resolution first,
    // then expose source-only names to the ordinary expression evaluator.
    let mut evaluation_row = current.clone();
    for (column, value) in incoming {
        evaluation_row
            .entry(column.clone())
            .or_insert_with(|| value.clone());
    }
    if let ast::ExprKind::Binary { Op, L, R } = &expression.Kind
        && matches!(Op.as_str(), "+" | "-" | "*")
    {
        let left = crate::dml_runtime::EvalExpr(L, &evaluation_row, Some(incoming))?;
        let right = crate::dml_runtime::EvalExpr(R, &evaluation_row, Some(incoming))?;
        if let (Some(left), Some(right)) = (left.as_deref(), right.as_deref())
            && let (Ok(left), Ok(right)) = (
                astersql_types::vector::ParseVectorFloat32(left),
                astersql_types::vector::ParseVectorFloat32(right),
            )
        {
            let result = match Op.as_str() {
                "+" => left.Add(&right),
                "-" => left.Sub(&right),
                "*" => left.Mul(&right),
                _ => unreachable!("vector arithmetic operator checked above"),
            }
            .map_err(|error| session_error("compute ON DUPLICATE vector arithmetic", error))?;
            return Ok(Some(result.String()));
        }
    }
    if matches!(
        &expression.Kind,
        ast::ExprKind::Function { FnName, .. } if FnName.L != "values"
    ) {
        return relational_expression_value(expression, &evaluation_row);
    }
    crate::dml_runtime::EvalExpr(expression, &evaluation_row, Some(incoming))
}

pub(super) fn explain_vector_number(value: f32) -> String {
    let value = value as f64;
    if value != 0.0 && value.abs() >= 100.0 {
        let exponent = value.abs().log10().floor() as i32;
        let mantissa = value / 10_f64.powi(exponent);
        let mut mantissa = format!("{mantissa:.1}");
        if mantissa.ends_with(".0") {
            mantissa.truncate(mantissa.len() - 2);
        }
        return format!(
            "{mantissa}e{}{exponent:02}",
            if exponent >= 0 { "+" } else { "-" },
            exponent = exponent.abs()
        );
    }
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        let mut value = format!("{value:.1}");
        while value.ends_with('0') {
            value.pop();
        }
        value.trim_end_matches('.').to_owned()
    }
}

pub(super) fn explain_vector_literal(value: &str) -> SessionResult<String> {
    let vector = astersql_types::vector::ParseVectorFloat32(value)
        .map_err(|error| session_error("parse EXPLAIN vector literal", error))?;
    let shown = vector
        .Elements()
        .iter()
        .take(5)
        .map(|value| explain_vector_number(*value))
        .collect::<Vec<_>>();
    let suffix = (vector.Len() > 5)
        .then(|| format!(",({} more)...", vector.Len() - 5))
        .unwrap_or_default();
    Ok(format!("[{}{}]", shown.join(","), suffix))
}

pub(super) fn explain_vector_distance_projection(
    expression: &ast::ExprNode,
    database: &str,
    table: &str,
    output_column: usize,
) -> SessionResult<Option<String>> {
    let ast::ExprKind::Function { FnName, Args, .. } = &expression.Kind else {
        return Ok(None);
    };
    if !matches!(
        FnName.L.as_str(),
        "vec_cosine_distance"
            | "vec_l2_distance"
            | "vec_l1_distance"
            | "vec_negative_inner_product"
    ) || Args.len() != 2
    {
        return Ok(None);
    }
    let Some((column, vector_argument, column_first)) = (match (&Args[0].Kind, &Args[1].Kind) {
        (ast::ExprKind::Column(column), _) => Some((column, &Args[1], true)),
        (_, ast::ExprKind::Column(column)) => Some((column, &Args[0], false)),
        _ => None,
    }) else {
        return Ok(None);
    };
    let vector_text = match &vector_argument.Kind {
        ast::ExprKind::Value(value) => value.text(),
        ast::ExprKind::Function {
            FnName: constructor,
            Args,
            ..
        } if constructor.L == "vec_from_text" && Args.len() == 1 => {
            relational_expression_value(&Args[0], &HashMap::new())?.unwrap_or_default()
        }
        _ => return Ok(None),
    };
    let column = format!("{database}.{table}.{}", column.Name.L);
    let vector = explain_vector_literal(&vector_text)?;
    let arguments = if column_first {
        format!("{column}, {vector}")
    } else {
        format!("{vector}, {column}")
    };
    Ok(Some(format!(
        "{}({arguments})->Column#{output_column}",
        FnName.L
    )))
}

pub(super) fn relational_expression_is_vector(expression: &ast::ExprNode) -> bool {
    match &expression.Kind {
        ast::ExprKind::Function { FnName, .. } => {
            FnName.L.eq_ignore_ascii_case("vec_from_text")
                || FnName.L.starts_with("vec_")
                    && !FnName.L.eq_ignore_ascii_case("vec_dims")
                    && !FnName.L.ends_with("_distance")
                    && !FnName.L.eq_ignore_ascii_case("vec_l2_norm")
        }
        ast::ExprKind::Cast { Tp, .. } => {
            Tp.GetType() == astersql_parser_mysql::r#type::TypeTiDBVectorFloat32
        }
        ast::ExprKind::Parentheses(inner) | ast::ExprKind::Collate { Expr: inner, .. } => {
            relational_expression_is_vector(inner)
        }
        _ => false,
    }
}

/// Format the runtime value of a SQL `HEX()` argument.
///
/// BIT columns are stored internally as fixed-width `0x...` markers so their
/// bytes survive row-codec round trips. MySQL treats BIT as a numeric argument
/// to `HEX()`, therefore that private padding must not leak into query results.
pub(super) fn relational_hex_value(argument: &ast::ExprNode, value: &str) -> String {
    if let Some(hex) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        if matches!(argument.Kind, ast::ExprKind::Column(_)) {
            let significant = hex.trim_start_matches('0');
            return if significant.is_empty() {
                "0".to_owned()
            } else {
                significant.to_ascii_uppercase()
            };
        }
        return hex.to_ascii_uppercase();
    }
    if let Ok(number) = value.parse::<u64>() {
        return format!("{number:X}");
    }
    value
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect()
}

/// Evaluate one scalar expression against a decoded relational row.
pub(super) fn relational_expression_value(
    expression: &ast::ExprNode,
    row: &HashMap<String, Option<String>>,
) -> SessionResult<Option<String>> {
    match &expression.Kind {
        ast::ExprKind::Value(value) => Ok(match &value.Datum {
            ast::ValueDatum::Null => None,
            ast::ValueDatum::Bool(value) => Some(if *value { "1" } else { "0" }.to_owned()),
            _ => Some(value.text()),
        }),
        ast::ExprKind::Column(column) => row
            .get(&column.Name.L)
            .cloned()
            .ok_or_else(|| SessionError::new(format!("Unknown column '{}'", column.Name.O))),
        ast::ExprKind::Parentheses(inner) | ast::ExprKind::Collate { Expr: inner, .. } => {
            relational_expression_value(inner, row)
        }
        ast::ExprKind::Unary { Op, V } if Op == "+" => relational_expression_value(V, row)?
            .map(|value| {
                if value.parse::<i128>().is_ok() || value.parse::<f64>().is_ok() {
                    Ok(value)
                } else {
                    Err(SessionError::new("invalid unary numeric expression"))
                }
            })
            .transpose(),
        ast::ExprKind::Unary { Op, V } if Op == "-" => relational_expression_value(V, row)?
            .map(|value| {
                if let Ok(value) = value.parse::<i128>() {
                    return value
                        .checked_neg()
                        .ok_or_else(|| SessionError::new("BIGINT value is out of range"))
                        .map(|value| value.to_string());
                }
                // MySQL coerces strings by their numeric prefix for unary
                // arithmetic.  A string without a numeric prefix (for
                // example `'a'`) therefore contributes zero plus a warning,
                // rather than aborting expression evaluation.
                let value = relational_numeric_prefix(&value).unwrap_or(0.0);
                Ok(relational_float_string(-value).unwrap_or_else(|| "0".to_owned()))
            })
            .transpose(),
        ast::ExprKind::Unary { Op, V } if Op == "~" => Ok(relational_expression_value(V, row)?
            .map(|value| {
                let numeric = relational_numeric_prefix(&value).unwrap_or(0.0) as u64;
                (!numeric).to_string()
            })),
        ast::ExprKind::Unary { Op, V } if Op.eq_ignore_ascii_case("not") || Op == "!" => {
            Ok(relational_boolean_value(
                relational_truth(relational_expression_value(V, row)?.as_deref())
                    .map(|value| !value),
            ))
        }
        ast::ExprKind::Binary { Op, L, R } if Op.eq_ignore_ascii_case("and") => {
            let left = relational_truth(relational_expression_value(L, row)?.as_deref());
            let right = relational_truth(relational_expression_value(R, row)?.as_deref());
            Ok(relational_boolean_value(match (left, right) {
                (Some(false), _) | (_, Some(false)) => Some(false),
                (Some(true), Some(true)) => Some(true),
                _ => None,
            }))
        }
        ast::ExprKind::Binary { Op, L, R } if Op.eq_ignore_ascii_case("or") => {
            let left = relational_truth(relational_expression_value(L, row)?.as_deref());
            let right = relational_truth(relational_expression_value(R, row)?.as_deref());
            Ok(relational_boolean_value(match (left, right) {
                (Some(true), _) | (_, Some(true)) => Some(true),
                (Some(false), Some(false)) => Some(false),
                _ => None,
            }))
        }
        ast::ExprKind::Binary { Op, L, R } if Op == "<=>" => {
            let left = relational_expression_value(L, row)?;
            let right = relational_expression_value(R, row)?;
            let equal = match (left.as_deref(), right.as_deref()) {
                (None, None) => true,
                (Some(left), Some(right)) => relational_compare(left, right).is_eq(),
                _ => false,
            };
            Ok(relational_boolean_value(Some(equal)))
        }
        ast::ExprKind::Binary { Op, L, R }
            if relational_row_items(L).is_some()
                && relational_row_items(R).is_some()
                && matches!(
                    Op.as_str(),
                    "=" | "==" | "!=" | "<>" | ">" | ">=" | "<" | "<="
                ) =>
        {
            let left = relational_row_items(L).expect("row shape checked");
            let right = relational_row_items(R).expect("row shape checked");
            if left.len() != right.len() {
                return Err(SessionError::new(
                    "Operand should contain the same number of columns",
                ));
            }
            let mut ordering = std::cmp::Ordering::Equal;
            for (left, right) in left.iter().zip(right) {
                let Some(left) = relational_expression_value(left, row)? else {
                    return Ok(None);
                };
                let Some(right) = relational_expression_value(right, row)? else {
                    return Ok(None);
                };
                ordering = relational_compare(&left, &right);
                if !ordering.is_eq() {
                    break;
                }
            }
            let matched = match Op.as_str() {
                "=" | "==" => ordering.is_eq(),
                "!=" | "<>" => !ordering.is_eq(),
                ">" => ordering.is_gt(),
                ">=" => ordering.is_ge(),
                "<" => ordering.is_lt(),
                "<=" => ordering.is_le(),
                _ => unreachable!("row comparison operator checked"),
            };
            Ok(relational_boolean_value(Some(matched)))
        }
        ast::ExprKind::Binary { Op, L, R }
            if matches!(
                Op.as_str(),
                "=" | "==" | "!=" | "<>" | ">" | ">=" | "<" | "<="
            ) =>
        {
            let left = relational_expression_value(L, row)?;
            let right = relational_expression_value(R, row)?;
            let compared = left.as_deref().zip(right.as_deref()).map(|(left, right)| {
                let ordering = relational_expression_compare(L, R, left, right, row);
                match Op.as_str() {
                    "=" | "==" => ordering.is_eq(),
                    "!=" | "<>" => !ordering.is_eq(),
                    ">" => ordering.is_gt(),
                    ">=" => ordering.is_ge(),
                    "<" => ordering.is_lt(),
                    "<=" => ordering.is_le(),
                    _ => unreachable!("comparison operator checked above"),
                }
            });
            Ok(relational_boolean_value(compared))
        }
        ast::ExprKind::Binary { Op, L, R } if matches!(Op.as_str(), "+" | "-" | "*") => {
            let Some(left) = relational_expression_value(L, row)? else {
                return Ok(None);
            };
            let Some(right) = relational_expression_value(R, row)? else {
                return Ok(None);
            };
            let left_is_vector = left.trim_start().starts_with('[');
            let right_is_vector = right.trim_start().starts_with('[');
            match (left_is_vector, right_is_vector) {
                (true, true) => {
                    let left = astersql_types::vector::ParseVectorFloat32(&left)
                        .map_err(|error| session_error("parse left vector operand", error))?;
                    let right = astersql_types::vector::ParseVectorFloat32(&right)
                        .map_err(|error| session_error("parse right vector operand", error))?;
                    let result = match Op.as_str() {
                        "+" => left.Add(&right),
                        "-" => left.Sub(&right),
                        "*" => left.Mul(&right),
                        _ => unreachable!("vector arithmetic operator checked above"),
                    }
                    .map_err(|error| session_error("compute vector arithmetic", error))?;
                    return Ok(Some(result.String()));
                }
                (true, false) | (false, true) => {
                    return Err(SessionError::new(
                        "vector arithmetic requires two VECTOR operands",
                    ));
                }
                (false, false) => {}
            }
            let left_text = if left.trim().is_empty() {
                "0"
            } else {
                left.trim()
            };
            let right_text = if right.trim().is_empty() {
                "0"
            } else {
                right.trim()
            };
            let (Ok(left_integer), Ok(right_integer)) =
                (left_text.parse::<i64>(), right_text.parse::<i64>())
            else {
                let left = left_text
                    .parse::<rust_decimal::Decimal>()
                    .map_err(|error| {
                        session_error("parse relational arithmetic left operand", error)
                    })?;
                let right = right_text
                    .parse::<rust_decimal::Decimal>()
                    .map_err(|error| {
                        session_error("parse relational arithmetic right operand", error)
                    })?;
                let value = match Op.as_str() {
                    "+" => left.checked_add(right),
                    "-" => left.checked_sub(right),
                    "*" => left.checked_mul(right),
                    _ => unreachable!("arithmetic operator checked above"),
                }
                .ok_or_else(|| SessionError::new("DECIMAL value is out of range"))?;
                return Ok(Some(value.normalize().to_string()));
            };
            let value = match Op.as_str() {
                "+" => left_integer.checked_add(right_integer),
                "-" => left_integer.checked_sub(right_integer),
                "*" => left_integer.checked_mul(right_integer),
                _ => unreachable!("arithmetic operator checked above"),
            }
            .ok_or_else(|| SessionError::new("integer overflow: BIGINT value is out of range"))?;
            Ok(Some(value.to_string()))
        }
        ast::ExprKind::Binary { Op, L, R } if matches!(Op.as_str(), "%" | "mod" | "MOD") => {
            let Some(left) = relational_numeric_value(L, row)? else {
                return Ok(None);
            };
            let Some(right) = relational_numeric_value(R, row)? else {
                return Ok(None);
            };
            if right == 0 {
                return Ok(None);
            }
            let value = left
                .checked_rem(right)
                .ok_or_else(|| SessionError::new("BIGINT value is out of range"))?;
            Ok(Some(value.to_string()))
        }
        ast::ExprKind::Binary { Op, L, R } if Op == "/" || Op.eq_ignore_ascii_case("div") => {
            if Op == "/"
                && let Some(precision) = BUILD_DIVISION_PRECISION.with(|slot| slot.get())
            {
                use astersql_types::decimal::mydecimal::{
                    DecimalDiv, DecimalError, ModeHalfUp, MyDecimal,
                };
                let Some((left, right)) =
                    relational_expression_value(L, row)?.zip(relational_expression_value(R, row)?)
                else {
                    return Ok(None);
                };
                // DOUBLE/scientific input retains the real arithmetic path.
                if !left.contains(['e', 'E']) && !right.contains(['e', 'E']) {
                    let mut lhs = MyDecimal::default();
                    let mut rhs = MyDecimal::default();
                    lhs.FromString(left.as_bytes())
                        .map_err(|e| session_error("parse build decimal dividend", e))?;
                    rhs.FromString(right.as_bytes())
                        .map_err(|e| session_error("parse build decimal divisor", e))?;
                    let mut quotient = MyDecimal::default();
                    match DecimalDiv(&lhs, &rhs, &mut quotient, precision as isize) {
                        Ok(()) | Err(DecimalError::Truncated) => {}
                        Err(DecimalError::DivByZero) => return Ok(None),
                        Err(error) => {
                            return Err(session_error("evaluate build decimal division", error));
                        }
                    }
                    let mut rounded = MyDecimal::default();
                    quotient
                        .Round(
                            &mut rounded,
                            (lhs.GetDigitsFrac() as i32 + precision).min(30) as isize,
                            ModeHalfUp,
                        )
                        .map_err(|e| session_error("round build decimal division", e))?;
                    return Ok(Some(
                        String::from_utf8(rounded.ToString())
                            .map_err(|e| session_error("render build decimal division", e))?,
                    ));
                }
            }
            let left = relational_expression_value(L, row)?
                .map(|value| {
                    value.parse::<f64>().map_err(|error| {
                        session_error("parse relational division left operand", error)
                    })
                })
                .transpose()?;
            let right = relational_expression_value(R, row)?
                .map(|value| {
                    value.parse::<f64>().map_err(|error| {
                        session_error("parse relational division right operand", error)
                    })
                })
                .transpose()?;
            let Some((left, right)) = left.zip(right) else {
                return Ok(None);
            };
            if right == 0.0 {
                return Ok(None);
            }
            if Op.eq_ignore_ascii_case("div") {
                return Ok(Some(((left / right).trunc() as i128).to_string()));
            }
            let value = left / right;
            Ok(Some(if value.fract() == 0.0 {
                format!("{value:.4}")
            } else {
                value.to_string()
            }))
        }
        ast::ExprKind::IsNull { Expr, Not } => {
            let is_null = relational_expression_value(Expr, row)?.is_none();
            Ok(relational_boolean_value(Some(if *Not {
                !is_null
            } else {
                is_null
            })))
        }
        ast::ExprKind::IsTruth {
            Expr, Not, True, ..
        } => {
            let truth = relational_truth(relational_expression_value(Expr, row)?.as_deref())
                .unwrap_or(false);
            let matched = if *True { truth } else { !truth };
            Ok(relational_boolean_value(Some(if *Not {
                !matched
            } else {
                matched
            })))
        }
        ast::ExprKind::InList {
            Expr, List, Not, ..
        } => {
            if let ast::ExprKind::Row(values) = &Expr.Kind {
                let value = values
                    .iter()
                    .map(|expression| relational_expression_value(expression, row))
                    .collect::<SessionResult<Vec<_>>>()?;
                let mut saw_null = value.iter().any(Option::is_none);
                for candidate in List {
                    let ast::ExprKind::Row(candidate_values) = &candidate.Kind else {
                        return Err(SessionError::new(
                            "Operand should contain the same number of columns",
                        ));
                    };
                    if candidate_values.len() != value.len() {
                        return Err(SessionError::new(
                            "Operand should contain the same number of columns",
                        ));
                    }
                    let candidate = candidate_values
                        .iter()
                        .map(|expression| relational_expression_value(expression, row))
                        .collect::<SessionResult<Vec<_>>>()?;
                    let mut matched = true;
                    let mut candidate_has_null = false;
                    for (left, right) in value.iter().zip(candidate.iter()) {
                        match (left.as_deref(), right.as_deref()) {
                            (Some(left), Some(right)) => {
                                if !relational_compare(left, right).is_eq() {
                                    matched = false;
                                    break;
                                }
                            }
                            _ => candidate_has_null = true,
                        }
                    }
                    if matched && !candidate_has_null {
                        return Ok(relational_boolean_value(Some(!*Not)));
                    }
                    saw_null |= matched && candidate_has_null;
                }
                return Ok(if saw_null {
                    None
                } else {
                    relational_boolean_value(Some(*Not))
                });
            }
            let Some(value) = relational_expression_value(Expr, row)? else {
                return Ok(None);
            };
            let mut saw_null = false;
            for candidate in List {
                let Some(candidate) = relational_expression_value(candidate, row)? else {
                    saw_null = true;
                    continue;
                };
                if relational_compare(&value, &candidate).is_eq() {
                    return Ok(relational_boolean_value(Some(!*Not)));
                }
            }
            Ok(if saw_null {
                None
            } else {
                relational_boolean_value(Some(*Not))
            })
        }
        ast::ExprKind::Between {
            Expr,
            Left,
            Right,
            Not,
        } => {
            let value = relational_expression_value(Expr, row)?;
            let left = relational_expression_value(Left, row)?;
            let right = relational_expression_value(Right, row)?;
            let Some((value, left, right)) = value
                .as_deref()
                .zip(left.as_deref())
                .zip(right.as_deref())
                .map(|((value, left), right)| (value, left, right))
            else {
                return Ok(None);
            };
            let matched =
                relational_compare(value, left).is_ge() && relational_compare(value, right).is_le();
            Ok(relational_boolean_value(Some(if *Not {
                !matched
            } else {
                matched
            })))
        }
        ast::ExprKind::Like {
            Expr,
            Pattern,
            Not,
            Escape,
            ..
        } => {
            let value = relational_expression_value(Expr, row)?;
            let pattern = relational_expression_value(Pattern, row)?;
            let Some((value, pattern)) = value.as_deref().zip(pattern.as_deref()) else {
                return Ok(None);
            };
            let matched =
                relational_like_with_escape(pattern, value, Escape.chars().next().unwrap_or('\\'));
            Ok(relational_boolean_value(Some(if *Not {
                !matched
            } else {
                matched
            })))
        }
        ast::ExprKind::Case {
            Value,
            WhenClauses,
            ElseClause,
        } => {
            let case_value = Value
                .as_ref()
                .map(|value| relational_expression_value(value, row))
                .transpose()?;
            for clause in WhenClauses {
                let matched = if let Some(case_value) = case_value.as_ref() {
                    let when_value = relational_expression_value(&clause.Expr, row)?;
                    case_value
                        .as_deref()
                        .zip(when_value.as_deref())
                        .is_some_and(|(case_value, when_value)| {
                            relational_compare(case_value, when_value).is_eq()
                        })
                } else {
                    relational_truth(relational_expression_value(&clause.Expr, row)?.as_deref())
                        == Some(true)
                };
                if matched {
                    return relational_expression_value(&clause.Result, row);
                }
            }
            ElseClause
                .as_ref()
                .map(|expression| relational_expression_value(expression, row))
                .unwrap_or(Ok(None))
        }
        ast::ExprKind::Cast { Expr, Tp, .. } => {
            let Some(value) = relational_expression_value(Expr, row)? else {
                return Ok(None);
            };
            let target = Tp.GetType();
            if target == astersql_parser_mysql::r#type::TypeTiDBVectorFloat32 {
                let vector = astersql_types::vector::ParseVectorFloat32(&value)
                    .map_err(|error| session_error("cast value as VECTOR", error))?;
                vector
                    .CheckDimsFitColumn(Tp.GetFlen() as i32)
                    .map_err(|error| SessionError::new(error.to_string()))?;
                return Ok(Some(vector.String()));
            }
            if relational_expression_is_vector(Expr)
                && !matches!(
                    target,
                    astersql_parser_mysql::r#type::TypeString
                        | astersql_parser_mysql::r#type::TypeVarchar
                        | astersql_parser_mysql::r#type::TypeVarString
                        | astersql_parser_mysql::r#type::TypeBlob
                        | astersql_parser_mysql::r#type::TypeTinyBlob
                        | astersql_parser_mysql::r#type::TypeMediumBlob
                        | astersql_parser_mysql::r#type::TypeLongBlob
                )
            {
                return Err(SessionError::new(format!(
                    "cannot cast VECTOR to {}",
                    Tp.InfoSchemaStr()
                )));
            }
            // Binary columns are carried through the relational runtime as a
            // hex marker to preserve arbitrary bytes.  CAST(... AS CHAR)
            // changes their SQL presentation back to text; leaking the marker
            // here made a BLOB primary-index read differ from TiDB.
            if matches!(
                target,
                astersql_parser_mysql::r#type::TypeString
                    | astersql_parser_mysql::r#type::TypeVarchar
                    | astersql_parser_mysql::r#type::TypeVarString
            ) && !astersql_parser_mysql::r#type::HasBinaryFlag(Tp.GetFlag())
            {
                return Ok(Some(display_runtime_value(value)));
            }
            Ok(Some(value))
        }
        ast::ExprKind::Function { FnName, Args, .. } => {
            let name = FnName.L.to_ascii_lowercase();
            match name.as_str() {
                // Relational predicates can contain deferred time functions as
                // well as constant SELECTs.  Evaluate them at statement time
                // so prepared range predicates such as `created < now(3)` do
                // not fail during execution.
                "now" | "current_timestamp" | "localtimestamp" | "utc_timestamp"
                    if Args.len() <= 1 =>
                {
                    let timestamp = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs_f64();
                    let precision = Args
                        .first()
                        .map(|argument| {
                            relational_expression_value(argument, row)?
                                .unwrap_or_default()
                                .parse::<usize>()
                                .map_err(|error| session_error("parse NOW precision", error))
                        })
                        .transpose()?
                        .unwrap_or(0);
                    Ok(Some(format_unix_timestamp(timestamp, precision)))
                }
                "unix_timestamp" if Args.is_empty() => Ok(Some(
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs()
                        .to_string(),
                )),
                "if" if Args.len() == 3 => {
                    let condition =
                        relational_truth(relational_expression_value(&Args[0], row)?.as_deref())
                            .unwrap_or(false);
                    relational_expression_value(&Args[if condition { 1 } else { 2 }], row)
                }
                "sign" if Args.len() == 1 => {
                    let Some(value) = relational_expression_value(&Args[0], row)? else {
                        return Ok(None);
                    };
                    let value = value
                        .parse::<rust_decimal::Decimal>()
                        .map_err(|error| session_error("parse SIGN argument", error))?;
                    Ok(Some(
                        if value.is_zero() {
                            "0"
                        } else if value.is_sign_negative() {
                            "-1"
                        } else {
                            "1"
                        }
                        .to_owned(),
                    ))
                }
                "hex" if Args.len() == 1 => {
                    let Some(value) = relational_expression_value(&Args[0], row)? else {
                        return Ok(None);
                    };
                    Ok(Some(relational_hex_value(&Args[0], &value)))
                }
                "coalesce" => {
                    for argument in Args {
                        if let Some(value) = relational_expression_value(argument, row)? {
                            return Ok(Some(value));
                        }
                    }
                    Ok(None)
                }
                "ifnull" if Args.len() == 2 => {
                    if let Some(value) = relational_expression_value(&Args[0], row)? {
                        Ok(Some(value))
                    } else {
                        relational_expression_value(&Args[1], row)
                    }
                }
                "nullif" if Args.len() == 2 => {
                    let left = relational_expression_value(&Args[0], row)?;
                    let right = relational_expression_value(&Args[1], row)?;
                    if left
                        .as_deref()
                        .zip(right.as_deref())
                        .is_some_and(|(left, right)| relational_compare(left, right).is_eq())
                    {
                        Ok(None)
                    } else {
                        Ok(left)
                    }
                }
                "interval" if Args.len() >= 2 => {
                    let Some(value) = relational_expression_value(&Args[0], row)? else {
                        return Ok(Some("-1".to_owned()));
                    };
                    let mut index = 0;
                    for candidate in &Args[1..] {
                        let candidate = relational_expression_value(candidate, row)?;
                        if candidate.as_deref().is_some_and(|candidate| {
                            match (
                                value.trim().parse::<i128>(),
                                candidate.trim().parse::<i128>(),
                            ) {
                                (Ok(value), Ok(candidate)) => value < candidate,
                                _ => {
                                    relational_numeric_prefix(&value).unwrap_or_default()
                                        < relational_numeric_prefix(candidate).unwrap_or_default()
                                }
                            }
                        }) {
                            break;
                        }
                        index += 1;
                    }
                    Ok(Some(index.to_string()))
                }
                "greatest" | "least" if !Args.is_empty() => {
                    let mut values = Vec::with_capacity(Args.len());
                    for argument in Args {
                        let Some(value) = relational_expression_value(argument, row)? else {
                            return Ok(None);
                        };
                        values.push(value);
                    }
                    let numeric_mode = values
                        .iter()
                        .all(|value| relational_numeric_prefix(value).is_some());
                    let mut selected: Option<String> = None;
                    for value in values {
                        if numeric_mode && value.parse::<f64>().is_err() {
                            continue;
                        }
                        let replace = selected.as_ref().is_none_or(|current| {
                            let ordering = if numeric_mode {
                                value
                                    .parse::<f64>()
                                    .unwrap_or_default()
                                    .total_cmp(&current.parse::<f64>().unwrap_or_default())
                            } else {
                                relational_compare(&value, current)
                            };
                            if name == "greatest" {
                                ordering.is_gt()
                            } else {
                                ordering.is_lt()
                            }
                        });
                        if replace {
                            selected = Some(value);
                        }
                    }
                    Ok(selected)
                }
                "upper" | "ucase" if Args.len() == 1 => {
                    Ok(relational_expression_value(&Args[0], row)?
                        .map(|value| value.to_uppercase()))
                }
                "lower" | "lcase" if Args.len() == 1 => {
                    Ok(relational_expression_value(&Args[0], row)?
                        .map(|value| value.to_lowercase()))
                }
                "concat" => {
                    let mut value = String::new();
                    for argument in Args {
                        let Some(part) = relational_expression_value(argument, row)? else {
                            return Ok(None);
                        };
                        value.push_str(&part);
                    }
                    Ok(Some(value))
                }
                "elt" | "lpad" | "timestampadd" => {
                    crate::dml_runtime::EvalExpr(expression, row, None)
                }
                "repeat" if Args.len() == 2 => {
                    let Some(value) = relational_expression_value(&Args[0], row)? else {
                        return Ok(None);
                    };
                    let Some(count) = relational_expression_value(&Args[1], row)? else {
                        return Ok(None);
                    };
                    let count = count
                        .parse::<usize>()
                        .map_err(|error| session_error("parse REPEAT count", error))?;
                    Ok(Some(value.repeat(count)))
                }
                "unhex" if Args.len() == 1 => {
                    let Some(value) = relational_expression_value(&Args[0], row)? else {
                        return Ok(None);
                    };
                    if value.len() % 2 != 0 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                        return Ok(None);
                    }
                    let mut bytes = Vec::with_capacity(value.len() / 2);
                    for pair in value.as_bytes().chunks(2) {
                        let high = (pair[0] as char).to_digit(16).unwrap_or_default();
                        let low = (pair[1] as char).to_digit(16).unwrap_or_default();
                        bytes.push(((high << 4) | low) as u8);
                    }
                    Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
                }
                "length" | "octet_length" if Args.len() == 1 => {
                    Ok(relational_expression_value(&Args[0], row)?
                        .map(|value| value.len().to_string()))
                }
                "hex" if Args.len() == 1 => {
                    let Some(value) = relational_expression_value(&Args[0], row)? else {
                        return Ok(None);
                    };
                    Ok(Some(relational_hex_value(&Args[0], &value)))
                }
                "benchmark" if Args.len() == 2 => {
                    let Some(count) = relational_expression_value(&Args[0], row)? else {
                        return Ok(None);
                    };
                    let count = count
                        .parse::<i64>()
                        .map_err(|error| session_error("parse BENCHMARK count", error))?;
                    if count > 0 {
                        for _ in 0..count {
                            let _ = relational_expression_value(&Args[1], row)?;
                        }
                    }
                    Ok(Some("0".to_owned()))
                }
                "date" if Args.len() == 1 => {
                    let Some(value) = relational_expression_value(&Args[0], row)? else {
                        return Ok(None);
                    };
                    Ok(parse_runtime_datetime(&value)
                        .map(|datetime| datetime.date().format("%Y-%m-%d").to_string()))
                }
                "year" | "month" | "quarter" | "dayofmonth" | "day" | "dayofyear" | "weekday"
                | "dayofweek"
                    if Args.len() == 1 =>
                {
                    let Some(value) = relational_expression_value(&Args[0], row)? else {
                        return Ok(None);
                    };
                    if value.starts_with("0000-") {
                        return Ok(Some("0".to_owned()).filter(|_| {
                            matches!(name.as_str(), "year" | "month" | "dayofmonth" | "day")
                        }));
                    }
                    let Some(datetime) = parse_runtime_datetime(&value) else {
                        return Ok(None);
                    };
                    let number = match name.as_str() {
                        "year" => i64::from(datetime.year()),
                        "month" => i64::from(datetime.month()),
                        "quarter" => i64::from((datetime.month() - 1) / 3 + 1),
                        "dayofmonth" | "day" => i64::from(datetime.day()),
                        "dayofyear" => i64::from(datetime.ordinal()),
                        "weekday" => i64::from(datetime.weekday().num_days_from_monday()),
                        "dayofweek" => i64::from(datetime.weekday().num_days_from_sunday() + 1),
                        _ => unreachable!("date part function checked"),
                    };
                    Ok(Some(number.to_string()))
                }
                "time" if Args.len() == 1 => {
                    let Some(value) = relational_expression_value(&Args[0], row)? else {
                        return Ok(None);
                    };
                    let Some((hours, minutes, seconds, micros)) = runtime_time_components(&value)
                    else {
                        return Ok(None);
                    };
                    if hours.unsigned_abs() > 838 {
                        return Ok(None);
                    }
                    let sign = if hours < 0 { "-" } else { "" };
                    Ok(Some(if micros == 0 {
                        format!("{sign}{:02}:{minutes:02}:{seconds:02}", hours.abs())
                    } else {
                        format!(
                            "{sign}{:02}:{minutes:02}:{seconds:02}.{micros:06}",
                            hours.abs()
                        )
                    }))
                }
                "hour" | "minute" | "second" | "microsecond" if Args.len() == 1 => {
                    let Some(value) = relational_expression_value(&Args[0], row)? else {
                        return Ok(None);
                    };
                    let Some((hours, minutes, seconds, micros)) = runtime_time_components(&value)
                    else {
                        return Ok(None);
                    };
                    let number = match name.as_str() {
                        "hour" => hours,
                        "minute" => minutes,
                        "second" => seconds,
                        "microsecond" => i64::from(micros),
                        _ => unreachable!("time part function checked"),
                    };
                    Ok(Some(number.to_string()))
                }
                "datediff" if Args.len() == 2 => {
                    let Some(left) = relational_expression_value(&Args[0], row)? else {
                        return Ok(None);
                    };
                    let Some(right) = relational_expression_value(&Args[1], row)? else {
                        return Ok(None);
                    };
                    Ok(parse_runtime_datetime(&left)
                        .zip(parse_runtime_datetime(&right))
                        .map(|(left, right)| {
                            left.date()
                                .signed_duration_since(right.date())
                                .num_days()
                                .to_string()
                        }))
                }
                "last_day" if Args.len() == 1 => {
                    let Some(value) = relational_expression_value(&Args[0], row)? else {
                        return Ok(None);
                    };
                    let Some(datetime) = parse_runtime_datetime(&value) else {
                        return Ok(None);
                    };
                    let (year, month) = if datetime.month() == 12 {
                        (datetime.year() + 1, 1)
                    } else {
                        (datetime.year(), datetime.month() + 1)
                    };
                    Ok(chrono::NaiveDate::from_ymd_opt(year, month, 1)
                        .and_then(|next| next.pred_opt())
                        .map(|date| date.format("%Y-%m-%d").to_string()))
                }
                "date_format" if Args.len() == 2 => {
                    let Some(value) = relational_expression_value(&Args[0], row)? else {
                        return Ok(None);
                    };
                    let Some(format) = relational_expression_value(&Args[1], row)? else {
                        return Ok(None);
                    };
                    let context = astersql_types::time::BasicTimeContext::default();
                    let parsed = astersql_types::time::ParseDatetime(&context, &value)
                        .map_err(|error| session_error("parse DATE_FORMAT value", error))?;
                    parsed
                        .DateFormat(&format)
                        .map(Some)
                        .map_err(|error| session_error("format DATE_FORMAT value", error))
                }
                "extract" if Args.len() == 2 => {
                    let ast::ExprKind::TimeUnit(unit) = Args[0].Kind else {
                        return Err(SessionError::new(
                            "EXTRACT first argument must be a time unit",
                        ));
                    };
                    let Some(value) = relational_expression_value(&Args[1], row)? else {
                        return Ok(None);
                    };
                    let (date, time) = value
                        .split_once(' ')
                        .map_or(("", value.as_str()), |(date, time)| (date, time));
                    let date_parts = date
                        .split('-')
                        .map(|part| part.parse::<i64>().unwrap_or_default())
                        .collect::<Vec<_>>();
                    let (clock, fraction) = time.split_once('.').unwrap_or((time, ""));
                    let time_parts = clock
                        .split(':')
                        .map(|part| part.parse::<i64>().unwrap_or_default())
                        .collect::<Vec<_>>();
                    let year = date_parts.first().copied().unwrap_or_default();
                    let month = date_parts.get(1).copied().unwrap_or_default();
                    let day = date_parts.get(2).copied().unwrap_or_default();
                    let hour = time_parts.first().copied().unwrap_or_default();
                    let minute = time_parts.get(1).copied().unwrap_or_default();
                    let second = time_parts.get(2).copied().unwrap_or_default();
                    let microsecond = format!("{fraction:0<6}")
                        .chars()
                        .take(6)
                        .collect::<String>()
                        .parse::<i64>()
                        .unwrap_or_default();
                    let unit = format!("{unit:?}").to_ascii_lowercase();
                    let result = match unit.as_str() {
                        "year" => year,
                        "quarter" => (month + 2) / 3,
                        "yearmonth" | "year_month" => year * 100 + month,
                        "month" => month,
                        "week" => 0,
                        "day" => day,
                        "dayhour" | "day_hour" => day * 100 + hour,
                        "dayminute" | "day_minute" => day * 10_000 + hour * 100 + minute,
                        "daysecond" | "day_second" => {
                            day * 1_000_000 + hour * 10_000 + minute * 100 + second
                        }
                        "daymicrosecond" | "day_microsecond" => {
                            day * 1_000_000_000_000
                                + hour * 10_000_000_000
                                + minute * 100_000_000
                                + second * 1_000_000
                                + microsecond
                        }
                        "hour" => hour,
                        "hourminute" | "hour_minute" => hour * 100 + minute,
                        "hoursecond" | "hour_second" => hour * 10_000 + minute * 100 + second,
                        "hourmicrosecond" | "hour_microsecond" => {
                            hour * 10_000_000_000
                                + minute * 100_000_000
                                + second * 1_000_000
                                + microsecond
                        }
                        "minute" => minute,
                        "minutesecond" | "minute_second" => minute * 100 + second,
                        "minutemicrosecond" | "minute_microsecond" => {
                            minute * 100_000_000 + second * 1_000_000 + microsecond
                        }
                        "second" => second,
                        "secondmicrosecond" | "second_microsecond" => {
                            second * 1_000_000 + microsecond
                        }
                        "microsecond" => microsecond,
                        _ => return Err(SessionError::new("unsupported EXTRACT time unit")),
                    };
                    Ok(Some(result.to_string()))
                }
                "timestampdiff" if Args.len() == 3 => {
                    let ast::ExprKind::TimeUnit(unit) = Args[0].Kind else {
                        return Err(SessionError::new(
                            "TIMESTAMPDIFF first argument must be a time unit",
                        ));
                    };
                    let Some(left) = relational_expression_value(&Args[1], row)? else {
                        return Ok(None);
                    };
                    let Some(right) = relational_expression_value(&Args[2], row)? else {
                        return Ok(None);
                    };
                    let context = astersql_types::time::BasicTimeContext::default();
                    let left = astersql_types::time::ParseDatetime(&context, &left)
                        .map_err(|error| session_error("parse TIMESTAMPDIFF left", error))?;
                    let right = astersql_types::time::ParseDatetime(&context, &right)
                        .map_err(|error| session_error("parse TIMESTAMPDIFF right", error))?;
                    Ok(Some(
                        astersql_types::time::TimestampDiff(
                            &format!("{unit:?}").to_ascii_uppercase(),
                            left,
                            right,
                        )
                        .to_string(),
                    ))
                }
                "period_add" | "period_diff" if Args.len() == 2 => {
                    let Some(left) = relational_expression_value(&Args[0], row)? else {
                        return Ok(None);
                    };
                    let Some(right) = relational_expression_value(&Args[1], row)? else {
                        return Ok(None);
                    };
                    let period_to_month = |period: i64| {
                        let year = period / 100;
                        let month = period % 100;
                        year * 12 + month - 1
                    };
                    let left = relational_numeric_prefix(&left).unwrap_or_default() as i64;
                    let right = relational_numeric_prefix(&right).unwrap_or_default() as i64;
                    if name == "period_diff" {
                        Ok(Some(
                            (period_to_month(left) - period_to_month(right)).to_string(),
                        ))
                    } else {
                        let month = period_to_month(left) + right;
                        Ok(Some(format!(
                            "{}{:02}",
                            month.div_euclid(12),
                            month.rem_euclid(12) + 1
                        )))
                    }
                }
                "json_extract" if Args.len() >= 2 => {
                    let Some(document) = relational_expression_value(&Args[0], row)? else {
                        return Ok(None);
                    };
                    let document =
                        astersql_types::json_functions::ParseBinaryJSONFromString(&document)
                            .map_err(|error| session_error("parse JSON_EXTRACT document", error))?;
                    let mut paths = Vec::with_capacity(Args.len() - 1);
                    for argument in &Args[1..] {
                        let Some(path) = relational_expression_value(argument, row)? else {
                            return Ok(None);
                        };
                        paths.push(path);
                    }
                    let paths = paths.iter().map(String::as_str).collect::<Vec<_>>();
                    astersql_expression::builtin_json::json_extract(&document, &paths)
                        .map_err(|error| SessionError::new(error.to_string()))?
                        .map(|value| {
                            astersql_types::json_functions::BinaryJSONToSerde(&value)
                                .map(|value| format_mysql_json(&value))
                                .map_err(|error| SessionError::new(error.to_string()))
                        })
                        .transpose()
                }
                "json_array" => {
                    let mut values = Vec::with_capacity(Args.len());
                    for argument in Args {
                        let value = match &argument.Kind {
                            ast::ExprKind::Value(value) => match &value.Datum {
                                ast::ValueDatum::Null => None,
                                ast::ValueDatum::Bool(value) => {
                                    Some(serde_json::Value::Bool(*value))
                                }
                                ast::ValueDatum::Int64(value) => {
                                    Some(serde_json::Value::Number((*value).into()))
                                }
                                ast::ValueDatum::Uint64(value) => {
                                    Some(serde_json::Value::Number((*value).into()))
                                }
                                ast::ValueDatum::Float32(bits) => {
                                    serde_json::Number::from_f64(f64::from(f32::from_bits(*bits)))
                                        .map(serde_json::Value::Number)
                                }
                                ast::ValueDatum::Float64(bits) => {
                                    serde_json::Number::from_f64(f64::from_bits(*bits))
                                        .map(serde_json::Value::Number)
                                }
                                ast::ValueDatum::Decimal(value) => serde_json::from_str(value)
                                    .ok()
                                    .or_else(|| Some(serde_json::Value::String(value.clone()))),
                                ast::ValueDatum::String(value) => {
                                    Some(serde_json::Value::String(value.clone()))
                                }
                                ast::ValueDatum::Bytes(value)
                                | ast::ValueDatum::BitLiteral(value)
                                | ast::ValueDatum::HexLiteral(value) => {
                                    Some(serde_json::Value::String(
                                        String::from_utf8_lossy(value).into_owned(),
                                    ))
                                }
                            },
                            _ => relational_expression_value(argument, row)?.map(|value| {
                                serde_json::from_str(&value)
                                    .unwrap_or(serde_json::Value::String(value))
                            }),
                        }
                        .map(astersql_types::json_functions::CreateBinaryJSON)
                        .transpose()
                        .map_err(|error| SessionError::new(error.to_string()))?;
                        values.push(value);
                    }
                    let value = astersql_expression::builtin_json::json_array(&values)
                        .map_err(|error| SessionError::new(error.to_string()))?;
                    let value = astersql_types::json_functions::BinaryJSONToSerde(&value)
                        .map_err(|error| SessionError::new(error.to_string()))?;
                    Ok(Some(format_mysql_json(&value)))
                }
                "json_array_append" if Args.len() >= 3 && Args.len() % 2 == 1 => {
                    let Some(document) = relational_expression_value(&Args[0], row)? else {
                        return Ok(None);
                    };
                    let document =
                        astersql_types::json_functions::ParseBinaryJSONFromString(&document)
                            .map_err(|error| {
                                SessionError::new(format!(
                                    "[types:3140]Invalid JSON text in argument to function \
                                     json_array_append: {error}"
                                ))
                            })?;
                    let mut pairs = Vec::with_capacity((Args.len() - 1) / 2);
                    for pair in Args[1..].chunks_exact(2) {
                        let Some(path) = relational_expression_value(&pair[0], row)? else {
                            return Ok(None);
                        };
                        let value = match &pair[1].Kind {
                            ast::ExprKind::Value(value) => match &value.Datum {
                                ast::ValueDatum::Null => None,
                                ast::ValueDatum::Bool(value) => {
                                    Some(serde_json::Value::Bool(*value))
                                }
                                ast::ValueDatum::Int64(value) => {
                                    Some(serde_json::Value::Number((*value).into()))
                                }
                                ast::ValueDatum::Uint64(value) => {
                                    Some(serde_json::Value::Number((*value).into()))
                                }
                                ast::ValueDatum::Float32(bits) => {
                                    serde_json::Number::from_f64(f64::from(f32::from_bits(*bits)))
                                        .map(serde_json::Value::Number)
                                }
                                ast::ValueDatum::Float64(bits) => {
                                    serde_json::Number::from_f64(f64::from_bits(*bits))
                                        .map(serde_json::Value::Number)
                                }
                                ast::ValueDatum::Decimal(value) => serde_json::from_str(value)
                                    .ok()
                                    .or_else(|| Some(serde_json::Value::String(value.clone()))),
                                ast::ValueDatum::String(value) => {
                                    Some(serde_json::Value::String(value.clone()))
                                }
                                ast::ValueDatum::Bytes(value)
                                | ast::ValueDatum::BitLiteral(value)
                                | ast::ValueDatum::HexLiteral(value) => {
                                    Some(serde_json::Value::String(
                                        String::from_utf8_lossy(value).into_owned(),
                                    ))
                                }
                            },
                            _ => relational_expression_value(&pair[1], row)?.map(|value| {
                                serde_json::from_str(&value)
                                    .unwrap_or(serde_json::Value::String(value))
                            }),
                        }
                        .map(astersql_types::json_functions::CreateBinaryJSON)
                        .transpose()
                        .map_err(|error| SessionError::new(error.to_string()))?;
                        pairs.push((path, value));
                    }
                    let pairs = pairs
                        .iter()
                        .map(|(path, value)| (path.as_str(), value.clone()))
                        .collect::<Vec<_>>();
                    let value =
                        astersql_expression::builtin_json::json_array_append(&document, &pairs)
                            .map_err(|error| SessionError::new(error.to_string()))?;
                    let value = astersql_types::json_functions::BinaryJSONToSerde(&value)
                        .map_err(|error| SessionError::new(error.to_string()))?;
                    Ok(Some(format_mysql_json(&value)))
                }
                "json_merge_patch" if Args.len() >= 2 => {
                    let mut documents = Vec::with_capacity(Args.len());
                    for argument in Args {
                        let Some(value) = relational_expression_value(argument, row)? else {
                            documents.push(None);
                            continue;
                        };
                        let parsed = serde_json::from_str(&value).map_err(|error| {
                            SessionError::new(format!(
                                "[types:3140]Invalid JSON text in argument to function \
                                 json_merge_patch: {error}"
                            ))
                        })?;
                        documents.push(Some(
                            astersql_types::json_functions::CreateBinaryJSON(parsed)
                                .map_err(|error| SessionError::new(error.to_string()))?,
                        ));
                    }
                    let references = documents.iter().map(Option::as_ref).collect::<Vec<_>>();
                    astersql_types::json_functions::MergePatchBinaryJSON(&references)
                        .map_err(|error| SessionError::new(error.to_string()))?
                        .map(|value| {
                            astersql_types::json_functions::BinaryJSONToSerde(&value)
                                .map(|value| format_mysql_json(&value))
                                .map_err(|error| SessionError::new(error.to_string()))
                        })
                        .transpose()
                }
                "member of" | "member_of" if Args.len() == 2 => {
                    let Some(target_text) = relational_expression_value(&Args[0], row)? else {
                        return Ok(None);
                    };
                    let Some(document_text) = relational_expression_value(&Args[1], row)? else {
                        return Ok(None);
                    };
                    let target = match &Args[0].Kind {
                        ast::ExprKind::Value(value) => match &value.Datum {
                            ast::ValueDatum::Null => return Ok(None),
                            ast::ValueDatum::Bool(value) => serde_json::Value::Bool(*value),
                            ast::ValueDatum::Int64(value) => serde_json::Value::from(*value),
                            ast::ValueDatum::Uint64(value) => serde_json::Value::from(*value),
                            ast::ValueDatum::Float32(bits) => {
                                serde_json::Value::from(f32::from_bits(*bits))
                            }
                            ast::ValueDatum::Float64(bits) => {
                                serde_json::Value::from(f64::from_bits(*bits))
                            }
                            ast::ValueDatum::Decimal(value) => serde_json::from_str(value)
                                .map_err(|error| {
                                    session_error("parse MEMBER OF decimal target", error)
                                })?,
                            ast::ValueDatum::String(value) => {
                                serde_json::Value::String(value.clone())
                            }
                            ast::ValueDatum::Bytes(value)
                            | ast::ValueDatum::BitLiteral(value)
                            | ast::ValueDatum::HexLiteral(value) => serde_json::Value::String(
                                String::from_utf8_lossy(value).into_owned(),
                            ),
                        },
                        _ => serde_json::from_str(&target_text)
                            .unwrap_or_else(|_| serde_json::Value::String(target_text)),
                    };
                    let document: serde_json::Value = serde_json::from_str(&document_text)
                        .map_err(|error| session_error("parse MEMBER OF JSON document", error))?;
                    let target = astersql_types::json_functions::CreateBinaryJSON(target)
                        .map_err(|error| SessionError::new(error.to_string()))?;
                    let document_binary =
                        astersql_types::json_functions::CreateBinaryJSON(document.clone())
                            .map_err(|error| SessionError::new(error.to_string()))?;
                    let is_member = match document {
                        serde_json::Value::Array(values) => values.into_iter().any(|value| {
                            astersql_types::json_functions::CreateBinaryJSON(value).is_ok_and(
                                |candidate| {
                                    astersql_types::json_functions::CompareBinaryJSON(
                                        &candidate, &target,
                                    ) == 0
                                },
                            )
                        }),
                        _ => {
                            astersql_types::json_functions::CompareBinaryJSON(
                                &document_binary,
                                &target,
                            ) == 0
                        }
                    };
                    Ok(Some(u8::from(is_member).to_string()))
                }
                "json_contains" if matches!(Args.len(), 2 | 3) => {
                    let Some(document) = relational_expression_value(&Args[0], row)? else {
                        return Ok(None);
                    };
                    let Some(target) = relational_expression_value(&Args[1], row)? else {
                        return Ok(None);
                    };
                    let document =
                        astersql_expression::builtin_json::ParseBinaryJSONFromString(&document)
                            .map_err(|error| {
                                session_error("parse JSON_CONTAINS document", error)
                            })?;
                    let target =
                        astersql_expression::builtin_json::ParseBinaryJSONFromString(&target)
                            .map_err(|error| session_error("parse JSON_CONTAINS target", error))?;
                    let path = if Args.len() == 3 {
                        let Some(path) = relational_expression_value(&Args[2], row)? else {
                            return Ok(None);
                        };
                        Some(path)
                    } else {
                        None
                    };
                    astersql_expression::builtin_json::json_contains(
                        &document,
                        &target,
                        path.as_deref(),
                    )
                    .map_err(|error| SessionError::new(error.to_string()))
                    .map(|result| result.map(|value| u8::from(value).to_string()))
                }
                "json_overlaps" if Args.len() == 2 => {
                    let Some(left) = relational_expression_value(&Args[0], row)? else {
                        return Ok(None);
                    };
                    let Some(right) = relational_expression_value(&Args[1], row)? else {
                        return Ok(None);
                    };
                    let left = astersql_expression::builtin_json::ParseBinaryJSONFromString(&left)
                        .map_err(|error| {
                            session_error("parse JSON_OVERLAPS left argument", error)
                        })?;
                    let right =
                        astersql_expression::builtin_json::ParseBinaryJSONFromString(&right)
                            .map_err(|error| {
                                session_error("parse JSON_OVERLAPS right argument", error)
                            })?;
                    Ok(Some(
                        u8::from(astersql_expression::builtin_json::json_overlaps(
                            &left, &right,
                        ))
                        .to_string(),
                    ))
                }
                "char_length" | "character_length" if Args.len() == 1 => {
                    Ok(relational_expression_value(&Args[0], row)?
                        .map(|value| value.chars().count().to_string()))
                }
                "space" if Args.len() == 1 => {
                    let Some(length) = relational_expression_value(&Args[0], row)? else {
                        return Ok(None);
                    };
                    let length = length
                        .parse::<usize>()
                        .map_err(|error| session_error("parse SPACE length", error))?;
                    Ok(Some(" ".repeat(length)))
                }
                "strcmp" if Args.len() == 2 => {
                    let left = relational_expression_value(&Args[0], row)?;
                    let right = relational_expression_value(&Args[1], row)?;
                    Ok(left
                        .as_deref()
                        .zip(right.as_deref())
                        .map(|(left, right)| match left.cmp(right) {
                            std::cmp::Ordering::Less => "-1",
                            std::cmp::Ordering::Equal => "0",
                            std::cmp::Ordering::Greater => "1",
                        })
                        .map(str::to_owned))
                }
                "abs" if Args.len() == 1 => Ok(relational_numeric_value(&Args[0], row)?
                    .map(|value| {
                        value
                            .checked_abs()
                            .ok_or_else(|| SessionError::new("BIGINT value is out of range"))
                            .map(|value| value.to_string())
                    })
                    .transpose()?),
                "round" if matches!(Args.len(), 1 | 2) => {
                    let Some(value) = relational_expression_value(&Args[0], row)? else {
                        return Ok(None);
                    };
                    let value = value
                        .parse::<f64>()
                        .map_err(|error| session_error("parse ROUND argument", error))?;
                    let digits = if Args.len() == 2 {
                        let Some(digits) = relational_expression_value(&Args[1], row)? else {
                            return Ok(None);
                        };
                        digits
                            .parse::<i32>()
                            .map_err(|error| session_error("parse ROUND precision", error))?
                    } else {
                        0
                    };
                    let scale = 10_f64.powi(digits);
                    let rounded = if digits >= 0 {
                        (value * scale).round() / scale
                    } else {
                        (value / scale.recip()).round() * scale.recip()
                    };
                    Ok(Some(rounded.to_string()))
                }
                "truncate" if Args.len() == 2 => {
                    let Some(value) = relational_expression_value(&Args[0], row)? else {
                        return Ok(None);
                    };
                    let Some(digits) = relational_expression_value(&Args[1], row)? else {
                        return Ok(None);
                    };
                    let value = value
                        .parse::<f64>()
                        .map_err(|error| session_error("parse TRUNCATE argument", error))?;
                    let digits = digits
                        .parse::<i32>()
                        .map_err(|error| session_error("parse TRUNCATE precision", error))?;
                    let scale = 10_f64.powi(digits);
                    let truncated = if digits >= 0 {
                        (value * scale).trunc() / scale
                    } else {
                        (value / scale.recip()).trunc() * scale.recip()
                    };
                    Ok(Some(truncated.to_string()))
                }
                "atan2" if Args.len() == 2 => {
                    let Some(left) = relational_expression_value(&Args[0], row)? else {
                        return Ok(None);
                    };
                    let Some(right) = relational_expression_value(&Args[1], row)? else {
                        return Ok(None);
                    };
                    let left = left
                        .parse::<f64>()
                        .map_err(|error| session_error("parse ATAN2 left argument", error))?;
                    let mut right = right
                        .parse::<f64>()
                        .map_err(|error| session_error("parse ATAN2 right argument", error))?;
                    if right == 0.0
                        && matches!(
                            Args[1].Kind,
                            ast::ExprKind::Unary { ref Op, .. } if Op == "-"
                        )
                    {
                        right = -0.0;
                    }
                    Ok(relational_float_string(left.atan2(right)))
                }
                "mid" | "substr" | "substring" if (2..=3).contains(&Args.len()) => {
                    let Some(value) = relational_expression_value(&Args[0], row)? else {
                        return Ok(None);
                    };
                    let start = relational_expression_value(&Args[1], row)?
                        .and_then(|start| start.parse::<i64>().ok())
                        .ok_or_else(|| SessionError::new("substring start must be an integer"))?;
                    let chars = value.chars().collect::<Vec<_>>();
                    let offset = if start > 0 {
                        usize::try_from(start - 1).unwrap_or(usize::MAX)
                    } else if start < 0 {
                        let distance = usize::try_from(start.unsigned_abs()).unwrap_or(usize::MAX);
                        let Some(offset) = chars.len().checked_sub(distance) else {
                            return Ok(Some(String::new()));
                        };
                        offset
                    } else {
                        return Ok(Some(String::new()));
                    };
                    let length = if Args.len() == 3 {
                        let length = relational_expression_value(&Args[2], row)?
                            .and_then(|length| length.parse::<i64>().ok())
                            .ok_or_else(|| {
                                SessionError::new("substring length must be an integer")
                            })?;
                        if length <= 0 {
                            return Ok(Some(String::new()));
                        }
                        usize::try_from(length).unwrap_or(usize::MAX)
                    } else {
                        chars.len().saturating_sub(offset)
                    };
                    Ok(Some(
                        chars
                            .into_iter()
                            .skip(offset)
                            .take(length)
                            .collect::<String>(),
                    ))
                }
                "isnull" if Args.len() == 1 => Ok(Some(
                    u8::from(relational_expression_value(&Args[0], row)?.is_none()).to_string(),
                )),
                "find_in_set" if Args.len() == 2 => {
                    let Some(needle) = relational_expression_value(&Args[0], row)? else {
                        return Ok(None);
                    };
                    let Some(list) = relational_expression_value(&Args[1], row)? else {
                        return Ok(None);
                    };
                    if needle.contains(',') {
                        return Ok(Some("0".to_owned()));
                    }
                    Ok(Some(
                        list.split(',')
                            .position(|candidate| candidate == needle)
                            .map_or(0, |index| index + 1)
                            .to_string(),
                    ))
                }
                "space" if Args.len() == 1 => {
                    let Some(count) = relational_expression_value(&Args[0], row)? else {
                        return Ok(None);
                    };
                    let count = count
                        .parse::<i64>()
                        .map_err(|error| session_error("parse SPACE count", error))?;
                    Ok(Some(" ".repeat(count.max(0) as usize)))
                }
                "vec_from_text" if Args.len() == 1 => relational_expression_value(&Args[0], row)?
                    .map(|value| {
                        astersql_types::vector::ParseVectorFloat32(&value)
                            .map(|vector| vector.String())
                            .map_err(|error| session_error("parse VEC_FROM_TEXT argument", error))
                    })
                    .transpose(),
                "vec_dims" if Args.len() == 1 => relational_expression_value(&Args[0], row)?
                    .map(|value| {
                        astersql_types::vector::ParseVectorFloat32(&value)
                            .map(|vector| vector.Len().to_string())
                            .map_err(|error| session_error("parse VEC_DIMS argument", error))
                    })
                    .transpose(),
                "vec_l1_distance"
                | "vec_l2_distance"
                | "vec_negative_inner_product"
                | "vec_cosine_distance"
                    if Args.len() == 2 =>
                {
                    let Some(left) = relational_expression_value(&Args[0], row)? else {
                        return Ok(None);
                    };
                    let Some(right) = relational_expression_value(&Args[1], row)? else {
                        return Ok(None);
                    };
                    let left = astersql_types::vector::ParseVectorFloat32(&left)
                        .map_err(|error| session_error("parse left vector argument", error))?;
                    let right = astersql_types::vector::ParseVectorFloat32(&right)
                        .map_err(|error| session_error("parse right vector argument", error))?;
                    let result = match name.as_str() {
                        "vec_l1_distance" => left.L1Distance(&right),
                        "vec_l2_distance" => left.L2Distance(&right),
                        "vec_negative_inner_product" => left.NegativeInnerProduct(&right),
                        "vec_cosine_distance" => left.CosineDistance(&right),
                        _ => unreachable!("vector distance function checked above"),
                    };
                    result
                        .map(relational_float_string)
                        .map_err(|error| session_error("compute vector cosine distance", error))
                }
                "vec_l2_norm" if Args.len() == 1 => {
                    let Some(value) = relational_expression_value(&Args[0], row)? else {
                        return Ok(None);
                    };
                    let vector = astersql_types::vector::ParseVectorFloat32(&value)
                        .map_err(|error| session_error("parse VEC_L2_NORM argument", error))?;
                    Ok(relational_float_string(vector.L2Norm()))
                }
                "adddate" | "date_add" | "subdate" | "date_sub" if Args.len() == 3 => {
                    // The parser expands ADDDATE(date, days) to the same three
                    // arguments as DATE_ADD(date, INTERVAL days DAY). Reuse the
                    // canonical temporal factory for casts, calendar arithmetic,
                    // NULLs, and overflow instead of approximating elapsed days.
                    use astersql_expression::BuildContext;
                    let values = Args
                        .iter()
                        .map(|arg| match &arg.Kind {
                            ast::ExprKind::TimeUnit(unit) => {
                                Ok(Some(format!("{unit:?}").to_ascii_uppercase()))
                            }
                            _ => relational_expression_value(arg, row),
                        })
                        .collect::<SessionResult<Vec<_>>>()?;
                    if values.iter().any(Option::is_none) {
                        return Ok(None);
                    }
                    let values = values.into_iter().map(Option::unwrap).collect::<Vec<_>>();
                    let context = astersql_expression_exprstatic::NewExprContext(Vec::new());
                    let arguments = values
                        .iter()
                        .map(|value| {
                            Box::new(astersql_expression::NewStrConst(value))
                                as Box<dyn astersql_expression::Expression>
                        })
                        .collect();
                    let function = astersql_expression::NewFunctionBase(
                        &context,
                        &name,
                        *astersql_expression::types::NewFieldType(
                            astersql_expression::mysql::TypeUnspecified,
                        ),
                        arguments,
                    )
                    .map_err(|error| session_error("build date arithmetic", error))?;
                    let (mut value, null) = function
                        .EvalTime(
                            context.GetEvalCtx(),
                            astersql_expression::chunk::Row::default(),
                        )
                        .map_err(|error| session_error("evaluate date arithmetic", error))?;
                    if null {
                        return Ok(None);
                    }
                    if !values[0].contains(':')
                        && matches!(
                            values[2].to_ascii_uppercase().as_str(),
                            "DAY" | "WEEK" | "MONTH" | "QUARTER" | "YEAR"
                        )
                    {
                        value.SetType(astersql_expression::mysql::TypeDate);
                    }
                    Ok(Some(value.String()))
                }
                "sleep" if Args.len() == 1 => {
                    let Some(seconds) = relational_expression_value(&Args[0], row)? else {
                        return Ok(None);
                    };
                    let seconds = seconds
                        .parse::<f64>()
                        .map_err(|error| session_error("parse SLEEP duration", error))?;
                    if !seconds.is_finite() || seconds < 0.0 {
                        return Err(SessionError::new("SLEEP duration must be nonnegative"));
                    }
                    std::thread::sleep(std::time::Duration::from_secs_f64(seconds));
                    Ok(Some("0".to_owned()))
                }
                _ => Err(SessionError::new(format!(
                    "unsupported scalar function {}",
                    FnName.O
                ))),
            }
        }
        _ => Err(SessionError::new(
            "unsupported relational scalar expression",
        )),
    }
}

/// 按 ORDER BY 对关系行排序。
pub(super) fn sort_relational_rows(
    rows: &mut [RelationalRow],
    order_by: &[ast::ByItem],
) -> SessionResult<()> {
    let expressions = order_by
        .iter()
        .map(|item| (item.Expr.clone(), item.Desc))
        .collect::<Vec<_>>();
    let mut decorated = rows
        .iter()
        .cloned()
        .map(|row| {
            let keys = expressions
                .iter()
                .map(|(expression, _)| relational_expression_value(expression, &row.1))
                .collect::<SessionResult<Vec<_>>>()?;
            Ok((row, keys))
        })
        .collect::<SessionResult<Vec<_>>>()?;
    decorated.sort_by(|left, right| {
        for (index, (_, desc)) in expressions.iter().enumerate() {
            let left_value = left.1[index].as_deref();
            let right_value = right.1[index].as_deref();
            let ordering = match (left_value, right_value) {
                (Some(left), Some(right)) => relational_compare(left, right),
                (None, Some(_)) => std::cmp::Ordering::Less,
                (Some(_), None) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            };
            if ordering != std::cmp::Ordering::Equal {
                return if *desc { ordering.reverse() } else { ordering };
            }
        }
        std::cmp::Ordering::Equal
    });
    for (output, (row, _)) in rows.iter_mut().zip(decorated) {
        *output = row;
    }
    Ok(())
}

fn select_projection_expressions(
    fields: &ast::FieldList,
    wildcard_columns: &[String],
) -> Vec<ast::ExprNode> {
    let mut expressions = Vec::new();
    for field in &fields.Fields {
        if field.WildCard.is_some() {
            expressions.extend(wildcard_columns.iter().map(|column| ast::ExprNode {
                node_text: Default::default(),
                Kind: ast::ExprKind::Column(ast::ColumnName {
                    Schema: ast::CIStr::default(),
                    Table: ast::CIStr::default(),
                    Name: ast::NewCIStr(column),
                }),
                OriginTextPosition: 0,
                Flag: Default::default(),
            }));
        } else if let Some(expression) = field.Expr.as_ref() {
            expressions.push(expression.clone());
        }
    }
    expressions
}

fn positional_index(value: &ast::ValueExpr) -> Option<usize> {
    match &value.Datum {
        ast::ValueDatum::Int64(_) | ast::ValueDatum::Uint64(_) => {
            value.text().parse::<usize>().ok()
        }
        _ => None,
    }
}

fn aggregate_label(expression: &ast::ExprNode) -> Option<String> {
    let ast::ExprKind::AggregateFunction { Name, Args, .. } = &expression.Kind else {
        return None;
    };
    let argument = Args.first().map_or("?", |argument| match &argument.Kind {
        ast::ExprKind::Column(column) => column.Name.L.as_str(),
        _ => "?",
    });
    Some(format!("{}({argument})", Name.to_ascii_lowercase()))
}

pub(super) fn resolve_select_order_by(
    order_by: &[ast::ByItem],
    fields: &ast::FieldList,
    wildcard_columns: &[String],
) -> SessionResult<Vec<ast::ByItem>> {
    let projections = select_projection_expressions(fields, wildcard_columns);
    order_by
        .iter()
        .map(|item| {
            let expression = match &item.Expr.Kind {
                ast::ExprKind::Column(column) => {
                    let aliased = fields.Fields.iter().find_map(|field| {
                        (field.AsName.L == column.Name.L)
                            .then(|| field.Expr.clone())
                            .flatten()
                    });
                    aliased.or_else(|| {
                        let mut matching = fields.Fields.iter().filter_map(|field| {
                            let expression = field.Expr.as_ref()?;
                            matches!(
                                &expression.Kind,
                                ast::ExprKind::Column(projected)
                                    if projected.Name.L == column.Name.L
                            )
                            .then(|| expression.clone())
                        });
                        let first = matching.next()?;
                        matching.next().is_none().then_some(first)
                    })
                }
                ast::ExprKind::Value(value) => {
                    let Some(position) = positional_index(value) else {
                        return Ok(item.clone());
                    };
                    let index = position.checked_sub(1).ok_or_else(|| {
                        SessionError::new("[planner:1054]Unknown column '?' in 'order clause'")
                    })?;
                    Some(projections.get(index).cloned().ok_or_else(|| {
                        SessionError::new("[planner:1054]Unknown column '?' in 'order clause'")
                    })?)
                }
                _ => None,
            };
            Ok(expression.map_or_else(
                || item.clone(),
                |expression| ast::ByItem {
                    Expr: expression,
                    Desc: item.Desc,
                },
            ))
        })
        .collect()
}

pub(super) fn resolve_select_group_by(
    group_by: &[ast::ByItem],
    fields: &ast::FieldList,
    wildcard_columns: &[String],
) -> SessionResult<Vec<ast::ByItem>> {
    let projections = select_projection_expressions(fields, wildcard_columns);
    group_by
        .iter()
        .map(|item| {
            let ast::ExprKind::Value(value) = &item.Expr.Kind else {
                return Ok(item.clone());
            };
            let Some(position) = positional_index(value) else {
                return Ok(item.clone());
            };
            let expression = position
                .checked_sub(1)
                .and_then(|index| projections.get(index))
                .cloned()
                .ok_or_else(|| {
                    SessionError::new("[planner:1054]Unknown column '?' in 'group statement'")
                })?;
            if let Some(label) = aggregate_label(&expression) {
                return Err(SessionError::new(format!(
                    "[planner:1056]Can't group on '{label}'"
                )));
            }
            Ok(ast::ByItem {
                Expr: expression,
                Desc: item.Desc,
            })
        })
        .collect()
}

pub(super) fn expression_contains_function(expression: &ast::ExprNode, target: &str) -> bool {
    match &expression.Kind {
        ast::ExprKind::Function { FnName, Args, .. } => {
            FnName.L.eq_ignore_ascii_case(target)
                || Args
                    .iter()
                    .any(|argument| expression_contains_function(argument, target))
        }
        ast::ExprKind::Binary { L, R, .. } => {
            expression_contains_function(L, target) || expression_contains_function(R, target)
        }
        ast::ExprKind::Unary { V, .. }
        | ast::ExprKind::Parentheses(V)
        | ast::ExprKind::Cast { Expr: V, .. }
        | ast::ExprKind::Collate { Expr: V, .. } => expression_contains_function(V, target),
        _ => false,
    }
}

pub(super) fn expression_contains_cast(expression: &ast::ExprNode) -> bool {
    match &expression.Kind {
        ast::ExprKind::Cast { .. } => true,
        ast::ExprKind::Function { Args, .. } => Args.iter().any(expression_contains_cast),
        ast::ExprKind::Binary { L, R, .. } => {
            expression_contains_cast(L) || expression_contains_cast(R)
        }
        ast::ExprKind::Unary { V, .. }
        | ast::ExprKind::Parentheses(V)
        | ast::ExprKind::Collate { Expr: V, .. } => expression_contains_cast(V),
        _ => false,
    }
}

pub(super) fn relational_expression_type(
    expression: &ast::ExprNode,
    table: &astersql_meta_model::TableInfo,
) -> astersql_parser_types::types::FieldType {
    let new_type = |tp| {
        let mut field_type = astersql_parser_types::NewFieldType(tp);
        if matches!(
            tp,
            astersql_parser_mysql::r#type::TypeString
                | astersql_parser_mysql::r#type::TypeVarchar
                | astersql_parser_mysql::r#type::TypeVarString
        ) {
            field_type.SetCharset("utf8mb4".to_owned());
            field_type.SetCollate("utf8mb4_bin".to_owned());
        } else {
            field_type.SetCharset("binary".to_owned());
            field_type.SetCollate("binary".to_owned());
            field_type.AddFlag(astersql_parser_mysql::r#type::BinaryFlag);
        }
        field_type
    };
    match &expression.Kind {
        ast::ExprKind::Value(value) => value.Type.clone(),
        ast::ExprKind::Column(column) => table
            .Columns
            .iter()
            .find(|candidate| candidate.Name.L == column.Name.L)
            .map(|column| column.FieldType.clone())
            .unwrap_or_else(|| new_type(astersql_parser_mysql::r#type::TypeVarString)),
        ast::ExprKind::Parentheses(inner) | ast::ExprKind::Collate { Expr: inner, .. } => {
            relational_expression_type(inner, table)
        }
        ast::ExprKind::Unary { Op, V } if Op == "+" || Op == "-" => {
            relational_expression_type(V, table)
        }
        ast::ExprKind::Unary { Op, .. } if Op.eq_ignore_ascii_case("not") || Op == "!" => {
            new_type(astersql_parser_mysql::r#type::TypeLonglong)
        }
        ast::ExprKind::Binary { Op, .. }
            if matches!(
                Op.to_ascii_lowercase().as_str(),
                "=" | "==" | "!=" | "<>" | "<=>" | ">" | ">=" | "<" | "<=" | "and" | "or"
            ) =>
        {
            new_type(astersql_parser_mysql::r#type::TypeLonglong)
        }
        ast::ExprKind::Binary { Op, .. } if Op == "/" => {
            new_type(astersql_parser_mysql::r#type::TypeNewDecimal)
        }
        ast::ExprKind::Binary { .. }
        | ast::ExprKind::IsNull { .. }
        | ast::ExprKind::IsTruth { .. }
        | ast::ExprKind::InList { .. }
        | ast::ExprKind::Between { .. }
        | ast::ExprKind::Like { .. } => new_type(astersql_parser_mysql::r#type::TypeLonglong),
        ast::ExprKind::Case {
            WhenClauses,
            ElseClause,
            ..
        } => {
            let mut types = WhenClauses
                .iter()
                .map(|clause| relational_expression_type(&clause.Result, table))
                .collect::<Vec<_>>();
            if let Some(else_clause) = ElseClause {
                types.push(relational_expression_type(else_clause, table));
            }
            let first_type = types
                .first()
                .map(|field_type| field_type.GetType())
                .unwrap_or(astersql_parser_mysql::r#type::TypeNull);
            if types
                .iter()
                .all(|field_type| field_type.GetType() == first_type)
            {
                types
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| new_type(astersql_parser_mysql::r#type::TypeNull))
            } else if types.iter().any(|field_type| {
                matches!(
                    field_type.GetType(),
                    astersql_parser_mysql::r#type::TypeString
                        | astersql_parser_mysql::r#type::TypeVarchar
                )
            }) {
                new_type(astersql_parser_mysql::r#type::TypeVarchar)
            } else if types.iter().any(|field_type| {
                field_type.GetType() == astersql_parser_mysql::r#type::TypeVarString
            }) {
                new_type(astersql_parser_mysql::r#type::TypeVarString)
            } else {
                new_type(astersql_parser_mysql::r#type::TypeLonglong)
            }
        }
        ast::ExprKind::Function { FnName, Args, .. } => match FnName.L.as_str() {
            "coalesce" | "ifnull" | "nullif" => Args
                .iter()
                .map(|argument| relational_expression_type(argument, table))
                .find(|field_type| field_type.GetType() != astersql_parser_mysql::r#type::TypeNull)
                .unwrap_or_else(|| new_type(astersql_parser_mysql::r#type::TypeNull)),
            "upper" | "ucase" | "lower" | "lcase" | "concat" | "lpad" => {
                new_type(astersql_parser_mysql::r#type::TypeVarString)
            }
            "abs" | "length" | "octet_length" | "char_length" | "character_length" => {
                new_type(astersql_parser_mysql::r#type::TypeLonglong)
            }
            _ => new_type(astersql_parser_mysql::r#type::TypeVarString),
        },
        ast::ExprKind::WindowFunction { Name, .. }
            if matches!(
                Name.to_ascii_lowercase().as_str(),
                "min_count" | "max_count"
            ) =>
        {
            new_type(astersql_parser_mysql::r#type::TypeLonglong)
        }
        ast::ExprKind::AggregateFunction { Name, Args, .. } => {
            match Name.to_ascii_lowercase().as_str() {
                "count" | "min_count" | "max_count" | "bit_xor" => {
                    new_type(astersql_parser_mysql::r#type::TypeLonglong)
                }
                "sum" | "avg" => new_type(astersql_parser_mysql::r#type::TypeNewDecimal),
                "min" | "max" => Args
                    .first()
                    .map(|argument| relational_expression_type(argument, table))
                    .unwrap_or_else(|| new_type(astersql_parser_mysql::r#type::TypeVarString)),
                _ => new_type(astersql_parser_mysql::r#type::TypeVarString),
            }
        }
        _ => new_type(astersql_parser_mysql::r#type::TypeVarString),
    }
}

pub(super) fn relational_expression_result_field(
    header: &str,
    expression: &ast::ExprNode,
    table: &astersql_meta_model::TableInfo,
) -> ConcreteResultField {
    ConcreteResultField {
        column: astersql_meta_model::ColumnInfo {
            Name: ast::NewCIStr(""),
            FieldType: relational_expression_type(expression, table),
            ..Default::default()
        },
        column_as_name: ast::NewCIStr(header),
        table_name: ast::CIStr::default(),
        table_as_name: ast::CIStr::default(),
        db_name: ast::CIStr::default(),
    }
}

pub(crate) fn relational_window_value<F>(
    expression: &ast::ExprNode,
    rows: &[RelationalRow],
    row_index: usize,
    named_specs: &[ast::WindowSpec],
    evaluate: F,
) -> SessionResult<Option<String>>
where
    F: Fn(&ast::ExprNode, &HashMap<String, Option<String>>) -> SessionResult<Option<String>>,
{
    let ast::ExprKind::WindowFunction {
        Name, Args, Spec, ..
    } = &expression.Kind
    else {
        return Err(SessionError::new("expected a window expression"));
    };
    let spec = if !Spec.Name.L.is_empty() {
        named_specs
            .iter()
            .find(|candidate| candidate.Name.L == Spec.Name.L)
            .unwrap_or(Spec)
    } else if !Spec.Ref.L.is_empty() {
        named_specs
            .iter()
            .find(|candidate| candidate.Name.L == Spec.Ref.L)
            .unwrap_or(Spec)
    } else {
        Spec
    };
    let partition_key = |index: usize| -> SessionResult<Vec<Option<String>>> {
        spec.PartitionBy
            .iter()
            .map(|item| evaluate(&item.Expr, &rows[index].1))
            .collect()
    };
    let current_partition = partition_key(row_index)?;
    let partition = (0..rows.len())
        .map(|index| partition_key(index).map(|key| (key == current_partition).then_some(index)))
        .collect::<SessionResult<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    let position = partition
        .iter()
        .position(|index| *index == row_index)
        .expect("current row belongs to its partition");
    let argument = |index: usize, offset: usize| -> SessionResult<Option<String>> {
        Args.get(offset)
            .map(|argument| evaluate(argument, &rows[index].1))
            .unwrap_or(Ok(None))
    };
    let order_key = |index: usize| -> SessionResult<Vec<Option<String>>> {
        spec.OrderBy
            .iter()
            .map(|item| evaluate(&item.Expr, &rows[index].1))
            .collect()
    };
    let bound_offset = |bound: &ast::FrameBound| -> SessionResult<usize> {
        let Some(expression) = bound.Expr.as_ref() else {
            return Ok(0);
        };
        evaluate(expression, &rows[row_index].1)?
            .ok_or_else(|| SessionError::new("window frame offset cannot be NULL"))?
            .parse::<usize>()
            .map_err(|error| session_error("parse ROWS window frame offset", error))
    };
    let (frame_start, frame_end) = if let Some(frame) = spec
        .Frame
        .as_ref()
        .filter(|frame| frame.Type == ast::FrameType::Rows)
    {
        let start = match frame.Extent.Start.Type {
            ast::BoundType::Preceding if frame.Extent.Start.UnBounded => 0,
            ast::BoundType::Preceding => {
                position.saturating_sub(bound_offset(&frame.Extent.Start)?)
            }
            ast::BoundType::CurrentRow => position,
            ast::BoundType::Following if frame.Extent.Start.UnBounded => partition.len(),
            ast::BoundType::Following => position
                .saturating_add(bound_offset(&frame.Extent.Start)?)
                .min(partition.len()),
        };
        let end = match frame.Extent.End.Type {
            ast::BoundType::Preceding if frame.Extent.End.UnBounded => 0,
            ast::BoundType::Preceding => {
                let offset = bound_offset(&frame.Extent.End)?;
                if offset > position {
                    0
                } else {
                    position - offset + 1
                }
            }
            ast::BoundType::CurrentRow => position + 1,
            ast::BoundType::Following if frame.Extent.End.UnBounded => partition.len(),
            ast::BoundType::Following => position
                .saturating_add(bound_offset(&frame.Extent.End)?)
                .saturating_add(1)
                .min(partition.len()),
        };
        (start, end)
    } else if spec.OrderBy.is_empty() {
        (0, partition.len())
    } else {
        (0, position + 1)
    };
    let frame = if frame_start < frame_end {
        &partition[frame_start..frame_end]
    } else {
        &partition[0..0]
    };
    match Name.to_ascii_lowercase().as_str() {
        "row_number" => Ok(Some((position + 1).to_string())),
        "rank" | "dense_rank" => {
            let current = order_key(row_index)?;
            let prior = partition[..position]
                .iter()
                .map(|index| order_key(*index))
                .collect::<SessionResult<Vec<_>>>()?;
            let rank = if Name.eq_ignore_ascii_case("rank") {
                prior
                    .iter()
                    .position(|key| key == &current)
                    .map_or(position + 1, |first| first + 1)
            } else {
                let mut distinct = Vec::new();
                for key in prior {
                    if !distinct.contains(&key) {
                        distinct.push(key);
                    }
                }
                distinct.len() + usize::from(!distinct.contains(&current))
            };
            Ok(Some(rank.to_string()))
        }
        "sum" => {
            let mut total = rust_decimal::Decimal::new(0, 0);
            let mut has_value = false;
            for candidate in frame {
                if let Some(value) = argument(*candidate, 0)? {
                    let value = value
                        .parse::<rust_decimal::Decimal>()
                        .map_err(|error| session_error("parse SUM window value", error))?;
                    total = total
                        .checked_add(value)
                        .ok_or_else(|| SessionError::new("DECIMAL value is out of range"))?;
                    has_value = true;
                }
            }
            Ok(has_value.then(|| total.normalize().to_string()))
        }
        "count" => {
            let mut count = 0_usize;
            for candidate in frame {
                if Args.is_empty() || argument(*candidate, 0)?.is_some() {
                    count += 1;
                }
            }
            Ok(Some(count.to_string()))
        }
        "min_count" | "max_count" => {
            let values = frame
                .iter()
                .map(|candidate| argument(*candidate, 0))
                .collect::<SessionResult<Vec<_>>>()?;
            Ok(Some(
                relational_count_extrema(
                    values.into_iter().flatten(),
                    Name.eq_ignore_ascii_case("max_count"),
                )
                .to_string(),
            ))
        }
        "min" | "max" => {
            let mut selected: Option<String> = None;
            for candidate in frame {
                let Some(value) = argument(*candidate, 0)? else {
                    continue;
                };
                if selected.as_ref().is_none_or(|current| {
                    let ordering = relational_compare(&value, current);
                    (Name.eq_ignore_ascii_case("min") && ordering.is_lt())
                        || (Name.eq_ignore_ascii_case("max") && ordering.is_gt())
                }) {
                    selected = Some(value);
                }
            }
            Ok(selected)
        }
        "first_value" => frame.first().map_or(Ok(None), |index| argument(*index, 0)),
        "last_value" => frame.last().map_or(Ok(None), |index| argument(*index, 0)),
        "nth_value" => {
            let nth = Args
                .get(1)
                .and_then(|argument| evaluate(argument, &rows[row_index].1).ok())
                .flatten()
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(0);
            if nth == 0 || nth > frame.len() {
                Ok(None)
            } else {
                argument(frame[nth - 1], 0)
            }
        }
        "lag" => {
            if position == 0 {
                Ok(None)
            } else {
                argument(partition[position - 1], 0)
            }
        }
        "lead" => {
            if position + 1 == partition.len() {
                Ok(None)
            } else {
                argument(partition[position + 1], 0)
            }
        }
        "var_pop" => {
            let mut values = Vec::new();
            for candidate in frame {
                if let Some(value) = argument(*candidate, 0)? {
                    values.push(
                        value
                            .parse::<f64>()
                            .map_err(|error| session_error("parse VAR_POP value", error))?,
                    );
                }
            }
            if values.is_empty() {
                return Ok(None);
            }
            let mean = values.iter().sum::<f64>() / values.len() as f64;
            let variance = values
                .iter()
                .map(|value| (value - mean).powi(2))
                .sum::<f64>()
                / values.len() as f64;
            Ok(relational_float_string(variance))
        }
        _ => Err(SessionError::new(format!(
            "unsupported relational window function {Name}"
        ))),
    }
}

/// 按投影列表输出关系行。
pub(super) fn relational_expression_value_with_embed(
    expression: &ast::ExprNode,
    row: &HashMap<String, Option<String>>,
    embed_text: &dyn Fn(
        &[ast::ExprNode],
        &HashMap<String, Option<String>>,
    ) -> SessionResult<String>,
) -> SessionResult<Option<String>> {
    struct EmbedFinder(bool);
    impl ast::ExprNodeVisitor for EmbedFinder {
        fn Enter(&mut self, input: &ast::ExprNode) -> (ast::ExprNode, bool) {
            if let ast::ExprKind::Function { FnName, .. } = &input.Kind
                && FnName.L.eq_ignore_ascii_case("embed_text")
            {
                self.0 = true;
                return (input.clone(), true);
            }
            (input.clone(), false)
        }
        fn Leave(&mut self, input: &ast::ExprNode) -> (ast::ExprNode, bool) {
            (input.clone(), !self.0)
        }
    }
    let mut finder = EmbedFinder(false);
    let _ = expression.Accept(&mut finder);
    if !finder.0 {
        return relational_expression_value(expression, row);
    }

    struct EmbedVisitor<'a> {
        row: &'a HashMap<String, Option<String>>,
        embed_text:
            &'a dyn Fn(&[ast::ExprNode], &HashMap<String, Option<String>>) -> SessionResult<String>,
        error: Option<SessionError>,
    }
    impl ast::ExprNodeVisitor for EmbedVisitor<'_> {
        fn Enter(&mut self, input: &ast::ExprNode) -> (ast::ExprNode, bool) {
            if let ast::ExprKind::Function { FnName, Args, .. } = &input.Kind
                && FnName.L.eq_ignore_ascii_case("if")
                && Args.len() == 3
            {
                match relational_expression_value_with_embed(&Args[0], self.row, self.embed_text) {
                    Ok(condition) => {
                        let selected = if relational_truth(condition.as_deref()) == Some(true) {
                            &Args[1]
                        } else {
                            &Args[2]
                        };
                        return (selected.clone(), false);
                    }
                    Err(error) => {
                        self.error = Some(error);
                        return (input.clone(), true);
                    }
                }
            }
            (input.clone(), false)
        }

        fn Leave(&mut self, input: &ast::ExprNode) -> (ast::ExprNode, bool) {
            if let ast::ExprKind::Function { FnName, Args, .. } = &input.Kind
                && FnName.L.eq_ignore_ascii_case("embed_text")
            {
                return match (self.embed_text)(Args, self.row) {
                    Ok(value) if value == CONCRETE_NULL_VALUE => (ast::ExprNode::NullValue(), true),
                    Ok(value) => (ast::ExprNode::Value(value), true),
                    Err(error) => {
                        self.error = Some(error);
                        (input.clone(), false)
                    }
                };
            }
            (input.clone(), true)
        }
    }
    let mut visitor = EmbedVisitor {
        row,
        embed_text,
        error: None,
    };
    let (rewritten, _) = expression.Accept(&mut visitor);
    if let Some(error) = visitor.error {
        return Err(error);
    }
    relational_expression_value(&rewritten, row)
}

pub(super) fn project_relational_rows(
    database: &str,
    table_alias: &ast::CIStr,
    table: &astersql_meta_model::TableInfo,
    rows: &[RelationalRow],
    fields: &ast::FieldList,
    window_specs: &[ast::WindowSpec],
    embed_text: impl Fn(&[ast::ExprNode], &HashMap<String, Option<String>>) -> SessionResult<String>,
) -> SessionResult<(
    Vec<String>,
    Vec<Vec<String>>,
    Vec<Option<ConcreteResultField>>,
)> {
    enum Projection {
        Column(String, bool),
        Expression(ast::ExprNode),
        Window(ast::ExprNode),
    }
    let mut columns = Vec::new();
    let mut projections = Vec::new();
    let mut result_fields = Vec::new();
    let table_columns = table
        .Columns
        .iter()
        .map(|column| column.Name.L.as_str())
        .collect::<HashSet<_>>();
    for field in &fields.Fields {
        if let Some(wildcard) = field.WildCard.as_ref() {
            let targets_base_table = wildcard.Table.L.is_empty()
                || wildcard.Table.L == table_alias.L
                || wildcard.Table.L == table.Name.L;
            for column in table.Columns.iter().filter(|column| {
                if column.Hidden {
                    return false;
                }
                if wildcard.Table.L.is_empty() {
                    return true;
                }
                if targets_base_table {
                    !column.Name.L.contains('.')
                } else {
                    column
                        .Name
                        .L
                        .strip_prefix(&wildcard.Table.L)
                        .is_some_and(|suffix| suffix.starts_with('.'))
                }
            }) {
                let output_name = column
                    .Name
                    .O
                    .rsplit_once('.')
                    .map_or(column.Name.O.as_str(), |(_, name)| name);
                columns.push(output_name.to_owned());
                projections.push(Projection::Column(
                    column.Name.L.clone(),
                    column.GetType() == astersql_parser_mysql::r#type::TypeBit,
                ));
                result_fields.push(Some(ConcreteResultField {
                    column: column.clone(),
                    column_as_name: ast::NewCIStr(output_name),
                    table_name: table.Name.clone(),
                    table_as_name: table_alias.clone(),
                    db_name: ast::NewCIStr(database),
                }));
            }
            continue;
        }
        let expr = field
            .Expr
            .as_ref()
            .ok_or_else(|| SessionError::new("projected SELECT field requires an expression"))?;
        match &expr.Kind {
            ast::ExprKind::Column(column) => {
                let hidden_row_id =
                    column.Name.L == "_tidb_rowid" && !table.PKIsHandle && !table.IsCommonHandle;
                if !hidden_row_id && !table_columns.contains(column.Name.L.as_str()) {
                    return Err(SessionError::new(format!(
                        "Unknown column '{}' in 'field list'",
                        column.Name.O
                    )));
                }
                let header = if !field.AsName.O.is_empty() {
                    field.AsName.O.clone()
                } else {
                    column.Name.O.clone()
                };
                columns.push(header.clone());
                let column_info = table
                    .Columns
                    .iter()
                    .find(|candidate| candidate.Name.L == column.Name.L)
                    .cloned()
                    .unwrap_or_else(|| astersql_meta_model::ColumnInfo {
                        Name: column.Name.clone(),
                        FieldType: astersql_parser_types::NewFieldType(
                            astersql_parser_mysql::r#type::TypeLonglong,
                        ),
                        ..astersql_meta_model::ColumnInfo::default()
                    });
                projections.push(Projection::Column(
                    column.Name.L.clone(),
                    column_info.GetType() == astersql_parser_mysql::r#type::TypeBit,
                ));
                result_fields.push(Some(ConcreteResultField {
                    column: column_info,
                    column_as_name: ast::NewCIStr(&header),
                    table_name: table.Name.clone(),
                    table_as_name: table_alias.clone(),
                    db_name: ast::NewCIStr(database),
                }));
            }
            ast::ExprKind::Parentheses(inner) if matches!(inner.Kind, ast::ExprKind::Column(_)) => {
                let ast::ExprKind::Column(column) = &inner.Kind else {
                    unreachable!("parenthesized column checked above")
                };
                let hidden_row_id =
                    column.Name.L == "_tidb_rowid" && !table.PKIsHandle && !table.IsCommonHandle;
                if !hidden_row_id && !table_columns.contains(column.Name.L.as_str()) {
                    return Err(SessionError::new(format!(
                        "Unknown column '{}' in 'field list'",
                        column.Name.O
                    )));
                }
                let header = if field.AsName.O.is_empty() {
                    column.Name.O.clone()
                } else {
                    field.AsName.O.clone()
                };
                columns.push(header.clone());
                let column_info = table
                    .Columns
                    .iter()
                    .find(|candidate| candidate.Name.L == column.Name.L)
                    .cloned()
                    .unwrap_or_else(|| astersql_meta_model::ColumnInfo {
                        Name: column.Name.clone(),
                        FieldType: astersql_parser_types::NewFieldType(
                            astersql_parser_mysql::r#type::TypeLonglong,
                        ),
                        ..astersql_meta_model::ColumnInfo::default()
                    });
                projections.push(Projection::Column(
                    column.Name.L.clone(),
                    column_info.GetType() == astersql_parser_mysql::r#type::TypeBit,
                ));
                result_fields.push(Some(ConcreteResultField {
                    column: column_info,
                    column_as_name: ast::NewCIStr(&header),
                    table_name: table.Name.clone(),
                    table_as_name: table_alias.clone(),
                    db_name: ast::NewCIStr(database),
                }));
            }
            ast::ExprKind::WindowFunction { .. } => {
                let header = if field.AsName.O.is_empty() {
                    "window".to_owned()
                } else {
                    field.AsName.O.clone()
                };
                columns.push(header.clone());
                projections.push(Projection::Window(expr.clone()));
                result_fields.push(Some(relational_expression_result_field(
                    &header, expr, table,
                )));
            }
            _ => {
                let header = if !field.AsName.O.is_empty() {
                    field.AsName.O.clone()
                } else {
                    match &expr.Kind {
                        ast::ExprKind::Value(value) => value.text(),
                        ast::ExprKind::Function { FnName, .. } => {
                            format!("{}(...)", FnName.O)
                        }
                        _ => "expression".to_owned(),
                    }
                };
                columns.push(header.clone());
                projections.push(Projection::Expression(expr.clone()));
                result_fields.push(Some(relational_expression_result_field(
                    &header, expr, table,
                )));
            }
        }
    }
    let projected = rows
        .iter()
        .enumerate()
        .map(|(row_index, (_, row))| {
            projections
                .iter()
                .map(|projection| match projection {
                    Projection::Column(name, is_bit) => Ok(row
                        .get(name)
                        .and_then(Option::as_ref)
                        .cloned()
                        .map(|value| {
                            if *is_bit
                                && let Some(hex) = value
                                    .strip_prefix("0x")
                                    .or_else(|| value.strip_prefix("0X"))
                                && hex.len() % 2 == 0
                                && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
                            {
                                let bytes = hex
                                    .as_bytes()
                                    .chunks(2)
                                    .map(|chunk| {
                                        let high = (chunk[0] as char).to_digit(16).unwrap_or(0);
                                        let low = (chunk[1] as char).to_digit(16).unwrap_or(0);
                                        ((high << 4) | low) as u8
                                    })
                                    .collect::<Vec<_>>();
                                return String::from_utf8_lossy(&bytes).into_owned();
                            }
                            display_runtime_value(value)
                        })
                        .unwrap_or_else(|| SHOW_NULL_CELL.to_owned())),
                    Projection::Expression(expression) => {
                        relational_expression_value_with_embed(expression, row, &embed_text)
                            .map(|value| value.unwrap_or_else(|| SHOW_NULL_CELL.to_owned()))
                    }
                    Projection::Window(expression) => relational_window_value(
                        expression,
                        rows,
                        row_index,
                        window_specs,
                        relational_expression_value,
                    )
                    .map(|value| value.unwrap_or_else(|| SHOW_NULL_CELL.to_owned())),
                })
                .collect::<SessionResult<Vec<_>>>()
        })
        .collect::<SessionResult<Vec<_>>>()?;
    Ok((columns, projected, result_fields))
}

/// Count peers of the selected extrema using the relational evaluator's comparison contract.
pub(super) fn relational_count_extrema(
    values: impl IntoIterator<Item = String>,
    is_max: bool,
) -> i64 {
    let mut selected: Option<String> = None;
    let mut count = 0_i64;
    for value in values {
        let ordering = selected
            .as_ref()
            .map(|current| relational_compare(&value, current));
        if ordering.is_none_or(|ordering| {
            if is_max {
                ordering.is_gt()
            } else {
                ordering.is_lt()
            }
        }) {
            selected = Some(value);
            count = 1;
        } else if ordering.is_some_and(|ordering| ordering.is_eq()) {
            count = count.wrapping_add(1);
        }
    }
    count
}

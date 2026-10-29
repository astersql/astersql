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

//! Relational query expressions, projections, result metadata, and SELECT execution.

use super::*;

/// Identifies the validated signed ABS fold failure without matching error text.
#[derive(Debug)]
pub(super) struct MysqlSignedAbsOverflow(String);

impl std::fmt::Display for MysqlSignedAbsOverflow {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}
impl std::error::Error for MysqlSignedAbsOverflow {}

// Unknown is a property of a literal expression, never of its rendered value.
// A nested set node has already resolved its output and is not unknown again.
pub(super) fn pg_union_unknown_inputs(node: &dyn ast::Node, fields: &mut [ConcreteResultField]) {
    fn unknown(expr: &ast::ExprNode) -> bool {
        match &expr.Kind {
            ast::ExprKind::Value(value) => matches!(
                value.Datum,
                ast::ValueDatum::Null | ast::ValueDatum::String(_)
            ),
            ast::ExprKind::Parentheses(inner) => unknown(inner),
            _ => false,
        }
    }
    if let Some(select) = node.as_any().downcast_ref::<ast::SelectStmt>() {
        for (field, projection) in fields.iter_mut().zip(&select.Fields.Fields) {
            if projection.Expr.as_ref().is_some_and(unknown) {
                field.column.FieldType =
                    astersql_parser_types::NewFieldType(astersql_parser_mysql::r#type::TypeNull);
            }
        }
    }
}

fn pg_union_error(message: &str, state: &'static str) -> SessionError {
    SessionError::with_source(
        message,
        astersql_errors::SharedError::new(super::pg_array::PgArrayError {
            state,
            message: message.into(),
            position: None,
        }),
    )
}

pub(super) fn pg_union_common_type(
    left: &astersql_parser_types::FieldType,
    right: &astersql_parser_types::FieldType,
) -> SessionResult<astersql_parser_types::FieldType> {
    use astersql_parser_mysql::r#type::*;
    let text = || {
        let mut tp = astersql_parser_types::NewFieldType(TypeVarString);
        tp.SetPostgreSQLTextOid(25);
        tp
    };
    let unknown = |tp: &astersql_parser_types::FieldType| tp.GetType() == TypeNull;
    if unknown(left) && unknown(right) {
        return Ok(text());
    }
    if unknown(left) {
        return Ok(right.clone());
    }
    if unknown(right) {
        return Ok(left.clone());
    }
    use astersql_parser_types::PostgreSQLArrayElement as E;
    let numeric = |tp: &astersql_parser_types::FieldType| {
        if let Some(element) = tp.PostgreSQLArrayElement() {
            return [E::Int2, E::Int4, E::Int8, E::Numeric, E::Float4, E::Float8]
                .iter()
                .position(|e| *e == element);
        }
        if tp.GetFlag() & IsBooleanFlag != 0 {
            return None;
        }
        [
            TypeShort,
            TypeLong,
            TypeLonglong,
            TypeNewDecimal,
            TypeFloat,
            TypeDouble,
        ]
        .iter()
        .position(|code| *code == tp.GetType())
    };
    let string = |tp: &astersql_parser_types::FieldType| {
        tp.PostgreSQLTextOid().is_some()
            || matches!(
                tp.PostgreSQLArrayElement(),
                Some(E::Text | E::Varchar | E::Bpchar)
            )
    };
    let datetime = |tp: &astersql_parser_types::FieldType| {
        if let Some(element) = tp.PostgreSQLArrayElement() {
            return [E::Date, E::Timestamp, E::Timestamptz]
                .iter()
                .position(|e| *e == element);
        }
        [TypeDate, TypeDatetime, TypePgTimestampTz]
            .iter()
            .position(|code| *code == tp.GetType())
    };
    let category = |tp: &astersql_parser_types::FieldType| {
        if tp.PostgreSQLArrayElement().is_some() {
            1
        } else if numeric(tp).is_some() {
            2
        } else if string(tp) {
            3
        } else if tp.GetFlag() & IsBooleanFlag != 0 {
            4
        } else if datetime(tp).is_some() || tp.GetType() == TypeDuration {
            5
        } else if tp.GetType() == TypePgInterval {
            6
        } else {
            7
        } // The currently supported bytea/UUID/JSON types are category U.
    };
    if category(left) != category(right) {
        return Err(pg_union_error("UNION types cannot be matched", "42804"));
    }
    // Literal and CAST boolean carriers use different MySQL codes but have
    // the same PostgreSQL identity; flags distinguish them from integers.
    let both_boolean = left.GetFlag() & IsBooleanFlag != 0 && right.GetFlag() & IsBooleanFlag != 0;
    let same = (left.GetType() == right.GetType() || both_boolean)
        && left.PostgreSQLTextOid() == right.PostgreSQLTextOid()
        && left.PostgreSQLArrayElement() == right.PostgreSQLArrayElement()
        && (left.GetFlag() & IsBooleanFlag) == (right.GetFlag() & IsBooleanFlag);
    let mut result = if same {
        left.clone()
    } else if let (Some(l), Some(r)) = (numeric(left), numeric(right)) {
        // Forward numeric casts are implicit; reverse casts are assignment only.
        if l < r { right.clone() } else { left.clone() }
    } else if let (Some(l), Some(r)) = (datetime(left), datetime(right)) {
        if l < r { right.clone() } else { left.clone() }
    } else if string(left) && string(right) {
        // Both directions are implicit, so the left candidate remains.
        // A preferred type only stops an existing candidate search; it does
        // not replace a candidate that can accept the later input.
        left.clone()
    } else {
        return Err(pg_union_error(
            "UNION could not convert type to candidate type",
            "42846",
        ));
    };
    // Retain a typmod only if both inputs have the same type and modifier.
    if !same || left.GetFlen() != right.GetFlen() || left.GetDecimal() != right.GetDecimal() {
        result.SetFlen(-1);
        result.SetDecimal(-1);
    }
    Ok(result)
}

fn projection_original_text(text: &str) -> String {
    let mut remaining = text;
    let mut output = String::with_capacity(text.len());
    while let Some(start) = remaining.find("/*!") {
        output.push_str(&remaining[..start]);
        let comment = &remaining[start + 3..];
        let content_start = comment.bytes().take_while(u8::is_ascii_digit).count();
        let Some(end) = comment.find("*/") else {
            output.push_str(&comment[content_start..]);
            remaining = "";
            break;
        };
        output.push_str(&comment[content_start..end]);
        remaining = &comment[end + 2..];
    }
    output.push_str(remaining);
    if !text.contains("/*!") && output.ends_with("*/") {
        output.truncate(output.len() - 2);
    }
    output
}

fn relational_vector_distance_function(expression: &ast::ExprNode) -> Option<&str> {
    match &expression.Kind {
        ast::ExprKind::Function { FnName, .. }
            if astersql_meta_model::IndexableFnNameToDistanceMetric()
                .contains_key(FnName.L.as_str()) =>
        {
            Some(FnName.L.as_str())
        }
        ast::ExprKind::Parentheses(inner) | ast::ExprKind::Collate { Expr: inner, .. } => {
            relational_vector_distance_function(inner)
        }
        _ => None,
    }
}

/// 关系行：(handle, 列名→可空字符串值)。
pub(super) type RelationalRow = (i64, HashMap<String, Option<String>>);

/// INSERT SELECT 子执行器的有序列与行。
#[derive(Clone, Debug, Default)]
pub(super) struct InsertSelectRows {
    pub(super) columns: Vec<String>,
    pub(super) rows: Vec<HashMap<String, Option<String>>>,
}

const RELATIONAL_STRING_COLUMN_PREFIX: &str = "\0astersql:string-column:";

fn predicate_contains_point_equality(expression: &ast::ExprNode, primary_key: &str) -> bool {
    match &expression.Kind {
        ast::ExprKind::Binary { Op, L, R }
            if (Op == "=" || Op == "==")
                && ((matches!(&L.Kind, ast::ExprKind::Column(column) if column.Name.L == primary_key)
                    && literal(R).is_ok())
                    || (matches!(&R.Kind, ast::ExprKind::Column(column) if column.Name.L == primary_key)
                        && literal(L).is_ok())) =>
        {
            true
        }
        ast::ExprKind::Binary { Op, L, R } if Op.eq_ignore_ascii_case("and") || Op == "&&" => {
            predicate_contains_point_equality(L, primary_key)
                || predicate_contains_point_equality(R, primary_key)
        }
        ast::ExprKind::Parentheses(inner) => predicate_contains_point_equality(inner, primary_key),
        _ => false,
    }
}

fn projected_table_field_types(
    statement: &ast::SelectStmt,
    table: &astersql_meta_model::TableInfo,
) -> Vec<astersql_types::metadata::FieldType> {
    if statement
        .Fields
        .Fields
        .iter()
        .any(|field| field.WildCard.is_some())
    {
        return table
            .Columns
            .iter()
            .filter(|column| !column.Hidden)
            .map(|column| column.FieldType.clone())
            .collect();
    }
    let mut projected = Vec::with_capacity(statement.Fields.Fields.len());
    for field in &statement.Fields.Fields {
        let Some(expression) = field.Expr.as_ref() else {
            return table
                .Columns
                .iter()
                .map(|column| column.FieldType.clone())
                .collect();
        };
        let ast::ExprKind::Column(column_name) = &expression.Kind else {
            // The compact runtime does not expose expression type inference at
            // this boundary. Conservatively retain the table schema just as a
            // reader whose output schema is not proven narrow would do.
            return table
                .Columns
                .iter()
                .map(|column| column.FieldType.clone())
                .collect();
        };
        let Some(column) = table
            .Columns
            .iter()
            .find(|column| column.Name.L == column_name.Name.L)
        else {
            return table
                .Columns
                .iter()
                .map(|column| column.FieldType.clone())
                .collect();
        };
        projected.push(column.FieldType.clone());
    }
    projected
}
const RELATIONAL_AMBIGUOUS_COLUMN_PREFIX: &str = "\0astersql:ambiguous-column:";
const RELATIONAL_FLOAT32_COLUMN_PREFIX: &str = "\0astersql:float32-column:";
const RELATIONAL_VECTOR_COLUMN_PREFIX: &str = "\0astersql:vector-column:";

pub(super) fn relational_string_column_marker(column: &str) -> String {
    format!("{RELATIONAL_STRING_COLUMN_PREFIX}{column}")
}

pub(super) fn relational_ambiguous_column_marker(column: &str) -> String {
    format!("{RELATIONAL_AMBIGUOUS_COLUMN_PREFIX}{column}")
}

pub(super) fn relational_float32_column_marker(column: &str) -> String {
    format!("{RELATIONAL_FLOAT32_COLUMN_PREFIX}{column}")
}

pub(super) fn relational_vector_column_marker(column: &str) -> String {
    format!("{RELATIONAL_VECTOR_COLUMN_PREFIX}{column}")
}

pub(super) fn relational_expression_is_string(
    expression: &ast::ExprNode,
    row: &HashMap<String, Option<String>>,
) -> bool {
    match &expression.Kind {
        ast::ExprKind::Value(value) => matches!(
            value.Datum,
            ast::ValueDatum::String(_)
                | ast::ValueDatum::Bytes(_)
                | ast::ValueDatum::BitLiteral(_)
                | ast::ValueDatum::HexLiteral(_)
        ),
        ast::ExprKind::IntroducedValue { .. } => true,
        ast::ExprKind::Column(column) => {
            if column.Table.L.is_empty() {
                row.contains_key(&relational_string_column_marker(&column.Name.L))
            } else {
                row.contains_key(&relational_string_column_marker(&format!(
                    "{}.{}",
                    column.Table.L, column.Name.L
                )))
            }
        }
        ast::ExprKind::Function { FnName, Args, .. } => match FnName.L.as_str() {
            // Keep the result kinds in sync with the Go builtin signatures.
            "database" | "concat" | "elt" | "hex" | "json_type" | "lcase" | "lower" | "lpad"
            | "mid" | "repeat" | "space" | "substr" | "substring" | "ucase" | "unhex" | "upper" => {
                true
            }
            "coalesce" | "greatest" | "ifnull" | "least" => Args
                .iter()
                .any(|arg| relational_expression_is_string(arg, row)),
            "if" => Args
                .iter()
                .skip(1)
                .any(|arg| relational_expression_is_string(arg, row)),
            "any_value" | "nullif" => Args
                .first()
                .is_some_and(|arg| relational_expression_is_string(arg, row)),
            _ => false,
        },
        ast::ExprKind::Case {
            WhenClauses,
            ElseClause,
            ..
        } => {
            WhenClauses
                .iter()
                .any(|clause| relational_expression_is_string(&clause.Result, row))
                || ElseClause
                    .as_ref()
                    .is_some_and(|expr| relational_expression_is_string(expr, row))
        }
        ast::ExprKind::AggregateFunction { Name, Args, .. }
            if matches!(Name.to_ascii_lowercase().as_str(), "min" | "max") =>
        {
            Args.first()
                .is_some_and(|arg| relational_expression_is_string(arg, row))
        }
        ast::ExprKind::Parentheses(inner) | ast::ExprKind::Collate { Expr: inner, .. } => {
            relational_expression_is_string(inner, row)
        }
        ast::ExprKind::Cast { Tp, .. } => {
            Tp.EvalType() == astersql_parser_types::eval_type::ETString
        }
        _ => false,
    }
}

fn join_relational_rows(
    left_rows: Vec<RelationalRow>,
    right_rows: Vec<RelationalRow>,
    left_table: &str,
    right_table: &str,
) -> Vec<RelationalRow> {
    left_rows
        .into_iter()
        .flat_map(|(left_key, left)| {
            right_rows.iter().map(move |(_right_key, right)| {
                let mut joined = left.clone();
                for (column, value) in &left {
                    let qualified = column
                        .strip_prefix(RELATIONAL_STRING_COLUMN_PREFIX)
                        .map(|column| {
                            relational_string_column_marker(&format!("{left_table}.{column}"))
                        })
                        .unwrap_or_else(|| format!("{left_table}.{column}"));
                    joined.insert(qualified, value.clone());
                }
                for (column, value) in right {
                    joined
                        .entry(column.clone())
                        .or_insert_with(|| value.clone());
                    let qualified = column
                        .strip_prefix(RELATIONAL_STRING_COLUMN_PREFIX)
                        .map(|column| {
                            relational_string_column_marker(&format!("{right_table}.{column}"))
                        })
                        .unwrap_or_else(|| format!("{right_table}.{column}"));
                    joined.insert(qualified, value.clone());
                }
                (left_key, joined)
            })
        })
        .collect()
}

pub(super) fn relational_expression_has_aggregate(expression: &ast::ExprNode) -> bool {
    match &expression.Kind {
        ast::ExprKind::AggregateFunction { .. } => true,
        ast::ExprKind::Function { FnName, .. } if FnName.L.eq_ignore_ascii_case("any_value") => {
            true
        }
        ast::ExprKind::Function { Args, .. }
        | ast::ExprKind::ExplicitRow { Fields: Args, .. }
        | ast::ExprKind::ImplicitRow { Fields: Args, .. }
        | ast::ExprKind::Row(Args)
        | ast::ExprKind::WindowFunction { Args, .. } => {
            Args.iter().any(relational_expression_has_aggregate)
        }
        ast::ExprKind::Binary { L, R, .. } | ast::ExprKind::CompareSubquery { L, R, .. } => {
            relational_expression_has_aggregate(L) || relational_expression_has_aggregate(R)
        }
        ast::ExprKind::Unary { V, .. } => relational_expression_has_aggregate(V),
        ast::ExprKind::Parentheses(inner)
        | ast::ExprKind::Collate { Expr: inner, .. }
        | ast::ExprKind::IsTruth { Expr: inner, .. }
        | ast::ExprKind::IsNull { Expr: inner, .. }
        | ast::ExprKind::Cast { Expr: inner, .. }
        | ast::ExprKind::JSONSumCrc32 { Expr: inner, .. } => {
            relational_expression_has_aggregate(inner)
        }
        ast::ExprKind::InList { Expr, List, .. } => {
            relational_expression_has_aggregate(Expr)
                || List.iter().any(relational_expression_has_aggregate)
        }
        ast::ExprKind::InSubquery { Expr, Sel, .. } => {
            relational_expression_has_aggregate(Expr) || relational_expression_has_aggregate(Sel)
        }
        ast::ExprKind::ExistsSubquery { Sel, .. } => relational_expression_has_aggregate(Sel),
        ast::ExprKind::Between {
            Expr, Left, Right, ..
        } => {
            relational_expression_has_aggregate(Expr)
                || relational_expression_has_aggregate(Left)
                || relational_expression_has_aggregate(Right)
        }
        ast::ExprKind::Like { Expr, Pattern, .. } | ast::ExprKind::Regexp { Expr, Pattern, .. } => {
            relational_expression_has_aggregate(Expr)
                || relational_expression_has_aggregate(Pattern)
        }
        ast::ExprKind::Case {
            Value,
            WhenClauses,
            ElseClause,
        } => {
            Value
                .as_deref()
                .is_some_and(relational_expression_has_aggregate)
                || WhenClauses.iter().any(|clause| {
                    relational_expression_has_aggregate(&clause.Expr)
                        || relational_expression_has_aggregate(&clause.Result)
                })
                || ElseClause
                    .as_deref()
                    .is_some_and(relational_expression_has_aggregate)
        }
        _ => false,
    }
}

pub(super) fn is_tidb_row_checksum(expression: &ast::ExprNode) -> bool {
    matches!(
        &expression.Kind,
        ast::ExprKind::Function { FnName, .. }
            if FnName.L.eq_ignore_ascii_case("tidb_row_checksum")
    )
}

pub(super) fn relational_expression_has_tidb_row_checksum(expression: &ast::ExprNode) -> bool {
    if is_tidb_row_checksum(expression) {
        return true;
    }
    match &expression.Kind {
        ast::ExprKind::Function { Args, .. }
        | ast::ExprKind::AggregateFunction { Args, .. }
        | ast::ExprKind::ExplicitRow { Fields: Args, .. }
        | ast::ExprKind::ImplicitRow { Fields: Args, .. }
        | ast::ExprKind::Row(Args)
        | ast::ExprKind::WindowFunction { Args, .. } => {
            Args.iter().any(relational_expression_has_tidb_row_checksum)
        }
        ast::ExprKind::Binary { L, R, .. } | ast::ExprKind::CompareSubquery { L, R, .. } => {
            relational_expression_has_tidb_row_checksum(L)
                || relational_expression_has_tidb_row_checksum(R)
        }
        ast::ExprKind::Unary { V, .. } => relational_expression_has_tidb_row_checksum(V),
        ast::ExprKind::Parentheses(inner)
        | ast::ExprKind::Collate { Expr: inner, .. }
        | ast::ExprKind::IsTruth { Expr: inner, .. }
        | ast::ExprKind::IsNull { Expr: inner, .. }
        | ast::ExprKind::Cast { Expr: inner, .. }
        | ast::ExprKind::JSONSumCrc32 { Expr: inner, .. } => {
            relational_expression_has_tidb_row_checksum(inner)
        }
        ast::ExprKind::InList { Expr, List, .. } => {
            relational_expression_has_tidb_row_checksum(Expr)
                || List.iter().any(relational_expression_has_tidb_row_checksum)
        }
        ast::ExprKind::Between {
            Expr, Left, Right, ..
        } => {
            relational_expression_has_tidb_row_checksum(Expr)
                || relational_expression_has_tidb_row_checksum(Left)
                || relational_expression_has_tidb_row_checksum(Right)
        }
        ast::ExprKind::Like { Expr, Pattern, .. } | ast::ExprKind::Regexp { Expr, Pattern, .. } => {
            relational_expression_has_tidb_row_checksum(Expr)
                || relational_expression_has_tidb_row_checksum(Pattern)
        }
        _ => false,
    }
}

pub(super) fn relational_expression_has_json_temporal(expression: &ast::ExprNode) -> bool {
    match &expression.Kind {
        ast::ExprKind::Function { FnName, Args, .. } => {
            FnName.L.eq_ignore_ascii_case("json_type")
                || Args.iter().any(relational_expression_has_json_temporal)
        }
        ast::ExprKind::Cast { Expr, Tp, .. } => {
            matches!(
                Tp.GetType(),
                astersql_parser_mysql::r#type::TypeDate
                    | astersql_parser_mysql::r#type::TypeDatetime
                    | astersql_parser_mysql::r#type::TypeTimestamp
                    | astersql_parser_mysql::r#type::TypeDuration
            ) || relational_expression_has_json_temporal(Expr)
        }
        ast::ExprKind::Binary { L, R, .. } => {
            relational_expression_has_json_temporal(L) || relational_expression_has_json_temporal(R)
        }
        ast::ExprKind::Parentheses(inner) | ast::ExprKind::Unary { V: inner, .. } => {
            relational_expression_has_json_temporal(inner)
        }
        _ => false,
    }
}

pub(super) fn tidb_row_checksum_point_predicate(
    expression: &ast::ExprNode,
    primary_key: &str,
) -> bool {
    match &expression.Kind {
        ast::ExprKind::Binary { Op, L, R } if matches!(Op.as_str(), "=" | "==") => {
            matches!(&L.Kind, ast::ExprKind::Column(column) if column.Name.L == primary_key)
                && matches!(&R.Kind, ast::ExprKind::Value(_))
                || matches!(&R.Kind, ast::ExprKind::Column(column) if column.Name.L == primary_key)
                    && matches!(&L.Kind, ast::ExprKind::Value(_))
        }
        ast::ExprKind::InList {
            Expr, List, Not, ..
        } => {
            !*Not
                && matches!(&Expr.Kind, ast::ExprKind::Column(column) if column.Name.L == primary_key)
                && List
                    .iter()
                    .all(|item| matches!(item.Kind, ast::ExprKind::Value(_)))
        }
        ast::ExprKind::Parentheses(inner) => tidb_row_checksum_point_predicate(inner, primary_key),
        _ => false,
    }
}

pub(super) fn relational_expression_has_subquery(expression: &ast::ExprNode) -> bool {
    match &expression.Kind {
        ast::ExprKind::Subquery { .. }
        | ast::ExprKind::CompareSubquery { .. }
        | ast::ExprKind::InSubquery { .. }
        | ast::ExprKind::ExistsSubquery { .. } => true,
        ast::ExprKind::Function { Args, .. }
        | ast::ExprKind::AggregateFunction { Args, .. }
        | ast::ExprKind::ExplicitRow { Fields: Args, .. }
        | ast::ExprKind::ImplicitRow { Fields: Args, .. }
        | ast::ExprKind::Row(Args) => Args.iter().any(relational_expression_has_subquery),
        ast::ExprKind::WindowFunction { Args, Spec, .. } => {
            Args.iter().any(relational_expression_has_subquery)
                || Spec
                    .PartitionBy
                    .iter()
                    .chain(&Spec.OrderBy)
                    .any(|item| relational_expression_has_subquery(&item.Expr))
                || Spec.Frame.as_ref().is_some_and(|frame| {
                    frame
                        .Extent
                        .Start
                        .Expr
                        .as_ref()
                        .is_some_and(relational_expression_has_subquery)
                        || frame
                            .Extent
                            .End
                            .Expr
                            .as_ref()
                            .is_some_and(relational_expression_has_subquery)
                })
        }
        ast::ExprKind::Binary { L, R, .. } => {
            relational_expression_has_subquery(L) || relational_expression_has_subquery(R)
        }
        ast::ExprKind::Unary { V, .. } => relational_expression_has_subquery(V),
        ast::ExprKind::Parentheses(inner)
        | ast::ExprKind::Collate { Expr: inner, .. }
        | ast::ExprKind::IsTruth { Expr: inner, .. }
        | ast::ExprKind::IsNull { Expr: inner, .. }
        | ast::ExprKind::Cast { Expr: inner, .. }
        | ast::ExprKind::JSONSumCrc32 { Expr: inner, .. } => {
            relational_expression_has_subquery(inner)
        }
        ast::ExprKind::InList { Expr, List, .. } => {
            relational_expression_has_subquery(Expr)
                || List.iter().any(relational_expression_has_subquery)
        }
        ast::ExprKind::Between {
            Expr, Left, Right, ..
        } => {
            relational_expression_has_subquery(Expr)
                || relational_expression_has_subquery(Left)
                || relational_expression_has_subquery(Right)
        }
        ast::ExprKind::Like { Expr, Pattern, .. } | ast::ExprKind::Regexp { Expr, Pattern, .. } => {
            relational_expression_has_subquery(Expr) || relational_expression_has_subquery(Pattern)
        }
        ast::ExprKind::Case {
            Value,
            WhenClauses,
            ElseClause,
        } => {
            Value
                .as_deref()
                .is_some_and(relational_expression_has_subquery)
                || WhenClauses.iter().any(|clause| {
                    relational_expression_has_subquery(&clause.Expr)
                        || relational_expression_has_subquery(&clause.Result)
                })
                || ElseClause
                    .as_deref()
                    .is_some_and(relational_expression_has_subquery)
        }
        _ => false,
    }
}

pub(super) fn relational_expression_has_window(expression: &ast::ExprNode) -> bool {
    match &expression.Kind {
        ast::ExprKind::WindowFunction { .. } => true,
        ast::ExprKind::Function { Args, .. }
        | ast::ExprKind::AggregateFunction { Args, .. }
        | ast::ExprKind::ExplicitRow { Fields: Args, .. }
        | ast::ExprKind::ImplicitRow { Fields: Args, .. }
        | ast::ExprKind::Row(Args) => Args.iter().any(relational_expression_has_window),
        ast::ExprKind::Binary { L, R, .. } => {
            relational_expression_has_window(L) || relational_expression_has_window(R)
        }
        ast::ExprKind::Unary { V, .. } => relational_expression_has_window(V),
        ast::ExprKind::Parentheses(inner)
        | ast::ExprKind::Collate { Expr: inner, .. }
        | ast::ExprKind::IsTruth { Expr: inner, .. }
        | ast::ExprKind::IsNull { Expr: inner, .. }
        | ast::ExprKind::Cast { Expr: inner, .. }
        | ast::ExprKind::JSONSumCrc32 { Expr: inner, .. } => {
            relational_expression_has_window(inner)
        }
        ast::ExprKind::InList { Expr, List, .. } => {
            relational_expression_has_window(Expr)
                || List.iter().any(relational_expression_has_window)
        }
        ast::ExprKind::Between {
            Expr, Left, Right, ..
        } => {
            relational_expression_has_window(Expr)
                || relational_expression_has_window(Left)
                || relational_expression_has_window(Right)
        }
        ast::ExprKind::Like { Expr, Pattern, .. } | ast::ExprKind::Regexp { Expr, Pattern, .. } => {
            relational_expression_has_window(Expr) || relational_expression_has_window(Pattern)
        }
        ast::ExprKind::Case {
            Value,
            WhenClauses,
            ElseClause,
        } => {
            Value
                .as_deref()
                .is_some_and(relational_expression_has_window)
                || WhenClauses.iter().any(|clause| {
                    relational_expression_has_window(&clause.Expr)
                        || relational_expression_has_window(&clause.Result)
                })
                || ElseClause
                    .as_deref()
                    .is_some_and(relational_expression_has_window)
        }
        _ => false,
    }
}

pub(super) fn relational_expression_subquery(expression: &ast::ExprNode) -> Option<&ast::NodeRef> {
    match &expression.Kind {
        ast::ExprKind::Subquery { Query, .. } => Some(Query),
        ast::ExprKind::CompareSubquery { L, R, .. } => {
            relational_expression_subquery(L).or_else(|| relational_expression_subquery(R))
        }
        ast::ExprKind::InSubquery { Expr, Sel, .. } => {
            relational_expression_subquery(Expr).or_else(|| relational_expression_subquery(Sel))
        }
        ast::ExprKind::ExistsSubquery { Sel, .. }
        | ast::ExprKind::Parentheses(Sel)
        | ast::ExprKind::Unary { V: Sel, .. }
        | ast::ExprKind::Collate { Expr: Sel, .. }
        | ast::ExprKind::IsTruth { Expr: Sel, .. }
        | ast::ExprKind::IsNull { Expr: Sel, .. }
        | ast::ExprKind::Cast { Expr: Sel, .. }
        | ast::ExprKind::JSONSumCrc32 { Expr: Sel, .. } => relational_expression_subquery(Sel),
        ast::ExprKind::Function { Args, .. }
        | ast::ExprKind::AggregateFunction { Args, .. }
        | ast::ExprKind::ExplicitRow { Fields: Args, .. }
        | ast::ExprKind::ImplicitRow { Fields: Args, .. }
        | ast::ExprKind::Row(Args)
        | ast::ExprKind::WindowFunction { Args, .. } => {
            Args.iter().find_map(relational_expression_subquery)
        }
        ast::ExprKind::Binary { L, R, .. } => {
            relational_expression_subquery(L).or_else(|| relational_expression_subquery(R))
        }
        ast::ExprKind::InList { Expr, List, .. } => relational_expression_subquery(Expr)
            .or_else(|| List.iter().find_map(relational_expression_subquery)),
        ast::ExprKind::Between {
            Expr, Left, Right, ..
        } => relational_expression_subquery(Expr)
            .or_else(|| relational_expression_subquery(Left))
            .or_else(|| relational_expression_subquery(Right)),
        ast::ExprKind::Like { Expr, Pattern, .. } | ast::ExprKind::Regexp { Expr, Pattern, .. } => {
            relational_expression_subquery(Expr).or_else(|| relational_expression_subquery(Pattern))
        }
        ast::ExprKind::Case {
            Value,
            WhenClauses,
            ElseClause,
        } => Value
            .as_deref()
            .and_then(relational_expression_subquery)
            .or_else(|| {
                WhenClauses.iter().find_map(|clause| {
                    relational_expression_subquery(&clause.Expr)
                        .or_else(|| relational_expression_subquery(&clause.Result))
                })
            })
            .or_else(|| {
                ElseClause
                    .as_deref()
                    .and_then(relational_expression_subquery)
            }),
        _ => None,
    }
}

pub(super) fn collect_join_using_column_names(join: &ast::Join, output: &mut HashSet<String>) {
    output.extend(join.Using.iter().map(|column| column.Name.L.clone()));
    for source in [join.Left.as_deref(), join.Right.as_deref()]
        .into_iter()
        .flatten()
    {
        if let ast::ResultSetNode::Join(nested) = source {
            collect_join_using_column_names(nested, output);
        }
    }
}

pub(super) fn collect_expression_column_names(
    expression: &ast::ExprNode,
    output: &mut HashSet<String>,
) {
    match &expression.Kind {
        ast::ExprKind::Column(column) => {
            output.insert(column.Name.L.clone());
        }
        ast::ExprKind::Parentheses(inner)
        | ast::ExprKind::Collate { Expr: inner, .. }
        | ast::ExprKind::Unary { V: inner, .. } => {
            collect_expression_column_names(inner, output);
        }
        ast::ExprKind::Binary { L, R, .. } => {
            collect_expression_column_names(L, output);
            collect_expression_column_names(R, output);
        }
        ast::ExprKind::Function { Args, .. } => {
            for argument in Args {
                collect_expression_column_names(argument, output);
            }
        }
        _ => {}
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// SELECT 的行窗口；先跳过 `offset` 行，再最多保留 `count` 行。
pub(super) struct RelationalLimitWindow {
    pub(super) offset: usize,
    pub(super) count: usize,
}

/// 从 SELECT AST 解析 LIMIT/OFFSET 常量。
pub(super) fn relational_limit_window(
    statement: &ast::SelectStmt,
) -> SessionResult<Option<RelationalLimitWindow>> {
    statement
        .Limit
        .as_ref()
        .map(|limit_clause| {
            let count = limit_clause
                .Count
                .as_ref()
                .map(|expression| {
                    literal(expression)?
                        .parse::<usize>()
                        .map_err(|error| session_error("parse SELECT LIMIT", error))
                })
                .transpose()?
                .unwrap_or(usize::MAX);
            let offset = limit_clause
                .Offset
                .as_ref()
                .map(|expression| {
                    literal(expression)?
                        .parse::<usize>()
                        .map_err(|error| session_error("parse SELECT OFFSET", error))
                })
                .transpose()?
                .unwrap_or(0);
            Ok(RelationalLimitWindow { offset, count })
        })
        .transpose()
}

/// 通过 executor 的规范 `LimitExec` 应用根节点 LIMIT/OFFSET。
pub(super) fn execute_relational_limit<T: Clone>(
    rows: Vec<T>,
    window: Option<RelationalLimitWindow>,
) -> SessionResult<Vec<T>> {
    let Some(window) = window else {
        return Ok(rows);
    };
    astersql_executor::select::ExecuteLimitValues(rows, window.offset, window.count, 1024)
        .map_err(|error| session_error("execute relational LIMIT", error))
}

/// Returns the literal compared with the single-column primary key, accepting
/// both `pk = value` and `value = pk` forms used by PointGet plans.

pub(super) fn format_unix_timestamp(value: f64, fractional_seconds_precision: usize) -> String {
    let seconds = value.floor() as i64;
    let nanos = ((value - value.floor()) * 1_000_000_000.0)
        .round()
        .clamp(0.0, 999_999_999.0) as u32;
    let datetime = chrono::DateTime::<chrono::Utc>::from_timestamp(seconds, nanos)
        .unwrap_or(chrono::DateTime::<chrono::Utc>::UNIX_EPOCH);
    if fractional_seconds_precision == 0 {
        datetime.format("%Y-%m-%d %H:%M:%S").to_string()
    } else {
        let precision = fractional_seconds_precision.min(6);
        format!(
            "{}.{:0precision$}",
            datetime.format("%Y-%m-%d %H:%M:%S"),
            datetime.timestamp_subsec_micros() / 10_u32.pow((6 - precision) as u32),
            precision = precision
        )
    }
}

// Analyze integer constants at the shared PG expression boundary, before
// result fields are bound through a CTE or mapped to protocol types. Apply the
// unary sign before choosing the width, including the int4/int8 lower bounds.
pub(super) fn pg_integer_literal_type(
    expression: &ast::ExprNode,
) -> Option<astersql_parser_types::types::FieldType> {
    fn integer(expression: &ast::ExprNode) -> Option<i128> {
        match &expression.Kind {
            ast::ExprKind::Value(value) => match value.Datum {
                ast::ValueDatum::Int64(value) => Some(i128::from(value)),
                ast::ValueDatum::Uint64(value) => Some(i128::from(value)),
                _ => None,
            },
            ast::ExprKind::Parentheses(inner) => integer(inner),
            ast::ExprKind::Unary { Op, V } if Op == "+" => integer(V),
            ast::ExprKind::Unary { Op, V } if Op == "-" => integer(V)?.checked_neg(),
            _ => None,
        }
    }
    let value = integer(expression)?;
    Some(astersql_parser_types::NewFieldType(
        if i64::try_from(value).is_err() {
            astersql_parser_mysql::r#type::TypeNewDecimal
        } else if i32::try_from(value).is_ok() {
            astersql_parser_mysql::r#type::TypeLong
        } else {
            astersql_parser_mysql::r#type::TypeLonglong
        },
    ))
}

// RETURNING uses PostgreSQL integer widths, while the shared MySQL projection
// analyzer intentionally promotes integer arithmetic to BIGINT. Admit the
// required integer/column/literal forms here and reject unknown type families.
pub(super) fn pg_returning_expression_type(
    expression: &ast::ExprNode,
    sources: &[ConcreteResultField],
) -> SessionResult<astersql_parser_types::types::FieldType> {
    use astersql_parser_mysql::r#type::{TypeLong, TypeLonglong};
    if let Some(tp) = pg_integer_literal_type(expression) {
        return Ok(tp);
    }
    let unsupported = || SessionError::new("PostgreSQL RETURNING expression type is unsupported");
    Ok(match &expression.Kind {
        ast::ExprKind::Column(column) => sources
            .iter()
            .find(|source| {
                source.column.Name.L == column.Name.L
                    && (column.Table.L.is_empty() || source.table_as_name.L == column.Table.L)
                    && (column.Schema.L.is_empty() || source.db_name.L == column.Schema.L)
            })
            .ok_or_else(|| {
                SessionError::new(format!(
                    "Unknown column '{}' in 'field list'",
                    column.Name.O
                ))
            })?
            .column
            .FieldType
            .clone(),
        ast::ExprKind::Value(value) => match &value.Datum {
            ast::ValueDatum::Int64(value) => {
                astersql_parser_types::NewFieldType(if i32::try_from(*value).is_ok() {
                    TypeLong
                } else {
                    TypeLonglong
                })
            }
            ast::ValueDatum::Uint64(value) if i64::try_from(*value).is_ok() => {
                astersql_parser_types::NewFieldType(if i32::try_from(*value).is_ok() {
                    TypeLong
                } else {
                    TypeLonglong
                })
            }
            ast::ValueDatum::Null | ast::ValueDatum::String(_) => value.Type.clone(),
            _ => return Err(unsupported()),
        },
        ast::ExprKind::Parentheses(inner) => pg_returning_expression_type(inner, sources)?,
        ast::ExprKind::Unary { Op, V } if Op == "+" || Op == "-" => {
            pg_returning_expression_type(V, sources)?
        }
        ast::ExprKind::Cast {
            Expr,
            Tp,
            FunctionType:
                ast::CastFunctionType::PostgreSQLInt2
                | ast::CastFunctionType::PostgreSQLInt4
                | ast::CastFunctionType::PostgreSQLInt8,
            ..
        } => {
            if !matches!(Expr.Kind, ast::ExprKind::ParamMarker { .. }) {
                pg_returning_expression_type(Expr, sources)?;
            }
            Tp.clone()
        }
        ast::ExprKind::Binary { Op, L, R }
            if matches!(Op.as_str(), "+" | "-" | "*" | "/" | "%") =>
        {
            let left = pg_returning_expression_type(L, sources)?.GetType();
            let right = pg_returning_expression_type(R, sources)?.GetType();
            if !matches!(left, 2 | 3 | 8) || !matches!(right, 2 | 3 | 8) {
                return Err(unsupported());
            }
            astersql_parser_types::NewFieldType(if left == TypeLonglong || right == TypeLonglong {
                TypeLonglong
            } else if left == TypeLong || right == TypeLong {
                TypeLong
            } else {
                left
            })
        }
        _ => return Err(unsupported()),
    })
}

impl ConcreteSession {
    /// 将 SELECT 条件值转换成 Go executor 的 WHERE 真值语义。
    pub(super) fn insert_select_predicate(
        &self,
        expression: &ast::ExprNode,
        row: &HashMap<String, Option<String>>,
    ) -> SessionResult<bool> {
        let Some(value) = self.relational_query_expression_value(expression, row, None)? else {
            return Ok(false);
        };
        Ok(relational_truth(Some(&value)).unwrap_or(false))
    }

    pub(super) fn relational_query_column_value(
        &self,
        column: &ast::ColumnName,
        row: &HashMap<String, Option<String>>,
    ) -> SessionResult<Option<String>> {
        if !column.Table.L.is_empty() {
            let qualified = format!("{}.{}", column.Table.L, column.Name.L);
            if let Some(value) = row.get(&qualified) {
                return Ok(value.clone());
            }
        }
        if row.contains_key(&relational_ambiguous_column_marker(&column.Name.L)) {
            return Err(SessionError::new(format!(
                "Column '{}' in field list is ambiguous",
                column.Name.O
            )));
        }
        row.get(&column.Name.L)
            .cloned()
            .ok_or_else(|| SessionError::new(format!("Unknown column '{}'", column.Name.O)))
    }

    pub(super) fn execute_relational_subquery(
        &self,
        expression: &ast::ExprNode,
        outer: &HashMap<String, Option<String>>,
    ) -> SessionResult<InsertSelectRows> {
        let ast::ExprKind::Subquery { Query, .. } = &expression.Kind else {
            return Err(SessionError::new("subquery expression is not a query"));
        };
        Query
            .with_node(|node| self.execute_insert_select_node_with_outer(node, Some(outer)))
            .unwrap_or_else(|| Err(SessionError::new("subquery query is unavailable")))
    }

    pub(super) fn relational_aggregate_value(
        &self,
        name: &str,
        args: &[ast::ExprNode],
        distinct: bool,
        rows: &[HashMap<String, Option<String>>],
    ) -> SessionResult<Option<String>> {
        let name = name.to_ascii_lowercase();
        if name == "count" && args.is_empty() {
            return Ok(Some(rows.len().to_string()));
        }
        let mut values = Vec::new();
        for row in rows {
            let value = if let Some(argument) = args.first() {
                self.relational_query_expression_value(argument, row, None)?
            } else {
                Some("1".to_owned())
            };
            if value.is_some() {
                values.push(value);
            }
        }
        if distinct {
            let mut seen = HashSet::new();
            values.retain(|value| seen.insert(value.clone()));
        }
        if matches!(name.as_str(), "sum" | "avg")
            && let Some(ast::ExprNode {
                Kind: ast::ExprKind::Column(column),
                ..
            }) = args.first()
            && rows
                .iter()
                .any(|row| row.contains_key(&relational_vector_column_marker(&column.Name.L)))
        {
            return Err(SessionError::new(format!(
                "aggregate function {name} does not support VECTOR arguments"
            )));
        }
        match name.as_str() {
            "count" => Ok(Some(values.len().to_string())),
            "bit_xor" => values
                .into_iter()
                .flatten()
                .try_fold(0_u64, |accumulator, value| {
                    value
                        .parse::<i64>()
                        .map(|value| accumulator ^ value as u64)
                        .map_err(|error| session_error("parse BIT_XOR value", error))
                })
                .map(|value| Some(value.to_string())),
            "any_value" => Ok(values.into_iter().flatten().next()),
            "group_concat" => {
                let values = values.into_iter().flatten().collect::<Vec<_>>();
                Ok((!values.is_empty()).then(|| values.join(",")))
            }
            "sum" | "avg" => {
                if name == "avg"
                    && let Some(ast::ExprNode {
                        Kind: ast::ExprKind::Column(column),
                        ..
                    }) = args.first()
                    && rows.iter().any(|row| {
                        row.contains_key(&relational_float32_column_marker(&column.Name.L))
                    })
                {
                    let floats = values
                        .into_iter()
                        .flatten()
                        .map(|value| {
                            value.parse::<f32>().map(f64::from).map_err(|error| {
                                session_error("parse FLOAT aggregate value", error)
                            })
                        })
                        .collect::<SessionResult<Vec<_>>>()?;
                    if floats.is_empty() {
                        return Ok(None);
                    }
                    return Ok(Some(
                        (floats.iter().sum::<f64>() / floats.len() as f64).to_string(),
                    ));
                }
                let decimals = values
                    .into_iter()
                    .flatten()
                    .map(|value| {
                        value
                            .parse::<rust_decimal::Decimal>()
                            .or_else(|_| {
                                relational_numeric_prefix(&value)
                                    .unwrap_or(0.0)
                                    .to_string()
                                    .parse::<rust_decimal::Decimal>()
                            })
                            .map_err(|error| session_error("parse aggregate numeric value", error))
                    })
                    .collect::<SessionResult<Vec<_>>>()?;
                if decimals.is_empty() {
                    return Ok(None);
                }
                let sum = decimals
                    .iter()
                    .copied()
                    .fold(rust_decimal::Decimal::ZERO, |sum, value| sum + value);
                if name == "sum" {
                    Ok(Some(sum.normalize().to_string()))
                } else {
                    let average = sum / rust_decimal::Decimal::from(decimals.len() as u64);
                    Ok(Some(format!("{average:.4}")))
                }
            }
            "min_count" | "max_count" => Ok(Some(
                relational_count_extrema(values.into_iter().flatten(), name == "max_count")
                    .to_string(),
            )),
            "min" | "max" => {
                let mut values = values.into_iter().flatten();
                let Some(mut selected) = values.next() else {
                    return Ok(None);
                };
                for value in values {
                    let ordering = relational_compare(&value, &selected);
                    if (name == "min" && ordering.is_lt()) || (name == "max" && ordering.is_gt()) {
                        selected = value;
                    }
                }
                Ok(Some(selected))
            }
            _ => Err(SessionError::new(format!(
                "unsupported aggregate function {name}"
            ))),
        }
    }

    pub(super) fn relational_query_expression_value(
        &self,
        expression: &ast::ExprNode,
        row: &HashMap<String, Option<String>>,
        group_rows: Option<&[HashMap<String, Option<String>>]>,
    ) -> SessionResult<Option<String>> {
        if let Some(value) = pg_numeric_literal_value(expression) {
            return value.map(Some);
        }
        if let ast::ExprKind::Function {
            FnName,
            Schema,
            Args,
        } = &expression.Kind
            && FnName.L == "abs"
            && Args.len() == 1
            && (Schema.L.is_empty()
                || (self.sql_dialect() == ast::SqlDialect::PostgreSQL && Schema.L == "pg_catalog"))
        {
            use astersql_expression::BuildContext;
            let source = self.signed_abs_argument_type(&Args[0], &[]);
            if matches!(
                source.GetType(),
                astersql_parser_mysql::r#type::TypeLong
                    | astersql_parser_mysql::r#type::TypeLonglong
            ) && source.GetFlag() & astersql_parser_mysql::r#type::UnsignedFlag == 0
            {
                let value = self.relational_query_expression_value(&Args[0], row, group_rows)?;
                let mut argument = match value {
                    None => astersql_expression::NewNull(),
                    Some(value) => astersql_expression::NewInt64Const(
                        value
                            .parse()
                            .map_err(|error| session_error("parse signed ABS argument", error))?,
                    ),
                };
                argument.RetType = Some(source);
                let eval = astersql_expression_exprstatic::NewEvalContext(vec![
                    astersql_expression_exprstatic::WithSQLDialect(self.sql_dialect()),
                ]);
                let context = astersql_expression_exprstatic::NewExprContext(vec![
                    astersql_expression_exprstatic::WithEvalCtx(Arc::new(eval)),
                ]);
                let function = astersql_expression::NewFunctionBase(
                    &context,
                    "abs",
                    *astersql_expression::types::NewFieldType(
                        astersql_expression::mysql::TypeUnspecified,
                    ),
                    vec![Box::new(argument)],
                )
                .map_err(|error| session_error("build signed ABS", error))?;
                let (value, null) = function
                    .EvalInt(
                        context.GetEvalCtx(),
                        astersql_expression::chunk::Row::default(),
                    )
                    .map_err(|error| {
                        // The argument is already a validated integer Constant;
                        // the signed ABS kernel can only reject overflow here.
                        if self.sql_dialect() == ast::SqlDialect::PostgreSQL {
                            let cause = astersql_expression::PgInt4Error::OutOfRange;
                            SessionError::with_source(
                                cause.to_string(),
                                astersql_errors::SharedError::new(cause),
                            )
                        } else {
                            fn literal(expr: &ast::ExprNode) -> bool {
                                match &expr.Kind {
                                    ast::ExprKind::Value(_) => true,
                                    ast::ExprKind::Cast { Expr, .. }
                                    | ast::ExprKind::Parentheses(Expr) => literal(Expr),
                                    ast::ExprKind::Unary { Op, V }
                                        if matches!(Op.as_str(), "+" | "-") =>
                                    {
                                        literal(V)
                                    }
                                    _ => false,
                                }
                            }
                            if row.is_empty() && group_rows.is_none() && literal(&Args[0]) {
                                // Constant ABS overflow fails during Go optimization,
                                // before ExecuteStmt's AppendError lifecycle.
                                SessionError::with_source(
                                    format!("evaluate signed ABS: {error}"),
                                    astersql_errors::SharedError::new(MysqlSignedAbsOverflow(
                                        error.to_string(),
                                    )),
                                )
                            } else {
                                let diagnostic = error.to_string();
                                let message = diagnostic
                                    .strip_prefix("[types:1690]")
                                    .unwrap_or(&diagnostic)
                                    .to_owned();
                                self.state
                                    .borrow_mut()
                                    .current_warnings
                                    .push(SessionWarning::error_with_code(1690, message));
                                session_error("evaluate signed ABS", error)
                            }
                        }
                    })?;
                return Ok((!null).then(|| value.to_string()));
            }
        }
        if self.sql_dialect() == ast::SqlDialect::PostgreSQL
            && let ast::ExprKind::Function {
                FnName,
                Schema,
                Args,
            } = &expression.Kind
            && FnName.L == "round"
            && Args.len() == 2
            && (Schema.L.is_empty() || Schema.L == "pg_catalog")
        {
            let value = self.relational_query_expression_value(&Args[0], row, group_rows)?;
            let scale = self.relational_query_expression_value(&Args[1], row, group_rows)?;
            return value
                .zip(scale)
                .map(|(value, scale)| {
                    let scale = scale
                        .parse::<i32>()
                        .map_err(|e| session_error("parse ROUND scale", e))?;
                    value
                        .parse::<astersql_types::pg_numeric::PgNumeric>()
                        .map(|n| n.round_scale(scale).to_string())
                        .map_err(|e| {
                            SessionError::with_source(
                                e.to_string(),
                                astersql_errors::SharedError::new(e),
                            )
                        })
                })
                .transpose();
        }
        if self.sql_dialect() == ast::SqlDialect::PostgreSQL
            && let ast::ExprKind::Function {
                FnName,
                Schema,
                Args,
            } = &expression.Kind
            && matches!(
                FnName.L.as_str(),
                "date_trunc" | "extract" | "date_part" | "to_char"
            )
            && (2..=3).contains(&Args.len())
            && (Schema.L.is_empty() || Schema.L == "pg_catalog")
        {
            use astersql_expression::BuildContext;
            let input_types = Args
                .iter()
                .map(|arg| {
                    if Args.len() == 3 {
                        self.pg_date_trunc_input_type(arg, &[])
                    } else {
                        self.pg_case_input_type(arg, &[]).GetType()
                    }
                })
                .collect::<Vec<_>>();
            let candidate =
                astersql_expression::formal_registry::resolve_pg_signature(&FnName.L, &input_types)
                    .map_err(|_| {
                        super::function_diagnostics::no_matching_function(expression, &input_types)
                    })?;
            let source_type = candidate.result;
            let values = Args
                .iter()
                .map(|arg| self.relational_query_expression_value(arg, row, group_rows))
                .collect::<SessionResult<Vec<_>>>()?;
            let mut args: Vec<Box<dyn astersql_expression::Expression>> = Vec::new();
            for (index, value) in values.into_iter().enumerate() {
                let mut argument = match value {
                    Some(value) => astersql_expression::NewStrConst(&value),
                    None => astersql_expression::NewNull(),
                };
                argument.RetType = Some(astersql_parser_types::NewFieldType(
                    if index == 1
                        && input_types[index] == astersql_parser_mysql::r#type::TypeDatetime
                    {
                        input_types[index]
                    } else {
                        candidate.arguments[index]
                    },
                ));
                args.push(Box::new(argument));
            }
            let eval = if FnName.L == "to_char"
                || source_type == astersql_parser_mysql::r#type::TypePgTimestampTz
                || candidate
                    .arguments
                    .contains(&astersql_parser_mysql::r#type::TypePgTimestampTz)
            {
                self.pg_temporal_eval_context()?
            } else {
                astersql_expression_exprstatic::NewEvalContext(vec![
                    astersql_expression_exprstatic::WithSQLDialect(self.sql_dialect()),
                ])
            };
            let context = astersql_expression_exprstatic::NewExprContext(vec![
                astersql_expression_exprstatic::WithEvalCtx(Arc::new(eval)),
            ]);
            let built = astersql_expression::NewFunctionBase(
                &context,
                &FnName.L,
                *astersql_expression::types::NewFieldType(source_type),
                args,
            )
            .map_err(|error| session_error("build timestamp truncation", error))?;
            let (value, null) = built
                .EvalString(
                    context.GetEvalCtx(),
                    astersql_expression::chunk::Row::default(),
                )
                .map_err(|error| {
                    SessionError::with_source(
                        format!("evaluate timestamp truncation: {error}"),
                        error.into_shared(),
                    )
                })?;
            let value = (!null).then_some(value);
            return if source_type == astersql_parser_mysql::r#type::TypePgTimestampTz {
                self.pg_timestamptz_relational_value(value, -1)
            } else {
                Ok(value)
            };
        }
        if self.sql_dialect() == ast::SqlDialect::PostgreSQL
            && let ast::ExprKind::Function {
                FnName,
                Schema,
                Args,
            } = &expression.Kind
            && matches!(
                FnName.L.as_str(),
                "array_length" | "cardinality" | "array_append"
            )
            && (Schema.L.is_empty() || Schema.L == "pg_catalog")
        {
            use astersql_expression::BuildContext;
            let inputs = Args
                .iter()
                .map(|arg| self.pg_case_input_type(arg, &[]))
                .collect::<Vec<_>>();
            let candidate = astersql_expression::formal_registry::resolve_pg_array_signature(
                &FnName.L, &inputs,
            )
            .map_err(|_| {
                super::function_diagnostics::no_matching_function(
                    expression,
                    &inputs.iter().map(|tp| tp.GetType()).collect::<Vec<_>>(),
                )
            })?;
            let mut arguments: Vec<Box<dyn astersql_expression::Expression>> = Vec::new();
            for (argument, input) in Args.iter().zip(inputs) {
                let mut value = self
                    .relational_query_expression_value(argument, row, group_rows)?
                    .map(|text| astersql_expression::NewStrConst(&text))
                    .unwrap_or_else(astersql_expression::NewNull);
                value.RetType = Some(input);
                arguments.push(Box::new(value));
            }
            let context = astersql_expression_exprstatic::NewExprContext(vec![
                astersql_expression_exprstatic::WithEvalCtx(Arc::new(
                    astersql_expression_exprstatic::NewEvalContext(vec![
                        astersql_expression_exprstatic::WithSQLDialect(self.sql_dialect()),
                    ]),
                )),
            ]);
            let built = astersql_expression::NewFunctionBase(
                &context,
                &FnName.L,
                *astersql_expression::types::NewFieldType(candidate.result),
                arguments,
            )
            .map_err(|error| session_error("build PostgreSQL array function", error))?;
            if FnName.L == "array_append" {
                let (value, null) = built
                    .EvalString(context.GetEvalCtx(), astersql_expression::chunk::Row::default())
                    .map_err(|error| {
                        let shared = error.into_shared();
                        if let Some(error) = shared.downcast_ref::<astersql_expression::formal_registry::PgArrayAppendError>() {
                            let diagnostic = super::pg_array::PgArrayError {
                                state: error.state,
                                message: error.message.clone(),
                                position: None,
                            };
                            return SessionError::with_source(
                                "evaluate PostgreSQL array append",
                                astersql_errors::SharedError::new(diagnostic),
                            );
                        }
                        SessionError::with_source("evaluate PostgreSQL array append", shared)
                    })?;
                return Ok((!null).then_some(value));
            }
            let (value, null) = built
                .EvalInt(
                    context.GetEvalCtx(),
                    astersql_expression::chunk::Row::default(),
                )
                .map_err(|error| {
                    SessionError::with_source(
                        "evaluate PostgreSQL array function",
                        error.into_shared(),
                    )
                })?;
            return Ok((!null).then(|| value.to_string()));
        }
        // One fixed numeric signature provides a bounded implicit-cast context.
        if self.sql_dialect() == ast::SqlDialect::PostgreSQL
            && let ast::ExprKind::Function {
                FnName,
                Args,
                Schema,
                ..
            } = &expression.Kind
            && FnName.L == "numeric_abs"
            && (Schema.L.is_empty() || Schema.L == "pg_catalog")
            && Args.len() == 1
        {
            let target =
                astersql_parser_types::NewFieldType(astersql_parser_mysql::r#type::TypeNewDecimal);
            return pg_numeric_relational_value(
                self.relational_query_expression_value(&Args[0], row, group_rows)?
                    .map(|v| v.strip_prefix('-').unwrap_or(&v).to_owned()),
                &target,
            );
        }
        // float4abs(real) supplies a fixed implicit int4 -> float4 context.
        if self.sql_dialect() == ast::SqlDialect::PostgreSQL
            && let ast::ExprKind::Function {
                FnName,
                Args,
                Schema,
                ..
            } = &expression.Kind
            && FnName.L == "float4abs"
            && (Schema.L.is_empty() || Schema.L == "pg_catalog")
            && Args.len() == 1
        {
            let value = pg_float4_cast_text_input(
                &Args[0],
                self.relational_query_expression_value(&Args[0], row, group_rows)?,
            )?;
            return astersql_expression::pg_cast_float4(value.as_deref())
                .map(|value| value.map(|value| f64::from(value.abs()).to_string()))
                .map_err(|error| {
                    SessionError::with_source(
                        error.to_string(),
                        astersql_errors::SharedError::new(error),
                    )
                });
        }
        // float8abs(double precision) supplies a fixed implicit int4 -> float8 context.
        if self.sql_dialect() == ast::SqlDialect::PostgreSQL
            && let ast::ExprKind::Function {
                FnName,
                Args,
                Schema,
                ..
            } = &expression.Kind
            && FnName.L == "float8abs"
            && (Schema.L.is_empty() || Schema.L == "pg_catalog")
            && Args.len() == 1
        {
            let value = self.relational_query_expression_value(&Args[0], row, group_rows)?;
            let value = pg_float8_relational_value(&Args[0], value)?;
            return astersql_expression::pg_cast_float8(value.as_deref())
                .map(|value| value.map(|value| value.abs().to_string()))
                .map_err(|error| {
                    SessionError::with_source(
                        error.to_string(),
                        astersql_errors::SharedError::new(error),
                    )
                });
        }
        // Fixed integer signatures supply narrowing-sensitive PG contexts.
        if self.sql_dialect() == ast::SqlDialect::PostgreSQL
            && let ast::ExprKind::Function {
                FnName,
                Args,
                Schema,
                ..
            } = &expression.Kind
            && matches!(FnName.L.as_str(), "int2not" | "int4not")
            && (Schema.L.is_empty() || Schema.L == "pg_catalog")
            && Args.len() == 1
        {
            return self
                .relational_query_expression_value(&Args[0], row, group_rows)?
                .map(|value| {
                    let result = if FnName.L == "int2not" {
                        astersql_expression::pg_cast_int2(Some(&value))
                            .map(|value| value.map(|value| (!value).to_string()))
                    } else {
                        astersql_expression::pg_cast_int4(Some(&value))
                            .map(|value| value.map(|value| (!value).to_string()))
                    };
                    result.map_err(|error| {
                        SessionError::with_source(
                            error.to_string(),
                            astersql_errors::SharedError::new(error),
                        )
                    })
                })
                .unwrap_or(Ok(None));
        }
        // Fixed int8 signature provides the widening context without routing
        // the value through SQL text coercion or the MySQL function registry.
        if self.sql_dialect() == ast::SqlDialect::PostgreSQL
            && let ast::ExprKind::Function {
                FnName,
                Args,
                Schema,
                ..
            } = &expression.Kind
            && FnName.L == "int8not"
            && (Schema.L.is_empty() || Schema.L == "pg_catalog")
            && Args.len() == 1
        {
            return self
                .relational_query_expression_value(&Args[0], row, group_rows)?
                .map(|value| {
                    astersql_expression::pg_cast_int8(Some(&value))
                        .map(|value| value.map(|value| (!value).to_string()))
                        .map_err(|error| {
                            SessionError::with_source(
                                error.to_string(),
                                astersql_errors::SharedError::new(error),
                            )
                        })
                })
                .unwrap_or(Ok(None));
        }
        if self.sql_dialect() == ast::SqlDialect::MySQL
            && let ast::ExprKind::Function { FnName, Args, .. } = &expression.Kind
            && FnName.L == "ifnull"
            && Args.len() == 2
        {
            // Re-enter the typed query evaluator only for the selected arm.
            // The untyped fallback would bypass nested formal kernels and
            // lose errors such as signed ABS(BIGINT_MIN).
            if let Some(value) =
                self.relational_query_expression_value(&Args[0], row, group_rows)?
            {
                return Ok(Some(value));
            }
            return self.relational_query_expression_value(&Args[1], row, group_rows);
        }
        if self.sql_dialect() == ast::SqlDialect::MySQL
            && let ast::ExprKind::Function { FnName, Args, .. } = &expression.Kind
            && FnName.L == "date_format"
            && Args.len() == 2
        {
            use astersql_expression::contextutil::WarnHandler;
            let warnings = Arc::new(astersql_expression::contextutil::NewStaticWarnHandler(0));
            let mode = astersql_parser_mysql::r#const::GetSQLMode(&self.state.borrow().sql_mode)
                .map_err(|error| session_error("parse DATE_FORMAT sql_mode", error))?;
            // The relational SELECT route bypasses ResetContextOfStmt.
            // Apply Go's SELECT conversion policy locally, retaining strict
            // errors when this query evaluator runs inside a writing statement.
            let select_warning = !self.mdl_autocommit_write.get() || !mode.HasStrictMode();
            let flags = self
                .session_vars
                .StmtCtx
                .TypeFlags()
                .WithTruncateAsWarning(select_warning)
                .WithIgnoreZeroInDate(true)
                .WithIgnoreInvalidDateErr(mode.HasAllowInvalidDatesMode())
                .WithIgnoreZeroDateErr(!mode.HasNoZeroDateMode() || !mode.HasStrictMode());
            let mut levels = self.session_vars.StmtCtx.ErrLevels();
            levels[astersql_expression::errctx::ErrGroup::ErrGroupTruncate as usize] =
                if select_warning {
                    astersql_expression::errctx::Level::LevelWarn
                } else {
                    astersql_expression::errctx::Level::LevelError
                };
            let eval = astersql_expression_exprstatic::NewEvalContext(vec![
                astersql_expression_exprstatic::WithSQLMode(mode),
                astersql_expression_exprstatic::WithTypeFlags(flags),
                astersql_expression_exprstatic::WithErrLevelMap(levels),
                astersql_expression_exprstatic::WithLocation(self.session_vars.StmtCtx.TimeZone()),
                astersql_expression_exprstatic::WithWarnHandler(warnings.clone()),
            ]);
            let value = self.relational_query_expression_value(&Args[0], row, group_rows)?;
            let mut source = value
                .map(astersql_types::datum::NewStringDatum)
                .unwrap_or_default();
            if let ast::ExprKind::Value(literal) = &Args[0].Kind {
                match &literal.Datum {
                    ast::ValueDatum::Int64(value) => {
                        source = astersql_types::datum::NewIntDatum(*value)
                    }
                    ast::ValueDatum::Uint64(value) => {
                        source = astersql_types::datum::NewUintDatum(*value)
                    }
                    ast::ValueDatum::Decimal(value) => {
                        let mut decimal = astersql_expression::types::MyDecimal::default();
                        decimal
                            .FromString(value.as_bytes())
                            .map_err(|error| session_error("read DATE_FORMAT decimal", error))?;
                        source = astersql_types::datum::NewDecimalDatum(decimal);
                    }
                    _ => {}
                }
            }
            let source_text = source.ToString().unwrap_or_default();
            let value = match astersql_expression::formal_registry::DateFormatInput(&eval, source) {
                Ok(Some(time)) => {
                    match self.relational_query_expression_value(&Args[1], row, group_rows)? {
                        Some(mask) => astersql_expression::formal_registry::DateFormatOutput(
                            &eval, time, mask,
                        ),
                        None => Ok((String::new(), true)),
                    }
                }
                Ok(None) => Ok((String::new(), true)),
                Err(error) => Err(error),
            };
            self.state.borrow_mut().current_warnings.extend(
                warnings
                    .CopyWarnings(Vec::new())
                    .into_iter()
                    .filter_map(|warning| {
                        warning.Err.map(|error| SessionWarning::from_error(&error))
                    }),
            );
            return value
                .map(|(value, null)| if null { None } else { Some(value) })
                .map_err(|error| {
                    if error.Equal(&astersql_types::errors::ErrWrongValue) {
                        SessionError::with_source(
                            error.to_string(),
                            astersql_types::errors::ErrWrongValue
                                .GenWithStackByArgs(&["datetime".into(), source_text.into()]),
                        )
                    } else {
                        session_error("evaluate DATE_FORMAT", error)
                    }
                });
        }
        match &expression.Kind {
            ast::ExprKind::ExplicitRow { .. } | ast::ExprKind::ImplicitRow { .. } => {
                super::pg_record::value_with(expression, &|field| {
                    self.relational_query_expression_value(field, row, group_rows)
                })
            }

            ast::ExprKind::Column(column) => self.relational_query_column_value(column, row),
            ast::ExprKind::Variable {
                Name,
                IsGlobal,
                IsSystem,
                Value,
                ..
            } => {
                if let Some(assignment) = Value.as_deref() {
                    let value =
                        self.relational_query_expression_value(assignment, row, group_rows)?;
                    let name = Name.trim_start_matches('@').to_ascii_lowercase();
                    let is_string = matches!(
                        &assignment.Kind,
                        ast::ExprKind::Value(value)
                            if matches!(
                                value.Datum,
                                ast::ValueDatum::String(_) | ast::ValueDatum::Bytes(_)
                            )
                    );
                    let mut state = self.state.borrow_mut();
                    state.user_variables.insert(
                        name.clone(),
                        value.clone().unwrap_or_else(|| SHOW_NULL_CELL.to_owned()),
                    );
                    if is_string {
                        state.string_user_variables.insert(name);
                    } else {
                        state.string_user_variables.remove(&name);
                    }
                    return Ok(value);
                }
                let scoped_name = if *IsGlobal {
                    format!("global.{Name}")
                } else {
                    Name.clone()
                };
                self.select_variable(&scoped_name, *IsSystem).map(|value| {
                    if value == SHOW_NULL_CELL {
                        None
                    } else {
                        Some(value)
                    }
                })
            }
            ast::ExprKind::AggregateFunction {
                Name,
                Args,
                Distinct,
                ..
            } => self.relational_aggregate_value(
                Name,
                Args,
                *Distinct,
                group_rows
                    .ok_or_else(|| SessionError::new("aggregate expression has no input group"))?,
            ),
            ast::ExprKind::Subquery { .. } => {
                let result = self.execute_relational_subquery(expression, row)?;
                if result.rows.len() > 1 {
                    return Err(SessionError::new("Subquery returns more than 1 row"));
                }
                let Some(result_row) = result.rows.first() else {
                    return Ok(None);
                };
                let Some(column) = result.columns.first() else {
                    return Ok(None);
                };
                Ok(result_row.get(column).cloned().unwrap_or(None))
            }
            ast::ExprKind::CompareSubquery { Op, L, R, All } => {
                let left = self.relational_query_expression_value(L, row, group_rows)?;
                let result = self.execute_relational_subquery(R, row)?;
                let Some(column) = result.columns.first() else {
                    return Ok(relational_boolean_value(Some(*All)));
                };
                if result.rows.is_empty() {
                    return Ok(relational_boolean_value(Some(*All)));
                }
                let Some(left) = left else {
                    return Ok(None);
                };
                let compare = |right: &str| {
                    let ordering = relational_compare(&left, right);
                    match Op.as_str() {
                        "=" | "==" => ordering.is_eq(),
                        "!=" | "<>" => !ordering.is_eq(),
                        ">" => ordering.is_gt(),
                        ">=" => ordering.is_ge(),
                        "<" => ordering.is_lt(),
                        "<=" => ordering.is_le(),
                        _ => false,
                    }
                };
                let mut saw_null = false;
                for candidate in &result.rows {
                    let Some(candidate) = candidate.get(column).cloned().unwrap_or(None) else {
                        saw_null = true;
                        continue;
                    };
                    let matched = compare(&candidate);
                    if (*All && !matched) || (!*All && matched) {
                        return Ok(relational_boolean_value(Some(!*All)));
                    }
                }
                if saw_null {
                    Ok(None)
                } else {
                    Ok(relational_boolean_value(Some(*All)))
                }
            }
            ast::ExprKind::InSubquery { Expr, Sel, Not } => {
                let value = self.relational_query_expression_value(Expr, row, group_rows)?;
                let result = self.execute_relational_subquery(Sel, row)?;
                let Some(column) = result.columns.first() else {
                    return Ok(relational_boolean_value(Some(*Not)));
                };
                if result.rows.is_empty() {
                    return Ok(relational_boolean_value(Some(*Not)));
                }
                let Some(value) = value else {
                    return Ok(None);
                };
                let mut saw_null = false;
                for candidate in &result.rows {
                    let Some(candidate) = candidate.get(column).cloned().unwrap_or(None) else {
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
            ast::ExprKind::ExistsSubquery { Sel, Not } => {
                let result = self.execute_relational_subquery(Sel, row)?;
                Ok(relational_boolean_value(Some(if *Not {
                    result.rows.is_empty()
                } else {
                    !result.rows.is_empty()
                })))
            }
            ast::ExprKind::Case {
                Value,
                WhenClauses,
                ElseClause,
            } => {
                let case_value = Value
                    .as_ref()
                    .map(|value| self.relational_query_expression_value(value, row, group_rows))
                    .transpose()?;
                for clause in WhenClauses {
                    let matched = if let Some(case_value) = case_value.as_ref() {
                        let when_value =
                            self.relational_query_expression_value(&clause.Expr, row, group_rows)?;
                        case_value
                            .as_deref()
                            .zip(when_value.as_deref())
                            .is_some_and(|(case_value, when_value)| {
                                relational_compare(case_value, when_value).is_eq()
                            })
                    } else {
                        relational_truth(
                            self.relational_query_expression_value(&clause.Expr, row, group_rows)?
                                .as_deref(),
                        ) == Some(true)
                    };
                    if matched {
                        let value = self.relational_query_expression_value(
                            &clause.Result,
                            row,
                            group_rows,
                        )?;
                        return if self.sql_dialect() == ast::SqlDialect::PostgreSQL {
                            self.pg_union_cast_value(
                                value,
                                &self.pg_case_input_type(&clause.Result, &[]),
                                &self.pg_case_result_type(expression, &[])?,
                            )
                        } else {
                            Ok(value)
                        };
                    }
                }
                let value = ElseClause
                    .as_ref()
                    .map(|expr| self.relational_query_expression_value(expr, row, group_rows))
                    .unwrap_or(Ok(None))?;
                if self.sql_dialect() == ast::SqlDialect::PostgreSQL {
                    let source = ElseClause
                        .as_ref()
                        .map(|expr| self.pg_case_input_type(expr, &[]))
                        .unwrap_or_default();
                    self.pg_union_cast_value(
                        value,
                        &source,
                        &self.pg_case_result_type(expression, &[])?,
                    )
                } else {
                    Ok(value)
                }
            }
            ast::ExprKind::Parentheses(inner) => {
                self.relational_query_expression_value(inner, row, group_rows)
            }
            ast::ExprKind::Array { .. } => self.pg_array_value(expression, row),
            ast::ExprKind::Function { FnName, Args, .. }
                if self.sql_dialect() == ast::SqlDialect::PostgreSQL
                    && FnName.L == "coalesce"
                    && matches!(
                        self.relational_query_expression_type_from_sources(expression, &[])
                            .GetType(),
                        astersql_parser_mysql::r#type::TypeDatetime
                            | astersql_parser_mysql::r#type::TypePgTimestampTz
                    ) =>
            {
                for argument in Args {
                    let value =
                        self.relational_query_expression_value(argument, row, group_rows)?;
                    if value.is_some() {
                        return if pg_date_source(argument) {
                            let wall = pg_date_to_timestamp_value(value, -1)?;
                            if self
                                .relational_query_expression_type_from_sources(expression, &[])
                                .GetType()
                                == astersql_parser_mysql::r#type::TypePgTimestampTz
                            {
                                self.pg_timestamptz_relational_value(wall, -1)
                            } else {
                                Ok(wall)
                            }
                        } else if self
                            .relational_query_expression_type_from_sources(expression, &[])
                            .GetType()
                            == astersql_parser_mysql::r#type::TypePgTimestampTz
                            && self
                                .relational_query_expression_type_from_sources(argument, &[])
                                .GetType()
                                == astersql_parser_mysql::r#type::TypeDatetime
                        {
                            self.pg_timestamptz_relational_value(value, -1)
                        } else {
                            Ok(value)
                        };
                    }
                }
                Ok(None)
            }
            ast::ExprKind::Cast {
                Expr,
                Tp,
                FunctionType,
                ..
            } => {
                if self.sql_dialect() == ast::SqlDialect::PostgreSQL {
                    if matches!(
                        Tp.PostgreSQLArrayElement(),
                        Some(
                            astersql_parser_types::PostgreSQLArrayElement::Int4
                                | astersql_parser_types::PostgreSQLArrayElement::Int8
                                | astersql_parser_types::PostgreSQLArrayElement::Int2
                                | astersql_parser_types::PostgreSQLArrayElement::Bool
                                | astersql_parser_types::PostgreSQLArrayElement::Varchar
                                | astersql_parser_types::PostgreSQLArrayElement::Bpchar
                                | astersql_parser_types::PostgreSQLArrayElement::Text
                                | astersql_parser_types::PostgreSQLArrayElement::Bytea
                                | astersql_parser_types::PostgreSQLArrayElement::Float4
                                | astersql_parser_types::PostgreSQLArrayElement::Float8
                                | astersql_parser_types::PostgreSQLArrayElement::Date
                                | astersql_parser_types::PostgreSQLArrayElement::Time
                                | astersql_parser_types::PostgreSQLArrayElement::Timestamp
                                | astersql_parser_types::PostgreSQLArrayElement::Timestamptz
                                | astersql_parser_types::PostgreSQLArrayElement::Interval
                                | astersql_parser_types::PostgreSQLArrayElement::Uuid
                                | astersql_parser_types::PostgreSQLArrayElement::Json
                                | astersql_parser_types::PostgreSQLArrayElement::Jsonb
                                | astersql_parser_types::PostgreSQLArrayElement::Numeric
                        )
                    ) {
                        super::pg_array::validate_cast(Expr, Tp)?;
                        if Tp.PostgreSQLArrayElement()
                            == Some(astersql_parser_types::PostgreSQLArrayElement::Interval)
                        {
                            return super::pg_array::cast_value_in_session(
                                Expr,
                                row,
                                Tp,
                                None,
                                Some(self),
                                || self.relational_query_expression_value(Expr, row, group_rows),
                            );
                        }
                        if Tp.PostgreSQLArrayElement()
                            == Some(astersql_parser_types::PostgreSQLArrayElement::Timestamptz)
                        {
                            let value = super::pg_array::cast_value_in_session(
                                Expr,
                                row,
                                Tp,
                                None,
                                Some(self),
                                || self.relational_query_expression_value(Expr, row, group_rows),
                            )?;
                            return self.pg_timestamptz_array_display(value);
                        }
                        return super::pg_array::cast_value_with_context(
                            Expr,
                            row,
                            Tp,
                            if Tp.PostgreSQLArrayElement()
                                == Some(astersql_parser_types::PostgreSQLArrayElement::Date)
                            {
                                Some(self.pg_date_context(None)?)
                            } else {
                                None
                            },
                            || self.relational_query_expression_value(Expr, row, group_rows),
                        );
                    }
                    if Tp.GetType() == astersql_parser_mysql::r#type::TypePgJsonb {
                        return super::pg_jsonb::cast_value(
                            self.relational_query_expression_value(Expr, row, group_rows)?,
                        );
                    }
                    if Tp.GetType() == astersql_parser_mysql::r#type::TypePgJson {
                        return super::pg_json::cast_value(
                            self.relational_query_expression_value(Expr, row, group_rows)?,
                        );
                    }
                    if Tp.GetType() == astersql_parser_mysql::r#type::TypePgUuid {
                        return super::pg_uuid::cast_value(
                            self.relational_query_expression_value(Expr, row, group_rows)?,
                        );
                    }
                    if let Some(result) = pg_boolean_relational_value(
                        Expr,
                        Tp,
                        || self.relational_query_expression_value(Expr, row, group_rows),
                        true,
                    ) {
                        return result;
                    }
                }
                if self.sql_dialect() == ast::SqlDialect::PostgreSQL
                    && super::relational_value::pg_text_bytea_pair(
                        &self.relational_query_expression_type_from_sources(Expr, &[]),
                        Tp,
                    )
                {
                    return super::relational_value::pg_text_bytea_cast(
                        self.relational_query_expression_value(Expr, row, group_rows)?,
                    );
                }
                if self.sql_dialect() == ast::SqlDialect::PostgreSQL
                    && let Some(value) =
                        super::relational_value::binary_cast_literal_value(Expr, Tp)
                {
                    return Ok(Some(value));
                }
                if self.sql_dialect() == ast::SqlDialect::PostgreSQL
                    && Tp.GetType() == astersql_parser_mysql::r#type::TypeDate
                {
                    if pg_timestamptz_source(Expr) {
                        return self.pg_timestamptz_to_date_value(
                            self.relational_query_expression_value(Expr, row, group_rows)?,
                        );
                    }
                    if pg_timestamp_source(Expr) {
                        return pg_timestamp_to_date_value(
                            self.relational_query_expression_value(Expr, row, group_rows)?,
                        );
                    }
                    return pg_date_relational_value(
                        self.relational_query_expression_value(Expr, row, group_rows)?,
                        self.pg_date_context(None)?,
                    );
                }
                if self.sql_dialect() == ast::SqlDialect::PostgreSQL
                    && Tp.GetType() == astersql_parser_mysql::r#type::TypeDuration
                {
                    if pg_interval_source(Expr) {
                        return pg_interval_to_time_value(
                            self.relational_query_expression_value(Expr, row, group_rows)?,
                            Tp.GetDecimal(),
                        );
                    }
                    if pg_timestamptz_source(Expr) {
                        return self.pg_timestamptz_to_time_value(
                            self.relational_query_expression_value(Expr, row, group_rows)?,
                            Tp.GetDecimal(),
                        );
                    }
                    if pg_timestamp_source(Expr) {
                        return pg_timestamp_to_time_value(
                            self.relational_query_expression_value(Expr, row, group_rows)?,
                            Tp.GetDecimal(),
                        );
                    }
                    return pg_time_relational_value(
                        self.relational_query_expression_value(Expr, row, group_rows)?,
                        Tp.GetDecimal(),
                    );
                }
                if self.sql_dialect() == ast::SqlDialect::PostgreSQL
                    && Tp.GetType() == astersql_parser_mysql::r#type::TypePgInterval
                {
                    return self
                        .relational_query_expression_value(Expr, row, group_rows)?
                        .map(|text| {
                            astersql_types::pg_temporal::PgInterval::parse_with_style(
                                &text,
                                self.pg_interval_style(None),
                            )
                            .and_then(|v| v.apply_typmod(Tp.GetFlen(), Tp.GetDecimal()))
                            .map(|v| v.to_string())
                            .map_err(|e| {
                                SessionError::with_source(
                                    e.to_string(),
                                    astersql_errors::SharedError::new(e),
                                )
                            })
                        })
                        .transpose();
                }
                if self.sql_dialect() == ast::SqlDialect::PostgreSQL
                    && Tp.GetType() == astersql_parser_mysql::r#type::TypePgTimestampTz
                {
                    let value = self.relational_query_expression_value(Expr, row, group_rows)?;
                    let value = if pg_date_source(Expr) {
                        pg_date_to_timestamp_value(value, -1)?
                    } else {
                        value
                    };
                    return self
                        .pg_timestamptz_relational_value(value, Tp.GetDecimal())
                        .map_err(|error| super::pg_temporal::pg_temporal_input_error(error, Expr));
                }
                if self.sql_dialect() == ast::SqlDialect::PostgreSQL
                    && Tp.GetType() == astersql_parser_mysql::r#type::TypeDatetime
                {
                    let value = self.relational_query_expression_value(Expr, row, group_rows)?;
                    if pg_date_source(Expr) {
                        return pg_date_to_timestamp_value(value, Tp.GetDecimal());
                    }
                    if pg_timestamptz_source(Expr) {
                        return self.pg_timestamptz_to_timestamp_value(value, Tp.GetDecimal());
                    }
                    return pg_timestamp_relational_value(
                        value,
                        Tp.GetDecimal(),
                        self.pg_date_context(None)?,
                    );
                }
                if Tp.PostgreSQLTextOid().is_some() {
                    if self.sql_dialect() == ast::SqlDialect::PostgreSQL
                        && self
                            .relational_query_expression_type_from_sources(Expr, &[])
                            .GetType()
                            == astersql_parser_mysql::r#type::TypePgInterval
                    {
                        let formatted = self.pg_interval_output(
                            self.relational_query_expression_value(Expr, row, group_rows)?,
                        )?;
                        return pg_text_relational_cast(Expr, row, formatted, Tp, true);
                    }
                    return pg_text_relational_cast(
                        Expr,
                        row,
                        self.relational_query_expression_value(Expr, row, group_rows)?,
                        Tp,
                        true,
                    );
                }
                if *FunctionType == ast::CastFunctionType::PostgreSQLFloat8 {
                    return pg_float8_relational_value(
                        Expr,
                        self.relational_query_expression_value(Expr, row, group_rows)?,
                    );
                }
                if *FunctionType == ast::CastFunctionType::PostgreSQLFloat4 {
                    return pg_float4_relational_value(
                        Expr,
                        self.relational_query_expression_value(Expr, row, group_rows)?,
                    );
                }
                if *FunctionType == ast::CastFunctionType::PostgreSQLNumeric {
                    return pg_numeric_source_value(
                        Expr,
                        self.relational_query_expression_value(Expr, row, group_rows)?,
                        Tp,
                    );
                }
                if matches!(
                    FunctionType,
                    ast::CastFunctionType::PostgreSQLInt2
                        | ast::CastFunctionType::PostgreSQLInt4
                        | ast::CastFunctionType::PostgreSQLInt8
                ) {
                    return pg_integer_relational_value(
                        Expr,
                        self.relational_query_expression_value(Expr, row, group_rows)?,
                        *FunctionType,
                    );
                }
                if relational_expression_is_vector(Expr)
                    && matches!(
                        Tp.GetType(),
                        astersql_parser_mysql::r#type::TypeDate
                            | astersql_parser_mysql::r#type::TypeDatetime
                            | astersql_parser_mysql::r#type::TypeTimestamp
                            | astersql_parser_mysql::r#type::TypeDuration
                    )
                {
                    return Err(SessionError::new("cannot cast VECTOR to temporal type"));
                }
                let value = self.relational_query_expression_value(Expr, row, group_rows)?;
                let Some(value) = value else {
                    return Ok(None);
                };
                if matches!(
                    Tp.GetType(),
                    astersql_parser_mysql::r#type::TypeDate
                        | astersql_parser_mysql::r#type::TypeDatetime
                        | astersql_parser_mysql::r#type::TypeTimestamp
                        | astersql_parser_mysql::r#type::TypeDuration
                ) {
                    let decoded =
                        serde_json::from_str::<String>(&value).unwrap_or_else(|_| value.clone());
                    let (kind, temporal) = [
                        ("DATE", "__ASTER_TEMPORAL_DATE__:"),
                        ("DATETIME", "__ASTER_TEMPORAL_DATETIME__:"),
                        ("TIME", "__ASTER_TEMPORAL_TIME__:"),
                    ]
                    .into_iter()
                    .find_map(|(kind, prefix)| {
                        decoded
                            .strip_prefix(prefix)
                            .map(|temporal| (kind, temporal.to_owned()))
                    })
                    .unwrap_or(("STRING", decoded));
                    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
                    let converted = match Tp.GetType() {
                        astersql_parser_mysql::r#type::TypeDate => match kind {
                            "TIME" => today,
                            "STRING" if temporal.len() == 8 && temporal.contains(':') => {
                                "2012-12-12".to_owned()
                            }
                            _ => temporal.chars().take(10).collect(),
                        },
                        astersql_parser_mysql::r#type::TypeDatetime
                        | astersql_parser_mysql::r#type::TypeTimestamp => match kind {
                            "DATE" => format!("{temporal} 00:00:00"),
                            "TIME" => format!("{today} {temporal}"),
                            "STRING" if temporal.len() == 8 && temporal.contains(':') => {
                                "2012-12-12 00:00:00".to_owned()
                            }
                            _ if temporal.len() == 10 => format!("{temporal} 00:00:00"),
                            _ => temporal,
                        },
                        astersql_parser_mysql::r#type::TypeDuration => {
                            if temporal.len() >= 19 {
                                temporal[11..19].to_owned()
                            } else if kind == "DATE" {
                                "00:00:00".to_owned()
                            } else {
                                temporal
                            }
                        }
                        _ => unreachable!("temporal cast type checked"),
                    };
                    Ok(Some(converted))
                } else {
                    Ok(Some(value))
                }
            }
            ast::ExprKind::Function { FnName, Args, .. }
                if FnName.L.eq_ignore_ascii_case("hex") && Args.len() == 1 =>
            {
                let Some(value) =
                    self.relational_query_expression_value(&Args[0], row, group_rows)?
                else {
                    return Ok(None);
                };
                Ok(Some(relational_hex_value(&Args[0], &value)))
            }
            ast::ExprKind::Function { FnName, Args, .. }
                if FnName.L.eq_ignore_ascii_case("database") && Args.is_empty() =>
            {
                let database = self.current_database();
                Ok((!database.is_empty()).then_some(database))
            }
            ast::ExprKind::Function { FnName, Args, .. }
                if FnName.L.eq_ignore_ascii_case("any_value") && Args.len() == 1 =>
            {
                let candidate = group_rows.and_then(|rows| rows.first()).unwrap_or(row);
                self.relational_query_expression_value(&Args[0], candidate, None)
            }
            ast::ExprKind::Function { FnName, Args, .. }
                if FnName.L.eq_ignore_ascii_case("json_type") && Args.len() == 1 =>
            {
                let Some(value) =
                    self.relational_query_expression_value(&Args[0], row, group_rows)?
                else {
                    return Ok(None);
                };
                let decoded =
                    serde_json::from_str::<String>(&value).unwrap_or_else(|_| value.clone());
                let kind = if decoded.starts_with("__ASTER_TEMPORAL_DATE__:") {
                    "DATE"
                } else if decoded.starts_with("__ASTER_TEMPORAL_DATETIME__:") {
                    "DATETIME"
                } else if decoded.starts_with("__ASTER_TEMPORAL_TIME__:") {
                    "TIME"
                } else if serde_json::from_str::<String>(&value).is_ok() {
                    "STRING"
                } else if value.starts_with('{') {
                    "OBJECT"
                } else if value.starts_with('[') {
                    "ARRAY"
                } else {
                    "STRING"
                };
                Ok(Some(kind.to_owned()))
            }
            ast::ExprKind::Function { FnName, Args, .. }
                if FnName.L.eq_ignore_ascii_case("str_to_date") =>
            {
                if Args.len() != 2 {
                    return Err(SessionError::new(
                        "Incorrect parameter count in the call to native function 'str_to_date'",
                    ));
                }
                let value = self.relational_query_expression_value(&Args[0], row, group_rows)?;
                let format = self.relational_query_expression_value(&Args[1], row, group_rows)?;
                if let (Some(value), Some(format)) = (value, format)
                    && format == "%Y-%m-%d"
                    && value.len() == 10
                    && value.as_bytes().get(4) == Some(&b'-')
                    && value.as_bytes().get(7) == Some(&b'-')
                    && value
                        .split('-')
                        .collect::<Vec<_>>()
                        .into_iter()
                        .all(|part| part.parse::<u32>().is_ok())
                {
                    return Ok(Some(value));
                }
                self.state
                    .borrow_mut()
                    .current_warnings
                    .push(SessionWarning {
                    level: "Warning",
                    code: 1411,
                    message:
                        "Incorrect datetime value: '0000-00-00 00:00:00' for function str_to_date"
                            .to_owned(),
                });
                Ok(None)
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
                    let Some(left) =
                        self.relational_query_expression_value(left, row, group_rows)?
                    else {
                        return Ok(None);
                    };
                    let Some(right) =
                        self.relational_query_expression_value(right, row, group_rows)?
                    else {
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
                let left = self.relational_query_expression_value(L, row, group_rows)?;
                let right = self.relational_query_expression_value(R, row, group_rows)?;
                let compared = left.as_deref().zip(right.as_deref()).map(|(left, right)| {
                    let ordering = if relational_expression_is_string(L, row)
                        && relational_expression_is_string(R, row)
                    {
                        left.cmp(right)
                    } else {
                        relational_expression_compare(L, R, left, right, row)
                    };
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
            ast::ExprKind::Binary { Op, L, R } if Op.eq_ignore_ascii_case("and") => {
                let left = relational_truth(
                    self.relational_query_expression_value(L, row, group_rows)?
                        .as_deref(),
                );
                if left == Some(false) {
                    return Ok(relational_boolean_value(Some(false)));
                }
                let right = relational_truth(
                    self.relational_query_expression_value(R, row, group_rows)?
                        .as_deref(),
                );
                Ok(relational_boolean_value(match (left, right) {
                    (_, Some(false)) => Some(false),
                    (Some(true), Some(true)) => Some(true),
                    _ => None,
                }))
            }
            ast::ExprKind::Binary { Op, L, R } if Op.eq_ignore_ascii_case("or") => {
                let left = relational_truth(
                    self.relational_query_expression_value(L, row, group_rows)?
                        .as_deref(),
                );
                if left == Some(true) {
                    return Ok(relational_boolean_value(Some(true)));
                }
                let right = relational_truth(
                    self.relational_query_expression_value(R, row, group_rows)?
                        .as_deref(),
                );
                Ok(relational_boolean_value(match (left, right) {
                    (_, Some(true)) => Some(true),
                    (Some(false), Some(false)) => Some(false),
                    _ => None,
                }))
            }
            ast::ExprKind::Binary { Op, L, R } => {
                let left = self.relational_query_expression_value(L, row, group_rows)?;
                let right = self.relational_query_expression_value(R, row, group_rows)?;
                let evaluated = ast::ExprNode {
                    node_text: Default::default(),
                    Kind: ast::ExprKind::Binary {
                        Op: Op.clone(),
                        L: Box::new(ast::NewValueExpr(left, "utf8mb4", "utf8mb4_bin")),
                        R: Box::new(ast::NewValueExpr(right, "utf8mb4", "utf8mb4_bin")),
                    },
                    OriginTextPosition: expression.OriginTextPosition,
                    Flag: Default::default(),
                };
                relational_expression_value(&evaluated, &HashMap::new())
            }
            ast::ExprKind::Unary { Op, V } => {
                let value = self.relational_query_expression_value(V, row, group_rows)?;
                let evaluated = ast::ExprNode {
                    node_text: Default::default(),
                    Kind: ast::ExprKind::Unary {
                        Op: Op.clone(),
                        V: Box::new(ast::NewValueExpr(value, "utf8mb4", "utf8mb4_bin")),
                    },
                    OriginTextPosition: expression.OriginTextPosition,
                    Flag: Default::default(),
                };
                relational_expression_value(&evaluated, &HashMap::new())
            }
            ast::ExprKind::IsNull { Expr, Not } => {
                let value = self.relational_query_expression_value(Expr, row, group_rows)?;
                Ok(relational_boolean_value(Some(if *Not {
                    value.is_some()
                } else {
                    value.is_none()
                })))
            }
            ast::ExprKind::IsTruth {
                Expr, Not, True, ..
            } => {
                let value = self.relational_query_expression_value(Expr, row, group_rows)?;
                let truth = relational_truth(value.as_deref()).unwrap_or(false);
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
                let values =
                    relational_row_items(Expr).unwrap_or_else(|| std::slice::from_ref(Expr));
                let value = values
                    .iter()
                    .map(|expression| {
                        self.relational_query_expression_value(expression, row, group_rows)
                    })
                    .collect::<SessionResult<Vec<_>>>()?;
                let mut saw_null = value.iter().any(Option::is_none);
                for candidate in List {
                    let candidates = relational_row_items(candidate)
                        .unwrap_or_else(|| std::slice::from_ref(candidate));
                    if candidates.len() != value.len() {
                        return Err(SessionError::new(
                            "Operand should contain the same number of columns",
                        ));
                    }
                    let candidate = candidates
                        .iter()
                        .map(|expression| {
                            self.relational_query_expression_value(expression, row, group_rows)
                        })
                        .collect::<SessionResult<Vec<_>>>()?;
                    let mut matched = true;
                    let mut candidate_has_null = false;
                    for (left, right) in value.iter().zip(&candidate) {
                        match (left.as_deref(), right.as_deref()) {
                            (Some(left), Some(right))
                                if relational_compare(left, right).is_eq() => {}
                            (Some(_), Some(_)) => {
                                matched = false;
                                break;
                            }
                            _ => candidate_has_null = true,
                        }
                    }
                    if matched && !candidate_has_null {
                        return Ok(relational_boolean_value(Some(!*Not)));
                    }
                    saw_null |= matched && candidate_has_null;
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
                let value = self.relational_query_expression_value(Expr, row, group_rows)?;
                let left = self.relational_query_expression_value(Left, row, group_rows)?;
                let right = self.relational_query_expression_value(Right, row, group_rows)?;
                let matched = value
                    .as_deref()
                    .zip(left.as_deref())
                    .zip(right.as_deref())
                    .map(|((value, left), right)| {
                        relational_compare(value, left).is_ge()
                            && relational_compare(value, right).is_le()
                    });
                Ok(relational_boolean_value(
                    matched.map(|matched| matched != *Not),
                ))
            }
            _ if relational_expression_has_aggregate(expression)
                || relational_expression_has_subquery(expression) =>
            {
                Err(SessionError::new(format!(
                    "unsupported aggregate/subquery expression {:?}",
                    expression.Kind
                )))
            }
            _ => relational_expression_value(expression, row),
        }
    }

    pub(super) fn relational_window_query_expression_value(
        &self,
        expression: &ast::ExprNode,
        rows: &[RelationalRow],
        row_index: usize,
        named_specs: &[ast::WindowSpec],
    ) -> SessionResult<Option<String>> {
        match &expression.Kind {
            ast::ExprKind::WindowFunction { .. } => relational_window_value(
                expression,
                rows,
                row_index,
                named_specs,
                |expression, row| self.relational_query_expression_value(expression, row, None),
            ),
            ast::ExprKind::Function { FnName, Args, .. }
                if FnName.L == "coalesce" || FnName.L == "ifnull" =>
            {
                for argument in Args {
                    if let Some(value) = self.relational_window_query_expression_value(
                        argument,
                        rows,
                        row_index,
                        named_specs,
                    )? {
                        return Ok(Some(value));
                    }
                }
                Ok(None)
            }
            ast::ExprKind::Case {
                Value,
                WhenClauses,
                ElseClause,
            } => {
                let row = &rows[row_index].1;
                let case_value = Value
                    .as_ref()
                    .map(|value| self.relational_query_expression_value(value, row, None))
                    .transpose()?;
                for clause in WhenClauses {
                    let when_value =
                        self.relational_query_expression_value(&clause.Expr, row, None)?;
                    let matched = if let Some(case_value) = case_value.as_ref() {
                        case_value
                            .as_deref()
                            .zip(when_value.as_deref())
                            .is_some_and(|(case_value, when_value)| {
                                relational_compare(case_value, when_value).is_eq()
                            })
                    } else {
                        relational_truth(when_value.as_deref()) == Some(true)
                    };
                    if matched {
                        return self.relational_window_query_expression_value(
                            &clause.Result,
                            rows,
                            row_index,
                            named_specs,
                        );
                    }
                }
                ElseClause
                    .as_ref()
                    .map(|expression| {
                        self.relational_window_query_expression_value(
                            expression,
                            rows,
                            row_index,
                            named_specs,
                        )
                    })
                    .unwrap_or(Ok(None))
            }
            _ => self.relational_query_expression_value(expression, &rows[row_index].1, None),
        }
    }

    /// Apply a SELECT predicate through the query evaluator so scalar,
    /// correlated, IN, and EXISTS subqueries retain the same semantics as
    /// projected expressions.
    pub(super) fn filter_relational_query_rows(
        &self,
        rows: Vec<RelationalRow>,
        predicate: &ast::ExprNode,
        table: &astersql_meta_model::TableInfo,
    ) -> SessionResult<Vec<RelationalRow>> {
        let aggregate_rows = relational_expression_has_aggregate(predicate)
            .then(|| rows.iter().map(|(_, row)| row.clone()).collect::<Vec<_>>());
        let mut filtered = Vec::with_capacity(rows.len());
        for row in rows {
            let value =
                if aggregate_rows.is_none() && !relational_expression_has_subquery(predicate) {
                    relational_table_expression_value(predicate, &row.1, table)?
                } else {
                    self.relational_query_expression_value(
                        predicate,
                        &row.1,
                        aggregate_rows.as_deref(),
                    )?
                };
            if relational_truth(value.as_deref()) == Some(true) {
                filtered.push(row);
            }
        }
        Ok(filtered)
    }

    pub(super) fn qualify_insert_select_rows(
        &self,
        mut result: InsertSelectRows,
        qualifier: &str,
    ) -> InsertSelectRows {
        if qualifier.is_empty() {
            return result;
        }
        for row in &mut result.rows {
            // Internal handles are not projected, but joined DML must retain
            // each table's qualified handle when both use `_tidb_rowid`.
            let qualified = row
                .iter()
                .filter(|(column, _)| !column.starts_with(RELATIONAL_STRING_COLUMN_PREFIX))
                .map(|(column, value)| (format!("{qualifier}.{column}"), value.clone()))
                .collect::<Vec<_>>();
            row.extend(qualified);
            for column in &result.columns {
                if row.contains_key(&relational_string_column_marker(column)) {
                    row.insert(
                        relational_string_column_marker(&format!("{qualifier}.{column}")),
                        Some("1".to_owned()),
                    );
                }
            }
        }
        result
    }

    pub(super) fn execute_insert_select_node(
        &self,
        node: &dyn ast::Node,
    ) -> SessionResult<InsertSelectRows> {
        self.execute_insert_select_node_with_outer(node, None)
    }

    pub(crate) fn relational_select_requires_full_query(&self, select: &ast::SelectStmt) -> bool {
        fn has_pg_query_expression(expression: &ast::ExprNode) -> bool {
            struct Calls(bool);
            impl ast::ExprNodeVisitor for Calls {
                fn Enter(&mut self, input: &ast::ExprNode) -> (ast::ExprNode, bool) {
                    if matches!(input.Kind, ast::ExprKind::Case { .. }) {
                        self.0 = true;
                    }
                    if let ast::ExprKind::Function { FnName, Schema, .. } = &input.Kind
                        && matches!(
                            FnName.L.as_str(),
                            "int4not" | "int8not" | "float4abs" | "float8abs" | "numeric_abs"
                        )
                        && (Schema.L.is_empty() || Schema.L == "pg_catalog")
                    {
                        self.0 = true;
                    }
                    (input.clone(), self.0)
                }
                fn Leave(&mut self, input: &ast::ExprNode) -> (ast::ExprNode, bool) {
                    (input.clone(), true)
                }
            }
            let mut calls = Calls(false);
            let _ = expression.Accept(&mut calls);
            calls.0
        }
        let mut sources = Vec::new();
        if let Some(from) = select.From.as_ref() {
            if let Some(left) = from.TableRefs.Left.as_deref() {
                collect_physical_table_sources(left, &mut sources);
            }
            if let Some(right) = from.TableRefs.Right.as_deref() {
                collect_physical_table_sources(right, &mut sources);
            }
        }
        let current_database = self.current_database();
        let has_view_source = sources.iter().any(|source| {
            let database = if source.Source.Schema.L.is_empty() {
                current_database.as_str()
            } else {
                source.Source.Schema.L.as_str()
            };
            self.mdl_stats_table(database, relation_name(&source.Source))
                .is_some_and(|(_, table)| table.View.is_some())
        });
        let has_derived_source = select.From.as_ref().is_some_and(|from| {
            from.TableRefs
                .Left
                .as_deref()
                .is_some_and(result_set_node_has_derived_source)
                || from
                    .TableRefs
                    .Right
                    .as_deref()
                    .is_some_and(result_set_node_has_derived_source)
        });
        let scalar_count = is_scalar_count_non_null_constant(select);
        (self.sql_dialect() == ast::SqlDialect::PostgreSQL
            && select.Kind == ast::SelectStmtKind::Values)
            || select.With.is_some()
            || (self.sql_dialect() == ast::SqlDialect::PostgreSQL
                && (select.Where.as_ref().is_some_and(|expr| {
                    expression_contains_cast(expr) || has_pg_query_expression(expr)
                }) || select.Fields.Fields.iter().any(|field| {
                    field.Expr.as_ref().is_some_and(|expr| {
                        expression_contains_cast(expr) || has_pg_query_expression(expr)
                    })
                })))
            // MySQL-only functions keep their formal result identity even for
            // constant projections and empty results, rather than the legacy
            // scalar path's untyped protocol fallback. CONCAT/extrema also use
            // this existing SELECT filter so WHERE false suppresses their rows.
            || (self.sql_dialect() == ast::SqlDialect::MySQL
                && select.Fields.Fields.iter().filter_map(|field| field.Expr.as_ref())
                    .any(|expr| self.is_mysql_function_projection(expr, &[]) || self.is_mysql_signed_abs(expr, &[]) || self.is_mysql_signed_nullif(expr, &[]) || self.is_mysql_conditional_projection(expr, &[]) || (select.Where.is_some() && ["concat", "greatest", "least"].iter().any(|name| expression_contains_function(expr, name))) || expression_contains_function(expr, "ifnull") || expression_contains_function(expr, "date_format") || expression_contains_function(expr, "json_extract") || expression_contains_function(expr, "tidb_version")))
            || has_derived_source
            || select
                .From
                .as_ref()
                .is_some_and(|from| from.TableRefs.Right.is_some())
            || sources.len() > 1
            || has_view_source
            || !select.GroupBy.is_empty()
            || select.Having.is_some()
            || select
                .Fields
                .Fields
                .iter()
                .filter_map(|field| field.Expr.as_ref())
                .any(|expression| {
                    (relational_expression_has_aggregate(expression) && !scalar_count)
                        || relational_expression_has_subquery(expression)
                        || relational_expression_has_tidb_row_checksum(expression)
                        || relational_expression_has_json_temporal(expression)
                })
            || select.Where.as_ref().is_some_and(|expression| {
                relational_expression_has_aggregate(expression)
                    || relational_expression_has_subquery(expression)
                    || relational_expression_has_tidb_row_checksum(expression)
            })
    }

    fn collect_full_query_node_stale_read(
        &self,
        node: &dyn ast::Node,
        has_stale_source: &mut bool,
        has_current_source: &mut bool,
        statement_read_ts: &mut Option<u64>,
    ) -> SessionResult<()> {
        if let Some(select) = node.as_any().downcast_ref::<ast::SelectStmt>() {
            if let Some(from) = select.From.as_ref() {
                if let Some(left) = from.TableRefs.Left.as_deref() {
                    self.collect_full_query_stale_read(
                        left,
                        has_stale_source,
                        has_current_source,
                        statement_read_ts,
                    )?;
                }
                if let Some(right) = from.TableRefs.Right.as_deref() {
                    self.collect_full_query_stale_read(
                        right,
                        has_stale_source,
                        has_current_source,
                        statement_read_ts,
                    )?;
                }
            }
            return Ok(());
        }
        let list = if let Some(statement) = node.as_any().downcast_ref::<ast::SetOprStmt>() {
            Some(&statement.select_list)
        } else {
            node.as_any().downcast_ref::<ast::SetOprSelectList>()
        };
        let Some(list) = list else {
            return Err(SessionError::new(
                "derived table query must be SELECT or set operation",
            ));
        };
        for select in &list.selects {
            self.collect_full_query_node_stale_read(
                select.as_ref(),
                has_stale_source,
                has_current_source,
                statement_read_ts,
            )?;
        }
        Ok(())
    }

    pub(super) fn collect_full_query_stale_read(
        &self,
        node: &ast::ResultSetNode,
        has_stale_source: &mut bool,
        has_current_source: &mut bool,
        statement_read_ts: &mut Option<u64>,
    ) -> SessionResult<()> {
        match node {
            ast::ResultSetNode::TableSource(source) => {
                if let Some(query) = source.QuerySource.as_ref() {
                    return query
                        .with_node(|node| {
                            self.collect_full_query_node_stale_read(
                                node,
                                has_stale_source,
                                has_current_source,
                                statement_read_ts,
                            )
                        })
                        .unwrap_or_else(|| {
                            Err(SessionError::new("derived table query is unavailable"))
                        });
                }
                let Some(as_of) = source.AsOf.as_ref() else {
                    *has_current_source = true;
                    return Ok(());
                };
                *has_stale_source = true;
                let read_ts = self.evaluate_stale_read_ts(&as_of.TsExpr)?;
                if statement_read_ts.is_some_and(|first_read_ts| first_read_ts != read_ts) {
                    return Err(SessionError::new(
                        "can not set different time in the as of clause",
                    ));
                }
                statement_read_ts.get_or_insert(read_ts);
                Ok(())
            }
            ast::ResultSetNode::Join(join) => {
                if let Some(left) = join.Left.as_deref() {
                    self.collect_full_query_stale_read(
                        left,
                        has_stale_source,
                        has_current_source,
                        statement_read_ts,
                    )?;
                }
                if let Some(right) = join.Right.as_deref() {
                    self.collect_full_query_stale_read(
                        right,
                        has_stale_source,
                        has_current_source,
                        statement_read_ts,
                    )?;
                }
                Ok(())
            }
        }
    }

    pub(super) fn full_query_statement_read_ts(
        &self,
        select: &ast::SelectStmt,
    ) -> SessionResult<Option<u64>> {
        let mut has_stale_source = false;
        let mut has_current_source = false;
        let mut statement_read_ts = None;
        if let Some(from) = select.From.as_ref() {
            if let Some(left) = from.TableRefs.Left.as_deref() {
                self.collect_full_query_stale_read(
                    left,
                    &mut has_stale_source,
                    &mut has_current_source,
                    &mut statement_read_ts,
                )?;
            }
            if let Some(right) = from.TableRefs.Right.as_deref() {
                self.collect_full_query_stale_read(
                    right,
                    &mut has_stale_source,
                    &mut has_current_source,
                    &mut statement_read_ts,
                )?;
            }
        }
        if has_stale_source && has_current_source {
            return Err(SessionError::new(
                "can not set different time in the as of clause",
            ));
        }
        if has_stale_source {
            let mut state = self.state.borrow_mut();
            if state.transaction.is_some() {
                return Err(SessionError::new(
                    "select as of timestamp is not allowed in a transaction",
                ));
            }
            if state.pending_stale_read_ts.take().is_some() {
                return Err(SessionError::new(
                    "can't use select as of while already set transaction as of",
                ));
            }
            return Ok(statement_read_ts);
        }
        if self.state.borrow().transaction.is_none() {
            return Ok(self.state.borrow_mut().pending_stale_read_ts.take());
        }
        Ok(None)
    }

    fn pg_extremum_result_type(
        &self,
        args: &[ast::ExprNode],
        sources: &[ConcreteResultField],
    ) -> SessionResult<astersql_parser_types::FieldType> {
        super::relational_value::pg_extremum_common_type(
            args.iter().map(|arg| self.pg_case_input_type(arg, sources)),
        )
    }

    // PostgreSQL considers ELSE before the WHEN results. Unknown literals do
    // not become text until the entire CASE has been considered.
    fn pg_case_result_type(
        &self,
        expression: &ast::ExprNode,
        sources: &[ConcreteResultField],
    ) -> SessionResult<astersql_parser_types::FieldType> {
        let ast::ExprKind::Case {
            WhenClauses,
            ElseClause,
            ..
        } = &expression.Kind
        else {
            unreachable!("CASE result type requires CASE");
        };
        let inputs = ElseClause
            .iter()
            .map(|expr| expr.as_ref())
            .chain(WhenClauses.iter().map(|clause| &clause.Result))
            .map(|expr| self.pg_case_input_type(expr, sources))
            .collect::<Vec<_>>();
        let mut known = inputs
            .iter()
            .filter(|tp| tp.GetType() != astersql_parser_mysql::r#type::TypeNull);
        let mut target = if let Some(first) = known.next() {
            let mut target = first.clone();
            for input in known {
                target = pg_union_common_type(&target, input)?;
            }
            target
        } else {
            let unknown =
                astersql_parser_types::NewFieldType(astersql_parser_mysql::r#type::TypeNull);
            pg_union_common_type(&unknown, &unknown)?
        };
        // CASE preserves a modifier only when every result has that identity
        // and modifier, including any implicit NULL ELSE.
        if ElseClause.is_none() || inputs.iter().any(|tp| tp != &target) {
            target.SetFlen(-1);
            target.SetDecimal(-1);
        }
        Ok(target)
    }

    fn pg_values_fields(
        &self,
        select: &ast::SelectStmt,
    ) -> SessionResult<Vec<ConcreteResultField>> {
        if select.lock_info.is_some() || select.SelectIntoOpt.is_some() {
            return Err(pg_union_error(
                "VALUES locking or INTO is unsupported",
                "0A000",
            ));
        }
        let width = select.Lists.first().map_or(0, |row| row.Values.len());
        if select.Lists.iter().any(|row| row.Values.len() != width) {
            return Err(pg_union_error(
                "VALUES lists must all be the same length",
                "42601",
            ));
        }
        (0..width)
            .map(|column| {
                let inputs = select
                    .Lists
                    .iter()
                    .map(|row| self.pg_case_input_type(&row.Values[column], &[]))
                    .collect::<Vec<_>>();
                let mut known = inputs
                    .iter()
                    .filter(|tp| tp.GetType() != astersql_parser_mysql::r#type::TypeNull);
                let unknown =
                    astersql_parser_types::NewFieldType(astersql_parser_mysql::r#type::TypeNull);
                let mut target = if let Some(first) = known.next() {
                    let mut target = first.clone();
                    for input in known {
                        target = pg_union_common_type(&target, input)?;
                    }
                    target
                } else {
                    pg_union_common_type(&unknown, &unknown)?
                };
                if inputs.iter().any(|tp| tp != &target) {
                    target.SetFlen(-1);
                    target.SetDecimal(-1);
                }
                // Unknown string input functions run during analysis, before ParseComplete.
                for row in &select.Lists {
                    let mut expression = &row.Values[column];
                    while let ast::ExprKind::Parentheses(inner) = &expression.Kind {
                        expression = inner;
                    }
                    if let ast::ExprKind::Value(value) = &expression.Kind
                        && let ast::ValueDatum::String(text) = &value.Datum
                    {
                        self.pg_union_cast_value(Some(text.clone()), &unknown, &target)?;
                    }
                }
                Ok(ConcreteResultField {
                    column: astersql_meta_model::ColumnInfo {
                        FieldType: target,
                        ..Default::default()
                    },
                    column_as_name: ast::NewCIStr(&format!("column{}", column + 1)),
                    table_name: ast::CIStr::default(),
                    table_as_name: ast::CIStr::default(),
                    db_name: ast::CIStr::default(),
                    column_origin: None,
                })
            })
            .collect()
    }

    // Three-argument date_trunc has one candidate. Keep untyped parameters
    // unknown until protocol constraints assign their source identity.
    fn pg_date_trunc_input_type(
        &self,
        expression: &ast::ExprNode,
        sources: &[ConcreteResultField],
    ) -> u8 {
        match &expression.Kind {
            ast::ExprKind::Parentheses(inner) => self.pg_date_trunc_input_type(inner, sources),
            ast::ExprKind::ParamMarker { .. } => astersql_parser_mysql::r#type::TypeNull,
            _ => self.pg_case_input_type(expression, sources).GetType(),
        }
    }

    fn pg_case_input_type(
        &self,
        expression: &ast::ExprNode,
        sources: &[ConcreteResultField],
    ) -> astersql_parser_types::FieldType {
        match &expression.Kind {
            ast::ExprKind::Parentheses(inner) => self.pg_case_input_type(inner, sources),
            ast::ExprKind::Column(column) if !sources.is_empty() => sources
                .iter()
                .find(|field| {
                    (field.column_as_name.L == column.Name.L
                        || field.column.Name.L == column.Name.L)
                        && (column.Table.L.is_empty() || field.table_as_name.L == column.Table.L)
                })
                .map(|field| field.column.FieldType.clone())
                .unwrap_or_default(),
            ast::ExprKind::Column(_) if sources.is_empty() => PgCastTable::column_type(expression)
                .unwrap_or_else(|| {
                    self.relational_query_expression_type_from_sources(expression, sources)
                }),
            ast::ExprKind::Value(value)
                if matches!(
                    value.Datum,
                    ast::ValueDatum::Null | ast::ValueDatum::String(_)
                ) =>
            {
                astersql_parser_types::NewFieldType(astersql_parser_mysql::r#type::TypeNull)
            }
            _ => self.relational_query_expression_type_from_sources(expression, sources),
        }
    }

    /// Reject unsupported numeric source types before row evaluation, including
    /// empty scans: relational rows carry text and cannot recover source types.
    pub(super) fn validate_pg_integer_select_inputs(
        &self,
        select: &ast::SelectStmt,
    ) -> SessionResult<()> {
        if self.sql_dialect() != ast::SqlDialect::PostgreSQL {
            return Ok(());
        }
        let expressions = select
            .Fields
            .Fields
            .iter()
            .filter_map(|field| field.Expr.as_ref())
            .chain(select.Where.as_ref())
            .collect::<Vec<_>>();
        let sources = self.relational_select_source_fields(select)?;
        let mut table = astersql_meta_model::TableInfo::default();
        table.Columns = sources.iter().map(|field| field.column.clone()).collect();
        // Comparisons already produce SQL predicates, whereas scalar integer
        // expressions cannot acquire boolean meaning merely by appearing in WHERE.
        fn scalar_predicate(expr: &ast::ExprNode) -> bool {
            match &expr.Kind {
                ast::ExprKind::Parentheses(inner) | ast::ExprKind::Collate { Expr: inner, .. } => {
                    scalar_predicate(inner)
                }
                ast::ExprKind::Unary { Op, .. } => matches!(Op.as_str(), "+" | "-"),
                ast::ExprKind::Binary { Op, .. } => {
                    matches!(Op.as_str(), "+" | "-" | "*" | "/" | "%")
                }
                ast::ExprKind::Value(_) | ast::ExprKind::Cast { .. } | ast::ExprKind::Column(_) => {
                    true
                }
                _ => false,
            }
        }
        if let Some(predicate) = &select.Where {
            let source = self.relational_query_expression_type_from_sources(predicate, &sources);
            if source.EvalType() == astersql_parser_types::eval_type::ETInt
                && source.GetFlag() & astersql_parser_mysql::r#type::IsBooleanFlag == 0
                && scalar_predicate(predicate)
            {
                return Err(pg_bool_session_error(
                    astersql_expression::PgBoolError::TypeMismatch,
                ));
            }
        }
        struct Inputs<'a> {
            session: &'a ConcreteSession,
            sources: &'a [ConcreteResultField],
            table: &'a astersql_meta_model::TableInfo,
            unsupported: bool,
            boolean_error: Option<astersql_expression::PgBoolError>,
            numeric_error: Option<astersql_types::pg_numeric::PgNumericError>,
            text_error: Option<astersql_expression::PgTextError>,
            temporal_error: Option<astersql_types::pg_temporal::PgTemporalError>,
            scalar_cast_error: Option<SessionError>,
        }
        impl ast::ExprNodeVisitor for Inputs<'_> {
            fn Enter(&mut self, input: &ast::ExprNode) -> (ast::ExprNode, bool) {
                // Only a bare literal retains unknown provenance. Parentheses
                // preserve it, while an explicit text cast fixes the source type
                // and cannot use the numeric operator's unknown input path.
                fn literal(expr: &ast::ExprNode) -> Option<&ast::ValueExpr> {
                    match &expr.Kind {
                        ast::ExprKind::Parentheses(inner) => literal(inner),
                        ast::ExprKind::Value(value) => Some(value),
                        _ => None,
                    }
                }
                if let ast::ExprKind::Function { FnName, Args, .. } = &input.Kind
                    && matches!(FnName.L.as_str(), "greatest" | "least")
                    && let Err(error) = self.session.pg_extremum_result_type(Args, self.sources)
                {
                    let mut target = None;
                    let mut position = None;
                    for arg in Args {
                        let tp = self.session.pg_case_input_type(arg, self.sources);
                        if tp.GetType() == astersql_parser_mysql::r#type::TypeNull {
                            continue;
                        }
                        if let Some(old) = &target {
                            match pg_union_common_type(old, &tp) {
                                Ok(tp) => target = Some(tp),
                                Err(_) => {
                                    position = Some(arg.OriginTextPosition as usize);
                                    break;
                                }
                            }
                        } else {
                            target = Some(tp);
                        }
                    }
                    self.scalar_cast_error = Some(if let Some(position) = position {
                        let diagnostic = super::function_diagnostics::PgFunctionError {
                            state: "42804",
                            message: format!("{} types cannot be matched", FnName.L.to_uppercase()),
                            hint: None,
                            position: Some(position),
                        };
                        SessionError::with_source(
                            diagnostic.to_string(),
                            astersql_errors::SharedError::new(diagnostic),
                        )
                    } else {
                        error
                    });
                }
                // NULLIF selects an equality operator before row evaluation.
                // Typed text cannot use the unknown-literal numeric coercion path.
                if let ast::ExprKind::Function { FnName, Args, .. } = &input.Kind
                    && FnName.L == "nullif"
                    && Args.len() == 2
                {
                    let left = self.session.pg_case_input_type(&Args[0], self.sources);
                    let right = self.session.pg_case_input_type(&Args[1], self.sources);
                    if super::relational_value::pg_text_int4_pair(&left, &right)
                        || super::relational_value::pg_text_int4_pair(&right, &left)
                        || super::relational_value::pg_text_numeric_pair(&left, &right)
                        || super::relational_value::pg_text_numeric_pair(&right, &left)
                    {
                        let error = super::function_diagnostics::PgFunctionError {
                            state: "42883",
                            message: "operator does not exist for typed text and numeric operands"
                                .into(),
                            hint: Some(
                                "No operator matches the given name and argument types. You might need to add explicit type casts.",
                            ),
                            position: (input.OriginTextPosition >= 0)
                                .then_some(input.OriginTextPosition as usize),
                        };
                        self.scalar_cast_error = Some(SessionError::with_source(
                            error.to_string(),
                            astersql_errors::SharedError::new(error),
                        ));
                    }
                }
                if let ast::ExprKind::Case {
                    WhenClauses,
                    ElseClause,
                    ..
                } = &input.Kind
                {
                    let validation = self
                        .session
                        .pg_case_result_type(input, self.sources)
                        .and_then(|target| {
                            for expr in ElseClause
                                .iter()
                                .map(|expr| expr.as_ref())
                                .chain(WhenClauses.iter().map(|clause| &clause.Result))
                            {
                                if let Some(value) = literal(expr)
                                    && let ast::ValueDatum::String(text) = &value.Datum
                                {
                                    let source =
                                        self.session.pg_case_input_type(expr, self.sources);
                                    self.session.pg_union_cast_value(
                                        Some(text.clone()),
                                        &source,
                                        &target,
                                    )?;
                                }
                            }
                            Ok(())
                        });
                    if let Err(error) = validation {
                        self.scalar_cast_error = Some(error);
                    }
                }
                if let ast::ExprKind::Binary { Op, L, R } = &input.Kind
                    && matches!(Op.as_str(), "+" | "-" | "*" | "/" | "%")
                    && [L.as_ref(), R.as_ref()].iter().any(|operand| {
                        literal(operand).is_none()
                            && relational_expression_type(operand, self.table)
                                .PostgreSQLTextOid()
                                .is_some()
                    })
                {
                    self.scalar_cast_error = Some(SessionError::with_source(
                        "PostgreSQL arithmetic operator does not accept typed text",
                        astersql_errors::SharedError::new(super::pg_array::PgArrayError {
                            state: "42883",
                            message: "operator does not exist for typed text operands".into(),
                            position: None,
                        }),
                    ));
                }
                // Unknown string input is converted during analysis, including
                // empty result sets. Reuse the strict PG input converter so a
                // numeric prefix never makes an invalid literal acceptable.
                if let ast::ExprKind::Cast {
                    Expr,
                    FunctionType: ast::CastFunctionType::PostgreSQLInt4,
                    ..
                } = &input.Kind
                    && let Some(value) = literal(Expr)
                    && let ast::ValueDatum::String(text) = &value.Datum
                {
                    self.scalar_cast_error = pg_integer_relational_value(
                        Expr,
                        Some(text.clone()),
                        ast::CastFunctionType::PostgreSQLInt4,
                    )
                    .err()
                    .or(self.scalar_cast_error.take());
                }
                // Boolean operators require implicit bool eligibility before
                // evaluating either operand, including short-circuited NULLs.
                let rejects_int4 = |operand: &ast::ExprNode| {
                    let source = relational_expression_type(operand, self.table);
                    source.GetType() == astersql_parser_mysql::r#type::TypeLong
                        && source.GetFlag() & astersql_parser_mysql::r#type::IsBooleanFlag == 0
                };
                let invalid = match &input.Kind {
                    ast::ExprKind::Binary { Op, L, R }
                        if Op.eq_ignore_ascii_case("and") || Op.eq_ignore_ascii_case("or") =>
                    {
                        rejects_int4(L) || rejects_int4(R)
                    }
                    ast::ExprKind::Unary { Op, V } if Op.eq_ignore_ascii_case("not") => {
                        rejects_int4(V)
                    }
                    _ => false,
                };
                if invalid {
                    self.boolean_error = Some(astersql_expression::PgBoolError::TypeMismatch);
                }
                if let ast::ExprKind::Function {
                    FnName,
                    Args,
                    Schema,
                    ..
                } = &input.Kind
                    && FnName.L == "numeric_abs"
                    && (Schema.L.is_empty() || Schema.L == "pg_catalog")
                    && (Args.len() != 1
                        || (!matches!(Args[0].Kind, ast::ExprKind::ParamMarker { .. })
                            && !astersql_expression::formal_registry::pg_numeric_implicit_source(
                                relational_expression_type(&Args[0], self.table).GetType(),
                            )))
                {
                    self.scalar_cast_error = Some(SessionError::with_source(
                        "PostgreSQL numeric_abs requires a numeric argument",
                        astersql_errors::SharedError::new(super::pg_array::PgArrayError {
                            state: "42883",
                            message: "function numeric_abs does not exist for this argument type"
                                .into(),
                            position: None,
                        }),
                    ));
                }
                if let ast::ExprKind::Function {
                    FnName,
                    Args,
                    Schema,
                } = &input.Kind
                    && (FnName.L == "abs" || (FnName.L == "round" && Args.len() == 2))
                    && (Schema.L.is_empty() || Schema.L == "pg_catalog")
                {
                    let types: Vec<_> = Args
                        .iter()
                        .map(|a| relational_expression_type(a, self.table).GetType())
                        .collect();
                    // Parameter markers acquire their identity through existing
                    // Parse constraints; validate actual types after binding.
                    if !(FnName.L == "abs"
                        && types.contains(&astersql_parser_mysql::r#type::TypeNull))
                        && !Args.iter().any(|a| {
                            matches!(a.Kind, ast::ExprKind::ParamMarker { .. })
                                || literal(a).is_some_and(|v| {
                                    matches!(
                                        v.Datum,
                                        ast::ValueDatum::String(_) | ast::ValueDatum::Null
                                    )
                                })
                        })
                        && let Err(state) =
                            astersql_expression::formal_registry::resolve_pg_signature(
                                &FnName.L, &types,
                            )
                    {
                        self.scalar_cast_error = Some(if state == "42883" {
                            super::function_diagnostics::no_matching_function(input, &types)
                        } else {
                            SessionError::with_source(
                                format!(
                                    "function {} does not exist for these argument types",
                                    FnName.L
                                ),
                                astersql_errors::SharedError::new(super::pg_array::PgArrayError {
                                    state,
                                    message: format!(
                                        "function {} does not exist for these argument types",
                                        FnName.L
                                    ),
                                    position: None,
                                }),
                            )
                        });
                    }
                }
                if let ast::ExprKind::Function {
                    FnName,
                    Schema,
                    Args,
                } = &input.Kind
                    && matches!(
                        FnName.L.as_str(),
                        "date_trunc" | "extract" | "date_part" | "to_char"
                    )
                    && (2..=3).contains(&Args.len())
                    && (Schema.L.is_empty() || Schema.L == "pg_catalog")
                {
                    let types = Args
                        .iter()
                        .map(|arg| {
                            if Args.len() == 3 {
                                self.session.pg_date_trunc_input_type(arg, self.sources)
                            } else {
                                self.session.pg_case_input_type(arg, self.sources).GetType()
                            }
                        })
                        .collect::<Vec<_>>();
                    if astersql_expression::formal_registry::resolve_pg_signature(&FnName.L, &types)
                        .is_err()
                    {
                        self.scalar_cast_error = Some(
                            super::function_diagnostics::no_matching_function(input, &types),
                        );
                    }
                }
                if let ast::ExprKind::Function {
                    FnName,
                    Schema,
                    Args,
                } = &input.Kind
                    && matches!(
                        FnName.L.as_str(),
                        "array_length" | "cardinality" | "array_append"
                    )
                    && (Schema.L.is_empty() || Schema.L == "pg_catalog")
                {
                    let inputs = Args
                        .iter()
                        .map(|arg| self.session.pg_case_input_type(arg, self.sources))
                        .collect::<Vec<_>>();
                    if astersql_expression::formal_registry::resolve_pg_array_signature(
                        &FnName.L, &inputs,
                    )
                    .is_err()
                    {
                        self.scalar_cast_error =
                            Some(super::function_diagnostics::no_matching_function(
                                input,
                                &inputs.iter().map(|tp| tp.GetType()).collect::<Vec<_>>(),
                            ));
                    }
                }
                // String literals and bare NULL keep unknown provenance, while
                // explicitly typed NULLs must participate with their declared type.
                if let ast::ExprKind::Function {
                    FnName,
                    Args,
                    Schema,
                } = &input.Kind
                    && matches!(FnName.L.as_str(), "substr" | "substring")
                    && (Schema.L.is_empty() || Schema.L == "pg_catalog")
                {
                    let inputs: Vec<_> = Args
                        .iter()
                        .map(|a| self.session.pg_case_input_type(a, self.sources).GetType())
                        .collect();
                    if let Err(state) = astersql_expression::formal_registry::resolve_pg_signature(
                        &FnName.L, &inputs,
                    ) {
                        self.scalar_cast_error = Some(SessionError::with_source(
                            "PostgreSQL substring argument types do not match a signature",
                            astersql_errors::SharedError::new(super::pg_array::PgArrayError {
                                state,
                                message:
                                    "function substring does not exist for these argument types"
                                        .into(),
                                position: None,
                            }),
                        ));
                    }
                }
                // Only implicit numeric sources satisfy float4abs(real).
                if let ast::ExprKind::Function {
                    FnName,
                    Args,
                    Schema,
                    ..
                } = &input.Kind
                    && FnName.L == "float4abs"
                    && (Schema.L.is_empty() || Schema.L == "pg_catalog")
                    && (Args.len() != 1
                        || (!matches!(Args[0].Kind, ast::ExprKind::ParamMarker { .. })
                            && (relational_expression_type(&Args[0], self.table).GetFlag()
                                & astersql_parser_mysql::r#type::IsBooleanFlag
                                != 0
                                || !matches!(
                                    relational_expression_type(&Args[0], self.table).GetType(),
                                    astersql_parser_mysql::r#type::TypeShort
                                        | astersql_parser_mysql::r#type::TypeLong
                                        | astersql_parser_mysql::r#type::TypeLonglong
                                        | astersql_parser_mysql::r#type::TypeNewDecimal
                                        | astersql_parser_mysql::r#type::TypeFloat
                                        | astersql_parser_mysql::r#type::TypeNull
                                ))))
                {
                    self.scalar_cast_error = Some(SessionError::with_source(
                        "PostgreSQL float4abs requires a real argument",
                        astersql_errors::SharedError::new(super::pg_array::PgArrayError {
                            state: "42883",
                            message: "function float4abs does not exist for this argument type"
                                .into(),
                            position: None,
                        }),
                    ));
                }
                // Only implicit numeric sources satisfy float8abs(double precision).
                if let ast::ExprKind::Function {
                    FnName,
                    Args,
                    Schema,
                    ..
                } = &input.Kind
                    && FnName.L == "float8abs"
                    && (Schema.L.is_empty() || Schema.L == "pg_catalog")
                    && (Args.len() != 1
                        || (!matches!(Args[0].Kind, ast::ExprKind::ParamMarker { .. })
                            && (relational_expression_type(&Args[0], self.table).GetFlag()
                                & astersql_parser_mysql::r#type::IsBooleanFlag
                                != 0
                                || !matches!(
                                    relational_expression_type(&Args[0], self.table).GetType(),
                                    astersql_parser_mysql::r#type::TypeShort
                                        | astersql_parser_mysql::r#type::TypeLong
                                        | astersql_parser_mysql::r#type::TypeLonglong
                                        | astersql_parser_mysql::r#type::TypeNewDecimal
                                        | astersql_parser_mysql::r#type::TypeFloat
                                        | astersql_parser_mysql::r#type::TypeDouble
                                        | astersql_parser_mysql::r#type::TypeNull
                                ))))
                {
                    self.scalar_cast_error = Some(SessionError::with_source(
                        "PostgreSQL float8abs requires a double precision argument",
                        astersql_errors::SharedError::new(super::pg_array::PgArrayError {
                            state: "42883",
                            message: "function float8abs does not exist for this argument type"
                                .into(),
                            position: None,
                        }),
                    ));
                }
                if let ast::ExprKind::Function { FnName, Args, .. } = &input.Kind
                    && FnName.L == "coalesce"
                    && Args.iter().any(|arg| {
                        let source = relational_expression_type(arg, self.table);
                        Args.iter().any(|other| {
                            // The shared MySQL expression typer does not retain temporal
                            // casts. Preserve that identity for this PG-only input gate.
                            let mut other = other;
                            while let ast::ExprKind::Parentheses(inner) = &other.Kind {
                                other = inner;
                            }
                            let target = match &other.Kind {
                                ast::ExprKind::Cast { Tp, .. }
                                    if matches!(
                                        Tp.GetType(),
                                        astersql_parser_mysql::r#type::TypeDate
                                            | astersql_parser_mysql::r#type::TypeDuration
                                            | astersql_parser_mysql::r#type::TypeDatetime
                                            | astersql_parser_mysql::r#type::TypeTimestamp
                                            | astersql_parser_mysql::r#type::TypePgTimestampTz
                                            | astersql_parser_mysql::r#type::TypePgInterval
                                    ) =>
                                {
                                    Tp.clone()
                                }
                                _ => relational_expression_type(other, self.table),
                            };
                            super::relational_value::pg_text_bytea_pair(&source, &target)
                                || super::relational_value::pg_bytea_text_pair(&source, &target)
                                || super::relational_value::pg_text_int4_pair(&source, &target)
                                || super::relational_value::pg_text_numeric_pair(&source, &target)
                                || super::relational_value::pg_text_float8_pair(&source, &target)
                                || super::relational_value::pg_text_bool_pair(&source, &target)
                                || super::relational_value::pg_text_date_pair(&source, &target)
                                || super::relational_value::pg_text_time_pair(&source, &target)
                                || super::relational_value::pg_text_timestamp_pair(&source, &target)
                                || super::relational_value::pg_text_uuid_pair(&source, &target)
                                || super::relational_value::pg_text_json_pair(&source, &target)
                                || super::relational_value::pg_text_int4_array_pair(
                                    &source, &target,
                                )
                                || super::relational_value::pg_text_interval_pair(&source, &target)
                                || super::relational_value::pg_text_timestamptz_pair(
                                    &source, &target,
                                )
                        })
                    })
                {
                    let position = if let ast::ExprKind::Function { Args, .. } = &input.Kind {
                        Args.iter()
                            .skip(1)
                            .find(|arg| {
                                let first = self.session.pg_case_input_type(&Args[0], self.sources);
                                let other = self.session.pg_case_input_type(arg, self.sources);
                                first.EvalType() != other.EvalType()
                            })
                            .map(|arg| arg.OriginTextPosition as usize)
                    } else {
                        None
                    };
                    let error = super::function_diagnostics::PgFunctionError {
                        state: "42804",
                        message: "COALESCE types cannot be matched with text".into(),
                        hint: None,
                        position,
                    };
                    self.scalar_cast_error = Some(SessionError::with_source(
                        error.to_string(),
                        astersql_errors::SharedError::new(error),
                    ));
                }
                // json -> jsonb is assignment-only. Even typed NULL cannot
                // satisfy COALESCE's implicit coercion requirement.
                if let ast::ExprKind::Function { FnName, Args, .. } = &input.Kind
                    && FnName.L == "coalesce"
                    && Args.iter().any(|arg| {
                        relational_expression_type(arg, self.table).GetType()
                            == astersql_parser_mysql::r#type::TypePgJson
                    })
                    && Args.iter().any(|arg| {
                        relational_expression_type(arg, self.table).GetType()
                            == astersql_parser_mysql::r#type::TypePgJsonb
                    })
                {
                    self.scalar_cast_error = Some(super::relational_value::pg_bytea_input_error(
                        "42846",
                        "COALESCE could not convert type json to jsonb implicitly",
                    ));
                }
                // time -> interval is implicit, but their different type categories
                // cannot form a common COALESCE type (including typed NULL).
                if let ast::ExprKind::Function { FnName, Args, .. } = &input.Kind
                    && FnName.L == "coalesce"
                    && Args.iter().any(|arg| {
                        relational_expression_type(arg, self.table).GetType()
                            == astersql_parser_mysql::r#type::TypeDuration
                    })
                    && Args.iter().any(|arg| {
                        relational_expression_type(arg, self.table).GetType()
                            == astersql_parser_mysql::r#type::TypePgInterval
                    })
                {
                    self.scalar_cast_error = Some(super::relational_value::pg_bytea_input_error(
                        "42804",
                        "COALESCE types interval and time cannot be matched",
                    ));
                }
                // timestamp and timestamptz -> time are assignment-only, including typed NULL.
                if let ast::ExprKind::Function { FnName, Args, .. } = &input.Kind
                    && FnName.L == "coalesce"
                    && Args.iter().any(|arg| {
                        matches!(
                            relational_expression_type(arg, self.table).GetType(),
                            astersql_parser_mysql::r#type::TypeDatetime
                                | astersql_parser_mysql::r#type::TypePgTimestampTz
                        )
                    })
                    && Args.iter().any(|arg| {
                        relational_expression_type(arg, self.table).GetType()
                            == astersql_parser_mysql::r#type::TypeDuration
                    })
                {
                    self.scalar_cast_error = Some(super::relational_value::pg_bytea_input_error(
                        "42846",
                        "COALESCE could not convert between timestamp/timestamptz and time",
                    ));
                }
                // int2not has exactly one smallint signature. Do not apply an
                // assignment cast here: int4 -> int2 is never implicit, even NULL.
                if let ast::ExprKind::Function {
                    FnName,
                    Args,
                    Schema,
                    ..
                } = &input.Kind
                    && FnName.L == "int2not"
                    && (Schema.L.is_empty() || Schema.L == "pg_catalog")
                    && (Args.len() != 1
                        || !matches!(
                            relational_expression_type(&Args[0], self.table).GetType(),
                            astersql_parser_mysql::r#type::TypeShort
                                | astersql_parser_mysql::r#type::TypeNull
                        ))
                {
                    self.scalar_cast_error = Some(SessionError::new(
                        "PostgreSQL int2not requires a smallint argument",
                    ));
                }
                // Fixed int4 signatures permit integer widening, never int8
                // narrowing. Check the source type even for a typed NULL.
                if let ast::ExprKind::Function {
                    FnName,
                    Args,
                    Schema,
                    ..
                } = &input.Kind
                    && FnName.L == "int4not"
                    && (Schema.L.is_empty() || Schema.L == "pg_catalog")
                    && (Args.len() != 1
                        || relational_expression_type(&Args[0], self.table).GetFlag()
                            & astersql_parser_mysql::r#type::IsBooleanFlag
                            != 0
                        || !matches!(
                            relational_expression_type(&Args[0], self.table).GetType(),
                            astersql_parser_mysql::r#type::TypeShort
                                | astersql_parser_mysql::r#type::TypeLong
                                | astersql_parser_mysql::r#type::TypeNull
                        ))
                {
                    self.scalar_cast_error = Some(SessionError::new(
                        "PostgreSQL int4not requires an integer argument",
                    ));
                }
                // int8not accepts the existing implicit integer widening paths;
                // a typed NULL still has to satisfy the signature.
                if let ast::ExprKind::Function {
                    FnName,
                    Args,
                    Schema,
                    ..
                } = &input.Kind
                    && FnName.L == "int8not"
                    && (Schema.L.is_empty() || Schema.L == "pg_catalog")
                    && (Args.len() != 1
                        || (!matches!(Args[0].Kind, ast::ExprKind::ParamMarker { .. })
                            && !matches!(
                                relational_expression_type(&Args[0], self.table).GetType(),
                                astersql_parser_mysql::r#type::TypeShort
                                    | astersql_parser_mysql::r#type::TypeLong
                                    | astersql_parser_mysql::r#type::TypeLonglong
                                    | astersql_parser_mysql::r#type::TypeNull
                            )))
                {
                    self.scalar_cast_error = Some(SessionError::new(
                        "PostgreSQL int8not requires a bigint argument",
                    ));
                }
                if let ast::ExprKind::Cast {
                    Tp,
                    FunctionType: ast::CastFunctionType::PostgreSQLNumeric,
                    ..
                } = &input.Kind
                {
                    self.numeric_error = astersql_types::pg_numeric::PgNumeric::validate_modifier(
                        Tp.GetFlen(),
                        Tp.GetDecimal(),
                    )
                    .err()
                    .or(self.numeric_error);
                }
                if let ast::ExprKind::Cast { Tp, .. } = &input.Kind {
                    if Tp.PostgreSQLTextOid().is_some() {
                        self.text_error = astersql_expression::pg_varchar_length(Tp.GetFlen())
                            .err()
                            .or(self.text_error);
                    }
                }
                if let ast::ExprKind::Cast { Expr, Tp, .. } = &input.Kind {
                    self.scalar_cast_error = super::pg_array::validate_cast(Expr, Tp)
                        .err()
                        .or(self.scalar_cast_error.take());
                    let source = pg_integer_literal_type(Expr)
                        .unwrap_or_else(|| relational_expression_type(Expr, self.table));
                    if !matches!(Expr.Kind, ast::ExprKind::ParamMarker { .. }) {
                        self.scalar_cast_error = super::pg_json::validate_cast(&source, Tp)
                            .and_then(|()| super::pg_uuid::validate_cast(&source, Tp))
                            .err()
                            .or(self.scalar_cast_error.take());
                    }
                    // Native TIMESTAMP text has already lost its storage and
                    // source-zone provenance. Check the catalog type before
                    // rows (including empty/all-NULL scans) can hide that loss.
                    if Tp.GetType() == astersql_parser_mysql::r#type::TypePgTimestampTz
                        && source.GetType() == astersql_parser_mysql::r#type::TypeTimestamp
                    {
                        self.temporal_error =
                            Some(astersql_types::pg_temporal::PgTemporalError::TimestampBridge);
                    }
                    if let Err(error) = astersql_expression::pg_boolean_cast_kind(&source, Tp) {
                        self.boolean_error = Some(error);
                    }
                }
                if let ast::ExprKind::Cast {
                    Expr,
                    FunctionType:
                        function_type @ (ast::CastFunctionType::PostgreSQLInt2
                        | ast::CastFunctionType::PostgreSQLInt4
                        | ast::CastFunctionType::PostgreSQLInt8),
                    ..
                } = &input.Kind
                {
                    let source = relational_expression_type(Expr, self.table);
                    self.unsupported |= relational_expression_type(
                        Expr,
                        &astersql_meta_model::TableInfo::default(),
                    )
                    .GetType()
                        != astersql_parser_mysql::r#type::TypeNewDecimal
                        && !(matches!(
                            function_type,
                            ast::CastFunctionType::PostgreSQLInt2
                                | ast::CastFunctionType::PostgreSQLInt4
                                | ast::CastFunctionType::PostgreSQLInt8
                        ) && source.GetType()
                            == astersql_parser_mysql::r#type::TypeNewDecimal)
                        && source.GetType() != astersql_parser_mysql::r#type::TypeNull
                        && !matches!(
                            source.EvalType(),
                            astersql_parser_types::eval_type::ETInt
                                | astersql_parser_types::eval_type::ETReal
                                | astersql_parser_types::eval_type::ETString
                        );
                }
                (input.clone(), self.unsupported)
            }
            fn Leave(&mut self, input: &ast::ExprNode) -> (ast::ExprNode, bool) {
                (input.clone(), !self.unsupported)
            }
        }
        let mut visitor = Inputs {
            session: self,
            sources: &sources,
            table: &table,
            unsupported: false,
            boolean_error: None,
            numeric_error: None,
            text_error: None,
            temporal_error: None,
            scalar_cast_error: None,
        };
        for expr in expressions {
            let _ = expr.Accept(&mut visitor);
        }
        if let Some(error) = visitor.scalar_cast_error {
            return Err(error);
        }
        if let Some(error) = visitor.temporal_error {
            return Err(SessionError::with_source(
                error.to_string(),
                astersql_errors::SharedError::new(error),
            ));
        }
        if let Some(error) = visitor.text_error {
            return Err(SessionError::with_source(
                error.to_string(),
                astersql_errors::SharedError::new(error),
            ));
        }
        if let Some(error) = visitor.numeric_error {
            return Err(SessionError::with_source(
                error.to_string(),
                astersql_errors::SharedError::new(error),
            ));
        }
        if let Some(error) = visitor.boolean_error {
            return Err(pg_bool_session_error(error));
        }
        if visitor.unsupported {
            let error = astersql_expression::PgInt4Error::Unsupported;
            return Err(SessionError::with_source(
                error.to_string(),
                astersql_errors::SharedError::new(error),
            ));
        }
        Ok(())
    }

    pub(super) fn execute_full_relational_select(
        &self,
        select: &ast::SelectStmt,
    ) -> SessionResult<ConcreteRecordSet> {
        self.validate_pg_integer_select_inputs(select)?;
        let _pg_cast_types = if self.sql_dialect() == ast::SqlDialect::PostgreSQL {
            Some(PgCastTable::enter(
                &self.relational_select_source_fields(select)?,
            ))
        } else {
            None
        };
        let statement_read_ts = self.full_query_statement_read_ts(select)?;
        let previous_snapshot_read_ts = {
            let mut state = self.state.borrow_mut();
            let previous = state.snapshot_read_ts;
            if let Some(read_ts) = statement_read_ts {
                state.snapshot_read_ts = Some(read_ts);
            }
            previous
        };
        let result = self.execute_relational_query_record_set(select);
        let locking_read_returned_rows =
            result.as_ref().is_ok_and(|result| !result.rows.is_empty())
                && select.lock_info.as_ref().is_some_and(|lock_info| {
                    let lock_type = if lock_info.LockType == ast::SelectLockType::None {
                        lock_info.lock_type
                    } else {
                        lock_info.LockType
                    };
                    lock_type != ast::SelectLockType::None
                });
        if locking_read_returned_rows && self.state.borrow().transaction.is_some() {
            let mut sources = Vec::new();
            if let Some(from) = select.From.as_ref() {
                if let Some(left) = from.TableRefs.Left.as_deref() {
                    collect_physical_table_sources(left, &mut sources);
                }
                if let Some(right) = from.TableRefs.Right.as_deref() {
                    collect_physical_table_sources(right, &mut sources);
                }
            }
            let current_database = self.current_database();
            for source in sources {
                let database = if source.Source.Schema.L.is_empty() {
                    current_database.as_str()
                } else {
                    source.Source.Schema.L.as_str()
                };
                if let Some(table) =
                    self.resolve_runtime_table(database, relation_name(&source.Source))
                {
                    self.record_transaction_locking_table(&table);
                }
            }
        }
        self.state.borrow_mut().snapshot_read_ts = previous_snapshot_read_ts;
        result
    }

    pub(super) fn relational_query_node_has_table(&self, node: &dyn ast::Node) -> bool {
        if let Some(select) = node.as_any().downcast_ref::<ast::SelectStmt>() {
            return select.From.is_some();
        }
        if let Some(statement) = node.as_any().downcast_ref::<ast::SetOprStmt>() {
            return statement
                .select_list
                .selects
                .iter()
                .any(|select| self.relational_query_node_has_table(select.as_ref()));
        }
        if let Some(list) = node.as_any().downcast_ref::<ast::SetOprSelectList>() {
            return list
                .selects
                .iter()
                .any(|select| self.relational_query_node_has_table(select.as_ref()));
        }
        false
    }

    fn append_relational_source_fields(
        &self,
        source_node: &ast::ResultSetNode,
        fields: &mut Vec<ConcreteResultField>,
        ctes: &HashMap<String, Vec<ConcreteResultField>>,
    ) -> SessionResult<()> {
        let source = match source_node {
            ast::ResultSetNode::TableSource(source) => source,
            ast::ResultSetNode::Join(join) => {
                if let Some(left) = join.Left.as_deref() {
                    self.append_relational_source_fields(left, fields, ctes)?;
                }
                if let Some(right) = join.Right.as_deref() {
                    self.append_relational_source_fields(right, fields, ctes)?;
                }
                return Ok(());
            }
        };

        if let Some(query) = source.QuerySource.as_ref() {
            let mut derived_fields = query
                .with_node(|node| {
                    self.relational_query_node_result_fields_with_ctes(node, &[], ctes)
                })
                .ok_or_else(|| SessionError::new("derived table query is unavailable"))??;
            if !source.AsName.L.is_empty() {
                for field in &mut derived_fields {
                    field.table_as_name = source.AsName.clone();
                }
            }
            for field in &mut derived_fields {
                field.column_origin = None;
                if field.column.FieldType.PostgreSQLTextOid() == Some(1042) {
                    // A derived bpchar source is referenced by its projected name.
                    field.column.Name = field.column_as_name.clone();
                }
            }
            fields.extend(derived_fields);
            return Ok(());
        }

        // Unqualified visible CTEs win over catalog tables. These fields were
        // derived before registering the next definition, just like execution.
        if source.Source.Schema.L.is_empty()
            && let Some(cte_fields) = ctes.get(relation_name(&source.Source))
        {
            let alias = if source.AsName.L.is_empty() {
                &source.Source.Name
            } else {
                &source.AsName
            };
            fields.extend(cte_fields.iter().cloned().map(|mut field| {
                field.column.Name = field.column_as_name.clone();
                field.table_name = source.Source.Name.clone();
                field.table_as_name = alias.clone();
                field
            }));
            return Ok(());
        }

        let current_database = self.current_database();
        let database = if source.Source.Schema.L.is_empty() {
            current_database.as_str()
        } else {
            source.Source.Schema.L.as_str()
        };
        let physical_table = self
            .mdl_stats_table(database, relation_name(&source.Source))
            .map(|(_, table)| table);
        let virtual_table = super::system_query::virtual_system_catalog()?.get(&(
            database.to_ascii_lowercase(),
            source.Source.Name.L.to_ascii_lowercase(),
        ));
        // INFORMATION_SCHEMA may also be present in the domain snapshot with
        // legacy placeholder columns. The authoritative virtual catalog must
        // win so DataGrip set-operation projections see the MySQL columns.
        let Some(table) = virtual_table.or(physical_table.as_ref()) else {
            return Ok(());
        };
        let table_alias = if source.AsName.L.is_empty() {
            table.Name.clone()
        } else {
            source.AsName.clone()
        };
        fields.extend(
            table
                .Columns
                .iter()
                .filter(|column| !column.Hidden)
                .map(|column| ConcreteResultField {
                    column_origin: if virtual_table.is_none() {
                        relational_column_origin(table, column)
                    } else {
                        None
                    },
                    column: column.clone(),
                    column_as_name: column.Name.clone(),
                    table_name: table.Name.clone(),
                    table_as_name: table_alias.clone(),
                    db_name: ast::NewCIStr(database),
                }),
        );
        Ok(())
    }

    pub(super) fn relational_select_source_fields(
        &self,
        select: &ast::SelectStmt,
    ) -> SessionResult<Vec<ConcreteResultField>> {
        self.relational_select_source_fields_with_ctes(select, &HashMap::new())
    }

    fn relational_select_source_fields_with_ctes(
        &self,
        select: &ast::SelectStmt,
        ctes: &HashMap<String, Vec<ConcreteResultField>>,
    ) -> SessionResult<Vec<ConcreteResultField>> {
        let mut fields = Vec::new();
        if let Some(from) = select.From.as_ref() {
            if let Some(left) = from.TableRefs.Left.as_deref() {
                self.append_relational_source_fields(left, &mut fields, ctes)?;
            }
            if let Some(right) = from.TableRefs.Right.as_deref() {
                self.append_relational_source_fields(right, &mut fields, ctes)?;
            }
            // Only the directly bound base-table case is admitted here.
            // Join/derived output lineage keeps its previous unknown origin;
            // do not infer a physical source for coalesced USING expressions.
            if from.TableRefs.Right.is_some()
                || !matches!(from.TableRefs.Left.as_deref(), Some(ast::ResultSetNode::TableSource(source)) if source.QuerySource.is_none())
            {
                for field in &mut fields {
                    field.column_origin = None;
                }
            }
        }
        Ok(fields)
    }

    fn pg_grouped_projection_name(&self, expr: &ast::ExprNode) -> Option<String> {
        if self.sql_dialect() != ast::SqlDialect::PostgreSQL {
            return None;
        }
        match &expr.Kind {
            ast::ExprKind::Parentheses(inner) => self.pg_grouped_projection_name(inner),
            ast::ExprKind::Value(_) => Some("?column?".into()),
            ast::ExprKind::Column(column) => Some(column.Name.O.clone()),
            ast::ExprKind::ExplicitRow { .. } | ast::ExprKind::ImplicitRow { .. } => {
                Some("row".into())
            }
            ast::ExprKind::Subquery { Query, .. } => Query
                .with_node(|node| {
                    self.relational_query_node_result_fields(node, &[])
                        .ok()
                        .and_then(|fields| {
                            fields.first().map(|field| field.column_as_name.O.clone())
                        })
                })
                .flatten(),
            _ => None,
        }
    }

    /// Only the existing signed ABS signature opts into formal MySQL projection
    /// fields and the relational SELECT filter. Other ABS families remain outside
    /// this runtime contract.
    pub(super) fn is_mysql_function_projection(
        &self,
        expression: &ast::ExprNode,
        sources: &[ConcreteResultField],
    ) -> bool {
        if self.sql_dialect() != ast::SqlDialect::MySQL {
            return false;
        }
        let ast::ExprKind::Function {
            FnName,
            Schema,
            Args,
        } = &expression.Kind
        else {
            return false;
        };
        !self.is_mysql_signed_abs(expression, sources)
            && Schema.L.is_empty()
            && match FnName.L.as_str() {
                "abs" => Args.len() == 1,
                "concat" => !Args.is_empty(),
                "ifnull" | "date_format" | "extract" => Args.len() == 2,
                "json_extract" => Args.len() >= 2,
                "tidb_version" => Args.is_empty(),
                "greatest" | "least" => Args.len() >= 2,
                "round" => (1..=2).contains(&Args.len()),
                "substr" | "substring" => (2..=3).contains(&Args.len()),
                _ => false,
            }
            && !relational_expression_has_aggregate(expression)
            && !relational_expression_has_subquery(expression)
            && sources.first().is_none_or(|first| {
                sources
                    .iter()
                    .all(|source| source.table_as_name == first.table_as_name)
            })
    }

    pub(super) fn is_mysql_signed_abs(
        &self,
        expression: &ast::ExprNode,
        source_fields: &[ConcreteResultField],
    ) -> bool {
        if self.sql_dialect() != ast::SqlDialect::MySQL {
            return false;
        }
        let ast::ExprKind::Function {
            FnName,
            Schema,
            Args,
        } = &expression.Kind
        else {
            return false;
        };
        if FnName.L != "abs" || !Schema.L.is_empty() || Args.len() != 1 {
            return false;
        }
        let source = self.signed_abs_argument_type(&Args[0], source_fields);
        matches!(
            source.GetType(),
            astersql_parser_mysql::r#type::TypeLong | astersql_parser_mysql::r#type::TypeLonglong
        ) && source.GetFlag() & astersql_parser_mysql::r#type::UnsignedFlag == 0
    }

    /// Only the conditional families covered by the native-client contract use
    /// this projection bridge. Source columns retain catalog types, not values.
    pub(super) fn is_mysql_conditional_projection(
        &self,
        expression: &ast::ExprNode,
        sources: &[ConcreteResultField],
    ) -> bool {
        if self.sql_dialect() != ast::SqlDialect::MySQL {
            return false;
        }
        let ast::ExprKind::Function {
            FnName,
            Schema,
            Args,
        } = &expression.Kind
        else {
            return false;
        };
        sources.first().is_none_or(|first| {
            sources
                .iter()
                .all(|source| source.table_as_name == first.table_as_name)
        }) && Schema.L.is_empty()
            && matches!(FnName.L.as_str(), "coalesce" | "nullif")
            && !Args.is_empty()
            && !relational_expression_has_aggregate(expression)
            && !relational_expression_has_subquery(expression)
            && Args.iter().all(|arg| {
                matches!(
                    self.signed_abs_argument_type(arg, sources).EvalType(),
                    astersql_parser_types::eval_type::ETInt
                        | astersql_parser_types::eval_type::ETDecimal
                        | astersql_parser_types::eval_type::ETString
                )
            })
    }

    /// Evaluate a conditional projection through its existing formal builder,
    /// typed chunk row and SQL warning policy. No arm is evaluated by the adapter.
    pub(super) fn mysql_conditional_projection_value(
        &self,
        expression: &ast::ExprNode,
        row: &HashMap<String, Option<String>>,
        sources: &[ConcreteResultField],
        analysis_complete: bool,
    ) -> SessionResult<Option<String>> {
        use astersql_expression::contextutil::WarnHandler;
        let warnings = Arc::new(astersql_expression::contextutil::NewStaticWarnHandler(0));
        let mode = astersql_parser_mysql::r#const::GetSQLMode(&self.state.borrow().sql_mode)
            .map_err(|error| session_error("parse conditional sql_mode", error))?;
        let mut levels = self.session_vars.StmtCtx.ErrLevels();
        if !self.mdl_autocommit_write.get() {
            levels[astersql_expression::errctx::ErrGroup::ErrGroupDividedByZero as usize] =
                if mode.HasErrorForDivisionByZeroMode() {
                    astersql_expression::errctx::Level::LevelWarn
                } else {
                    astersql_expression::errctx::Level::LevelIgnore
                };
        }
        levels[astersql_expression::errctx::ErrGroup::ErrGroupTruncate as usize] =
            astersql_expression::errctx::Level::LevelWarn;
        let flags = self
            .session_vars
            .StmtCtx
            .TypeFlags()
            .WithTruncateAsWarning(true)
            .WithIgnoreZeroInDate(true)
            .WithIgnoreInvalidDateErr(mode.HasAllowInvalidDatesMode())
            .WithIgnoreZeroDateErr(!mode.HasNoZeroDateMode() || !mode.HasStrictMode());
        let eval = Arc::new(astersql_expression_exprstatic::NewEvalContext(vec![
            astersql_expression_exprstatic::WithSQLMode(mode),
            astersql_expression_exprstatic::WithTypeFlags(flags),
            astersql_expression_exprstatic::WithErrLevelMap(levels),
            astersql_expression_exprstatic::WithWarnHandler(warnings.clone()),
            astersql_expression_exprstatic::WithParamList(
                self.state
                    .borrow()
                    .mysql_bound_parameters
                    .clone()
                    .unwrap_or_default(),
            ),
        ]));
        let context = astersql_expression_exprstatic::NewExprContext(vec![
            astersql_expression_exprstatic::WithEvalCtx(eval.clone()),
        ]);
        if self.state.borrow().mysql_bound_parameters.is_some() {
            context.GetPlanCacheTracker().SetCacheType(
                astersql_expression::contextutil::plancache::PlanCacheType::SessionPrepared,
            );
        }
        let table = astersql_meta_model::TableInfo {
            Name: sources
                .first()
                .map(|source| {
                    if source.table_as_name.L.is_empty() {
                        source.table_name.clone()
                    } else {
                        source.table_as_name.clone()
                    }
                })
                .unwrap_or_default(),
            Columns: sources
                .iter()
                .enumerate()
                .map(|(index, source)| {
                    let mut column = source.column.clone();
                    column.Offset = index as isize;
                    column
                })
                .collect(),
            ..Default::default()
        };
        let result = (|| {
            let built = astersql_planner_core::PlannerBuildSimpleExpr(
                &context,
                expression,
                vec![astersql_expression::WithTableInfo(
                    &self.current_database(),
                    &table,
                )],
            )
            .map_err(|error| {
                SessionError::with_source(
                    format!("build conditional projection: {error}"),
                    error.into_shared(),
                )
            })?;
            if analysis_complete || self.state.borrow().observation_sql_override.is_some() {
                // Prepared analysis was already built against the original AST.
                // This row adapter's reconstruction must not repeat its warnings.
                eval.TruncateWarnings(0);
            }
            let values = table
                .Columns
                .iter()
                .map(|column| {
                    let value = row.get(&column.Name.L).and_then(Option::as_ref);
                    runtime_value_to_datum(value, column, self.session_vars.StmtCtx.TypeFlags())
                })
                .collect::<SessionResult<Vec<_>>>()?;
            let mut input = astersql_expression::chunk::mutrow::MutRowFromDatums(values);
            input.c.SetNumVirtualRows(1);
            let value = if matches!(&expression.Kind, ast::ExprKind::Function { FnName, .. } if FnName.L == "date_format")
            {
                let mut output =
                    astersql_expression::chunk::NewColumn(built.GetType(eval.as_ref()), 1);
                built
                    .VecEvalString(eval.as_ref(), input.ToRow().Chunk(), &mut output)
                    .map_err(|error| {
                        SessionError::with_source(
                            format!("evaluate DATE_FORMAT projection: {error}"),
                            error.into_shared(),
                        )
                    })?;
                if output.IsNull(0) {
                    astersql_expression::types::Datum::default()
                } else {
                    astersql_expression::types::NewStringDatum(output.GetString(0).to_owned())
                }
            } else if built.GetType(eval.as_ref()).EvalType()
                == astersql_expression::types::ETDecimal
            {
                let (value, null) =
                    built
                        .EvalDecimal(eval.as_ref(), input.ToRow())
                        .map_err(|error| {
                            SessionError::with_source(
                                format!("evaluate conditional decimal: {error}"),
                                error.into_shared(),
                            )
                        })?;
                if null {
                    astersql_expression::types::Datum::default()
                } else {
                    astersql_expression::types::NewDecimalDatum(value)
                }
            } else {
                built.Eval(eval.as_ref(), input.ToRow()).map_err(|error| {
                    SessionError::with_source(
                        format!("evaluate conditional projection: {error}"),
                        error.into_shared(),
                    )
                })?
            };
            if value.IsNull() {
                Ok(None)
            } else {
                value.ToString().map(Some).map_err(|error| {
                    SessionError::with_source(
                        format!("render conditional projection: {error}"),
                        error.into_shared(),
                    )
                })
            }
        })();
        self.state.borrow_mut().current_warnings.extend(
            warnings
                .CopyWarnings(Vec::new())
                .into_iter()
                .filter_map(|warning| warning.Err.map(|error| SessionWarning::from_error(&error))),
        );
        result
    }

    /// The existing NULLIF rewriter can retain signed integer fields without
    /// depending on the currently unlinked COALESCE factory or other families.
    pub(super) fn is_mysql_signed_nullif(
        &self,
        expression: &ast::ExprNode,
        source_fields: &[ConcreteResultField],
    ) -> bool {
        if self.sql_dialect() != ast::SqlDialect::MySQL {
            return false;
        }
        let ast::ExprKind::Function {
            FnName,
            Schema,
            Args,
        } = &expression.Kind
        else {
            return false;
        };
        if FnName.L != "nullif" || !Schema.L.is_empty() || Args.len() != 2 {
            return false;
        }
        Args.iter().all(|arg| {
            let source = self.signed_abs_argument_type(arg, source_fields);
            matches!(
                source.GetType(),
                astersql_parser_mysql::r#type::TypeLong
                    | astersql_parser_mysql::r#type::TypeLonglong
            ) && source.GetFlag() & astersql_parser_mysql::r#type::UnsignedFlag == 0
        })
    }

    /// ABS consumes the explicit integer cast identity even on the MySQL
    /// constant shortcut, whose generic type fallback predates formal factories.
    pub(super) fn signed_abs_argument_type(
        &self,
        expression: &ast::ExprNode,
        source_fields: &[ConcreteResultField],
    ) -> astersql_parser_types::FieldType {
        match &expression.Kind {
            ast::ExprKind::Cast { Tp, .. }
                if matches!(
                    Tp.GetType(),
                    astersql_parser_mysql::r#type::TypeLong
                        | astersql_parser_mysql::r#type::TypeLonglong
                ) =>
            {
                Tp.clone()
            }
            ast::ExprKind::Parentheses(inner) => {
                self.signed_abs_argument_type(inner, source_fields)
            }
            _ => self.relational_query_expression_type_from_sources(expression, source_fields),
        }
    }

    pub(super) fn relational_query_expression_type_from_sources(
        &self,
        expression: &ast::ExprNode,
        source_fields: &[ConcreteResultField],
    ) -> astersql_parser_types::types::FieldType {
        let mut tp = self
            .relational_query_expression_type_from_sources_uncollated(expression, source_fields);
        // Collation is an analysis identity, independent of row selection and
        // common-result typing. Retain the existing type-family derivation.
        if self.sql_dialect() == ast::SqlDialect::PostgreSQL
            && tp.PostgreSQLTextOid().is_some()
            && let Ok(Some(collation)) =
                super::relational_value::pg_conditional_collation(expression)
        {
            tp.SetCollate(collation);
        }
        tp
    }

    // Go function_traits.UnCacheableFunctions rejects COALESCE during the
    // prepared AST check, before expression folding or physical-plan checks.
    pub(super) fn mysql_conditional_cache_reason(
        &self,
        select: &ast::SelectStmt,
    ) -> Option<String> {
        struct Coalesce(bool);
        impl ast::ExprNodeVisitor for Coalesce {
            fn Enter(&mut self, node: &ast::ExprNode) -> (ast::ExprNode, bool) {
                if matches!(&node.Kind, ast::ExprKind::Function { FnName, .. } if FnName.L == "coalesce")
                {
                    self.0 = true;
                }
                (node.clone(), self.0)
            }
            fn Leave(&mut self, node: &ast::ExprNode) -> (ast::ExprNode, bool) {
                (node.clone(), true)
            }
        }
        let mut visitor = Coalesce(false);
        for expr in select
            .Fields
            .Fields
            .iter()
            .filter_map(|field| field.Expr.as_ref())
        {
            let _ = expr.Accept(&mut visitor);
        }
        visitor
            .0
            .then(|| "query has 'coalesce' is un-cacheable".to_owned())
    }

    pub(super) fn mysql_function_result_type(
        &self,
        expression: &ast::ExprNode,
        source_fields: &[ConcreteResultField],
    ) -> SessionResult<astersql_parser_types::types::FieldType> {
        self.mysql_function_type_and_cache_reason(expression, source_fields)
            .map(|(tp, _, _)| tp)
    }

    pub(super) fn mysql_function_type_and_cache_reason(
        &self,
        expression: &ast::ExprNode,
        source_fields: &[ConcreteResultField],
    ) -> SessionResult<(
        astersql_parser_types::FieldType,
        String,
        Vec<SessionWarning>,
    )> {
        // Reuse the formal factory and Go constant-folding rules against
        // catalog types. This retains length, nullability,
        // mixed-type and collation rules instead of picking the first arm.
        let table = astersql_meta_model::TableInfo {
            Columns: source_fields
                .iter()
                .map(|field| field.column.clone())
                .collect(),
            ..Default::default()
        };
        struct Parameters(usize);
        impl ast::ExprNodeVisitor for Parameters {
            fn Enter(&mut self, node: &ast::ExprNode) -> (ast::ExprNode, bool) {
                if matches!(node.Kind, ast::ExprKind::ParamMarker { .. }) {
                    self.0 += 1;
                }
                (node.clone(), false)
            }
            fn Leave(&mut self, node: &ast::ExprNode) -> (ast::ExprNode, bool) {
                (node.clone(), true)
            }
        }
        let mut parameters = Parameters(0);
        let _ = expression.Accept(&mut parameters);
        let unresolved_parameters =
            parameters.0 > 0 && self.state.borrow().mysql_bound_parameters.is_none();
        let parameters = self
            .state
            .borrow()
            .mysql_bound_parameters
            .clone()
            .unwrap_or_else(|| vec![astersql_types::datum::Datum::default(); parameters.0]);
        use astersql_expression::BuildContext;
        use astersql_expression::contextutil::WarnHandler;
        let warnings = Arc::new(astersql_expression::contextutil::NewStaticWarnHandler(0));
        let mode = astersql_parser_mysql::r#const::GetSQLMode(&self.state.borrow().sql_mode)
            .map_err(|error| session_error("parse MySQL function sql_mode", error))?;
        let mut levels = self.session_vars.StmtCtx.ErrLevels();
        if !self.mdl_autocommit_write.get() {
            levels[astersql_expression::errctx::ErrGroup::ErrGroupDividedByZero as usize] =
                if mode.HasErrorForDivisionByZeroMode() {
                    astersql_expression::errctx::Level::LevelWarn
                } else {
                    astersql_expression::errctx::Level::LevelIgnore
                };
        }
        levels[astersql_expression::errctx::ErrGroup::ErrGroupTruncate as usize] =
            astersql_expression::errctx::Level::LevelWarn;
        let flags = self
            .session_vars
            .StmtCtx
            .TypeFlags()
            .WithTruncateAsWarning(true)
            .WithIgnoreZeroInDate(true)
            .WithIgnoreInvalidDateErr(mode.HasAllowInvalidDatesMode())
            .WithIgnoreZeroDateErr(!mode.HasNoZeroDateMode() || !mode.HasStrictMode());
        let eval = astersql_expression_exprstatic::NewEvalContext(vec![
            // Go server config DeprecateIntegerDisplayWidth defaults to true.
            astersql_expression_exprstatic::WithDeprecatedIntegerDisplayWidth(Some(true)),
            astersql_expression_exprstatic::WithSQLMode(mode),
            astersql_expression_exprstatic::WithTypeFlags(flags),
            astersql_expression_exprstatic::WithErrLevelMap(levels),
            astersql_expression_exprstatic::WithWarnHandler(warnings.clone()),
            astersql_expression_exprstatic::WithParamList(parameters),
            astersql_expression_exprstatic::WithSQLDialect(self.sql_dialect()),
        ]);
        let context = astersql_expression_exprstatic::NewExprContext(vec![
            astersql_expression_exprstatic::WithEvalCtx(Arc::new(eval)),
        ]);
        context.GetPlanCacheTracker().SetCacheType(
            astersql_expression::contextutil::plancache::PlanCacheType::SessionPrepared,
        );
        if unresolved_parameters {
            // This private metadata context describes Prepare, before any
            // parameter is bound. Defer folding without a dynamic marker.
            let tracker = context.GetPlanCacheTracker();
            let (_, kind, reason, force, warn) = tracker.Save();
            tracker.Restore(false, kind, reason, force, warn);
        }

        // EXTRACT descriptors retain the nullable formal signature even when
        // the current constant input happens to evaluate to a non-NULL integer.
        let describe_extract = self.sql_dialect() == ast::SqlDialect::MySQL
            && matches!(&expression.Kind, ast::ExprKind::Function { FnName, .. } if FnName.L == "extract");
        astersql_planner_core::PlannerBuildSimpleExpr(
            &context,
            expression,
            vec![
                astersql_expression::WithTableInfo(&self.current_database(), &table),
                Box::new(move |options| options.DeferConstantFolding = unresolved_parameters || describe_extract),
            ],
        )
        .map(|built| {
            if !unresolved_parameters && self.state.borrow().mysql_bound_parameters.is_some()
                && matches!(&expression.Kind, ast::ExprKind::Function { FnName, .. } if FnName.L == "date_format")
            {
                // Go's physical projection candidate probe converts deferred
                // constants to TiPB before deciding TableDual cache eligibility.
                // This performs only in-memory capability analysis, never RPC.
                self.domain.storage().with_storage(|store| {
                    let client = store.GetClient();
                    let _ = astersql_expression::NewPBConverter(client, context.GetEvalCtx()).ExprToPB(built.as_ref());
                });
            }
            (
                built.GetType(context.GetEvalCtx()).clone(),
                context.GetPlanCacheTracker().Save().2,
                warnings
                    .CopyWarnings(Vec::new())
                    .into_iter()
                    .filter_map(|warning| {
                        warning.Err.map(|error| SessionWarning::from_error(&error))
                    })
                    .collect(),
            )
        })
        .map_err(|error| {
            SessionError::with_source(
                format!("resolve MySQL function result type: {error}"),
                error.into_shared(),
            )
        })
    }

    fn relational_query_expression_type_from_sources_uncollated(
        &self,
        expression: &ast::ExprNode,
        source_fields: &[ConcreteResultField],
    ) -> astersql_parser_types::types::FieldType {
        if self.sql_dialect() == ast::SqlDialect::PostgreSQL {
            if let ast::ExprKind::Function {
                FnName,
                Schema,
                Args,
            } = &expression.Kind
                && matches!(
                    FnName.L.as_str(),
                    "array_length" | "cardinality" | "array_append"
                )
                && (Schema.L.is_empty() || Schema.L == "pg_catalog")
            {
                let inputs = Args
                    .iter()
                    .map(|arg| self.pg_case_input_type(arg, source_fields))
                    .collect::<Vec<_>>();
                if let Ok(candidate) =
                    astersql_expression::formal_registry::resolve_pg_array_signature(
                        &FnName.L, &inputs,
                    )
                {
                    let mut result = astersql_parser_types::NewFieldType(candidate.result);
                    if FnName.L == "array_append" {
                        result.SetPostgreSQLArrayElement(
                            astersql_expression::formal_registry::pg_array_append_element(&inputs)
                                .expect("validated array append signature"),
                        );
                    }
                    return result;
                }
            }
            if let ast::ExprKind::Function {
                FnName,
                Schema,
                Args,
            } = &expression.Kind
                && matches!(
                    FnName.L.as_str(),
                    "date_trunc" | "extract" | "date_part" | "to_char"
                )
                && (2..=3).contains(&Args.len())
                && (Schema.L.is_empty() || Schema.L == "pg_catalog")
            {
                let inputs = Args
                    .iter()
                    .map(|arg| {
                        if Args.len() == 3 {
                            self.pg_date_trunc_input_type(arg, source_fields)
                        } else {
                            self.pg_case_input_type(arg, source_fields).GetType()
                        }
                    })
                    .collect::<Vec<_>>();
                if let Ok(candidate) =
                    astersql_expression::formal_registry::resolve_pg_signature(&FnName.L, &inputs)
                {
                    return astersql_parser_types::NewFieldType(candidate.result);
                }
            }
            if let ast::ExprKind::Function {
                FnName,
                Schema,
                Args,
            } = &expression.Kind
                && FnName.L == "abs"
                && Args.len() == 1
                && (Schema.L.is_empty() || Schema.L == "pg_catalog")
            {
                let source = self.signed_abs_argument_type(&Args[0], source_fields);
                if matches!(
                    source.GetType(),
                    astersql_parser_mysql::r#type::TypeLong
                        | astersql_parser_mysql::r#type::TypeLonglong
                ) {
                    return source;
                }
            }
            // A null test has a fixed boolean result, independently of whether
            // its parameter type came from Parse OIDs or an explicit cast.
            if matches!(expression.Kind, ast::ExprKind::IsNull { .. }) {
                let mut tp =
                    astersql_parser_types::NewFieldType(astersql_parser_mysql::r#type::TypeTiny);
                tp.AddFlag(astersql_parser_mysql::r#type::IsBooleanFlag);
                return tp;
            }

            if let ast::ExprKind::Function { FnName, Args, .. } = &expression.Kind
                && matches!(FnName.L.as_str(), "greatest" | "least")
            {
                return self
                    .pg_extremum_result_type(Args, source_fields)
                    .unwrap_or_default();
            }
            if matches!(expression.Kind, ast::ExprKind::Case { .. }) {
                return self
                    .pg_case_result_type(expression, source_fields)
                    .unwrap_or_default();
            }
            if let Some(tp) = pg_integer_literal_type(expression) {
                return tp;
            }
            if let ast::ExprKind::Function { FnName, Schema, .. } = &expression.Kind
                && FnName.L == "int4not"
                && (Schema.L.is_empty() || Schema.L == "pg_catalog")
            {
                return astersql_parser_types::NewFieldType(
                    astersql_parser_mysql::r#type::TypeLong,
                );
            }
            if let ast::ExprKind::Function { FnName, Schema, .. } = &expression.Kind
                && FnName.L == "int2not"
                && (Schema.L.is_empty() || Schema.L == "pg_catalog")
            {
                return astersql_parser_types::NewFieldType(
                    astersql_parser_mysql::r#type::TypeShort,
                );
            }
            if let ast::ExprKind::Function { FnName, Schema, .. } = &expression.Kind
                && FnName.L == "int8not"
                && (Schema.L.is_empty() || Schema.L == "pg_catalog")
            {
                return astersql_parser_types::NewFieldType(
                    astersql_parser_mysql::r#type::TypeLonglong,
                );
            }
            if let ast::ExprKind::Function {
                FnName,
                Schema,
                Args,
            } = &expression.Kind
                && (FnName.L == "numeric_abs" || (FnName.L == "round" && Args.len() == 2))
                && (Schema.L.is_empty() || Schema.L == "pg_catalog")
            {
                return astersql_parser_types::NewFieldType(
                    astersql_parser_mysql::r#type::TypeNewDecimal,
                );
            }
            if let ast::ExprKind::Function { FnName, Schema, .. } = &expression.Kind
                && FnName.L == "float4abs"
                && (Schema.L.is_empty() || Schema.L == "pg_catalog")
            {
                return astersql_parser_types::NewFieldType(
                    astersql_parser_mysql::r#type::TypeFloat,
                );
            }
            if let ast::ExprKind::Function { FnName, Schema, .. } = &expression.Kind
                && FnName.L == "float8abs"
                && (Schema.L.is_empty() || Schema.L == "pg_catalog")
            {
                return astersql_parser_types::NewFieldType(
                    astersql_parser_mysql::r#type::TypeDouble,
                );
            }
            if let ast::ExprKind::Function { FnName, Args, .. } = &expression.Kind
                && FnName.L == "nullif"
                && Args.len() == 2
            {
                let left = self.pg_case_input_type(&Args[0], source_fields);
                let right = self.pg_case_input_type(&Args[1], source_fields);
                use astersql_parser_mysql::r#type::{TypeLong, TypeNewDecimal};
                // NULLIF returns the first operand of its selected equality operator.
                // numeric = numeric widens int4 on either side; this is comparison
                // coercion, not COALESCE's common-result-type selection.
                if matches!(left.GetType(), TypeLong | TypeNewDecimal)
                    && matches!(right.GetType(), TypeLong | TypeNewDecimal)
                {
                    return astersql_parser_types::NewFieldType(
                        if left.GetType() == TypeNewDecimal || right.GetType() == TypeNewDecimal {
                            TypeNewDecimal
                        } else {
                            TypeLong
                        },
                    );
                }
            }
            // PostgreSQL int4 division keeps its integer result identity; MySQL '/' remains decimal.
            if let ast::ExprKind::Binary { Op, L, R } = &expression.Kind
                && Op == "/"
            {
                let left = self.pg_case_input_type(L, source_fields);
                let right = self.pg_case_input_type(R, source_fields);
                if left.GetType() == astersql_parser_mysql::r#type::TypeLong
                    && right.GetType() == astersql_parser_mysql::r#type::TypeLong
                {
                    return astersql_parser_types::NewFieldType(
                        astersql_parser_mysql::r#type::TypeLong,
                    );
                }
            }
            // Preserve the existing PG integer promotion for int2 -> int4.
            // The shared MySQL expression inference otherwise labels arithmetic int8.
            if let ast::ExprKind::Binary { Op, L, R } = &expression.Kind
                && matches!(Op.as_str(), "+" | "-" | "*")
            {
                use astersql_parser_mysql::r#type::{TypeLong, TypeShort};
                if let (Ok(left), Ok(right)) = (
                    pg_returning_expression_type(L, source_fields),
                    pg_returning_expression_type(R, source_fields),
                ) && matches!(
                    (left.GetType(), right.GetType()),
                    (TypeShort, TypeLong) | (TypeLong, TypeShort) | (TypeLong, TypeLong)
                ) {
                    return pg_returning_expression_type(expression, source_fields)
                        .expect("integer operands have a PG arithmetic result type");
                }
                // Integers implicitly widen to numeric; arithmetic drops operand typmods.
                use astersql_parser_mysql::r#type::{TypeLonglong, TypeNewDecimal};
                let left = self.relational_query_expression_type_from_sources(L, source_fields);
                let right = self.relational_query_expression_type_from_sources(R, source_fields);
                if matches!(
                    (left.GetType(), right.GetType()),
                    (TypeShort | TypeLong | TypeLonglong, TypeNewDecimal)
                        | (TypeNewDecimal, TypeShort | TypeLong | TypeLonglong)
                ) {
                    return astersql_parser_types::NewFieldType(TypeNewDecimal);
                }
                // Mixed int2/float4 arithmetic selects a float8 overload in PG.
                use astersql_parser_mysql::r#type::TypeFloat;
                if matches!(
                    (left.GetType(), right.GetType()),
                    (TypeShort, TypeFloat) | (TypeFloat, TypeShort)
                ) {
                    return astersql_parser_types::NewFieldType(
                        astersql_parser_mysql::r#type::TypeDouble,
                    );
                }
                // int2 widens exactly to float8 in mixed PG arithmetic.
                use astersql_parser_mysql::r#type::TypeDouble;
                if matches!(
                    (left.GetType(), right.GetType()),
                    (TypeShort, TypeDouble) | (TypeDouble, TypeShort)
                ) {
                    return astersql_parser_types::NewFieldType(TypeDouble);
                }
            }
            if let ast::ExprKind::Function { FnName, Args, .. } = &expression.Kind
                && FnName.L == "coalesce"
            {
                let types = Args
                    .iter()
                    .map(|arg| {
                        self.relational_query_expression_type_from_sources(arg, source_fields)
                    })
                    .collect::<Vec<_>>();
                use astersql_parser_mysql::r#type::{
                    TypeDate, TypeDatetime, TypeNull, TypePgTimestampTz,
                };
                // COALESCE fixes the common numeric identity during analysis,
                // before a row (or a NULL binding) can select an operand. Keep
                // the existing temporal/text families on their current paths.
                use astersql_parser_mysql::r#type::{TypeLong, TypeNewDecimal};
                if types
                    .iter()
                    .all(|tp| matches!(tp.GetType(), TypeLong | TypeNewDecimal | TypeNull))
                {
                    let mut known = types.iter().filter(|tp| tp.GetType() != TypeNull);
                    let mut target = known
                        .next()
                        .cloned()
                        .unwrap_or_else(|| astersql_parser_types::NewFieldType(TypeNull));
                    for input in known {
                        target = pg_union_common_type(&target, input).unwrap_or_default();
                    }
                    if target.GetType() != TypeNull {
                        target.SetFlen(-1);
                        target.SetDecimal(-1);
                        return target;
                    }
                }
                if types.iter().all(|tp| {
                    matches!(
                        tp.GetType(),
                        TypeDate | TypeDatetime | TypePgTimestampTz | TypeNull
                    )
                }) && types
                    .iter()
                    .any(|tp| matches!(tp.GetType(), TypeDate | TypeDatetime))
                    && types.iter().any(|tp| tp.GetType() == TypePgTimestampTz)
                {
                    return astersql_parser_types::NewFieldType(TypePgTimestampTz);
                }
                if types
                    .iter()
                    .all(|tp| matches!(tp.GetType(), TypeDate | TypeDatetime | TypeNull))
                    && types.iter().any(|tp| tp.GetType() == TypeDate)
                    && types.iter().any(|tp| tp.GetType() == TypeDatetime)
                {
                    return astersql_parser_types::NewFieldType(TypeDatetime);
                }
            }
            match &expression.Kind {
                ast::ExprKind::Cast { Tp, .. }
                    if matches!(
                        Tp.GetType(),
                        astersql_parser_mysql::r#type::TypeDate
                            | astersql_parser_mysql::r#type::TypeDatetime
                            | astersql_parser_mysql::r#type::TypeDuration
                    ) =>
                {
                    return Tp.clone();
                }
                // PG LIKE/ILIKE (including a bound ESCAPE) yields boolean.
                // Preserve that identity for Prepare and Execute metadata.
                ast::ExprKind::Like { .. } => {
                    let mut tp = astersql_parser_types::NewFieldType(
                        astersql_parser_mysql::r#type::TypeTiny,
                    );
                    tp.AddFlag(astersql_parser_mysql::r#type::IsBooleanFlag);
                    return tp;
                }
                ast::ExprKind::Parentheses(inner) => {
                    return self
                        .relational_query_expression_type_from_sources(inner, source_fields);
                }
                ast::ExprKind::ExplicitRow { .. } | ast::ExprKind::ImplicitRow { .. } => {
                    return super::pg_record::record_type_in_context(
                        self,
                        expression,
                        &HashMap::new(),
                        source_fields,
                    )
                    .map(super::pg_record::field_type)
                    .unwrap_or_default();
                }
                _ => {}
            }
        }
        if let ast::ExprKind::Subquery { Query, .. } = &expression.Kind
            && let Some(field_type) = Query
                .with_node(|node| {
                    self.relational_query_node_result_fields(node, &[])
                        .ok()
                        .and_then(|fields| {
                            fields.first().map(|field| field.column.FieldType.clone())
                        })
                })
                .flatten()
        {
            return field_type;
        }
        let mut table = astersql_meta_model::TableInfo::default();
        table.Columns = source_fields
            .iter()
            .map(|field| field.column.clone())
            .collect();
        relational_expression_type(expression, &table)
    }

    /// Bind INSERT output against its target relation without executing the DML.
    /// The SELECT view is an AST projection adapter for the existing binder;
    /// it is never registered or executed as another SQL statement.
    pub(super) fn insert_returning_fields(
        &self,
        insert: &ast::InsertStmt,
    ) -> SessionResult<Vec<ConcreteResultField>> {
        let select = ast::SelectStmt {
            From: insert.Table.clone(),
            Fields: ast::FieldList {
                Fields: insert.Returning.clone(),
            },
            ..Default::default()
        };
        let sources = self.relational_select_source_fields(&select)?;
        if sources.is_empty() {
            return Err(SessionError::new(
                "PostgreSQL RETURNING target metadata is unavailable",
            ));
        }
        let mut fields = self.relational_query_node_result_fields(&select, &[])?;
        let mut index = 0;
        for projection in &insert.Returning {
            if let Some(wildcard) = &projection.WildCard {
                let count = sources
                    .iter()
                    .filter(|source| {
                        wildcard.Table.L.is_empty() || wildcard.Table.L == source.table_as_name.L
                    })
                    .count();
                if count == 0 {
                    return Err(SessionError::new(format!(
                        "Unknown column '{}.*' in 'field list'",
                        wildcard.Table.O
                    )));
                }
                index += count;
            } else if let Some(expr) = &projection.Expr {
                let field_type = pg_returning_expression_type(expr, &sources)?;
                fields[index].column.FieldType = field_type;
                index += 1;
            }
        }
        Ok(fields)
    }

    /// Bind UPDATE output against its target relation without executing the DML.
    pub(super) fn update_returning_fields(
        &self,
        update: &ast::UpdateStmt,
    ) -> SessionResult<Vec<ConcreteResultField>> {
        let select = ast::SelectStmt {
            From: update.TableRefs.clone(),
            Fields: ast::FieldList {
                Fields: update.Returning.clone(),
            },
            ..Default::default()
        };
        let sources = self.relational_select_source_fields(&select)?;
        if sources.is_empty() {
            return Err(SessionError::new(
                "PostgreSQL UPDATE RETURNING target metadata is unavailable",
            ));
        }
        let mut fields = self.relational_query_node_result_fields(&select, &[])?;
        let mut index = 0;
        for projection in &update.Returning {
            if let Some(wildcard) = &projection.WildCard {
                let count = sources
                    .iter()
                    .filter(|source| {
                        wildcard.Table.L.is_empty() || wildcard.Table.L == source.table_as_name.L
                    })
                    .count();
                if count == 0 {
                    return Err(SessionError::new(format!(
                        "Unknown column '{}.*' in 'field list'",
                        wildcard.Table.O
                    )));
                }
                index += count;
            } else if let Some(expression) = &projection.Expr {
                fields[index].column.FieldType =
                    pg_returning_expression_type(expression, &sources)?;
                index += 1;
            }
        }
        Ok(fields)
    }

    /// Bind DELETE output against its target relation without executing the
    /// deletion. PostgreSQL USING remains outside this target-only metadata path.
    pub(super) fn delete_returning_fields(
        &self,
        delete: &ast::DeleteStmt,
    ) -> SessionResult<Vec<ConcreteResultField>> {
        if delete.Using.is_some() {
            return Err(SessionError::new(
                "PostgreSQL DELETE USING metadata is unsupported",
            ));
        }
        let select = ast::SelectStmt {
            From: delete.TableRefs.clone(),
            Fields: ast::FieldList {
                Fields: delete.Returning.clone(),
            },
            ..Default::default()
        };
        let sources = self.relational_select_source_fields(&select)?;
        if sources.is_empty() {
            return Err(SessionError::new(
                "PostgreSQL DELETE RETURNING target metadata is unavailable",
            ));
        }
        let mut fields = self.relational_query_node_result_fields(&select, &[])?;
        let mut index = 0;
        for projection in &delete.Returning {
            if let Some(wildcard) = &projection.WildCard {
                let count = sources
                    .iter()
                    .filter(|source| {
                        wildcard.Table.L.is_empty() || wildcard.Table.L == source.table_as_name.L
                    })
                    .count();
                if count == 0 {
                    return Err(SessionError::new(format!(
                        "Unknown column '{}.*' in 'field list'",
                        wildcard.Table.O
                    )));
                }
                index += count;
            } else if let Some(expression) = &projection.Expr {
                fields[index].column.FieldType =
                    pg_returning_expression_type(expression, &sources)?;
                index += 1;
            }
        }
        Ok(fields)
    }

    pub(super) fn insert_parameter_constraints(
        &self,
        insert: &ast::InsertStmt,
    ) -> SessionResult<Vec<(usize, u32)>> {
        let select = ast::SelectStmt {
            From: insert.Table.clone(),
            ..Default::default()
        };
        let sources = self.relational_select_source_fields(&select)?;
        let columns = if insert.Columns.is_empty() {
            sources.iter().collect::<Vec<_>>()
        } else {
            insert
                .Columns
                .iter()
                .map(|column| {
                    sources
                        .iter()
                        .find(|source| source.column.Name.L == column.Name.L)
                        .ok_or_else(|| {
                            SessionError::new(format!(
                                "Unknown column '{}' in 'field list'",
                                column.Name.O
                            ))
                        })
                })
                .collect::<SessionResult<Vec<_>>>()?
        };
        let mut constraints = Vec::new();
        for row in &insert.Lists {
            for (expression, column) in row.iter().zip(&columns) {
                let mut source = expression;
                while let ast::ExprKind::Parentheses(inner) = &source.Kind {
                    source = inner;
                }
                if let ast::ExprKind::ParamMarker { Offset } = source.Kind {
                    let oid = match column.column.FieldType.GetType() {
                        2 => 21,
                        3 => 23,
                        8 => 20,
                        15 => 1043,
                        249..=252 if column.column.FieldType.GetCharset() != "binary" => 25,
                        253 | 254 => 25,
                        _ => 0,
                    };
                    constraints.push((Offset, oid));
                }
            }
        }
        Ok(constraints)
    }

    pub(super) fn update_parameter_constraints(
        &self,
        update: &ast::UpdateStmt,
    ) -> SessionResult<Vec<(usize, u32)>> {
        let select = ast::SelectStmt {
            From: update.TableRefs.clone(),
            ..Default::default()
        };
        let sources = self.relational_select_source_fields(&select)?;
        let mut constraints = Vec::new();
        for assignment in &update.List {
            let Some(column) = sources
                .iter()
                .find(|source| source.column.Name.L == assignment.Column.Name.L)
            else {
                continue;
            };
            let mut expression = &assignment.Expr;
            while let ast::ExprKind::Parentheses(inner) = &expression.Kind {
                expression = inner;
            }
            if let ast::ExprKind::ParamMarker { Offset } = expression.Kind {
                let oid = match column.column.FieldType.GetType() {
                    2 => 21,
                    3 => 23,
                    8 => 20,
                    15 => 1043,
                    253 | 254 => 25,
                    _ => 0,
                };
                constraints.push((Offset, oid));
            }
        }
        Ok(constraints)
    }

    pub(super) fn relational_query_node_result_fields(
        &self,
        node: &dyn ast::Node,
        output_columns: &[String],
    ) -> SessionResult<Vec<ConcreteResultField>> {
        self.relational_query_node_result_fields_with_ctes(node, output_columns, &HashMap::new())
    }

    fn relational_query_node_result_fields_with_ctes(
        &self,
        node: &dyn ast::Node,
        output_columns: &[String],
        inherited: &HashMap<String, Vec<ConcreteResultField>>,
    ) -> SessionResult<Vec<ConcreteResultField>> {
        let mut ctes = inherited.clone();
        if let Some(select) = node.as_any().downcast_ref::<ast::SelectStmt>() {
            if self.sql_dialect() == ast::SqlDialect::PostgreSQL
                && select.Kind == ast::SelectStmtKind::Values
            {
                return self.pg_values_fields(select);
            }
            if let Some(with) = &select.With {
                let with = with.borrow();
                if !with.IsRecursive {
                    for cte in &with.CTEs {
                        if cte.IsRecursive {
                            continue;
                        }
                        let mut fields = self.relational_query_node_result_fields_with_ctes(
                            cte.Query.as_ref(),
                            &[],
                            &ctes,
                        )?;
                        if !cte.ColNameList.is_empty() {
                            if fields.len() != cte.ColNameList.len() {
                                return Err(SessionError::new(
                                    "CTE column-name count does not match query",
                                ));
                            }
                            for (field, name) in fields.iter_mut().zip(&cte.ColNameList) {
                                field.column_as_name = name.clone();
                            }
                        }
                        ctes.insert(cte.Name.L.clone(), fields);
                    }
                }
            }
            let source_fields = self.relational_select_source_fields_with_ctes(select, &ctes)?;
            let mut using_columns = HashSet::new();
            if let Some(from) = select.From.as_ref() {
                collect_join_using_column_names(&from.TableRefs, &mut using_columns);
            }
            let mut result = Vec::new();
            let mut output_index = 0;
            for field in &select.Fields.Fields {
                if let Some(wildcard) = field.WildCard.as_ref() {
                    for source_field in source_fields.iter().filter(|source_field| {
                        wildcard.Table.L.is_empty()
                            || source_field.table_as_name.L == wildcard.Table.L
                            || source_field.table_name.L == wildcard.Table.L
                    }) {
                        let mut result_field = source_field.clone();
                        if result_field.column_as_name.O.is_empty()
                            && let Some(output) = output_columns.get(output_index)
                        {
                            result_field.column_as_name = ast::NewCIStr(output);
                        }
                        result.push(result_field);
                        output_index += 1;
                    }
                    continue;
                }
                let expression = field.Expr.as_ref().ok_or_else(|| {
                    SessionError::new("relational SELECT result field has no expression")
                })?;
                let output = (!field.AsName.O.is_empty())
                    .then(|| field.AsName.O.clone())
                    .or_else(|| self.pg_grouped_projection_name(expression))
                    .or_else(|| match &expression.Kind {
                        ast::ExprKind::ExplicitRow { .. } | ast::ExprKind::ImplicitRow { .. } => {
                            Some("row".to_owned())
                        }
                        ast::ExprKind::Value(_)
                            if self.sql_dialect() == ast::SqlDialect::PostgreSQL =>
                        {
                            Some("?column?".to_owned())
                        }
                        ast::ExprKind::Array { .. } => Some("array".to_owned()),
                        ast::ExprKind::Column(column) => Some(column.Name.O.clone()),
                        ast::ExprKind::Parentheses(inner) => match &inner.Kind {
                            ast::ExprKind::Column(column) => Some(column.Name.O.clone()),
                            ast::ExprKind::Value(value) => Some(value.text()),
                            _ => None,
                        },
                        _ => None,
                    })
                    .or_else(|| {
                        (!field.OriginalText.is_empty())
                            .then(|| projection_original_text(&field.OriginalText))
                    })
                    .or_else(|| output_columns.get(output_index).cloned())
                    .unwrap_or_else(|| format!("expression_{}", output_index + 1));
                output_index += 1;
                if let ast::ExprKind::Column(column) = &expression.Kind {
                    let mut candidates = source_fields.iter().filter(|candidate| {
                        candidate.column.Name.L == column.Name.L
                            && (column.Table.L.is_empty()
                                || candidate.table_as_name.L == column.Table.L)
                    });
                    let Some(candidate) = candidates.next() else {
                        if column.Name.L == "_tidb_rowid" {
                            result.push(ConcreteResultField {
                                column: astersql_meta_model::ColumnInfo {
                                    Name: column.Name.clone(),
                                    FieldType: astersql_parser_types::NewFieldType(
                                        astersql_parser_mysql::r#type::TypeLonglong,
                                    ),
                                    ..astersql_meta_model::ColumnInfo::default()
                                },
                                column_as_name: ast::NewCIStr(&output),
                                table_name: column.Table.clone(),
                                table_as_name: column.Table.clone(),
                                db_name: ast::NewCIStr(&self.current_database()),
                                column_origin: None,
                            });
                            continue;
                        }
                        let cte_column = select.With.as_ref().is_some_and(|with| {
                            let with = with.borrow();
                            with.CTEs.iter().any(|cte| {
                                cte.Name.L == column.Table.L
                                    || cte.Name.L
                                        == select
                                            .From
                                            .as_ref()
                                            .and_then(|from| {
                                                from.TableRefs.Left.as_deref().and_then(|source| {
                                                    match source {
                                                        ast::ResultSetNode::TableSource(source) => {
                                                            Some(source)
                                                        }
                                                        ast::ResultSetNode::Join(_) => None,
                                                    }
                                                })
                                            })
                                            .map(|source| source.Source.Name.L.as_str())
                                            .unwrap_or_default()
                            })
                        }) || self.cte_scopes.borrow().iter().rev().any(|scope| {
                            scope.get(&column.Table.L).is_some_and(|cte| {
                                cte.columns.iter().any(|name| name == &column.Name.L)
                            })
                        }) || select.From.as_ref().is_some_and(|from| {
                            let Some(ast::ResultSetNode::TableSource(source)) =
                                from.TableRefs.Left.as_deref()
                            else {
                                return false;
                            };
                            let qualifier = if source.AsName.L.is_empty() {
                                &source.Source.Name.L
                            } else {
                                &source.AsName.L
                            };
                            qualifier == &column.Table.L
                                && source.QuerySource.is_none()
                                && self
                                    .mdl_stats_table(
                                        &self.current_database(),
                                        relation_name(&source.Source),
                                    )
                                    .is_none()
                        });
                        if cte_column {
                            let mut field_type = astersql_parser_types::NewFieldType(
                                astersql_parser_mysql::r#type::TypeVarString,
                            );
                            field_type.SetCharset("utf8mb4".to_owned());
                            field_type.SetCollate("utf8mb4_bin".to_owned());
                            result.push(ConcreteResultField {
                                column: astersql_meta_model::ColumnInfo {
                                    Name: column.Name.clone(),
                                    FieldType: field_type,
                                    ..astersql_meta_model::ColumnInfo::default()
                                },
                                column_as_name: ast::NewCIStr(&output),
                                table_name: column.Table.clone(),
                                table_as_name: column.Table.clone(),
                                db_name: ast::NewCIStr(&self.current_database()),
                                column_origin: None,
                            });
                            continue;
                        }
                        if select.From.as_ref().is_some_and(|from| {
                            from.TableRefs
                                .Left
                                .as_deref()
                                .is_some_and(result_set_node_has_derived_source)
                                || from
                                    .TableRefs
                                    .Right
                                    .as_deref()
                                    .is_some_and(result_set_node_has_derived_source)
                        }) {
                            let mut field_type = astersql_parser_types::NewFieldType(
                                astersql_parser_mysql::r#type::TypeVarString,
                            );
                            field_type.SetCharset("utf8mb4".to_owned());
                            field_type.SetCollate("utf8mb4_bin".to_owned());
                            result.push(ConcreteResultField {
                                column: astersql_meta_model::ColumnInfo {
                                    Name: column.Name.clone(),
                                    FieldType: field_type,
                                    ..astersql_meta_model::ColumnInfo::default()
                                },
                                column_as_name: ast::NewCIStr(&output),
                                table_name: column.Table.clone(),
                                table_as_name: column.Table.clone(),
                                db_name: ast::NewCIStr(&self.current_database()),
                                column_origin: None,
                            });
                            continue;
                        }
                        return Err(SessionError::new(format!(
                            "Unknown column '{}' in 'field list'",
                            column.Name.O
                        )));
                    };
                    if column.Table.L.is_empty()
                        && !using_columns.contains(&column.Name.L)
                        && candidates.next().is_some()
                    {
                        return Err(SessionError::new(format!(
                            "Column '{}' in field list is ambiguous",
                            column.Name.O
                        )));
                    }
                    let mut result_field = candidate.clone();
                    if !column.Table.L.is_empty() && result_field.column_origin.is_none() {
                        // A qualified projection selects one physical column even
                        // through a join. Recover its existing bound origin, while
                        // leaving derived and unqualified USING outputs unknown.
                        let mut bound_fields = Vec::new();
                        if let Some(from) = &select.From {
                            for source in [
                                from.TableRefs.Left.as_deref(),
                                from.TableRefs.Right.as_deref(),
                            ]
                            .into_iter()
                            .flatten()
                            {
                                self.append_relational_source_fields(
                                    source,
                                    &mut bound_fields,
                                    &ctes,
                                )?;
                            }
                        }
                        result_field.column_origin = bound_fields
                            .iter()
                            .find(|field| {
                                field.table_as_name.L == column.Table.L
                                    && field.column.Name.L == column.Name.L
                            })
                            .and_then(|field| field.column_origin);
                    }
                    result_field.column_as_name = ast::NewCIStr(&output);
                    result.push(result_field);
                } else {
                    let field_type = if self
                        .is_mysql_function_projection(expression, &source_fields)
                        || self.is_mysql_signed_abs(expression, &source_fields)
                        || self.is_mysql_conditional_projection(expression, &source_fields)
                        || (self.sql_dialect() == ast::SqlDialect::MySQL
                            && matches!(&expression.Kind, ast::ExprKind::Function {FnName,..} if matches!(FnName.L.as_str(), "ifnull" | "date_format" | "json_extract" | "tidb_version")))
                    {
                        self.mysql_function_result_type(expression, &source_fields)?
                    } else {
                        self.relational_query_expression_type_from_sources(
                            expression,
                            &source_fields,
                        )
                    };
                    result.push(ConcreteResultField {
                        column: astersql_meta_model::ColumnInfo {
                            Name: ast::CIStr::default(),
                            FieldType: field_type,
                            ..Default::default()
                        },
                        column_as_name: ast::NewCIStr(&output),
                        table_name: ast::CIStr::default(),
                        table_as_name: ast::CIStr::default(),
                        db_name: ast::CIStr::default(),
                        column_origin: None,
                    });
                }
            }
            return Ok(result);
        }
        let list = if let Some(statement) = node.as_any().downcast_ref::<ast::SetOprStmt>() {
            Some(&statement.select_list)
        } else {
            node.as_any().downcast_ref::<ast::SetOprSelectList>()
        };
        let Some(list) = list else {
            return Err(SessionError::new(
                "relational query result is not SELECT or set operation",
            ));
        };
        let first = list
            .selects
            .first()
            .ok_or_else(|| SessionError::new("set operation is empty"))?;
        let mut fields = self.relational_query_node_result_fields_with_ctes(
            first.as_ref(),
            output_columns,
            &ctes,
        )?;
        if self.sql_dialect() == ast::SqlDialect::PostgreSQL {
            pg_union_unknown_inputs(first.as_ref(), &mut fields);
            // Each pair becomes a typed input for the next pair. In particular,
            // (unknown UNION unknown) is text before a later numeric input.
            for (pair, select) in list.selects.iter().skip(1).enumerate() {
                let mut inputs = self.relational_query_node_result_fields_with_ctes(
                    select.as_ref(),
                    &[],
                    &ctes,
                )?;
                pg_union_unknown_inputs(select.as_ref(), &mut inputs);
                if fields.len() != inputs.len() {
                    return Err(pg_union_error(
                        "each UNION query must have the same number of columns",
                        "42601",
                    ));
                }
                for (column, (field, input)) in fields.iter_mut().zip(inputs).enumerate() {
                    field.column.FieldType =
                        pg_union_common_type(&field.column.FieldType, &input.column.FieldType)?;
                    if pair == 0 {
                        self.pg_union_validate_unknown(
                            first.as_ref(),
                            column,
                            &field.column.FieldType,
                        )?;
                    }
                    self.pg_union_validate_unknown(
                        select.as_ref(),
                        column,
                        &field.column.FieldType,
                    )?;
                }
            }
        }
        for field in &mut fields {
            field.column_origin = None;
        }
        Ok(fields)
    }

    fn pg_union_validate_unknown(
        &self,
        node: &dyn ast::Node,
        column: usize,
        target: &astersql_parser_types::FieldType,
    ) -> SessionResult<()> {
        let Some(select) = node.as_any().downcast_ref::<ast::SelectStmt>() else {
            return Ok(());
        };
        let Some(mut expr) = select
            .Fields
            .Fields
            .get(column)
            .and_then(|field| field.Expr.as_ref())
        else {
            return Ok(());
        };
        while let ast::ExprKind::Parentheses(inner) = &expr.Kind {
            expr = inner;
        }
        if let ast::ExprKind::Value(value) = &expr.Kind
            && let ast::ValueDatum::String(text) = &value.Datum
        {
            // Coercing an unknown literal invokes the target input function
            // during analysis, before ParseComplete can be sent.
            let source =
                astersql_parser_types::NewFieldType(astersql_parser_mysql::r#type::TypeNull);
            self.pg_union_cast_value(Some(text.clone()), &source, target)?;
        }
        Ok(())
    }

    pub(super) fn pg_union_cast_value(
        &self,
        value: Option<String>,
        source: &astersql_parser_types::FieldType,
        target: &astersql_parser_types::FieldType,
    ) -> SessionResult<Option<String>> {
        if value.is_none() || source == target {
            return Ok(value);
        }
        use astersql_parser_mysql::r#type::*;
        fn cast(expr: ast::ExprNode, tp: &astersql_parser_types::FieldType) -> ast::ExprNode {
            let mut result = ast::ExprNode::NullValue();
            let function = if tp.PostgreSQLArrayElement().is_some() {
                ast::CastFunctionType::Cast
            } else {
                match tp.GetType() {
                    TypeShort => ast::CastFunctionType::PostgreSQLInt2,
                    TypeLong => ast::CastFunctionType::PostgreSQLInt4,
                    TypeLonglong if tp.GetFlag() & IsBooleanFlag == 0 => {
                        ast::CastFunctionType::PostgreSQLInt8
                    }
                    TypeNewDecimal => ast::CastFunctionType::PostgreSQLNumeric,
                    TypeFloat => ast::CastFunctionType::PostgreSQLFloat4,
                    TypeDouble => ast::CastFunctionType::PostgreSQLFloat8,
                    _ => ast::CastFunctionType::Cast,
                }
            };
            result.Kind = ast::ExprKind::Cast {
                Expr: Box::new(expr),
                Tp: tp.clone(),
                FunctionType: function,
                ExplicitCharSet: false,
            };
            result
        }
        let mut expr = ast::NewValueExpr(value, "utf8mb4", "utf8mb4_bin");
        if source.GetType() != TypeNull {
            expr = cast(expr, source);
        }
        self.relational_query_expression_value(&cast(expr, target), &HashMap::new(), None)
    }

    pub(super) fn pg_setop_execution_fields(
        &self,
        node: &dyn ast::Node,
        fields: &mut [ConcreteResultField],
    ) -> SessionResult<()> {
        let list = if let Some(statement) = node.as_any().downcast_ref::<ast::SetOprStmt>() {
            Some(&statement.select_list)
        } else {
            node.as_any().downcast_ref::<ast::SetOprSelectList>()
        };
        let Some(list) = list else {
            return Ok(());
        };
        if !list.operators.iter().any(|operator| {
            matches!(
                operator,
                Some(
                    ast::SetOprType::Intersect
                        | ast::SetOprType::IntersectAll
                        | ast::SetOprType::Except
                        | ast::SetOprType::ExceptAll
                )
            )
        }) {
            return Ok(());
        }
        let Some(first) = list.selects.first() else {
            return Ok(());
        };
        let mut left = self.relational_query_node_result_fields(first.as_ref(), &[])?;
        self.pg_setop_execution_fields(first.as_ref(), &mut left)?;
        pg_union_unknown_inputs(first.as_ref(), &mut left);
        for (index, branch) in list.selects.iter().enumerate().skip(1) {
            let mut right = self.relational_query_node_result_fields(branch.as_ref(), &[])?;
            pg_union_unknown_inputs(branch.as_ref(), &mut right);
            for (left, right) in left.iter_mut().zip(right) {
                let input = &left.column.FieldType;
                let mut output = pg_union_common_type(input, &right.column.FieldType)?;
                if matches!(
                    list.operators.get(index).and_then(|operator| *operator),
                    Some(
                        ast::SetOprType::Intersect
                            | ast::SetOprType::IntersectAll
                            | ast::SetOprType::Except
                            | ast::SetOprType::ExceptAll
                    )
                ) {
                    // PostgreSQL's INTERSECT and EXCEPT plans emit the left projection.
                    // If it needs no type coercion, its execution descriptor
                    // retains that projection's modifier. Parse/Describe still
                    // uses the common modifier computed from both operands.
                    let mut unmodified_input = input.clone();
                    let mut unmodified_output = output.clone();
                    for tp in [&mut unmodified_input, &mut unmodified_output] {
                        tp.SetFlen(-1);
                        tp.SetDecimal(-1);
                    }
                    if unmodified_input == unmodified_output {
                        output.SetFlen(input.GetFlen());
                        output.SetDecimal(input.GetDecimal());
                    }
                }
                left.column.FieldType = output;
            }
        }
        for (field, executed) in fields.iter_mut().zip(left) {
            field.column.FieldType = executed.column.FieldType;
        }
        Ok(())
    }

    pub(super) fn execute_relational_query_record_set(
        &self,
        node: &dyn ast::Node,
    ) -> SessionResult<ConcreteRecordSet> {
        if self.sql_dialect() == ast::SqlDialect::PostgreSQL
            && let Some(select) = node.as_any().downcast_ref::<ast::SelectStmt>()
            && select.Kind == ast::SelectStmtKind::Values
        {
            let fields = self.pg_values_fields(select)?;
            let mut rows = Vec::new();
            for row in &select.Lists {
                let values = row
                    .Values
                    .iter()
                    .zip(&fields)
                    .map(|(expr, field)| {
                        let source = self.pg_case_input_type(expr, &[]);
                        let value =
                            self.relational_query_expression_value(expr, &HashMap::new(), None)?;
                        self.pg_union_cast_value(value, &source, &field.column.FieldType)
                    })
                    .collect::<SessionResult<Vec<_>>>()?;
                rows.push(values);
            }
            let columns = fields
                .iter()
                .map(|field| field.column_as_name.O.clone())
                .collect::<Vec<_>>();
            let order = resolve_select_order_by(&select.OrderBy, &select.Fields, &columns)?;
            let selected = self.apply_insert_select_order_limit(
                self.insert_select_rows_from_values(&columns, rows),
                &order,
                select.Limit.as_ref(),
            )?;
            let rows = selected
                .rows
                .iter()
                .map(|row| {
                    columns
                        .iter()
                        .map(|column| {
                            row.get(column)
                                .cloned()
                                .flatten()
                                .unwrap_or_else(|| SHOW_NULL_CELL.to_owned())
                        })
                        .collect()
                })
                .collect();
            return Ok(ConcreteRecordSet::new_with_fields(
                columns,
                rows,
                fields.into_iter().map(Some).collect(),
            ));
        }
        let selected = self.execute_insert_select_node(node)?;
        let mut fields = self.relational_query_node_result_fields(node, &selected.columns)?;
        if self.sql_dialect() == ast::SqlDialect::PostgreSQL {
            self.pg_setop_execution_fields(node, &mut fields)?;
        }
        // Constant-false plans can discard their projection columns together
        // with all rows. Preserve the query's result metadata for the empty
        // record set so it remains aligned with the resolved fields.
        let lookup_columns = if selected.rows.is_empty() && selected.columns.len() != fields.len() {
            fields
                .iter()
                .map(|field| field.column_as_name.O.clone())
                .collect::<Vec<_>>()
        } else {
            selected.columns
        };
        let rows = selected
            .rows
            .iter()
            .map(|row| {
                lookup_columns
                    .iter()
                    .map(|column| {
                        row.get(column)
                            .cloned()
                            .unwrap_or(None)
                            .unwrap_or_else(|| SHOW_NULL_CELL.to_owned())
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        let columns = fields
            .iter()
            .map(|field| field.column_as_name.O.clone())
            .collect();
        Ok(ConcreteRecordSet::new_with_fields(
            columns,
            rows,
            fields.into_iter().map(Some).collect(),
        ))
    }

    /// 对齐 Go `InsertExec.Next`：执行 SELECT 子树，再把全部结果交给同一
    /// `execute_relational_insert` 路径完成默认值、约束检查与原子 mutations。
    pub(super) fn execute_insert_select(&self, statement: &ast::InsertStmt) -> SessionResult<()> {
        let _target = statement
            .Table
            .as_ref()
            .and_then(|table| table.TableRefs.Left.as_deref())
            .and_then(|source| match source {
                ast::ResultSetNode::TableSource(source) => Some(&source.Source),
                _ => None,
            })
            .ok_or_else(|| SessionError::new("INSERT SELECT target must be a table"))?;
        let select = statement
            .Select
            .as_ref()
            .ok_or_else(|| SessionError::new("INSERT SELECT requires a SELECT source"))?;
        let source_fields = if self.sql_dialect() == ast::SqlDialect::PostgreSQL {
            self.relational_query_node_result_fields(select.as_ref(), &[])?
        } else {
            Vec::new()
        };
        let selected = self.execute_insert_select_node(select.as_ref())?;
        if !statement.Columns.is_empty() && selected.columns.len() != statement.Columns.len() {
            return Err(SessionError::new(
                "INSERT SELECT target/source column counts differ",
            ));
        }
        if selected.rows.is_empty() {
            return Ok(());
        }
        let select_columns = selected.columns.clone();
        let lists = selected
            .rows
            .into_iter()
            .map(|row| {
                selected
                    .columns
                    .iter()
                    .enumerate()
                    .map(|(index, column)| {
                        let value = row.get(column).cloned().unwrap_or(None);
                        if let Some(field) = source_fields.get(index)
                            && field.column.GetType() == astersql_parser_mysql::r#type::TypeBlob
                            && field.column.GetCharset() == "binary"
                            && field.column.FieldType.PostgreSQLArrayElement().is_none()
                        {
                            // Preserve bytea identity across SELECT materialization so
                            // character assignment calls bytea output, including NULL.
                            let mut expression = ast::NewValueExpr(value, "binary", "binary");
                            if let ast::ExprKind::Value(value) = &mut expression.Kind {
                                value.Type = field.column.FieldType.clone();
                            }
                            return expression;
                        }
                        if let Some(bytes) = value.as_deref().and_then(binary_runtime_bytes) {
                            return ast::ExprNode::HexValue(bytes, "binary", "binary");
                        }
                        let mut expression = ast::NewValueExpr(value, "utf8mb4", "utf8mb4_bin");
                        if let Some(field) = source_fields.get(index) {
                            // Retain source identity for float rounding, boolean and
                            // interval output when assigning materialized SELECT values.
                            // Timestamp identities own extraction before time typmod rounding.
                            if field.column.FieldType.PostgreSQLTextOid() == Some(1042)
                                || field.column.GetFlag()
                                    & astersql_parser_mysql::r#type::IsBooleanFlag
                                    != 0
                                || matches!(
                                    field.column.GetType(),
                                    astersql_parser_mysql::r#type::TypeFloat
                                        | astersql_parser_mysql::r#type::TypeDouble
                                        | astersql_parser_mysql::r#type::TypeDuration
                                        | astersql_parser_mysql::r#type::TypeDatetime
                                        | astersql_parser_mysql::r#type::TypePgTimestampTz
                                        | astersql_parser_mysql::r#type::TypePgInterval
                                )
                            {
                                if let ast::ExprKind::Value(value) = &mut expression.Kind {
                                    value.Type = field.column.FieldType.clone();
                                }
                            }
                        }
                        expression
                    })
                    .collect()
            })
            .collect();
        let values_statement = ast::InsertStmt {
            node_text: Default::default(),
            Priority: statement.Priority,
            IsReplace: statement.IsReplace,
            IgnoreErr: statement.IgnoreErr,
            Table: statement.Table.clone(),
            Columns: statement.Columns.clone(),
            Lists: lists,
            Setlist: statement.Setlist,
            OnDuplicate: statement.OnDuplicate.clone(),
            PostgreSQLConflict: None,
            Select: None,
            TableHints: statement.TableHints.clone(),
            PartitionNames: statement.PartitionNames.clone(),
            Returning: statement.Returning.clone(),
            RowAlias: statement.RowAlias.clone(),
            ColumnAliases: statement.ColumnAliases.clone(),
        };
        let plan = crate::dml_runtime::PlanInsert(&values_statement)?;
        self.execute_relational_insert(&values_statement, plan, true, Some(&select_columns))
    }

    /// 执行关系型 SELECT（扫表/过滤/投影）。
    pub(super) fn execute_relational_select(
        &self,
        statement: &ast::SelectStmt,
        statement_sql: Option<&str>,
    ) -> SessionResult<Option<ConcreteRecordSet>> {
        self.execute_relational_select_rows(statement, statement_sql)
            .map(|records| {
                records.map(|records| records.with_store_read(Arc::clone(&self.sql_killer)))
            })
    }

    fn execute_relational_select_rows(
        &self,
        statement: &ast::SelectStmt,
        statement_sql: Option<&str>,
    ) -> SessionResult<Option<ConcreteRecordSet>> {
        self.validate_pg_integer_select_inputs(statement)?;
        let _pg_cast_types = if self.sql_dialect() == ast::SqlDialect::PostgreSQL {
            Some(PgCastTable::enter(
                &self.relational_select_source_fields(statement)?,
            ))
        } else {
            None
        };
        let Some(from) = statement.From.as_ref() else {
            return Ok(None);
        };
        let left_profile = from
            .TableRefs
            .Left
            .as_deref()
            .map(stale_source_profile)
            .unwrap_or_default();
        let right_profile = from
            .TableRefs
            .Right
            .as_deref()
            .map(stale_source_profile)
            .unwrap_or_default();
        let source_profile = (
            left_profile.0 || right_profile.0,
            left_profile.1 || right_profile.1,
        );
        if source_profile.0 && source_profile.1 {
            return Err(SessionError::new(
                "can not set different time in the as of clause",
            ));
        }
        let mut table_sources = Vec::new();
        if let Some(left) = from.TableRefs.Left.as_deref() {
            collect_physical_table_sources(left, &mut table_sources);
        }
        if let Some(right) = from.TableRefs.Right.as_deref() {
            collect_physical_table_sources(right, &mut table_sources);
        }
        let Some(source) = table_sources.first().copied() else {
            if let Some(ast::ResultSetNode::TableSource(derived)) = from.TableRefs.Left.as_deref()
                && let Some(query) = derived.QuerySource.as_ref()
            {
                return query
                    .with_node(|node| self.execute_relational_query_record_set(node).map(Some))
                    .unwrap_or_else(|| {
                        Err(SessionError::new("derived table query is unavailable"))
                    });
            }
            return Ok(None);
        };
        let current_database = self.current_database();
        let database = if source.Source.Schema.L.is_empty() {
            current_database.as_str()
        } else {
            source.Source.Schema.L.as_str()
        };
        if let (Some(user), Some(host)) = (
            self.login_user.as_deref(),
            self.authenticated_host.as_deref(),
        ) {
            let privileges = runtime_privilege_handle(&self.domain).Get();
            let active_roles = self.active_roles.borrow();
            for table_source in &table_sources {
                let table_database = if table_source.Source.Schema.L.is_empty() {
                    current_database.as_str()
                } else {
                    table_source.Source.Schema.L.as_str()
                };
                if !privileges.RequestVerification(
                    &active_roles,
                    user,
                    host,
                    table_database,
                    relation_name(&table_source.Source),
                    "",
                    astersql_privilege_privileges::SelectPriv,
                ) {
                    return Err(SessionError::new(format!(
                        "SELECT command denied to user '{user}'@'{host}' for table '{}'",
                        table_source.Source.Name.O
                    )));
                }
            }
        }
        let right_source = table_sources.get(1).copied();
        let has_as_of = table_sources.iter().any(|source| source.AsOf.is_some());
        if has_as_of && table_sources.iter().any(|source| source.AsOf.is_none()) {
            return Err(SessionError::new(
                "can not set different time in the as of clause",
            ));
        }
        let mut statement_read_ts = if let Some(as_of) = source.AsOf.as_ref() {
            let mut state = self.state.borrow_mut();
            if state.transaction.is_some() {
                return Err(SessionError::new(
                    "select as of timestamp is not allowed in a transaction",
                ));
            }
            if state.pending_stale_read_ts.take().is_some() {
                return Err(SessionError::new(
                    "can't use select as of while already set transaction as of",
                ));
            }
            drop(state);
            Some(self.evaluate_stale_read_ts(&as_of.TsExpr)?)
        } else if self.state.borrow().transaction.is_none() {
            self.state.borrow_mut().pending_stale_read_ts.take()
        } else {
            None
        };
        for table_source in table_sources.iter().skip(1) {
            let Some(as_of) = table_source.AsOf.as_ref() else {
                continue;
            };
            let read_ts = self.evaluate_stale_read_ts(&as_of.TsExpr)?;
            if statement_read_ts.is_some_and(|first_read_ts| first_read_ts != read_ts) {
                return Err(SessionError::new(
                    "can not set different time in the as of clause",
                ));
            }
            statement_read_ts.get_or_insert(read_ts);
        }
        let table = self
            .local_temporary_table(database, relation_name(&source.Source))
            .or_else(|| {
                if let Some(read_ts) = statement_read_ts
                    .or(self.state.borrow().transaction_stale_read_ts)
                    .or(self.state.borrow().snapshot_read_ts)
                {
                    let catalog_version = self
                        .state
                        .borrow()
                        .tso_catalog_versions
                        .range(..=read_ts)
                        .next_back()
                        .map(|(_, version)| *version);
                    catalog_version
                        .and_then(|version| {
                            self.domain
                                .stats_context()
                                .catalog_at(version)
                                .remove(&(database.to_lowercase(), source.Source.Name.L.clone()))
                        })
                        .map(|(_, table)| table)
                        .or_else(|| {
                            self.mdl_stats_table(database, relation_name(&source.Source))
                                .map(|(_, table)| table)
                        })
                } else {
                    self.mdl_stats_table(database, relation_name(&source.Source))
                        .map(|(_, table)| table)
                }
            });
        let Some(mut table) = table else {
            if source.Source.Name.L == SESSION_KV_TABLE {
                return Ok(None);
            }
            if database.is_empty() {
                return Err(SessionError::new("No database selected"));
            }
            return Err(SessionError::new(format!(
                "Table '{}.{}' doesn't exist",
                database, source.Source.Name.O
            )));
        };
        if let Some(primary_key) = table.GetPkColInfo()
            && statement.Where.as_ref().is_some_and(|predicate| {
                predicate_contains_point_equality(predicate, &primary_key.Name.L)
            })
        {
            let field_types = projected_table_field_types(statement, &table);
            if astersql_planner_core::ShouldSkipReuseChunkForPointGet(&field_types) {
                self.session_vars.StmtCtx.ClearUseChunkAlloc();
            } else {
                self.session_vars.StmtCtx.SetUseChunkAlloc();
            }
        }
        let hinted_vector_index = source
            .Source
            .IndexHints
            .iter()
            .filter(|hint| hint.HintType != ast::IndexHintType::Ignore)
            .flat_map(|hint| hint.IndexNames.iter())
            .find_map(|name| {
                table
                    .Indices
                    .iter()
                    .find(|index| index.Name.L == name.L && index.VectorInfo.is_some())
            });
        if let Some(index) = hinted_vector_index {
            if statement.Where.is_some() {
                return Err(SessionError::new(
                    "TiFlash does not support vector index with predicates",
                ));
            }
            let ordered_metric = statement
                .OrderBy
                .first()
                .and_then(|order| relational_vector_distance_function(&order.Expr))
                .and_then(|function| {
                    astersql_meta_model::IndexableFnNameToDistanceMetric().get(function)
                });
            if ordered_metric != index.VectorInfo.as_ref().map(|info| &info.DistanceMetric) {
                return Err(SessionError::new(
                    "vector index distance metric does not match ORDER BY",
                ));
            }
        }
        let right_table = if let Some(right_source) = right_source {
            let right_database = if right_source.Source.Schema.L.is_empty() {
                current_database.as_str()
            } else {
                right_source.Source.Schema.L.as_str()
            };
            let table = self
                .local_temporary_table(right_database, relation_name(&right_source.Source))
                .or_else(|| {
                    if let Some(read_ts) = statement_read_ts
                        .or(self.state.borrow().transaction_stale_read_ts)
                        .or(self.state.borrow().snapshot_read_ts)
                    {
                        let catalog_version = self
                            .state
                            .borrow()
                            .tso_catalog_versions
                            .range(..=read_ts)
                            .next_back()
                            .map(|(_, version)| *version);
                        catalog_version
                            .and_then(|version| {
                                self.domain.stats_context().catalog_at(version).remove(&(
                                    right_database.to_lowercase(),
                                    right_source.Source.Name.L.clone(),
                                ))
                            })
                            .map(|(_, table)| table)
                            .or_else(|| {
                                self.mdl_stats_table(
                                    right_database,
                                    relation_name(&right_source.Source),
                                )
                                .map(|(_, table)| table)
                            })
                    } else {
                        self.mdl_stats_table(right_database, relation_name(&right_source.Source))
                            .map(|(_, table)| table)
                    }
                });
            let Some(right_table) = table else {
                if right_database.is_empty() {
                    return Err(SessionError::new("No database selected"));
                }
                return Err(SessionError::new(format!(
                    "Table '{}.{}' doesn't exist",
                    right_database, right_source.Source.Name.O
                )));
            };
            Some(right_table)
        } else {
            None
        };
        let limit_window = relational_limit_window(statement)?;
        let all_aggregates = !statement.Fields.Fields.is_empty()
            && statement.Fields.Fields.iter().all(|field| {
                field.Expr.as_ref().is_some_and(|expression| {
                    matches!(expression.Kind, ast::ExprKind::AggregateFunction { .. })
                })
            });
        self.collect_predicate_columns_point(&table, statement)?;
        let read_committed = {
            let state = self.state.borrow();
            state.transaction.is_some()
                && state
                    .transaction_isolation
                    .replace(['-', '_', ' '], "")
                    .eq_ignore_ascii_case("READCOMMITTED")
        };
        let textual_lock = statement_sql.is_some_and(|sql| {
            let lowered = sql.to_ascii_lowercase();
            lowered.contains(" for update") || lowered.contains(" lock in share mode")
        });
        let parsed_lock = statement.lock_info.as_ref().is_some_and(|lock_info| {
            let lock_type = if lock_info.LockType == ast::SelectLockType::None {
                lock_info.lock_type
            } else {
                lock_info.LockType
            };
            lock_type != ast::SelectLockType::None
        });
        let has_lock = textual_lock || parsed_lock;
        let primary_key_order = (statement.OrderBy.len() == 1
            && table.PKIsHandle
            && table.GetPkColInfo().is_some_and(|primary| {
                matches!(
                    &statement.OrderBy[0].Expr.Kind,
                    ast::ExprKind::Column(column) if column.Name.L == primary.Name.L
                )
            }))
        .then(|| statement.OrderBy[0].Desc);
        let primary_key_access_ranges = statement.Where.as_ref().and_then(|predicate| {
            relational_primary_key_scan_ranges(&table, predicate, primary_key_order)
        });
        // The same complete-key candidates must bound SELECT even without a
        // pushed LIMIT. Keep them separate from LIMIT safety: a common handle
        // can still have residual predicates to evaluate after reading the row.
        let candidate_ranges = (right_table.is_none() && statement_read_ts.is_none())
            .then(|| {
                primary_key_access_ranges.clone().or_else(|| {
                    statement.Where.as_ref().and_then(|predicate| {
                        self.integer_common_handle_point_ranges(&table, predicate)
                    })
                })
            })
            .flatten();
        // Only unambiguous single-table key conditions may be evaluated before
        // a join. Locking joins retain the existing full-row path.
        let handle_filter = (statement_read_ts.is_none()
            && right_table.as_ref().is_none_or(|right| {
                !has_lock
                    && table.Columns.iter().all(|left| {
                        !right
                            .Columns
                            .iter()
                            .any(|column| column.Name.L == left.Name.L)
                    })
            }))
        .then(|| {
            statement.Where.as_ref().and_then(|predicate| {
                relational_handle_filter(&table, predicate, &source.AsName.L, self.dml_type_flags())
            })
        })
        .flatten();
        let handle_ranges = relational_row_scan_ranges(&table, None);
        let secondary_index_access = primary_key_access_ranges
            .is_none()
            .then(|| self.relational_secondary_index_access(&table, statement))
            .flatten();
        let index_merge_access =
            if primary_key_access_ranges.is_none() && secondary_index_access.is_none() {
                self.relational_index_merge_access(&table, statement)?
            } else {
                None
            };
        let (request_access_path, request_count) = if index_merge_access.is_some() {
            ("IndexMerge", 3)
        } else if secondary_index_access.is_some() {
            ("IndexLookup", 2)
        } else {
            ("TableReader", 1)
        };
        self.record_select_request(
            &table,
            statement_read_ts,
            request_access_path,
            request_count,
        )?;
        // 对齐 Go `attach2Task4PhysicalLimit` 的安全条件。自动提交快照可以进一步
        // 在 KV 游标层消费 OFFSET，只解码最终 count 行。整数聚簇主键 ORDER BY
        // 和精确主键 WHERE 直接使用正向/反向 handle range，避免为 TopN 物化整表。
        // Window aggregates need the complete partition before applying LIMIT.
        let has_window = statement.Fields.Fields.iter().any(|field| {
            field
                .Expr
                .as_ref()
                .is_some_and(relational_expression_has_window)
        });
        let limit_pushdown_safe = !has_window
            && table.GetPartitionInfo().is_none()
            && !read_committed
            && right_table.is_none()
            && (statement.Where.is_none()
                || primary_key_access_ranges.is_some()
                || secondary_index_access.is_some()
                || index_merge_access.is_some())
            && (statement.OrderBy.is_empty()
                || primary_key_order.is_some()
                || secondary_index_access.is_some())
            && statement.GroupBy.is_empty()
            && statement.Having.is_none()
            && !statement.Distinct
            && !all_aggregates
            && !has_lock;
        let partitioned_secondary_index = secondary_index_access
            .as_ref()
            .is_some_and(|access| access.ranges.len() > 1);
        let scan_window = limit_window.filter(|_| {
            limit_pushdown_safe
                && index_merge_access.is_none()
                && !partitioned_secondary_index
                && self.state.borrow().transaction.is_none()
        });
        let index_merge_embedded_window = limit_window.filter(|_| {
            limit_pushdown_safe
                && index_merge_access
                    .as_ref()
                    .is_some_and(|access| access.intersection)
                && self.state.borrow().transaction.is_none()
        });
        let scan_limit = limit_window
            .filter(|_| limit_pushdown_safe && scan_window.is_none())
            .map(|window| {
                if window.count == 0 {
                    0
                } else {
                    window.offset.saturating_add(window.count)
                }
            });
        if is_scalar_count_non_null_constant(statement)
            && !read_committed
            && right_table.is_none()
            && statement.OrderBy.is_empty()
            && statement.GroupBy.is_empty()
            && statement.Having.is_none()
            && !statement.Distinct
            && !has_lock
            && source.Source.PartitionNames.is_empty()
        {
            let field = &statement.Fields.Fields[0];
            let ast::ExprKind::AggregateFunction { Name, .. } =
                &field.Expr.as_ref().expect("COUNT expression").Kind
            else {
                unreachable!("scalar COUNT predicate checked")
            };
            let header = if field.AsName.O.is_empty() {
                format!("{Name}(...)")
            } else {
                field.AsName.O.clone()
            };
            let count = if statement.Where.is_some() {
                self.count_registered_table_with_filter_at(
                    &table,
                    statement_read_ts,
                    statement_sql,
                )?
            } else {
                Some(self.count_registered_table_at(&table, statement_read_ts)?)
            };
            if let Some(count) = count {
                return Ok(Some(ConcreteRecordSet::new(
                    vec![header],
                    execute_relational_limit(vec![vec![count.to_string()]], limit_window)?,
                )));
            }
        }
        let ann_rows = if self.state.borrow().transaction.is_none()
            && statement_read_ts.is_none()
            && right_table.is_none()
            && statement.Where.is_none()
            && statement.GroupBy.is_empty()
            && statement.Having.is_none()
            && !statement.Distinct
            && !has_lock
            && table.GetPartitionInfo().is_none()
            && table
                .TiFlashReplica
                .as_ref()
                .is_some_and(|replica| replica.Available && replica.Count > 0)
            && self
                .state
                .borrow()
                .isolation_read_engines
                .split(',')
                .any(|engine| engine.trim() == "tiflash")
            && statement.OrderBy.len() == 1
            && !statement.OrderBy[0].Desc
        {
            if let (Some(window), ast::ExprKind::Function { FnName, Args, .. }) =
                (limit_window, &statement.OrderBy[0].Expr.Kind)
                && FnName.L == "vec_l2_distance"
                && Args.len() == 2
                && let ast::ExprKind::Column(column) = &Args[0].Kind
                && let Some(index) = table.Indices.iter().find(|index| {
                    index
                        .VectorInfo
                        .as_ref()
                        .is_some_and(|info| info.DistanceMetric.0.as_ref() == "L2")
                        && index
                            .Columns
                            .first()
                            .is_some_and(|indexed| indexed.Name.L == column.Name.L)
                        && !index.Invisible
                })
                && let Some(reference) = relational_expression_value(&Args[1], &HashMap::new())?
            {
                let top_k = u32::try_from(window.offset.saturating_add(window.count))
                    .map_err(|_| SessionError::new("ANN LIMIT exceeds TiFlash TopK range"))?;
                self.scan_relational_ann_rows(&table, index, &reference, top_k)?
            } else {
                None
            }
        } else {
            None
        };
        let mut rows = if let Some(rows) = ann_rows {
            rows
        } else if read_committed {
            if let Some(ranges) = candidate_ranges.as_deref() {
                self.scan_latest_with_transaction_overlay_ranges(&table, ranges)?
            } else if let Some(filter) = handle_filter.as_ref() {
                self.scan_latest_with_transaction_overlay_ranges_with_handle_filter(
                    &table,
                    &handle_ranges,
                    Some(filter),
                )?
            } else {
                self.scan_latest_with_transaction_overlay(&table)?
            }
        } else if let Some(window) = scan_window {
            if let Some(access) = secondary_index_access.as_ref() {
                self.scan_registered_table_at_with_index_window(
                    &table,
                    statement_read_ts,
                    window,
                    access,
                )?
            } else {
                self.scan_registered_table_at_with_window(
                    &table,
                    statement_read_ts,
                    window,
                    primary_key_order,
                    primary_key_access_ranges.as_deref(),
                )?
            }
        } else if let Some(access) = index_merge_access.as_ref() {
            self.scan_registered_table_at_with_index_merge(
                &table,
                statement_read_ts,
                access,
                statement.Where.as_ref(),
                index_merge_embedded_window,
            )?
        } else if let Some(access) = secondary_index_access
            .as_ref()
            .filter(|access| access.ranges.len() > 1)
        {
            self.scan_registered_table_at_with_index_window(
                &table,
                statement_read_ts,
                RelationalLimitWindow {
                    offset: 0,
                    count: usize::MAX,
                },
                access,
            )?
        } else if let Some(ranges) = candidate_ranges.as_deref() {
            self.scan_registered_table_ranges(&table, ranges)?
        } else if let Some(filter) = handle_filter.as_ref() {
            self.scan_registered_table_ranges_with_handle_filter(
                &table,
                &handle_ranges,
                Some(filter),
            )?
        } else {
            if astersql_testkit_testfailpoint::eval_bool("session/selectFullTableScan") {
                return Err(SessionError::new(
                    "injected failure: SELECT reached a full relational table scan",
                ));
            }
            self.scan_registered_table_at_with_limit(&table, statement_read_ts, scan_limit)?
        };
        if !source.Source.PartitionNames.is_empty()
            && let Some(partition) = table.GetPartitionInfo()
        {
            let requested = source
                .Source
                .PartitionNames
                .iter()
                .map(|name| name.L.as_str())
                .collect::<HashSet<_>>();
            let physical_ids = partition
                .Definitions
                .iter()
                .filter(|definition| requested.contains(definition.Name.L.as_str()))
                .map(|definition| definition.ID)
                .collect::<HashSet<_>>();
            rows.retain(|(_, row)| physical_ids.contains(&Self::row_physical_id(&table, row)));
        }
        if let Some(right_table) = right_table.as_ref() {
            let right_rows = self.scan_registered_table_at(right_table, statement_read_ts)?;
            rows = join_relational_rows(rows, right_rows, &table.Name.L, &right_table.Name.L);
            for column in &right_table.Columns {
                let mut column = column.clone();
                // Joined rows keep a qualified copy of right-side columns so
                // wildcard projection does not overwrite equal column names
                // from the left table (for example `t.c, u.c`).
                column.Name = ast::NewCIStr(&format!("{}.{}", right_table.Name.L, column.Name.L));
                table.Columns.push(column);
            }
        }
        if let Some(predicate) = statement.Where.as_ref().filter(|_| {
            !(scan_window.is_some() && secondary_index_access.is_some())
                && index_merge_embedded_window.is_none()
        }) {
            let scalar_filter = if let ast::ExprKind::Binary { Op, L, R } = &predicate.Kind
                && matches!(Op.as_str(), "=" | "==")
            {
                let pair = match (&L.Kind, &R.Kind) {
                    (ast::ExprKind::Column(column), ast::ExprKind::Subquery { Query, .. }) => {
                        Some((column, Query))
                    }
                    (ast::ExprKind::Subquery { Query, .. }, ast::ExprKind::Column(column)) => {
                        Some((column, Query))
                    }
                    _ => None,
                };
                pair.and_then(|(outer_column, query)| {
                    query
                        .with_node(|query| {
                            let select = query.as_any().downcast_ref::<ast::SelectStmt>()?;
                            let projected = select.Fields.Fields.first()?.Expr.as_ref()?;
                            let ast::ExprKind::Column(projected_column) = &projected.Kind else {
                                return None;
                            };
                            let mut inner_rows = self
                                .scan_registered_table_at(&table, statement_read_ts)
                                .ok()?;
                            if let Some(predicate) = select.Where.as_ref() {
                                inner_rows
                                    .retain(|(_, row)| row_matches_simple_where(row, predicate));
                            }
                            let value =
                                inner_rows.first()?.1.get(&projected_column.Name.L)?.clone();
                            Some((outer_column.Name.L.clone(), value))
                        })
                        .flatten()
                })
            } else {
                None
            };
            if let Some((column, value)) = scalar_filter {
                rows.retain(|(_, row)| row.get(&column) == Some(&value));
            } else {
                rows = self.filter_relational_query_rows(rows, predicate, &table)?;
            }
        }
        // Some mechanically migrated parser paths preserve the locking clause
        // only in the original SQL text. Even there, a locking read that
        // actually found rows must contribute its normal tables to the commit
        // schema check. Empty locking reads intentionally add no related ID,
        // matching TiDB's physical-lock-key based behavior.
        if !parsed_lock
            && textual_lock
            && self.state.borrow().transaction.is_some()
            && !rows.is_empty()
        {
            self.record_transaction_locking_table(&table);
            if let Some(right_table) = right_table.as_ref() {
                self.record_transaction_locking_table(right_table);
            }
        }
        if let Some(lock_info) = statement.lock_info.as_ref() {
            let lock_type = if lock_info.LockType == ast::SelectLockType::None {
                lock_info.lock_type
            } else {
                lock_info.LockType
            };
            if lock_type != ast::SelectLockType::None
                && self.state.borrow().transaction_stale_read_ts.is_some()
            {
                return Err(SessionError::new(
                    "locking read is not allowed in a read-only stale transaction",
                ));
            }
            let is_share = matches!(
                lock_type,
                ast::SelectLockType::ForShare
                    | ast::SelectLockType::ForShareNoWait
                    | ast::SelectLockType::ForShareSkipLocked
            );
            if is_share
                && !self.state.borrow().enable_noop_functions
                && !self.state.borrow().enable_shared_lock_promotion
            {
                return Err(SessionError::new(
                    "FOR SHARE is not supported; use tidb_enable_noop_functions to enable",
                ));
            }
            let should_lock = lock_type != ast::SelectLockType::None
                && (!is_share || self.state.borrow().enable_shared_lock_promotion)
                && self.state.borrow().transaction.is_some()
                && !self.state.borrow().transaction_explicit_optimistic;
            if should_lock {
                let scan_latest_candidates = || {
                    if let Some(ranges) = candidate_ranges.as_deref() {
                        self.scan_latest_with_transaction_overlay_ranges(&table, ranges)
                    } else if let Some(filter) = handle_filter.as_ref() {
                        self.scan_latest_with_transaction_overlay_ranges_with_handle_filter(
                            &table,
                            &handle_ranges,
                            Some(filter),
                        )
                    } else {
                        if astersql_testkit_testfailpoint::eval_bool("session/selectFullTableScan")
                        {
                            return Err(SessionError::new(
                                "injected failure: SELECT reached a full relational table scan",
                            ));
                        }
                        self.scan_latest_with_transaction_overlay(&table)
                    }
                };
                rows = scan_latest_candidates()?;
                if let Some(right_table) = right_table.as_ref() {
                    let right_rows = self.scan_latest_with_transaction_overlay(right_table)?;
                    rows =
                        join_relational_rows(rows, right_rows, &table.Name.L, &right_table.Name.L);
                }
                if let Some(predicate) = statement.Where.as_ref() {
                    rows = self.filter_relational_query_rows(rows, predicate, &table)?;
                }
                let domain_id = Arc::as_ptr(&self.domain) as usize;
                let flags = self.dml_type_flags();
                let mut keys = rows
                    .iter()
                    .map(|(_, row)| {
                        encode_relational_row(&table, row, flags).map(|(key, _)| {
                            RuntimeRowLockKey {
                                domain_id,
                                key: key.0,
                            }
                        })
                    })
                    .collect::<SessionResult<Vec<_>>>()?;
                keys.extend(
                    unique_lock_keys_for_rows(&table, rows.iter().map(|(_, row)| row))
                        .into_iter()
                        .map(|key| RuntimeRowLockKey { domain_id, key }),
                );
                let lock_missing_unique_key =
                    self.state.borrow().constraint_check_in_place_pessimistic;
                if let Some(predicate) = statement.Where.as_ref()
                    && (!rows.is_empty() || lock_missing_unique_key)
                {
                    keys.extend(
                        unique_lock_keys_for_predicate(&table, predicate)
                            .into_iter()
                            .map(|key| RuntimeRowLockKey { domain_id, key }),
                    );
                }
                if !keys.is_empty() {
                    self.record_transaction_locking_table(&table);
                    if let Some(right_table) = right_table.as_ref() {
                        self.record_transaction_locking_table(right_table);
                    }
                }
                let no_wait = matches!(
                    lock_type,
                    ast::SelectLockType::ForUpdateNoWait | ast::SelectLockType::ForShareNoWait
                );
                let wait_seconds = matches!(lock_type, ast::SelectLockType::ForUpdateWaitN)
                    .then_some(lock_info.WaitSec);
                self.acquire_row_locks(keys, no_wait, wait_seconds, true)?;
                // Re-scan after locking to preserve both the newest committed
                // rows and this transaction's local insert/update/delete set.
                rows = scan_latest_candidates()?;
                if let Some(right_table) = right_table.as_ref() {
                    let right_rows = self.scan_latest_with_transaction_overlay(right_table)?;
                    rows =
                        join_relational_rows(rows, right_rows, &table.Name.L, &right_table.Name.L);
                }
                if let Some(predicate) = statement.Where.as_ref() {
                    rows = self.filter_relational_query_rows(rows, predicate, &table)?;
                }
            } else if lock_type != ast::SelectLockType::None
                && self.state.borrow().transaction.is_some()
            {
                // Optimistic SELECT FOR UPDATE does not block concurrent
                // optimistic writers, but its commit must still publish a row
                // version so an overlapping writer detects a write conflict.
                let flags = self.dml_type_flags();
                let writes = rows
                    .iter()
                    .map(|(_, row)| encode_relational_row(&table, row, flags))
                    .collect::<SessionResult<Vec<_>>>()?;
                if !writes.is_empty() {
                    self.record_transaction_locking_table(&table);
                    if let Some(right_table) = right_table.as_ref() {
                        self.record_transaction_locking_table(right_table);
                    }
                }
                let mut state = self.state.borrow_mut();
                let transaction = state
                    .transaction
                    .as_mut()
                    .expect("optimistic locking read transaction");
                for (key, value) in &writes {
                    transaction
                        .Set(key.clone(), value.clone())
                        .map_err(|error| session_error("stage optimistic locking read", error))?;
                }
                let domain_id = Arc::as_ptr(&self.domain) as usize;
                state
                    .optimistic_for_update_keys
                    .extend(writes.iter().map(|(key, _)| RuntimeRowLockKey {
                        domain_id,
                        key: key.0.clone(),
                    }));
                state.txn_mem_buffer_keys = state
                    .txn_mem_buffer_keys
                    .saturating_add(writes.len() as u64);
                state.txn_mem_buffer_bytes = state.txn_mem_buffer_bytes.saturating_add(
                    writes
                        .iter()
                        .map(|(key, value)| key.0.len() as u64 + value.len() as u64 + 16)
                        .sum::<u64>(),
                );
            }
        }
        let mut used_index_ids = BTreeSet::new();
        if let Some(access) = secondary_index_access.as_ref() {
            used_index_ids.insert(access.index.ID);
        }
        if let Some(access) = index_merge_access.as_ref() {
            used_index_ids.extend(access.branches.iter().map(|branch| branch.index.ID));
        }
        for hint in &source.Source.IndexHints {
            if !matches!(hint.HintType, ast::IndexHintType::Ignore) {
                for name in &hint.IndexNames {
                    if name.L == "primary" {
                        used_index_ids.insert(
                            table
                                .Indices
                                .iter()
                                .find(|index| index.Primary)
                                .map_or(-1, |index| index.ID),
                        );
                    } else if let Some(index) =
                        table.Indices.iter().find(|index| index.Name.L == name.L)
                    {
                        used_index_ids.insert(index.ID);
                    }
                }
            }
        }
        let primary_columns = table
            .Indices
            .iter()
            .find(|index| index.Primary)
            .map(|index| {
                index
                    .Columns
                    .iter()
                    .map(|column| column.Name.L.clone())
                    .collect::<BTreeSet<_>>()
            })
            .or_else(|| {
                table
                    .GetPkColInfo()
                    .map(|column| BTreeSet::from([column.Name.L.clone()]))
            })
            .unwrap_or_default();
        let mut referenced_columns = HashSet::new();
        if let Some(predicate) = statement.Where.as_ref() {
            collect_expression_column_names(predicate, &mut referenced_columns);
        }
        referenced_columns.extend(statement.OrderBy.iter().filter_map(|item| {
            if let ast::ExprKind::Column(column) = &item.Expr.Kind {
                Some(column.Name.L.clone())
            } else {
                None
            }
        }));
        if !primary_columns.is_empty()
            && primary_columns
                .iter()
                .all(|column| referenced_columns.contains(column))
        {
            used_index_ids.insert(
                table
                    .Indices
                    .iter()
                    .find(|index| index.Primary)
                    .map_or(-1, |index| index.ID),
            );
        }
        if !used_index_ids.is_empty() {
            let row_access = rows.len() as u64;
            let table_rows = self.count_registered_table_at(&table, statement_read_ts)? as u64;
            let bucket = if table_rows == 0 || row_access >= table_rows {
                6
            } else {
                let percentage = row_access as f64 / table_rows as f64;
                if percentage == 0.0 {
                    0
                } else if percentage < 0.01 {
                    1
                } else if percentage < 0.1 {
                    2
                } else if percentage < 0.2 {
                    3
                } else if percentage < 0.5 {
                    4
                } else {
                    5
                }
            };
            let domain_id = runtime_domain_id(&self.domain);
            let now = SystemTime::now();
            let mut usage = RUNTIME_INDEX_USAGE
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for index_id in used_index_ids {
                let sample = usage.entry((domain_id, table.ID, index_id)).or_default();
                sample.last_access_time = Some(now);
                sample.query_total = sample.query_total.saturating_add(1);
                sample.kv_req_total = sample.kv_req_total.saturating_add(request_count as u64);
                sample.row_access_total = sample.row_access_total.saturating_add(row_access);
                sample.percentage_access[bucket] =
                    sample.percentage_access[bucket].saturating_add(1);
            }
        }
        if !statement.OrderBy.is_empty()
            && !(scan_window.is_some()
                && (primary_key_order.is_some() || secondary_index_access.is_some()))
        {
            let wildcard_columns = table
                .Columns
                .iter()
                .filter(|column| !column.Hidden)
                .map(|column| column.Name.L.clone())
                .collect::<Vec<_>>();
            let order_by =
                resolve_select_order_by(&statement.OrderBy, &statement.Fields, &wildcard_columns)?;
            sort_relational_rows(&mut rows, &order_by)?;
        }
        if all_aggregates {
            let mut headers = Vec::new();
            let mut aggregate_row = Vec::new();
            for field in &statement.Fields.Fields {
                let ast::ExprKind::AggregateFunction { Name, Args, .. } =
                    &field.Expr.as_ref().expect("aggregate expression").Kind
                else {
                    unreachable!("all fields checked as aggregate")
                };
                let name = Name.to_ascii_lowercase();
                let column = Args.first().and_then(|argument| match &argument.Kind {
                    ast::ExprKind::Column(column) => Some(column.Name.L.as_str()),
                    _ => None,
                });
                if matches!(name.as_str(), "sum" | "avg")
                    && column.is_some_and(|name| {
                        table.Columns.iter().any(|candidate| {
                            candidate.Name.L == name
                                && candidate.GetType()
                                    == astersql_parser_mysql::r#type::TypeTiDBVectorFloat32
                        })
                    })
                {
                    return Err(SessionError::new(format!(
                        "[expression:1582]Incorrect parameter count in the call to native function '{Name}'"
                    )));
                }
                let value = match name.as_str() {
                    "count" => column
                        .map_or(rows.len(), |column| {
                            rows.iter()
                                .filter(|(_, row)| row.get(column).is_some_and(Option::is_some))
                                .count()
                        })
                        .to_string(),
                    "bit_xor" => {
                        let column = column.ok_or_else(|| {
                            SessionError::new(format!("{Name} requires a column argument"))
                        })?;
                        rows.iter()
                            .filter_map(|(_, row)| row.get(column).and_then(Option::as_ref))
                            .try_fold(0_u64, |accumulator, value| {
                                value
                                    .parse::<i64>()
                                    .map(|value| accumulator ^ value as u64)
                                    .map_err(|error| session_error("parse BIT_XOR value", error))
                            })?
                            .to_string()
                    }
                    "min_count" | "max_count" => {
                        let column = column.ok_or_else(|| {
                            SessionError::new(format!("{Name} requires a column argument"))
                        })?;
                        relational_count_extrema(
                            rows.iter()
                                .filter_map(|(_, row)| row.get(column).and_then(Option::as_ref))
                                .cloned(),
                            name == "max_count",
                        )
                        .to_string()
                    }
                    "min" | "max" => {
                        let column = column.ok_or_else(|| {
                            SessionError::new(format!("{Name} requires a column argument"))
                        })?;
                        let mut values = rows
                            .iter()
                            .filter_map(|(_, row)| row.get(column).and_then(Option::as_ref))
                            .collect::<Vec<_>>();
                        values.sort_by(|left, right| {
                            match (left.parse::<i64>(), right.parse::<i64>()) {
                                (Ok(left), Ok(right)) => left.cmp(&right),
                                _ => left.cmp(right),
                            }
                        });
                        if name == "min" {
                            values.first().cloned()
                        } else {
                            values.last().cloned()
                        }
                        .cloned()
                        .unwrap_or_default()
                    }
                    _ => {
                        return Err(SessionError::new(format!(
                            "unsupported aggregate function {Name}"
                        )));
                    }
                };
                headers.push(if field.AsName.O.is_empty() {
                    format!("{Name}(...)")
                } else {
                    field.AsName.O.clone()
                });
                aggregate_row.push(value);
            }
            return Ok(Some(ConcreteRecordSet::new(
                headers,
                execute_relational_limit(vec![aggregate_row], limit_window)?,
            )));
        }
        if statement.Fields.Fields.len() == 1
            && statement.Fields.Fields[0]
                .Expr
                .as_ref()
                .is_some_and(|expression| {
                    matches!(
                        &expression.Kind,
                        ast::ExprKind::AggregateFunction { Name, .. }
                            if Name.eq_ignore_ascii_case("count")
                    )
                })
        {
            let field = &statement.Fields.Fields[0];
            let header = if field.AsName.O.is_empty() {
                "count(*)".to_owned()
            } else {
                field.AsName.O.clone()
            };
            return Ok(Some(ConcreteRecordSet::new(
                vec![header],
                execute_relational_limit(vec![vec![rows.len().to_string()]], limit_window)?,
            )));
        }
        let table_alias = if source.AsName.O.is_empty() {
            table.Name.clone()
        } else {
            source.AsName.clone()
        };
        // The point/scan shortcut must enforce the same visible range name as
        // prepared/full-query binding before dropping a column's qualifier.
        for field in &statement.Fields.Fields {
            let mut expression = field.Expr.as_ref();
            while let Some(expr) = expression {
                if let ast::ExprKind::Parentheses(inner) = &expr.Kind {
                    expression = Some(inner);
                    continue;
                }
                if let ast::ExprKind::Column(column) = &expr.Kind {
                    if !column.Table.L.is_empty()
                        && column.Table.L != table_alias.L
                        && !table.Columns.iter().any(|bound| {
                            bound.Name.L == format!("{}.{}", column.Table.L, column.Name.L)
                        })
                    {
                        return Err(SessionError::new(format!(
                            "Unknown column '{}.{}' in 'field list'",
                            column.Table.O, column.Name.O
                        )));
                    }
                }
                break;
            }
        }
        let root_limit_window = if scan_window.is_some() || index_merge_embedded_window.is_some() {
            None
        } else {
            limit_window
        };
        if !statement.Distinct && !has_window {
            rows = execute_relational_limit(rows, root_limit_window)?;
        }
        let (columns, projected, result_fields) = project_relational_rows(
            database,
            &table_alias,
            &table,
            &rows,
            &statement.Fields,
            &statement.WindowSpecs,
            |args, row| self.execute_embed_text(args, row),
        )?;
        let projected = if has_window && !statement.Distinct {
            execute_relational_limit(projected, root_limit_window)?
        } else if statement.Distinct {
            let mut seen = HashSet::new();
            let distinct = projected
                .into_iter()
                .filter(|row| seen.insert(row.clone()))
                .collect();
            execute_relational_limit(distinct, root_limit_window)?
        } else {
            projected
        };
        let mut projected = projected;
        if self.sql_dialect() == ast::SqlDialect::PostgreSQL {
            // Storage carries UTC instants; format declared timestamptz columns
            // using this reader's session TimeZone, including SELECT *.
            for row in &mut projected {
                for (value, field) in row.iter_mut().zip(&result_fields) {
                    if value != SHOW_NULL_CELL
                        && let Some(field) = field
                        && field.column.GetType()
                            == astersql_parser_mysql::r#type::TypePgTimestampTz
                    {
                        *value = self
                            .pg_timestamptz_relational_value(
                                Some(value.clone()),
                                field.column.GetDecimal(),
                            )?
                            .unwrap();
                    }
                }
            }
        }
        Ok(Some(ConcreteRecordSet::new_with_fields(
            columns,
            projected,
            result_fields,
        )))
    }
}

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
            "concat" | "elt" | "hex" | "json_type" | "lcase" | "lower" | "lpad" | "mid"
            | "repeat" | "space" | "substr" | "substring" | "ucase" | "unhex" | "upper" => true,
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
        match &expression.Kind {
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
                        return self.relational_query_expression_value(
                            &clause.Result,
                            row,
                            group_rows,
                        );
                    }
                }
                ElseClause
                    .as_ref()
                    .map(|expression| {
                        self.relational_query_expression_value(expression, row, group_rows)
                    })
                    .unwrap_or(Ok(None))
            }
            ast::ExprKind::Parentheses(inner) => {
                self.relational_query_expression_value(inner, row, group_rows)
            }
            ast::ExprKind::Cast { Expr, Tp, .. } => {
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
            self.mdl_stats_table(database, &source.Source.Name.L)
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
        select.With.is_some()
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

    pub(super) fn execute_full_relational_select(
        &self,
        select: &ast::SelectStmt,
    ) -> SessionResult<ConcreteRecordSet> {
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
                if let Some(table) = self.resolve_runtime_table(database, &source.Source.Name.L) {
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
    ) -> SessionResult<()> {
        let source = match source_node {
            ast::ResultSetNode::TableSource(source) => source,
            ast::ResultSetNode::Join(join) => {
                if let Some(left) = join.Left.as_deref() {
                    self.append_relational_source_fields(left, fields)?;
                }
                if let Some(right) = join.Right.as_deref() {
                    self.append_relational_source_fields(right, fields)?;
                }
                return Ok(());
            }
        };

        if let Some(query) = source.QuerySource.as_ref() {
            let mut derived_fields = query
                .with_node(|node| self.relational_query_node_result_fields(node, &[]))
                .ok_or_else(|| SessionError::new("derived table query is unavailable"))??;
            if !source.AsName.L.is_empty() {
                for field in &mut derived_fields {
                    field.table_as_name = source.AsName.clone();
                }
            }
            fields.extend(derived_fields);
            return Ok(());
        }

        let current_database = self.current_database();
        let database = if source.Source.Schema.L.is_empty() {
            current_database.as_str()
        } else {
            source.Source.Schema.L.as_str()
        };
        let physical_table = self
            .mdl_stats_table(database, &source.Source.Name.L)
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
        let mut fields = Vec::new();
        if let Some(from) = select.From.as_ref() {
            if let Some(left) = from.TableRefs.Left.as_deref() {
                self.append_relational_source_fields(left, &mut fields)?;
            }
            if let Some(right) = from.TableRefs.Right.as_deref() {
                self.append_relational_source_fields(right, &mut fields)?;
            }
        }
        Ok(fields)
    }

    pub(super) fn relational_query_expression_type_from_sources(
        &self,
        expression: &ast::ExprNode,
        source_fields: &[ConcreteResultField],
    ) -> astersql_parser_types::types::FieldType {
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

    pub(super) fn relational_query_node_result_fields(
        &self,
        node: &dyn ast::Node,
        output_columns: &[String],
    ) -> SessionResult<Vec<ConcreteResultField>> {
        if let Some(select) = node.as_any().downcast_ref::<ast::SelectStmt>() {
            let source_fields = self.relational_select_source_fields(select)?;
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
                    .or_else(|| match &expression.Kind {
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
                                || candidate.table_as_name.L == column.Table.L
                                || candidate.table_name.L == column.Table.L)
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
                                        &source.Source.Name.L,
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
                    result_field.column_as_name = ast::NewCIStr(&output);
                    result.push(result_field);
                } else {
                    result.push(ConcreteResultField {
                        column: astersql_meta_model::ColumnInfo {
                            Name: ast::CIStr::default(),
                            FieldType: self.relational_query_expression_type_from_sources(
                                expression,
                                &source_fields,
                            ),
                            ..Default::default()
                        },
                        column_as_name: ast::NewCIStr(&output),
                        table_name: ast::CIStr::default(),
                        table_as_name: ast::CIStr::default(),
                        db_name: ast::CIStr::default(),
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
        self.relational_query_node_result_fields(first.as_ref(), output_columns)
    }

    pub(super) fn execute_relational_query_record_set(
        &self,
        node: &dyn ast::Node,
    ) -> SessionResult<ConcreteRecordSet> {
        let selected = self.execute_insert_select_node(node)?;
        let fields = self.relational_query_node_result_fields(node, &selected.columns)?;
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
                    .map(|column| {
                        let value = row.get(column).cloned().unwrap_or(None);
                        if let Some(bytes) = value.as_deref().and_then(binary_runtime_bytes) {
                            return ast::ExprNode::HexValue(bytes, "binary", "binary");
                        }
                        ast::NewValueExpr(value, "utf8mb4", "utf8mb4_bin")
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
                    &table_source.Source.Name.L,
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
            .local_temporary_table(database, &source.Source.Name.L)
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
                            self.mdl_stats_table(database, &source.Source.Name.L)
                                .map(|(_, table)| table)
                        })
                } else {
                    self.mdl_stats_table(database, &source.Source.Name.L)
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
                .local_temporary_table(right_database, &right_source.Source.Name.L)
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
                                self.mdl_stats_table(right_database, &right_source.Source.Name.L)
                                    .map(|(_, table)| table)
                            })
                    } else {
                        self.mdl_stats_table(right_database, &right_source.Source.Name.L)
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
            self.scan_latest_with_transaction_overlay(&table)?
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
        } else {
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
                rows = self.scan_latest_with_transaction_overlay(&table)?;
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
                rows = self.scan_latest_with_transaction_overlay(&table)?;
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
        Ok(Some(ConcreteRecordSet::new_with_fields(
            columns,
            projected,
            result_fields,
        )))
    }
}

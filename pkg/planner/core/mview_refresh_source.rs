// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

use crate::ast::{self, Node};
use crate::mview_refresh_builder::*;
use crate::{MViewAggKind, MViewCompleteDeltaApply, MViewDeltaMerge};

fn expr(kind: ast::ExprKind) -> ast::ExprNode {
    ast::ExprNode {
        Kind: kind,
        ..Default::default()
    }
}
fn column(table: &str, name: &str) -> ast::ExprNode {
    expr(ast::ExprKind::Column(ast::ColumnName {
        Table: ast::NewCIStr(table),
        Name: ast::NewCIStr(name),
        ..Default::default()
    }))
}
fn value(value: i64) -> ast::ExprNode {
    ast::NewValueExpr(value, "utf8mb4", "utf8mb4_bin")
}
fn null() -> ast::ExprNode {
    ast::ExprNode::NullValue()
}
fn binary(op: &str, left: ast::ExprNode, right: ast::ExprNode) -> ast::ExprNode {
    expr(ast::ExprKind::Binary {
        Op: op.into(),
        L: Box::new(left),
        R: Box::new(right),
    })
}
fn is_null(input: ast::ExprNode, not: bool) -> ast::ExprNode {
    expr(ast::ExprKind::IsNull {
        Expr: Box::new(input),
        Not: not,
    })
}
fn function(name: &str, args: Vec<ast::ExprNode>) -> ast::ExprNode {
    expr(ast::ExprKind::Function {
        Schema: Default::default(),
        FnName: ast::NewCIStr(name),
        Args: args,
    })
}
fn aggregate(name: &str, arg: ast::ExprNode) -> ast::ExprNode {
    expr(ast::ExprKind::AggregateFunction {
        Name: name.into(),
        Args: vec![arg],
        Distinct: false,
        Order: vec![],
        Filter: None,
    })
}
fn field(expression: ast::ExprNode, name: &str) -> ast::SelectField {
    ast::SelectField {
        Expr: Some(expression),
        AsName: ast::NewCIStr(name),
        ..Default::default()
    }
}
fn table(database: &str, name: &str, alias: &str) -> ast::ResultSetNode {
    ast::ResultSetNode::TableSource(ast::TableSource {
        Source: ast::TableName {
            Schema: ast::NewCIStr(database),
            Name: ast::NewCIStr(name),
            ..Default::default()
        },
        AsName: ast::NewCIStr(alias),
        ..Default::default()
    })
}
fn derived(select: ast::SelectStmt, alias: &str) -> ast::ResultSetNode {
    ast::ResultSetNode::TableSource(ast::TableSource {
        QuerySource: Some(ast::NodeRef::new(Box::new(select))),
        AsName: ast::NewCIStr(alias),
        ..Default::default()
    })
}
fn from(
    left: ast::ResultSetNode,
    right: Option<ast::ResultSetNode>,
    kind: ast::JoinType,
    on: Option<ast::ExprNode>,
) -> Option<ast::TableRefsClause> {
    Some(ast::TableRefsClause {
        TableRefs: ast::Join {
            Left: Some(Box::new(left)),
            Right: right.map(Box::new),
            Tp: kind,
            On: on,
            ..Default::default()
        },
    })
}
fn conjunction(items: impl IntoIterator<Item = ast::ExprNode>) -> Option<ast::ExprNode> {
    items
        .into_iter()
        .reduce(|left, right| binary("and", left, right))
}
struct StripQualifier;
impl ast::InPlaceVisitor for StripQualifier {
    fn enter(&mut self, node: &mut dyn ast::Node) -> bool {
        if let Some(expression) = node.as_any_mut().downcast_mut::<ast::ExprNode>() {
            if let ast::ExprKind::Column(column) = &mut expression.Kind {
                column.Schema = Default::default();
                column.Table = Default::default();
            }
        }
        false
    }
    fn leave(&mut self, _: &mut dyn ast::Node) -> bool {
        true
    }
}
fn unqualified(mut expression: ast::ExprNode) -> ast::ExprNode {
    expression.accept_in_place(&mut StripQualifier);
    expression
}
fn arguments(
    select: &ast::SelectStmt,
    definition: &MViewDefinition,
) -> Result<Vec<ast::ExprNode>, String> {
    if select.Fields.Fields.len() != definition.mv_columns.len() {
        return Err("materialized view select/output column count mismatch".into());
    }
    definition
        .aggregates
        .iter()
        .map(|aggregate| {
            let expression = select
                .Fields
                .Fields
                .get(aggregate.mv_offset)
                .and_then(|field| field.Expr.as_ref())
                .ok_or("materialized view aggregate field is missing")?;
            let ast::ExprKind::AggregateFunction {
                Args,
                Distinct,
                Filter,
                ..
            } = &expression.Kind
            else {
                return Err("materialized view aggregate field is not an aggregate".into());
            };
            if *Distinct || Filter.is_some() {
                return Err("materialized view aggregate DISTINCT/FILTER is unsupported".into());
            }
            Args.first()
                .cloned()
                .map(unqualified)
                .ok_or_else(|| "materialized view aggregate argument missing".into())
        })
        .collect()
}
fn key_conditions(definition: &MViewDefinition, left: &str, right: &str) -> Option<ast::ExprNode> {
    conjunction(definition.group_key_mv_offsets.iter().map(|offset| {
        let column_info = &definition.mv_columns[*offset];
        binary(
            if column_info.not_null { "=" } else { "<=>" },
            column(left, &column_info.name),
            column(right, &column_info.name),
        )
    }))
}

/// Build the real signed delta aggregate and current-MV LEFT JOIN. The original
/// WHERE is cloned structurally and only its table qualifiers are removed.
pub fn BuildFastRefreshSource(
    definition: &MViewDefinition,
    mview_name: &str,
    mlog_name: &str,
    select: &ast::SelectStmt,
    options: MViewBuildOptions,
) -> Result<(ast::SelectStmt, MViewDeltaMerge), String> {
    let RefreshImplementPlan::Fast(plan) = BuildRefreshMaterializedViewImplement(
        RefreshImplementMode::Fast,
        definition,
        options,
        options.to_ts,
    )?
    else {
        unreachable!()
    };
    let layout = BuildMView(definition, options)?;
    let arguments = arguments(select, definition)?;
    let mut fields = definition
        .group_key_mv_offsets
        .iter()
        .zip(&definition.group_key_base_columns)
        .map(|(offset, name)| field(column("", name), &definition.mv_columns[*offset].name))
        .collect::<Vec<_>>();
    let old_new = column("", MLOG_OLD_NEW_COLUMN);
    fields.push(field(
        aggregate("sum_int", old_new.clone()),
        "__mview_delta_cnt_star",
    ));
    for (aggregate_info, arg) in definition.aggregates.iter().zip(arguments) {
        use MViewAggKind::*;
        let offset = aggregate_info.mv_offset;
        let added = binary("=", old_new.clone(), value(1));
        let removed = binary("=", old_new.clone(), value(-1));
        match aggregate_info.kind {
            CountStar => {}
            Count => fields.push(field(
                aggregate(
                    "sum_int",
                    function("if", vec![is_null(arg, true), old_new.clone(), value(0)]),
                ),
                &format!("__mview_delta_cnt_{offset}"),
            )),
            Sum => fields.push(field(
                aggregate(
                    "sum",
                    function(
                        "if",
                        vec![
                            added,
                            arg.clone(),
                            expr(ast::ExprKind::Unary {
                                Op: "-".into(),
                                V: Box::new(arg),
                            }),
                        ],
                    ),
                ),
                &format!("__mview_delta_sum_{offset}"),
            )),
            Min | Max => {
                let name = if aggregate_info.kind == Min {
                    "min"
                } else {
                    "max"
                };
                let added_arg = function("if", vec![added, arg.clone(), null()]);
                let removed_arg = function("if", vec![removed, arg, null()]);
                for (function_name, input, alias) in [
                    (
                        name,
                        added_arg.clone(),
                        format!("__mview_{name}_in_added_{offset}"),
                    ),
                    (
                        if name == "min" {
                            "min_count"
                        } else {
                            "max_count"
                        },
                        added_arg,
                        format!("__mview_{name}_cnt_in_added_{offset}"),
                    ),
                    (
                        name,
                        removed_arg.clone(),
                        format!("__mview_{name}_in_removed_{offset}"),
                    ),
                    (
                        if name == "min" {
                            "min_count"
                        } else {
                            "max_count"
                        },
                        removed_arg,
                        format!("__mview_{name}_cnt_in_removed_{offset}"),
                    ),
                ] {
                    fields.push(field(aggregate(function_name, input), &alias));
                }
            }
        }
    }
    let mut predicates = vec![binary(
        ">",
        column("", "_tidb_commit_ts"),
        ast::NewValueExpr(options.from_ts, "utf8mb4", "utf8mb4_bin"),
    )];
    if options.to_ts > 0 {
        predicates.push(binary(
            "<=",
            column("", "_tidb_commit_ts"),
            ast::NewValueExpr(options.to_ts, "utf8mb4", "utf8mb4_bin"),
        ));
    }
    if let Some(predicate) = select.Where.clone() {
        predicates.push(unqualified(predicate));
    }
    let delta = ast::SelectStmt {
        Fields: ast::FieldList { Fields: fields },
        From: from(
            table(&definition.database, mlog_name, ""),
            None,
            ast::JoinType::CrossJoin,
            None,
        ),
        Where: conjunction(predicates),
        GroupBy: definition
            .group_key_base_columns
            .iter()
            .map(|name| ast::ByItem {
                Expr: column("", name),
                Desc: false,
            })
            .collect(),
        ..Default::default()
    };
    let mut projection = layout.source_columns[..layout.delta_column_count]
        .iter()
        .map(|name| field(column("delta", name), name))
        .collect::<Vec<_>>();
    for (offset, info) in definition.mv_columns.iter().enumerate() {
        projection.push(field(
            column(
                if definition.group_key_mv_offsets.contains(&offset) {
                    "delta"
                } else {
                    "mv"
                },
                &info.name,
            ),
            &info.name,
        ));
    }
    if definition.handle_kind == MViewHandleKind::ExtraRowId {
        projection.push(field(column("mv", "_tidb_rowid"), "__mview_mv_rowid"));
    }
    Ok((
        ast::SelectStmt {
            Fields: ast::FieldList { Fields: projection },
            From: from(
                derived(delta, "delta"),
                Some(table(&definition.database, mview_name, "mv")),
                ast::JoinType::LeftJoin,
                key_conditions(definition, "delta", "mv"),
            ),
            ..Default::default()
        },
        plan,
    ))
}

/// Full outer diff preserves nullable group keys and old/new typed row images.
pub fn BuildCompleteRefreshSource(
    definition: &MViewDefinition,
    mview_name: &str,
    mut select: ast::SelectStmt,
) -> Result<(ast::SelectStmt, MViewCompleteDeltaApply), String> {
    let RefreshImplementPlan::CompleteDeltaApply(plan) = BuildRefreshMaterializedViewImplement(
        RefreshImplementMode::CompleteDeltaApply,
        definition,
        MViewBuildOptions::default(),
        0,
    )?
    else {
        unreachable!()
    };
    if select.Fields.Fields.len() != definition.mv_columns.len() {
        return Err("materialized view select/output column count mismatch".into());
    }
    for (field, info) in select.Fields.Fields.iter_mut().zip(&definition.mv_columns) {
        field.AsName = ast::NewCIStr(&info.name);
    }
    let layout = BuildCompleteDiffSource(definition)?;
    let marker = &definition.mv_columns[layout.marker_mv_offset].name;
    let missing_new = is_null(column("new", marker), false);
    let missing_old = is_null(column("old", marker), false);
    let op = function(
        "if",
        vec![
            missing_old.clone(),
            value(1),
            function("if", vec![missing_new.clone(), value(2), value(3)]),
        ],
    );
    let mut fields = vec![field(op, "__mview_diff_op")];
    if definition.handle_kind == MViewHandleKind::ExtraRowId {
        fields.push(field(
            column("old", "_tidb_rowid"),
            "__mview_handle__tidb_rowid",
        ));
    }
    for alias in ["old", "new"] {
        for info in &definition.mv_columns {
            fields.push(field(
                column(alias, &info.name),
                &format!("__mview_{alias}_{}", info.name),
            ));
        }
    }
    let payload = conjunction(
        definition
            .mv_columns
            .iter()
            .enumerate()
            .filter(|(offset, _)| !definition.group_key_mv_offsets.contains(offset))
            .map(|(_, info)| {
                binary(
                    if info.not_null { "=" } else { "<=>" },
                    column("old", &info.name),
                    column("new", &info.name),
                )
            }),
    )
    .unwrap_or_else(|| value(1));
    Ok((
        ast::SelectStmt {
            Fields: ast::FieldList { Fields: fields },
            From: from(
                derived(select, "new"),
                Some(table(&definition.database, mview_name, "old")),
                ast::JoinType::FullJoin,
                key_conditions(definition, "new", "old"),
            ),
            Where: Some(binary(
                "or",
                binary("or", missing_new, missing_old),
                expr(ast::ExprKind::Unary {
                    Op: "not".into(),
                    V: Box::new(payload),
                }),
            )),
            ..Default::default()
        },
        plan,
    ))
}

/// Bind only requested groups for MIN/MAX recomputation at the pinned read TSO.
/// Keep the original WHERE and supporting index hints on the actual base scan.
pub fn BuildBoundMinMaxLookup(
    definition: &MViewDefinition,
    base_name: &str,
    select: &ast::SelectStmt,
    keys: &[Vec<ast::ExprNode>],
) -> Result<ast::SelectStmt, String> {
    let layout = BuildMView(definition, MViewBuildOptions::default())?;
    if layout.full_update_supporting_indexes.is_empty() {
        return Err("MIN/MAX lookup has no supporting index".into());
    }
    let arguments = arguments(select, definition)?;
    let mut fields = Vec::new();
    for offset in &layout.full_update_lookup_mv_offsets {
        let info = &definition.mv_columns[*offset];
        let expression = if let Some(key) = definition
            .group_key_mv_offsets
            .iter()
            .position(|value| value == offset)
        {
            column("", &definition.group_key_base_columns[key])
        } else {
            let index = definition
                .aggregates
                .iter()
                .position(|aggregate| aggregate.mv_offset == *offset)
                .ok_or("lookup output mapping missing")?;
            aggregate(
                if definition.aggregates[index].kind == MViewAggKind::Min {
                    "min"
                } else {
                    "max"
                },
                arguments[index].clone(),
            )
        };
        fields.push(field(expression, &info.name));
    }
    let mut probes = Vec::new();
    for key in keys {
        if key.len() != definition.group_key_base_columns.len() {
            return Err("lookup group-key arity mismatch".into());
        }
        probes.push(
            conjunction(
                key.iter()
                    .zip(&definition.group_key_base_columns)
                    .map(|(key, name)| binary("<=>", column("", name), key.clone())),
            )
            .ok_or("empty lookup key")?,
        );
    }
    let mut predicate = probes
        .into_iter()
        .reduce(|left, right| binary("or", left, right))
        .unwrap_or_else(|| value(0));
    if let Some(original) = select.Where.clone() {
        predicate = binary("and", unqualified(original), predicate);
    }
    let mut source = table(&definition.database, base_name, "");
    if let ast::ResultSetNode::TableSource(source) = &mut source {
        source.Source.IndexHints.push(ast::IndexHint {
            IndexNames: layout
                .full_update_supporting_indexes
                .iter()
                .map(|name| ast::NewCIStr(name))
                .collect(),
            HintType: ast::IndexHintType::Use,
            HintScope: ast::IndexHintScope::Scan,
        });
    }
    Ok(ast::SelectStmt {
        Fields: ast::FieldList { Fields: fields },
        From: from(source, None, ast::JoinType::CrossJoin, None),
        Where: Some(predicate),
        GroupBy: definition
            .group_key_base_columns
            .iter()
            .map(|name| ast::ByItem {
                Expr: column("", name),
                Desc: false,
            })
            .collect(),
        ..Default::default()
    })
}

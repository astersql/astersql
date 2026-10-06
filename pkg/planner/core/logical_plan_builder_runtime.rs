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

// 逻辑计划运行时构建器：从解析器 AST 生成真实逻辑算子树。
//
// 覆盖 SELECT、集合运算、SHOW、DO、WITH/CTE、JOIN/LATERAL Apply、
// 聚合、窗口、投影、LIMIT/DISTINCT 等路径，并维护 CTE 环境与权限访问信息。
// 对应 Go `planner/core` 中 PlanBuilder 的结果集构建实现。

#![allow(non_snake_case)]

use crate::{CteInfoRef, PlanBuilder, VisitInfo};
use aggregation_dependency as aggregation;
use base_dependency as base;
use coreusage_dependency as coreusage;
use expression_dependency as expression;
use hint_dependency as hint;
use logicalop_dependency as logicalop;
use plannererrors_dependency as plannererrors;
use rule_dependency as rule;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, RwLock};
use types_dependency as types;

use logicalop::LogicalPlan as _;

/// CTE 绑定：类、Schema、列名、递归引用标记与统计。
pub(crate) struct CteBinding {
    class: logicalop::CTEClassRef,
    schema: expression::Schema,
    names: expression::types::NameSlice,
    name: crate::ast::CIStr,
    recursive_reference: bool,
    seed_stat: Arc<RwLock<logicalop::StatsInfo>>,
    storage_id: i32,
}

impl Clone for CteBinding {
    fn clone(&self) -> Self {
        Self {
            class: self.class.clone(),
            schema: self.schema.Clone(),
            names: self.names.Shallow(),
            name: self.name.clone(),
            recursive_reference: self.recursive_reference,
            seed_stat: self.seed_stat.clone(),
            storage_id: self.storage_id,
        }
    }
}

/// CTE 名称到绑定的环境映射。
pub(crate) type CteEnvironment = HashMap<String, CteBinding>;

/// Default `PlanBuilder` result-set entry point.  It consumes the parser AST
/// directly; callers that need a specialized builder can still replace it via
/// `withResultSetBuilder`.
pub(crate) fn BuildResultSetNode(
    builder: &mut PlanBuilder,
    ctx: &dyn crate::context::Context,
    node: &crate::ast::NodeRef,
    _is_cte: bool,
) -> Result<logicalop::LogicalPlanRef, expression::Error> {
    node.with_node(|node| BuildBorrowedResultSetNode(builder, ctx, node))
        .ok_or_else(|| expression::errors::New("result-set AST node has already been consumed"))?
}

/// 借用式结果集构建：压入 handle 映射后走运行时查询构建。
pub(crate) fn BuildBorrowedResultSetNode(
    builder: &mut PlanBuilder,
    ctx: &dyn crate::context::Context,
    node: &dyn crate::ast::Node,
) -> Result<logicalop::LogicalPlanRef, expression::Error> {
    builder.handleHelper.pushMap();
    // A subquery is built through this same callback.  Preserve the CTEs that
    // are visible in its outer query while isolating any CTEs declared by the
    // subquery itself.
    let mut ctes = builder.runtimeCTEs.clone().unwrap_or_default();
    let previous_ctes = std::mem::replace(&mut builder.runtimeCTEs, Some(ctes.clone()));
    let plan_result = build_query_node_with_ctes(builder, ctx, node, &mut ctes);
    builder.runtimeCTEs = previous_ctes;
    let mut plan = plan_result?;
    // Go runs RecheckCTE after the complete logical tree has been built.  The
    // outermost marker controls whether predicates from CTE consumers may be
    // collected into the shared seed plan.
    crate::recheck_cte::RecheckLogicalCTE(&mut plan);
    Ok(plan)
}

/// 在 CTE 环境下分派 SELECT / 集合运算 / SHOW / DO。
fn build_query_node_with_ctes(
    builder: &mut PlanBuilder,
    ctx: &dyn crate::context::Context,
    node: &dyn crate::ast::Node,
    ctes: &mut CteEnvironment,
) -> Result<logicalop::LogicalPlanRef, expression::Error> {
    if let Some(select) = node.as_any().downcast_ref::<crate::ast::SelectStmt>() {
        return build_select_runtime(builder, ctx, select, ctes);
    }
    if let Some(set_operation) = node.as_any().downcast_ref::<crate::ast::SetOprStmt>() {
        return build_set_operation_runtime(builder, ctx, set_operation, ctes);
    }
    if let Some(show) = node.as_any().downcast_ref::<crate::ast::ShowStmt>() {
        return build_show_runtime(builder, show);
    }
    if let Some(statement) = node.as_any().downcast_ref::<crate::ast::DoStmt>() {
        return build_do_runtime(builder, statement);
    }
    if let Some(list) = node.as_any().downcast_ref::<crate::ast::SetOprSelectList>() {
        if list.selects.len() != 1 {
            return Err(expression::errors::New(
                "parenthesized set-operation list requires exactly one input here",
            ));
        }
        let mut plan = build_query_node_with_ctes(builder, ctx, list.selects[0].as_ref(), ctes)?;
        plan = build_order_limit_runtime(builder, &list.OrderBy, list.Limit.as_ref(), plan)?;
        return Ok(plan);
    }
    Err(expression::errors::New(
        "the default result-set builder supports SELECT and set operations",
    ))
}

/// 按 SHOW 类型构造内存表 Schema 与计划。
fn build_show_runtime(
    builder: &mut PlanBuilder,
    show: &crate::ast::ShowStmt,
) -> Result<logicalop::LogicalPlanRef, expression::Error> {
    use crate::ast::ShowStmtType;

    let columns: &[&str] = match show.Tp {
        ShowStmtType::Columns => &["Field", "Type", "Null", "Key", "Default", "Extra"],
        ShowStmtType::Databases => &["Database"],
        ShowStmtType::Tables => &["Tables_in_database"],
        ShowStmtType::Variables | ShowStmtType::Status => &["Variable_name", "Value"],
        ShowStmtType::Warnings | ShowStmtType::Errors if show.CountWarningsOrErrors => &["Count"],
        ShowStmtType::Warnings | ShowStmtType::Errors => &["Level", "Code", "Message"],
        ShowStmtType::Engines => &[
            "Engine",
            "Support",
            "Comment",
            "Transactions",
            "XA",
            "Savepoints",
        ],
        ShowStmtType::Collation => &[
            "Collation",
            "Charset",
            "Id",
            "Default",
            "Compiled",
            "Sortlen",
            "Pad_attribute",
        ],
        ShowStmtType::Charset => &["Charset", "Description", "Default collation", "Maxlen"],
        _ => &["Result"],
    };
    let mut schema_columns = Vec::with_capacity(columns.len());
    let mut names = expression::types::NameSlice(Vec::with_capacity(columns.len()));
    for (index, name) in columns.iter().enumerate() {
        let mut field_type = expression::types::NewFieldType(expression::mysql::TypeVarchar);
        if matches!(show.Tp, ShowStmtType::Warnings | ShowStmtType::Errors)
            && ((*name == "Code") || (*name == "Count"))
        {
            field_type = expression::types::NewFieldType(expression::mysql::TypeLonglong);
        }
        schema_columns.push(expression::Column::new(
            *field_type,
            0,
            builder.ctx.GetSessionVars().AllocPlanColumnID(),
            index as isize,
        ));
        names.0.push(Some(Arc::new(expression::types::FieldName {
            ColName: crate::ast::NewCIStr(name),
            ..Default::default()
        })));
    }
    let schema = expression::NewSchema(schema_columns);
    let mut logical_show = logicalop::LogicalShow {
        ShowContents: logicalop::ShowContents {
            Tp: if show.Tp == ShowStmtType::StatsMeta {
                logicalop::ShowKind::StatsMeta
            } else {
                logicalop::ShowKind::Other
            },
            DBName: show.DBName.clone(),
            Partition: show.Partition.clone(),
            IndexName: show.IndexName.clone(),
            ResourceGroupName: show.ResourceGroupName.clone(),
            Flag: show.Flag as i32,
            CountWarningsOrErrors: show.CountWarningsOrErrors,
            Full: show.Full,
            IfNotExists: show.IfNotExists,
            GlobalScope: show.GlobalScope,
            Extended: show.Extended,
            ImportJobID: show.ImportJobID,
            ImportGroupKey: show.ShowGroupKey.clone(),
            DistributionJobID: show.DistributionJobID,
        },
        ..Default::default()
    }
    .Init(builder.ctx.clone());
    logical_show.SetSchema(schema);
    logical_show.SetOutputNames(names);
    let mut plan: logicalop::LogicalPlanRef = Box::new(logical_show);

    if let Some(predicate) = &show.Where {
        let (condition, rewritten_plan) = crate::expression_rewriter::rewrite(
            builder,
            crate::context::TODOArc(),
            predicate,
            plan,
            crate::expression_rewriter::AggregateMapper::default(),
            false,
        )?;
        plan = rewritten_plan;
        let conditions = condition.map_or_else(Vec::new, |condition| {
            expression::SplitCNFItems(condition.as_ref())
        });
        if !conditions.is_empty() {
            let mut selection = logicalop::LogicalSelection {
                Conditions: conditions,
                ..Default::default()
            }
            .Init(builder.ctx.clone(), 0);
            selection.SetSchema(plan.Schema().Clone());
            selection.SetOutputNames(plan.OutputNames().Shallow());
            selection.SetChildren(vec![plan]);
            plan = Box::new(selection);
        }
    }

    if show.Where.is_some() || show.Pattern.is_some() {
        let expressions = plan
            .Schema()
            .Columns
            .iter()
            .map(|column| Box::new(column.Clone()) as expression::ExprBox)
            .collect();
        let projected_schema = expression::NewSchema(
            plan.Schema()
                .Columns
                .iter()
                .enumerate()
                .map(|(index, column)| {
                    expression::Column::new(
                        column.RetType.clone().unwrap_or_default(),
                        column.ID,
                        builder.ctx.GetSessionVars().AllocPlanColumnID(),
                        index as isize,
                    )
                })
                .collect(),
        );
        let mut projection = logicalop::LogicalProjection {
            Exprs: expressions,
            ..Default::default()
        }
        .Init(builder.ctx.clone(), 0);
        projection.SetSchema(projected_schema);
        projection.SetOutputNames(plan.OutputNames().Shallow());
        projection.SetChildren(vec![plan]);
        plan = Box::new(projection);
        builder.optFlag |= rule::FLAG_ELIMINATE_PROJECTION;
    }
    Ok(plan)
}

/// 构建 DO 语句为投影求值计划。
fn build_do_runtime(
    builder: &mut PlanBuilder,
    statement: &crate::ast::DoStmt,
) -> Result<logicalop::LogicalPlanRef, expression::Error> {
    let mut dual = logicalop::LogicalTableDual {
        RowCount: 1,
        ..Default::default()
    }
    .Init(builder.ctx.clone(), 0);
    dual.SetSchema(expression::NewSchema(Vec::new()));
    dual.SetOutputNames(expression::types::NameSlice(Vec::new()));
    let mut plan: logicalop::LogicalPlanRef = Box::new(dual);
    let mut expressions = Vec::with_capacity(statement.Exprs.len());
    for node in &statement.Exprs {
        let (rewritten, rewritten_plan) = crate::expression_rewriter::rewrite(
            builder,
            crate::context::TODOArc(),
            node,
            plan,
            crate::expression_rewriter::AggregateMapper::default(),
            true,
        )?;
        plan = rewritten_plan;
        expressions
            .push(rewritten.ok_or_else(|| {
                expression::errors::New("DO expression produced no scalar value")
            })?);
    }
    let schema = expression::NewSchema(
        expressions
            .iter()
            .enumerate()
            .map(|(index, expression)| {
                expression::Column::new(
                    expression
                        .GetType(builder.ctx.GetExprCtx().GetEvalCtx())
                        .clone(),
                    0,
                    builder.ctx.GetSessionVars().AllocPlanColumnID(),
                    index as isize,
                )
            })
            .collect(),
    );
    let mut projection = logicalop::LogicalProjection {
        Exprs: expressions,
        ..Default::default()
    }
    .Init(builder.ctx.clone(), 0);
    projection.SetSchema(schema);
    projection.SetOutputNames(expression::types::NameSlice(Vec::new()));
    projection.SetChildren(vec![plan]);
    Ok(Box::new(projection))
}

/// 在聚合后附加 HAVING Selection。
fn build_select_having(
    builder: &mut PlanBuilder,
    having: &crate::ast::ExprNode,
    plan: logicalop::LogicalPlanRef,
    aggregate_mapper: crate::expression_rewriter::AggregateMapper,
    query_block: i32,
) -> Result<logicalop::LogicalPlanRef, expression::Error> {
    let previous_clause = builder.curClause;
    builder.curClause = crate::expression_rewriter::havingClause;
    let rewritten = crate::expression_rewriter::rewrite(
        builder,
        crate::context::TODOArc(),
        having,
        plan,
        aggregate_mapper,
        false,
    );
    builder.curClause = previous_clause;
    let (condition, mut plan) = rewritten?;
    let mut conditions = condition.map_or_else(Vec::new, |condition| {
        expression::SplitCNFItems(condition.as_ref())
    });
    conditions.retain(|condition| {
        condition
            .as_any()
            .downcast_ref::<expression::Constant>()
            .is_none_or(|constant| {
                constant.DeferredExpr.is_some()
                    || constant.ParamMarker.is_some()
                    || !matches!(
                        constant
                            .Value
                            .ToBool(builder.ctx.GetExprCtx().GetEvalCtx().TypeCtx()),
                        Ok(1)
                    )
            })
    });
    if conditions.is_empty() {
        return Ok(plan);
    }
    let mut selection = logicalop::LogicalSelection {
        Conditions: conditions,
        ..Default::default()
    }
    .Init(builder.ctx.clone(), query_block);
    selection.SetSchema(plan.Schema().Clone());
    selection.SetOutputNames(plan.OutputNames().Shallow());
    selection.SetChildren(vec![plan]);
    Ok(Box::new(selection))
}

/// 构建 UNION/INTERSECT/EXCEPT 集合运算计划。
fn build_set_operation_runtime(
    builder: &mut PlanBuilder,
    ctx: &dyn crate::context::Context,
    statement: &crate::ast::SetOprStmt,
    ctes: &mut CteEnvironment,
) -> Result<logicalop::LogicalPlanRef, expression::Error> {
    let original_cte_names = ctes
        .keys()
        .cloned()
        .collect::<std::collections::HashSet<_>>();
    if let Some(with) = statement
        .With
        .as_ref()
        .or(statement.select_list.With.as_ref())
    {
        build_with_runtime(builder, ctx, &with.borrow(), ctes)?;
    }
    let selects = &statement.select_list.selects;
    if selects.is_empty() {
        return Err(expression::errors::New("set operation has no input"));
    }
    if statement.select_list.operators.len() != selects.len() {
        return Err(expression::errors::New(
            "set-operation input/operator counts do not match",
        ));
    }
    let mut children = selects
        .iter()
        .map(|select| build_query_node_with_ctes(builder, ctx, select.as_ref(), ctes))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter();
    let mut plans = vec![children.next().expect("non-empty set operands")];
    let mut operators = vec![None];
    for (child, operator) in children.zip(statement.select_list.operators.iter().skip(1)) {
        match operator {
            Some(crate::ast::SetOprType::Intersect) => {
                let left = plans.pop().expect("INTERSECT has a left operand");
                plans.push(build_set_semi_join(
                    builder,
                    left,
                    child,
                    logicalop::JoinType::SemiJoin,
                )?);
            }
            Some(crate::ast::SetOprType::IntersectAll) => {
                return Err(expression::errors::New(
                    "TiDB does not support INTERSECT ALL",
                ));
            }
            _ => {
                plans.push(child);
                operators.push(*operator);
            }
        }
    }

    let mut union_plans = vec![plans.remove(0)];
    let mut union_operators = vec![None];
    for (plan, operator) in plans.into_iter().zip(operators.into_iter().skip(1)) {
        match operator {
            Some(crate::ast::SetOprType::Except) => {
                let left = build_union_runtime(builder, union_plans, &union_operators)?;
                union_plans = vec![build_set_semi_join(
                    builder,
                    left,
                    plan,
                    logicalop::JoinType::AntiSemiJoin,
                )?];
                union_operators = vec![None];
            }
            Some(crate::ast::SetOprType::ExceptAll) => {
                return Err(expression::errors::New("TiDB does not support EXCEPT ALL"));
            }
            _ => {
                union_plans.push(plan);
                union_operators.push(operator);
            }
        }
    }
    let mut result = build_union_runtime(builder, union_plans, &union_operators)?;
    result = build_set_order_limit_runtime(builder, statement, result)?;
    ctes.retain(|name, _| original_cte_names.contains(name));
    Ok(result)
}

fn build_set_semi_join(
    builder: &mut PlanBuilder,
    left: logicalop::LogicalPlanRef,
    right: logicalop::LogicalPlanRef,
    join_type: logicalop::JoinType,
) -> Result<logicalop::LogicalPlanRef, expression::Error> {
    if left.Schema().Len() != right.Schema().Len() {
        return Err(expression::errors::New(
            "set operands have different column counts",
        ));
    }
    let length = left.Schema().Len();
    let left = builder.buildDistinct(left, length)?;
    let mut conditions = Vec::with_capacity(length);
    for (left_column, right_column) in left.Schema().Columns.iter().zip(&right.Schema().Columns) {
        conditions.push(expression::NewFunction(
            builder.ctx.GetExprCtx(),
            crate::ast::NullEQ,
            *expression::types::NewFieldType(expression::mysql::TypeTiny),
            vec![
                Box::new(left_column.Clone()),
                Box::new(right_column.Clone()),
            ],
        )?);
    }
    let schema = left.Schema().Clone();
    let names = left.OutputNames().Shallow();
    let query_block = left.QueryBlockOffset();
    let mut join = logicalop::LogicalJoin {
        JoinType: join_type,
        OtherConditions: conditions,
        ..Default::default()
    }
    .Init(builder.ctx.clone(), query_block);
    join.SetSchema(schema);
    join.SetOutputNames(names);
    join.SetChildren(vec![left, right]);
    Ok(Box::new(join))
}

fn build_union_runtime(
    builder: &mut PlanBuilder,
    mut plans: Vec<logicalop::LogicalPlanRef>,
    operators: &[Option<crate::ast::SetOprType>],
) -> Result<logicalop::LogicalPlanRef, expression::Error> {
    if plans.len() == 1 {
        return Ok(plans.remove(0));
    }
    let split = operators
        .iter()
        .enumerate()
        .skip(1)
        .rev()
        .find_map(|(index, operator)| {
            (!matches!(operator, Some(crate::ast::SetOprType::UnionAll))).then_some(index + 1)
        })
        .unwrap_or(0);
    let mut trailing = if split < plans.len() {
        plans.split_off(split)
    } else {
        Vec::new()
    };
    let mut union = if plans.is_empty() {
        trailing.remove(0)
    } else {
        let distinct_union = build_union_all_runtime(builder, plans)?;
        let length = distinct_union.Schema().Len();
        builder.buildDistinct(distinct_union, length)?
    };
    if !trailing.is_empty() {
        trailing.insert(0, union);
        union = build_union_all_runtime(builder, trailing)?;
    }
    Ok(union)
}

/// 为集合运算结果附加 ORDER BY / LIMIT。
fn build_set_order_limit_runtime(
    builder: &mut PlanBuilder,
    statement: &crate::ast::SetOprStmt,
    mut plan: logicalop::LogicalPlanRef,
) -> Result<logicalop::LogicalPlanRef, expression::Error> {
    let order_by = if statement.OrderBy.is_empty() {
        &statement.select_list.OrderBy
    } else {
        &statement.OrderBy
    };
    let limit = statement
        .Limit
        .as_ref()
        .or(statement.select_list.Limit.as_ref());
    build_order_limit_runtime(builder, order_by, limit, plan)
}

/// 构建 Sort 与 Limit 算子链。
fn build_order_limit_runtime(
    builder: &mut PlanBuilder,
    order_by: &[crate::ast::ByItem],
    limit: Option<&crate::ast::Limit>,
    mut plan: logicalop::LogicalPlanRef,
) -> Result<logicalop::LogicalPlanRef, expression::Error> {
    let query_block = plan.QueryBlockOffset();
    if !order_by.is_empty() {
        let mut by_items = Vec::with_capacity(order_by.len());
        for item in order_by {
            // In a set-operation query, an integer ORDER BY item names the
            // corresponding output column (1-based), as in Go's resolver.
            // Rewriting it as a literal makes the optimizer drop the sort.
            let ordinal = match &item.Expr.Kind {
                crate::ast::ExprKind::Value(value) => match &value.Datum {
                    crate::ast::ValueDatum::Int64(value) if *value > 0 => {
                        usize::try_from(*value).ok()
                    }
                    crate::ast::ValueDatum::Uint64(value) if *value > 0 => {
                        usize::try_from(*value).ok()
                    }
                    _ => None,
                },
                _ => None,
            };
            if let Some(ordinal) = ordinal {
                let column = plan.Schema().Columns.get(ordinal - 1).ok_or_else(|| {
                    expression::errors::New("ORDER BY position exceeds output column count")
                })?;
                by_items.push(planner_util_dependency::ByItems {
                    Expr: Box::new(column.Clone()),
                    Desc: item.Desc,
                });
                continue;
            }
            let constant_scalar_subquery = match &item.Expr.Kind {
                crate::ast::ExprKind::Subquery { Query, .. } => Query
                    .with_node(|node| {
                        node.as_any()
                            .downcast_ref::<crate::ast::SelectStmt>()
                            .is_some_and(|select| {
                                select.From.is_none()
                                    && select.Fields.Fields.len() == 1
                                    && select.Fields.Fields[0].Expr.as_ref().is_some_and(
                                        |expression| {
                                            matches!(
                                                expression.Kind,
                                                crate::ast::ExprKind::Value(_)
                                            )
                                        },
                                    )
                            })
                    })
                    .unwrap_or(false),
                _ => false,
            };
            if constant_scalar_subquery {
                continue;
            }
            let (expression, rewritten_plan) = crate::expression_rewriter::rewrite(
                builder,
                crate::context::TODOArc(),
                &item.Expr,
                plan,
                crate::expression_rewriter::AggregateMapper::default(),
                false,
            )?;
            plan = rewritten_plan;
            by_items.push(planner_util_dependency::ByItems {
                Expr: expression.ok_or_else(|| {
                    expression::errors::New("ORDER BY expression rewrite returned no expression")
                })?,
                Desc: item.Desc,
            });
        }
        let mut sort = logicalop::LogicalSort {
            ByItems: by_items,
            ..Default::default()
        }
        .Init(builder.ctx.clone(), query_block);
        sort.SetSchema(plan.Schema().Clone());
        sort.SetOutputNames(plan.OutputNames().Shallow());
        sort.SetChildren(vec![plan]);
        plan = Box::new(sort);
        builder.optFlag |= rule::FLAG_PUSH_DOWN_TOP_N;
    }
    if let Some(limit) = limit {
        plan = builder.buildLimit(plan, limit, query_block)?;
    }
    Ok(plan)
}

/// 构建 UnionAll 并统一各分支 Schema。
fn build_union_all_runtime(
    builder: &mut PlanBuilder,
    mut children: Vec<logicalop::LogicalPlanRef>,
) -> Result<logicalop::LogicalPlanRef, expression::Error> {
    if children.is_empty() {
        return Err(expression::errors::New("set operation has no input"));
    }
    let column_count = children[0].Schema().Len();
    if children
        .iter()
        .any(|child| child.Schema().Len() != column_count)
    {
        return Err(expression::errors::New(
            "set operands have different column counts",
        ));
    }
    let query_block = children[0].QueryBlockOffset();
    let mut output_columns = Vec::with_capacity(column_count);
    for index in 0..column_count {
        let field_types = children
            .iter()
            .map(|child| {
                child.Schema().Columns[index]
                    .RetType
                    .as_ref()
                    .ok_or_else(|| expression::errors::New("UNION input column has no type"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let result_type = union_join_field_type(&field_types);
        let unique_id = builder.ctx.GetExprCtx().AllocPlanColumnID();
        let mut column = expression::Column::new(result_type, 0, unique_id, index as isize);
        column.OrigName = format!("Column#{unique_id}");
        output_columns.push(column);
    }
    let union_schema = expression::NewSchema(output_columns);
    // Go initializes the UnionAll before allocating its per-arm coercion
    // projections. Plan IDs are observable in pruning fixtures, and the
    // projections also provide the shared union output IDs to every arm.
    let names = children[0].OutputNames().Shallow();
    let mut union = logicalop::LogicalUnionAll::default().Init(builder.ctx.clone(), query_block);
    for child in &mut children {
        let expressions = child
            .Schema()
            .Columns
            .iter()
            .zip(&union_schema.Columns)
            .map(|(source, destination)| {
                let source_expression = Box::new(source.Clone()) as expression::ExprBox;
                let source_type = source.RetType.as_ref();
                let destination_type = destination
                    .RetType
                    .as_ref()
                    .expect("UNION destination type");
                if source_type.is_some_and(|source_type| source_type.Equal(destination_type)) {
                    Ok(source_expression)
                } else {
                    expression::BuildCastFunctionWithCheck(
                        builder.ctx.GetExprCtx(),
                        source_expression,
                        destination_type.clone(),
                        true,
                        false,
                    )
                }
            })
            .collect::<Result<Vec<_>, expression::Error>>()?;
        let names = child.OutputNames().Shallow();
        let source = std::mem::replace(child, Box::new(logicalop::LogicalTableDual::default()));
        let mut projection = logicalop::LogicalProjection {
            Exprs: expressions,
            ..Default::default()
        }
        .Init(builder.ctx.clone(), query_block);
        projection.SetSchema(union_schema.Clone());
        projection.SetOutputNames(names);
        projection.SetChildren(vec![source]);
        *child = Box::new(projection);
    }

    union.SetSchema(union_schema);
    union.SetOutputNames(names);
    union.SetChildren(children);
    builder.optFlag |= rule::FLAG_PRUNE_COLUMNS
        | rule::FLAG_PRUNE_COLUMNS_AGAIN
        | rule::FLAG_BUILD_KEY_INFO
        | rule::FLAG_ELIMINATE_PROJECTION
        | rule::FLAG_ELIMINATE_UNION_ALL_DUAL_ITEM;
    Ok(Box::new(union))
}

/// 根据种子 Schema 推导 CTE 结果 Schema。
fn cte_result_schema(builder: &PlanBuilder, seed: &expression::Schema) -> expression::Schema {
    let mut schema = seed.Clone();
    for column in &mut schema.Columns {
        column.UniqueID = builder.ctx.GetExprCtx().AllocPlanColumnID();
        if let Some(field_type) = column.RetType.as_mut() {
            field_type.DelFlag(expression::mysql::NotNullFlag);
        }
        column.CleanHashCode();
    }
    schema
}

/// 调整 CTE 输出列名以匹配声明列。
fn adjust_cte_output_names(
    builder: &PlanBuilder,
    plan: &mut logicalop::LogicalPlanRef,
    definition: &crate::ast::CommonTableExpression,
) -> Result<(), expression::Error> {
    let mut names = plan.OutputNames().Shallow();
    if !definition.ColNameList.is_empty() && definition.ColNameList.len() != names.0.len() {
        return Err(expression::errors::New(
            "CTE column list has a different column count",
        ));
    }
    for (index, name) in names.0.iter_mut().enumerate() {
        let mut field = name.as_ref().map(|name| name.Clone()).unwrap_or_default();
        if field.DBName.O.is_empty() {
            field.DBName = crate::ast::NewCIStr(&builder.ctx.GetSessionVars().CurrentDB());
        }
        field.TblName = definition.Name.clone();
        field.OrigTblName = definition.Name.clone();
        if let Some(column_name) = definition.ColNameList.get(index) {
            field.ColName = column_name.clone();
            field.OrigColName = column_name.clone();
        }
        *name = Some(Arc::new(field));
    }
    plan.SetOutputNames(names);
    Ok(())
}

/// 结果集是否引用给定表名。
fn result_set_references_table(node: &crate::ast::ResultSetNode, table_name: &str) -> bool {
    match node {
        crate::ast::ResultSetNode::TableSource(source) => {
            source.Source.Schema.O.is_empty() && source.Source.Name.L == table_name
                || source.QuerySource.as_ref().is_some_and(|query| {
                    query
                        .with_node(|node| query_references_table(node, table_name))
                        .unwrap_or(false)
                })
        }
        crate::ast::ResultSetNode::Join(join) => {
            join.Left
                .as_deref()
                .is_some_and(|node| result_set_references_table(node, table_name))
                || join
                    .Right
                    .as_deref()
                    .is_some_and(|node| result_set_references_table(node, table_name))
        }
    }
}

/// Join AST 是否引用给定表名。
fn join_references_table(join: &crate::ast::Join, table_name: &str) -> bool {
    join.Left
        .as_deref()
        .is_some_and(|node| result_set_references_table(node, table_name))
        || join
            .Right
            .as_deref()
            .is_some_and(|node| result_set_references_table(node, table_name))
}

/// 查询 AST 是否引用给定表名。
fn query_references_table(node: &dyn crate::ast::Node, table_name: &str) -> bool {
    if let Some(select) = node.as_any().downcast_ref::<crate::ast::SelectStmt>() {
        return select
            .From
            .as_ref()
            .is_some_and(|from| join_references_table(&from.TableRefs, table_name))
            || select
                .children
                .iter()
                .any(|child| query_references_table(child.as_ref(), table_name));
    }
    if let Some(list) = node.as_any().downcast_ref::<crate::ast::SetOprSelectList>() {
        return list
            .selects
            .iter()
            .any(|select| query_references_table(select.as_ref(), table_name));
    }
    node.as_any()
        .downcast_ref::<crate::ast::SetOprStmt>()
        .is_some_and(|set| {
            set.select_list
                .selects
                .iter()
                .any(|select| query_references_table(select.as_ref(), table_name))
        })
}

/// 递归结果集是否含禁止的 ORDER BY/LIMIT。
fn result_set_has_forbidden_recursive_order_limit(
    node: &crate::ast::ResultSetNode,
    inside_lateral: bool,
) -> bool {
    match node {
        crate::ast::ResultSetNode::TableSource(source) => {
            source.QuerySource.as_ref().is_some_and(|query| {
                query
                    .with_node(|node| {
                        query_has_forbidden_recursive_order_limit(
                            node,
                            inside_lateral || source.Lateral,
                        )
                    })
                    .unwrap_or(false)
            })
        }
        crate::ast::ResultSetNode::Join(join) => {
            join.Left.as_deref().is_some_and(|node| {
                result_set_has_forbidden_recursive_order_limit(node, inside_lateral)
            }) || join.Right.as_deref().is_some_and(|node| {
                result_set_has_forbidden_recursive_order_limit(node, inside_lateral)
            })
        }
    }
}

/// 递归查询是否含禁止的 ORDER BY/LIMIT。
fn query_has_forbidden_recursive_order_limit(
    node: &dyn crate::ast::Node,
    inside_lateral: bool,
) -> bool {
    if let Some(select) = node.as_any().downcast_ref::<crate::ast::SelectStmt>() {
        return (!inside_lateral && (!select.OrderBy.is_empty() || select.Limit.is_some()))
            || select.From.as_ref().is_some_and(|from| {
                from.TableRefs.Left.as_deref().is_some_and(|node| {
                    result_set_has_forbidden_recursive_order_limit(node, inside_lateral)
                }) || from.TableRefs.Right.as_deref().is_some_and(|node| {
                    result_set_has_forbidden_recursive_order_limit(node, inside_lateral)
                })
            })
            || select.children.iter().any(|child| {
                query_has_forbidden_recursive_order_limit(child.as_ref(), inside_lateral)
            });
    }
    if let Some(list) = node.as_any().downcast_ref::<crate::ast::SetOprSelectList>() {
        return (!inside_lateral && (!list.OrderBy.is_empty() || list.Limit.is_some()))
            || list.selects.iter().any(|select| {
                query_has_forbidden_recursive_order_limit(select.as_ref(), inside_lateral)
            });
    }
    node.as_any()
        .downcast_ref::<crate::ast::SetOprStmt>()
        .is_some_and(|set| {
            (!inside_lateral
                && (!set.OrderBy.is_empty()
                    || set.Limit.is_some()
                    || !set.select_list.OrderBy.is_empty()
                    || set.select_list.Limit.is_some()))
                || set.select_list.selects.iter().any(|select| {
                    query_has_forbidden_recursive_order_limit(select.as_ref(), inside_lateral)
                })
        })
}

/// 为递归 CTE 分支构建投影对齐。
fn build_recursive_projection(
    builder: &mut PlanBuilder,
    seed_schema: &expression::Schema,
    mut recursive: logicalop::LogicalPlanRef,
) -> Result<logicalop::LogicalPlanRef, expression::Error> {
    if seed_schema.Len() != recursive.Schema().Len() {
        return Err(expression::errors::New(
            "recursive CTE seed and recursive member have different column counts",
        ));
    }
    let result_schema = cte_result_schema(builder, seed_schema);
    let expressions = recursive
        .Schema()
        .Columns
        .iter()
        .zip(&result_schema.Columns)
        .map(|(source, destination)| {
            let source_expression = Box::new(source.Clone()) as expression::ExprBox;
            match (source.RetType.as_ref(), destination.RetType.as_ref()) {
                (Some(source_type), Some(destination_type)) if source_type != destination_type => {
                    expression::BuildCastFunctionWithCheck(
                        builder.ctx.GetExprCtx(),
                        source_expression,
                        destination_type.clone(),
                        true,
                        false,
                    )
                }
                _ => Ok(source_expression),
            }
        })
        .collect::<Result<Vec<_>, expression::Error>>()?;
    let names = recursive.OutputNames().Shallow();
    let query_block = recursive.QueryBlockOffset();
    let mut projection = logicalop::LogicalProjection {
        Exprs: expressions,
        ..Default::default()
    }
    .Init(builder.ctx.clone(), query_block);
    projection.SetSchema(result_schema);
    projection.SetOutputNames(names);
    projection.SetChildren(vec![recursive]);
    builder.optFlag |= rule::FLAG_ELIMINATE_PROJECTION;
    Ok(Box::new(projection))
}

/// 构建 WITH / CTE 定义并注册到环境。
fn build_with_runtime(
    builder: &mut PlanBuilder,
    ctx: &dyn crate::context::Context,
    with: &crate::ast::WithClause,
    ctes: &mut CteEnvironment,
) -> Result<(), expression::Error> {
    let mut names = std::collections::HashSet::new();
    for definition in &with.CTEs {
        if !names.insert(definition.Name.L.clone()) {
            return Err(expression::errors::New(format!(
                "non-unique CTE name '{}'",
                definition.Name.O
            )));
        }

        let recursive = with.IsRecursive
            && query_references_table(definition.Query.as_ref(), &definition.Name.L);
        let storage_id = builder.ctx.GetExprCtx().AllocPlanColumnID() as i32;
        let seed_stat = Arc::new(RwLock::new(logicalop::StatsInfo::default()));
        if recursive {
            let set = definition
                .Query
                .as_any()
                .downcast_ref::<crate::ast::SetOprStmt>()
                .ok_or_else(|| {
                    expression::errors::New(format!(
                        "recursive CTE '{}' requires UNION",
                        definition.Name.O
                    ))
                })?;
            if set.select_list.selects.len() < 2 {
                return Err(expression::errors::New(format!(
                    "recursive CTE '{}' requires a non-recursive seed",
                    definition.Name.O
                )));
            }
            for operator in set.select_list.operators.iter().skip(1) {
                if !matches!(
                    operator,
                    Some(crate::ast::SetOprType::Union | crate::ast::SetOprType::UnionAll)
                ) {
                    return Err(expression::errors::New(
                        "recursive CTE members must be joined by UNION or UNION ALL",
                    ));
                }
            }

            let mut seed = build_query_node_with_ctes(
                builder,
                ctx,
                set.select_list.selects[0].as_ref(),
                ctes,
            )?;
            adjust_cte_output_names(builder, &mut seed, definition)?;
            let seed_schema = seed.Schema().Clone();
            let seed_names = seed.OutputNames().Shallow();
            let class = Rc::new(RefCell::new(logicalop::CTEClass {
                IsDistinct: matches!(
                    set.select_list.operators.get(1),
                    Some(Some(crate::ast::SetOprType::Union))
                ),
                SeedPartLogicalPlan: Some(seed),
                IDForStorage: storage_id,
                OptFlag: builder.optFlag,
                ..Default::default()
            }));
            ctes.insert(
                definition.Name.L.clone(),
                CteBinding {
                    class: class.clone(),
                    schema: seed_schema.Clone(),
                    names: seed_names,
                    name: definition.Name.clone(),
                    recursive_reference: true,
                    seed_stat: seed_stat.clone(),
                    storage_id,
                },
            );
            builder.runtimeCTEs = Some(ctes.clone());
            let recursive_children = set
                .select_list
                .selects
                .iter()
                .skip(1)
                .map(|select| {
                    if query_has_forbidden_recursive_order_limit(select.as_ref(), false) {
                        return Err(expression::errors::New(
                            "ORDER BY / LIMIT in recursive query block of Common Table Expression (except within LATERAL subqueries)",
                        ));
                    }
                    build_query_node_with_ctes(builder, ctx, select.as_ref(), ctes)
                })
                .collect::<Result<Vec<_>, _>>()?;
            if let Some(binding) = ctes.get_mut(&definition.Name.L) {
                binding.recursive_reference = false;
            }
            builder.runtimeCTEs = Some(ctes.clone());
            let recursive_plan = build_union_all_runtime(builder, recursive_children)?;
            let recursive_plan = build_recursive_projection(builder, &seed_schema, recursive_plan)?;
            class.borrow_mut().RecursivePartLogicalPlan = Some(recursive_plan);
        } else {
            let mut seed =
                build_query_node_with_ctes(builder, ctx, definition.Query.as_ref(), ctes)?;
            adjust_cte_output_names(builder, &mut seed, definition)?;
            let schema = seed.Schema().Clone();
            let names = seed.OutputNames().Shallow();
            let class = Rc::new(RefCell::new(logicalop::CTEClass {
                SeedPartLogicalPlan: Some(seed),
                IDForStorage: storage_id,
                OptFlag: builder.optFlag,
                ..Default::default()
            }));
            ctes.insert(
                definition.Name.L.clone(),
                CteBinding {
                    class,
                    schema,
                    names,
                    name: definition.Name.clone(),
                    recursive_reference: false,
                    seed_stat,
                    storage_id,
                },
            );
            builder.runtimeCTEs = Some(ctes.clone());
        }
    }
    Ok(())
}

/// UNION 字段类型合并（运行时）。
fn union_join_field_type(
    field_types: &[&expression::types::FieldType],
) -> expression::types::FieldType {
    let non_null = field_types
        .iter()
        .copied()
        .filter(|field_type| field_type.GetType() != expression::mysql::TypeNull)
        .collect::<Vec<_>>();
    let mut result = if non_null.is_empty() {
        *expression::types::NewFieldType(expression::mysql::TypeNull)
    } else {
        *types::field::AggFieldType(&non_null)
    };
    if !non_null.is_empty() {
        let all_unsigned = non_null
            .iter()
            .all(|field_type| expression::mysql::HasUnsignedFlag(field_type.GetFlag()));
        if all_unsigned {
            result.AddFlag(expression::mysql::UnsignedFlag);
        } else {
            result.DelFlag(expression::mysql::UnsignedFlag);
        }
        let decimal = non_null
            .iter()
            .map(|field_type| field_type.GetDecimal())
            .max()
            .unwrap_or(result.GetDecimal());
        result.SetDecimalUnderLimit(decimal);
        if non_null
            .iter()
            .any(|field_type| field_type.GetFlen() == types::field::UnspecifiedLength)
        {
            result.SetFlenUnderLimit(types::field::UnspecifiedLength);
        } else {
            let integer_digits = non_null
                .iter()
                .map(|field_type| field_type.GetFlen() - field_type.GetDecimal())
                .max()
                .unwrap_or(0);
            result.SetFlenUnderLimit(integer_digits + result.GetDecimal());
        }
        types::field::TryToFixFlenOfDatetime(&mut result);
    }
    result
}

/// 收集运行时 hint 警告字符串。
#[derive(Default)]
struct RuntimeHintWarnings(Vec<String>);

impl hint::hintWarnHandler for RuntimeHintWarnings {
    fn SetHintWarning(&mut self, warning: String) {
        self.0.push(warning);
    }

    fn SetHintWarningFromError(&mut self, error: &dyn std::error::Error) {
        self.0.push(error.to_string());
    }
}

/// SELECT 运行时构建入口（含 WITH）。
fn build_select_runtime(
    builder: &mut PlanBuilder,
    ctx: &dyn crate::context::Context,
    select: &crate::ast::SelectStmt,
    ctes: &mut CteEnvironment,
) -> Result<logicalop::LogicalPlanRef, expression::Error> {
    let query_block = select.QueryBlockOffset as i32;
    let mut processor = hint::NewQBHintHandler(None);
    let mut warnings = RuntimeHintWarnings::default();
    let subquery_context = builder.subQueryCtx;
    let (plan_hints, hint_flags) = hint::ParsePlanHints(
        select.TableHints.clone(),
        query_block,
        builder.ctx.GetSessionVars().CurrentDB(),
        &mut processor,
        select
            .From
            .as_ref()
            .is_some_and(|from| from.TableRefs.StraightJoin),
        subquery_context != crate::expression_rewriter::notHandlingSubquery,
        subquery_context == crate::expression_rewriter::handlingExistsSubquery,
        subquery_context == crate::expression_rewriter::notHandlingSubquery,
        &mut warnings,
    )
    .map_err(|error| expression::errors::New(error.to_string()))?;
    for warning in warnings.0 {
        set_hint_warning_once(builder, warning);
    }
    let previous_straight_join = builder.inStraightJoin;
    builder.inStraightJoin |= select.SelectStmtOpts.StraightJoin || plan_hints.StraightJoinOrder;
    builder.tableHintInfo.push(plan_hints);
    builder.subQueryHintFlags = hint_flags;
    let result = build_select_runtime_inner(builder, ctx, select, ctes);
    if let Some(plan_hints) = builder.tableHintInfo.pop() {
        for warning in hint::CollectUnmatchedHintWarnings(&plan_hints) {
            set_hint_warning_once(builder, warning);
        }
    }
    builder.inStraightJoin = previous_straight_join;
    result
}

/// Enforce the Go planner's GROUPING argument rule before the runtime builder
/// lowers a ROLLUP query. The parser carries the WITH ROLLUP modifier on the
/// SELECT AST; keeping this check here makes ORDER BY/HAVING validation use the
/// same planner error as the expression rewriter's Expand path.
fn validate_grouping_function_arguments(
    select: &crate::ast::SelectStmt,
) -> Result<(), expression::Error> {
    if !select.GroupByRollup {
        return Ok(());
    }

    fn visit(
        node: &crate::ast::ExprNode,
        group_by: &[crate::ast::ByItem],
    ) -> Result<(), expression::Error> {
        use crate::ast::ExprKind;

        match &node.Kind {
            ExprKind::Function { FnName, Args, .. } => {
                if FnName.L == crate::ast::Grouping {
                    for (index, argument) in Args.iter().enumerate() {
                        if !group_by.iter().any(|item| item.Expr == *argument) {
                            return Err(plannererrors::ErrFieldInGroupingNotGroupBy
                                .GenWithStackByArgs(&[format!("#{index}").into()])
                                .into());
                        }
                    }
                }
                for argument in Args {
                    visit(argument, group_by)?;
                }
            }
            ExprKind::AggregateFunction { Args, .. } | ExprKind::Row(Args) => {
                for argument in Args {
                    visit(argument, group_by)?;
                }
            }
            ExprKind::Binary { L, R, .. } | ExprKind::CompareSubquery { L, R, .. } => {
                visit(L, group_by)?;
                visit(R, group_by)?;
            }
            ExprKind::Unary { V, .. }
            | ExprKind::IsTruth { Expr: V, .. }
            | ExprKind::IsNull { Expr: V, .. }
            | ExprKind::Collate { Expr: V, .. }
            | ExprKind::Parentheses(V) => visit(V, group_by)?,
            ExprKind::Subquery { .. }
            | ExprKind::ExistsSubquery { .. }
            | ExprKind::Column(_)
            | ExprKind::Value(_) => {}
            _ => {}
        }
        Ok(())
    }

    for field in &select.Fields.Fields {
        if let Some(expression) = &field.Expr {
            visit(expression, &select.GroupBy)?;
        }
    }
    if let Some(having) = &select.Having {
        visit(having, &select.GroupBy)?;
    }
    for item in &select.OrderBy {
        visit(&item.Expr, &select.GroupBy)?;
    }
    Ok(())
}

/// Enforce the legacy `ONLY_FULL_GROUP_BY` contract before GROUP BY
/// expressions are rewritten. Go performs this check after expanding `*` and
/// before building the aggregation; the runtime builder must do the same or a
/// non-grouped projection is silently accepted.
fn validate_only_full_group_by(
    builder: &PlanBuilder,
    select: &crate::ast::SelectStmt,
    source: &dyn logicalop::LogicalPlan,
) -> Result<(), expression::Error> {
    let only_full_group_by = builder
        .ctx
        .GetExprCtx()
        .GetEvalCtx()
        .SQLMode()
        .HasOnlyFullGroupBy()
        || builder
            .ctx
            .GetSessionVars()
            .GetSystemVar("sql_mode")
            .is_some_and(|mode| {
                mode.split(',')
                    .any(|item| item.trim().eq_ignore_ascii_case("ONLY_FULL_GROUP_BY"))
            });
    if select.From.is_none() || select.GroupBy.is_empty() || !only_full_group_by {
        return Ok(());
    }

    for (field_index, field) in select.Fields.Fields.iter().enumerate() {
        if field.Auxiliary {
            continue;
        }
        if field.WildCard.is_some() {
            let name = source
                .OutputNames()
                .0
                .iter()
                .flatten()
                .find(|name| !name.Hidden)
                .map(|name| name.String())
                .unwrap_or_else(|| "unknown".to_owned());
            return Err(plannererrors::ErrFieldNotInGroupBy
                .GenWithStackByArgs(&[(field_index + 1).into(), "SELECT list".into(), name.into()])
                .into());
        }
    }
    Ok(())
}

/// SELECT 主体：FROM、聚合、窗口、投影、排序、LIMIT。
fn build_select_runtime_inner(
    builder: &mut PlanBuilder,
    ctx: &dyn crate::context::Context,
    select: &crate::ast::SelectStmt,
    ctes: &mut CteEnvironment,
) -> Result<logicalop::LogicalPlanRef, expression::Error> {
    let original_cte_names = ctes
        .keys()
        .cloned()
        .collect::<std::collections::HashSet<_>>();
    if let Some(with) = &select.With {
        build_with_runtime(builder, ctx, &with.borrow(), ctes)?;
    }

    validate_grouping_function_arguments(select)?;

    let query_block = select.QueryBlockOffset as i32;
    builder.optFlag |= rule::FLAG_PRUNE_COLUMNS | rule::FLAG_PRUNE_COLUMNS_AGAIN;
    let result = build_select_source(builder, ctx, select, query_block, ctes);
    let (mut plan, table_info) = result?;
    validate_having_window_alias(select)?;

    // Go resolves correlated aggregates before WHERE/projection construction.
    // That resolver builds each subquery's FROM tree once to establish its
    // local schema, even when no correlated aggregate is ultimately found.
    // Preserve that real planning pass: besides resolving nested scopes, the
    // intermediate DataSource column IDs are part of the stable plan contract.
    prebuild_subquery_sources_for_correlated_aggregates(builder, ctx, select, plan.as_ref(), ctes)?;

    if let Some(predicate) = &select.Where {
        // Go allocates LogicalSelection before rewriting its conditions;
        // condition rewriting may itself build subquery plans.
        let mut selection =
            logicalop::LogicalSelection::default().Init(builder.ctx.clone(), query_block);
        let previous_clause = builder.curClause;
        builder.curClause = crate::expression_rewriter::whereClause;
        let rewritten = rewrite_where_conditions(builder, predicate, plan);
        builder.curClause = previous_clause;
        let (mut conditions, rewritten_plan) = rewritten?;
        plan = rewritten_plan;
        conditions.retain(|condition| {
            condition
                .as_any()
                .downcast_ref::<expression::Constant>()
                .is_none_or(|constant| {
                    constant.DeferredExpr.is_some()
                        || constant.ParamMarker.is_some()
                        || !matches!(
                            constant
                                .Value
                                .ToBool(builder.ctx.GetExprCtx().GetEvalCtx().TypeCtx()),
                            Ok(1)
                        )
                })
        });
        if logicalop::Conds2TableDual(&conditions) {
            let (schema, names) = plan
                .as_any()
                .downcast_ref::<logicalop::LogicalJoin>()
                .and_then(|join| {
                    join.FullSchema
                        .as_ref()
                        .map(|schema| (schema.Clone(), join.FullNames.Shallow()))
                })
                .unwrap_or_else(|| (plan.Schema().Clone(), plan.OutputNames().Shallow()));
            let mut dual = logicalop::LogicalTableDual {
                RowCount: 0,
                ..Default::default()
            }
            .Init(builder.ctx.clone(), query_block);
            dual.SetSchema(schema);
            dual.SetOutputNames(names);
            plan = Box::new(dual);
            builder.optFlag |= rule::FLAG_PREDICATE_PUSH_DOWN
                | rule::FLAG_BUILD_KEY_INFO
                | rule::FLAG_PREDICATE_SIMPLIFICATION;
        } else if conditions.is_empty() {
            builder.optFlag |= rule::FLAG_PREDICATE_PUSH_DOWN
                | rule::FLAG_BUILD_KEY_INFO
                | rule::FLAG_PREDICATE_SIMPLIFICATION;
        } else {
            if let Some(union_scan) = plan
                .as_any_mut()
                .downcast_mut::<logicalop::LogicalUnionScan>()
            {
                // UnionScan evaluates the predicate independently for transaction
                // local rows.  The parent Selection must remain for snapshot rows,
                // so both operators intentionally retain equivalent conditions.
                union_scan.Conditions = conditions
                    .iter()
                    .map(|condition| condition.CloneExpr())
                    .collect();
            }
            selection.Conditions = conditions;
            selection.SetSchema(plan.Schema().Clone());
            selection.SetOutputNames(plan.OutputNames().Shallow());
            selection.SetChildren(vec![plan]);
            plan = Box::new(selection);
            builder.optFlag |= rule::FLAG_PREDICATE_PUSH_DOWN
                | rule::FLAG_BUILD_KEY_INFO
                | rule::FLAG_PREDICATE_SIMPLIFICATION;
        }
    }

    if let Some(lock) = select
        .lock_info
        .as_ref()
        .filter(|lock| lock.LockType != crate::ast::SelectLockType::None)
    {
        // Go inserts LogicalLock immediately above FROM/WHERE, before
        // aggregation, windowing and the final SELECT projection.  Besides
        // preserving that operator boundary, locking reads must use the latest
        // committed version when plan-cache validation chooses its snapshot.
        if logicalop::isSelectForUpdateLockType(lock.LockType) {
            builder.isForUpdateRead = true;
            fn mark_for_update(plan: &mut dyn logicalop::LogicalPlan) {
                if let Some(source) = plan.as_any_mut().downcast_mut::<logicalop::DataSource>() {
                    source.IsForUpdateRead = true;
                }
                for child in plan.base_mut().Children_mut() {
                    mark_for_update(child.as_mut());
                }
            }
            mark_for_update(plan.as_mut());
        }
        let mut logical_lock = logicalop::LogicalLock {
            Lock: lock.clone(),
            ..Default::default()
        }
        .Init(builder.ctx.clone());
        logical_lock.SetSchema(plan.Schema().Clone());
        logical_lock.SetOutputNames(plan.OutputNames().Shallow());
        logical_lock.SetChildren(vec![plan]);
        plan = Box::new(logical_lock);
    }

    // UPDATE uses SELECT-shaped AST only as a transport in the Rust runtime.
    // Go builds FROM/WHERE/ORDER/LIMIT directly and adds its single freeze
    // projection in buildUpdate, so do not build the normal SELECT projection.
    let update_source_transport = builder.inUpdateStmt
        && select.Fields.Fields.len() == 1
        && select.Fields.Fields[0].WildCard.is_some();
    if update_source_transport {
        if !select.OrderBy.is_empty() {
            let by_items = select
                .OrderBy
                .iter()
                .map(|item| {
                    Ok(planner_util_dependency::ByItems {
                        Expr: rewrite_for_plan(
                            builder,
                            &item.Expr,
                            plan.as_ref(),
                            table_info.as_ref(),
                        )?,
                        Desc: item.Desc,
                    })
                })
                .collect::<Result<Vec<_>, expression::Error>>()?;
            let mut sort = logicalop::LogicalSort {
                ByItems: by_items,
                ..Default::default()
            }
            .Init(builder.ctx.clone(), query_block);
            sort.SetSchema(plan.Schema().Clone());
            sort.SetOutputNames(plan.OutputNames().Shallow());
            sort.SetChildren(vec![plan]);
            plan = Box::new(sort);
            builder.optFlag |= rule::FLAG_PUSH_DOWN_TOP_N;
        }
        if let Some(limit) = &select.Limit {
            plan = builder.buildLimit(plan, limit, query_block)?;
        }
        return Ok(plan);
    }

    validate_only_full_group_by(builder, select, plan.as_ref())?;

    let aggregate_nodes = select_aggregate_nodes(select);
    let has_aggregate = !aggregate_nodes.is_empty();
    let window_aggregate_mapper = aggregate_nodes
        .iter()
        .enumerate()
        .map(|(index, node)| {
            (
                crate::expression_rewriter::AggregateMapperKey(node),
                index as isize,
            )
        })
        .collect::<crate::expression_rewriter::AggregateMapper>();
    let mut order_aggregate_mapper = crate::expression_rewriter::AggregateMapper::default();
    let resolved_window_specs = build_window_specs(&select.WindowSpecs)?;
    let mut window_select = crate::ast::SelectStmt {
        Fields: select.Fields.clone(),
        OrderBy: select.OrderBy.clone(),
        WindowSpecs: resolved_window_specs,
        ..Default::default()
    };
    let original_field_count = window_select.Fields.Fields.len();
    for field_index in 0..original_field_count {
        let Some(expression) = window_select.Fields.Fields[field_index].Expr.clone() else {
            continue;
        };
        let mut windows = Vec::new();
        collect_window_expressions(&expression, &mut windows);
        for window in windows {
            if expression == window {
                continue;
            }
            if window_select
                .Fields
                .Fields
                .iter()
                .any(|field| field.Expr.as_ref() == Some(&window))
            {
                continue;
            }
            window_select.Fields.Fields.push(crate::ast::SelectField {
                Expr: Some(window),
                Auxiliary: true,
                ..Default::default()
            });
        }
    }
    let mut order_window_field_indexes = HashMap::<usize, Vec<usize>>::new();
    for (order_index, item) in select.OrderBy.iter().enumerate() {
        let resolved = resolve_select_order_aliases(select, &item.Expr);
        let mut windows = Vec::new();
        collect_window_expressions(&resolved, &mut windows);
        for window in windows {
            let field_index = window_select
                .Fields
                .Fields
                .iter()
                .position(|field| field.Expr.as_ref() == Some(&window))
                .unwrap_or_else(|| {
                    let index = window_select.Fields.Fields.len();
                    window_select.Fields.Fields.push(crate::ast::SelectField {
                        Expr: Some(window),
                        Auxiliary: true,
                        AuxiliaryColInOrderBy: true,
                        ..Default::default()
                    });
                    index
                });
            order_window_field_indexes
                .entry(order_index)
                .or_default()
                .push(field_index);
        }
    }
    let has_window = !order_window_field_indexes.is_empty()
        || window_select.Fields.Fields.iter().any(|field| {
            field.Expr.as_ref().is_some_and(|expression| {
                let mut windows = Vec::new();
                collect_window_expressions(expression, &mut windows);
                !windows.is_empty()
            })
        });
    let mut order_window_mappers = HashMap::new();
    let mut final_output_len = None;
    if has_aggregate || !select.GroupBy.is_empty() {
        plan = build_select_aggregation(builder, select, plan, table_info.as_ref(), query_block)?;
    }
    plan = if has_window {
        // Window resolution retains referenced input columns as auxiliary
        // SELECT fields until the final visible-column projection, as in Go.
        let mut referenced_columns = Vec::new();
        for field in &window_select.Fields.Fields {
            if let Some(expression) = &field.Expr {
                let mut windows = Vec::new();
                collect_window_expressions(expression, &mut windows);
                for window in windows {
                    referenced_columns.extend(ast_columns_outside_subqueries(&window));
                }
            }
        }
        for column in referenced_columns {
            if window_select.Fields.Fields.iter().any(|field| {
                field.Expr.as_ref().is_some_and(|expression| {
                    matches!(&expression.Kind, crate::ast::ExprKind::Column(existing)
                        if existing.Name.L == column.Name.L
                            && existing.Table.L == column.Table.L
                            && existing.Schema.L == column.Schema.L)
                })
            }) {
                continue;
            }
            let expression = crate::ast::ExprNode::Column(column);
            window_select.Fields.Fields.push(crate::ast::SelectField {
                Expr: Some(expression),
                Auxiliary: true,
                ..Default::default()
            });
        }
        let pre_window_input_len = plan.Schema().Len();
        plan = build_pre_window_projection(
            builder,
            &window_select,
            plan,
            table_info.as_ref(),
            query_block,
        )?;
        if let Some(having) = &select.Having {
            plan = build_select_having(
                builder,
                having,
                plan,
                window_aggregate_mapper.clone(),
                query_block,
            )?;
        }
        let (window, columns) = build_select_windows(
            builder,
            &window_select,
            plan,
            table_info.as_ref(),
            query_block,
            &window_aggregate_mapper,
        )?;
        let projection_window_mapper = columns
            .iter()
            .filter_map(|(field_index, column)| {
                let expression = window_select.Fields.Fields[*field_index].Expr.as_ref()?;
                let position = window
                    .Schema()
                    .Columns
                    .iter()
                    .position(|candidate| candidate.UniqueID == column.UniqueID)?;
                Some((
                    crate::expression_rewriter::AggregateMapperKey(expression),
                    position as isize,
                ))
            })
            .collect::<crate::expression_rewriter::WindowMapper>();
        let projected = build_select_projection_with_windows(
            builder,
            &window_select,
            window,
            table_info.as_ref(),
            query_block,
            &columns,
            &window_aggregate_mapper,
            &projection_window_mapper,
            pre_window_input_len,
        )?;
        if let Some(projection) = projected
            .as_any()
            .downcast_ref::<logicalop::LogicalProjection>()
        {
            for (order_index, field_indexes) in &order_window_field_indexes {
                let mut mapper = crate::expression_rewriter::WindowMapper::default();
                for field_index in field_indexes {
                    let Some(window_column) = columns.get(field_index) else {
                        continue;
                    };
                    let Some(position) = projection.Exprs.iter().position(|expression| {
                        expression
                            .as_column()
                            .is_some_and(|column| column.UniqueID == window_column.UniqueID)
                    }) else {
                        continue;
                    };
                    let Some(window_expression) =
                        window_select.Fields.Fields[*field_index].Expr.as_ref()
                    else {
                        continue;
                    };
                    mapper.insert(
                        crate::expression_rewriter::AggregateMapperKey(window_expression),
                        position as isize,
                    );
                }
                if !mapper.is_empty() {
                    order_window_mappers.insert(*order_index, mapper);
                }
            }
        }
        projected
    } else if !has_aggregate && select.GroupBy.is_empty() {
        let projected =
            build_select_projection(builder, select, plan, table_info.as_ref(), query_block)?;
        final_output_len = Some(
            projected
                .Schema()
                .Columns
                .iter()
                .filter(|column| !column.IsHidden)
                .count(),
        );
        projected
    } else {
        plan
    };
    if (has_aggregate || !select.GroupBy.is_empty()) && !has_window {
        let aggregate_mapper = aggregate_nodes
            .iter()
            .enumerate()
            .map(|(index, node)| {
                (
                    crate::expression_rewriter::AggregateMapperKey(node),
                    index as isize,
                )
            })
            .collect::<crate::expression_rewriter::AggregateMapper>();
        plan = build_post_aggregation_projection(
            builder,
            select,
            plan,
            table_info.as_ref(),
            query_block,
            aggregate_mapper.clone(),
        )?;
        final_output_len = Some(
            plan.Schema()
                .Columns
                .iter()
                .filter(|column| !column.IsHidden)
                .count(),
        );
        let projected_source_columns = plan
            .as_any()
            .downcast_ref::<logicalop::LogicalProjection>()
            .map(|projection| {
                projection
                    .Children()
                    .first()
                    .map(|child| child.Schema().Columns.clone())
                    .unwrap_or_default()
                    .into_iter()
                    .enumerate()
                    .filter_map(|(aggregate_index, aggregate_column)| {
                        projection
                            .Exprs
                            .iter()
                            .rposition(|expression| {
                                expression.as_column().is_some_and(|projected_column| {
                                    projected_column.EqualColumn(&aggregate_column)
                                })
                            })
                            .map(|projection_index| (aggregate_index, projection_index))
                    })
                    .collect::<std::collections::HashMap<_, _>>()
            })
            .unwrap_or_default();
        order_aggregate_mapper = aggregate_nodes
            .iter()
            .enumerate()
            .filter_map(|(aggregate_index, node)| {
                projected_source_columns
                    .get(&aggregate_index)
                    .map(|projection_index| {
                        (
                            crate::expression_rewriter::AggregateMapperKey(node),
                            *projection_index as isize,
                        )
                    })
            })
            .collect();
        if let Some(having) = &select.Having {
            let having_mapper = aggregate_nodes
                .iter()
                .enumerate()
                .map(|(aggregate_index, node)| {
                    let projection_index = projected_source_columns
                        .get(&aggregate_index)
                        .copied()
                        .unwrap_or(aggregate_index);
                    (
                        crate::expression_rewriter::AggregateMapperKey(node),
                        projection_index as isize,
                    )
                })
                .collect::<crate::expression_rewriter::AggregateMapper>();
            plan = build_select_having(builder, having, plan, having_mapper, query_block)?;
        }
    }
    if !has_window
        && !has_aggregate
        && select.GroupBy.is_empty()
        && let Some(having) = &select.Having
    {
        plan = build_select_having(
            builder,
            having,
            plan,
            crate::expression_rewriter::AggregateMapper::default(),
            query_block,
        )?;
    }
    if select.Distinct {
        let length = plan
            .Schema()
            .Columns
            .iter()
            .filter(|column| !column.IsHidden)
            .count();
        plan = builder.buildDistinct(plan, length)?;
    }

    if !select.OrderBy.is_empty() {
        let mut by_items = Vec::with_capacity(select.OrderBy.len());
        for (order_index, item) in select.OrderBy.iter().enumerate() {
            let resolved_order = resolve_select_order_aliases(select, &item.Expr);
            let mut order_aggregates = Vec::new();
            collect_aggregate_nodes(&resolved_order, &mut order_aggregates);
            let ordinal = match &item.Expr.Kind {
                crate::ast::ExprKind::Value(value) => match &value.Datum {
                    crate::ast::ValueDatum::Int64(value) if *value > 0 => {
                        usize::try_from(*value).ok()
                    }
                    crate::ast::ValueDatum::Uint64(value) if *value > 0 => {
                        usize::try_from(*value).ok()
                    }
                    _ => None,
                },
                _ => None,
            };
            let selected_order_column = ordinal
                .and_then(|index| plan.Schema().Columns.get(index - 1))
                .map(|column| Box::new(column.Clone()) as expression::ExprBox)
                .or_else(|| {
                    order_aggregates
                        .is_empty()
                        .then(|| {
                            select
                                .Fields
                                .Fields
                                .iter()
                                .position(|field| {
                                    field.WildCard.is_none()
                                        && field.Expr.as_ref().is_some_and(|expression| {
                                            ast_expressions_equal(expression, &resolved_order)
                                        })
                                })
                                .and_then(|index| plan.Schema().Columns.get(index))
                                .map(|column| Box::new(column.Clone()) as expression::ExprBox)
                        })
                        .flatten()
                });
            let rewritten = if selected_order_column.is_some() {
                selected_order_column
            } else if let Some(window_mapper) = order_window_mappers.get(&order_index) {
                let (rewritten, rewritten_plan) =
                    crate::expression_rewriter::rewriteWithPreprocess(
                        builder,
                        crate::context::TODOArc(),
                        &resolved_order,
                        plan,
                        crate::expression_rewriter::AggregateMapper::default(),
                        Some(window_mapper.clone()),
                        true,
                        None,
                    )?;
                plan = rewritten_plan;
                rewritten
            } else {
                let simple = hidden_aggregation_order_column(plan.as_ref(), &item.Expr)
                    .map_or_else(
                        || {
                            rewrite_for_plan(
                                builder,
                                &item.Expr,
                                plan.as_ref(),
                                table_info.as_ref(),
                            )
                        },
                        Ok,
                    )
                    .or_else(|error| {
                        plan.as_any()
                            .downcast_ref::<logicalop::LogicalProjection>()
                            .and_then(|projection| projection.Children().first())
                            .map(|child| {
                                rewrite_for_plan(
                                    builder,
                                    &resolved_order,
                                    child.as_ref(),
                                    table_info.as_ref(),
                                )
                            })
                            .unwrap_or(Err(error))
                    });
                match simple {
                    Ok(expression) => Some(expression),
                    Err(_) => {
                        let (rewritten, rewritten_plan) = crate::expression_rewriter::rewrite(
                            builder,
                            crate::context::TODOArc(),
                            &item.Expr,
                            plan,
                            order_aggregate_mapper.clone(),
                            true,
                        )?;
                        plan = rewritten_plan;
                        rewritten
                    }
                }
            };
            let mut order_expression = rewritten.ok_or_else(|| {
                expression::errors::New("ORDER BY expression produced no scalar value")
            })?;
            if let Some(projection) = plan.as_any().downcast_ref::<logicalop::LogicalProjection>()
                && let Some(index) = projection.Exprs.iter().position(|candidate| {
                    candidate.Equal(
                        builder.ctx.GetExprCtx().GetEvalCtx(),
                        order_expression.as_ref(),
                    )
                })
                && let Some(column) = projection.Schema().Columns.get(index)
            {
                order_expression = Box::new(column.Clone());
            }
            by_items.push(planner_util_dependency::ByItems {
                Expr: order_expression,
                Desc: item.Desc,
            });
        }
        let mut sort = logicalop::LogicalSort {
            ByItems: by_items,
            ..Default::default()
        }
        .Init(builder.ctx.clone(), query_block);
        sort.SetSchema(plan.Schema().Clone());
        sort.SetOutputNames(plan.OutputNames().Shallow());
        sort.SetChildren(vec![plan]);
        plan = Box::new(sort);
        builder.optFlag |= rule::FLAG_PUSH_DOWN_TOP_N;
    }

    if let Some(limit) = &select.Limit {
        plan = builder.buildLimit(plan, limit, query_block)?;
    }
    let visible_indexes = plan
        .Schema()
        .Columns
        .iter()
        .enumerate()
        .filter_map(|(index, column)| (!column.IsHidden).then_some(index))
        .collect::<Vec<_>>();
    if visible_indexes.len() < plan.Schema().Len() {
        let expressions = visible_indexes
            .iter()
            .map(|index| Box::new(plan.Schema().Columns[*index].Clone()) as expression::ExprBox)
            .collect::<Vec<_>>();
        let columns = expressions
            .iter()
            .enumerate()
            .map(|(index, expression)| {
                let mut column = expression
                    .as_column()
                    .expect("visible cleanup projection contains only columns")
                    .Clone();
                column.UniqueID = builder.ctx.GetExprCtx().AllocPlanColumnID();
                column.Index = index as isize;
                column.IsHidden = false;
                column
            })
            .collect::<Vec<_>>();
        let names = visible_indexes
            .iter()
            .map(|index| plan.OutputNames().0.get(*index).cloned().flatten())
            .collect::<Vec<_>>();
        let mut projection = logicalop::LogicalProjection {
            Exprs: expressions,
            ..Default::default()
        }
        .Init(builder.ctx.clone(), query_block);
        projection.SetSchema(expression::NewSchema(columns));
        projection.SetOutputNames(expression::types::NameSlice(names));
        projection.SetChildren(vec![plan]);
        plan = Box::new(projection);
        builder.optFlag |= rule::FLAG_ELIMINATE_PROJECTION;
    }
    if let Some(old_len) = final_output_len
        && old_len != plan.Schema().Len()
    {
        let expressions = plan.Schema().Columns[..old_len]
            .iter()
            .map(|column| Box::new(column.Clone()) as expression::ExprBox)
            .collect::<Vec<_>>();
        let columns = plan.Schema().Columns[..old_len]
            .iter()
            .enumerate()
            .map(|(index, column)| {
                let mut output = column.Clone();
                output.UniqueID = builder.ctx.GetExprCtx().AllocPlanColumnID();
                output.Index = index as isize;
                output.IsHidden = false;
                output
            })
            .collect::<Vec<_>>();
        let names = expression::types::NameSlice(plan.OutputNames().0[..old_len].to_vec());
        let mut projection = logicalop::LogicalProjection {
            Exprs: expressions,
            ..Default::default()
        }
        .Init(builder.ctx.clone(), query_block);
        projection.SetSchema(expression::NewSchema(columns));
        projection.SetOutputNames(names);
        projection.SetChildren(vec![plan]);
        plan = Box::new(projection);
        builder.optFlag |= rule::FLAG_ELIMINATE_PROJECTION;
    }
    ctes.retain(|name, _| original_cte_names.contains(name));
    Ok(plan)
}

/// Rewrite each top-level WHERE conjunct independently, matching Go's
/// `buildSelection` scalar-context handling for subqueries.
fn rewrite_where_conditions(
    builder: &mut PlanBuilder,
    predicate: &crate::ast::ExprNode,
    mut plan: logicalop::LogicalPlanRef,
) -> Result<(Vec<expression::ExprBox>, logicalop::LogicalPlanRef), expression::Error> {
    fn split_top_level_and<'a>(
        expression: &'a crate::ast::ExprNode,
        conditions: &mut Vec<&'a crate::ast::ExprNode>,
    ) {
        match &expression.Kind {
            crate::ast::ExprKind::Binary { Op, L, R } if Op.eq_ignore_ascii_case("and") => {
                split_top_level_and(L, conditions);
                split_top_level_and(R, conditions);
            }
            crate::ast::ExprKind::Parentheses(inner) => {
                split_top_level_and(inner, conditions);
            }
            _ => conditions.push(expression),
        }
    }

    let mut predicates = Vec::new();
    split_top_level_and(predicate, &mut predicates);
    let mut conditions = Vec::new();
    for predicate in predicates {
        let (condition, rewritten_plan) = crate::expression_rewriter::rewrite(
            builder,
            crate::context::TODOArc(),
            predicate,
            plan,
            crate::expression_rewriter::AggregateMapper::default(),
            false,
        )?;
        plan = rewritten_plan;
        if let Some(condition) = condition {
            conditions.extend(expression::SplitCNFItems(condition.as_ref()));
        }
    }
    Ok((conditions, plan))
}

/// 为聚合 ORDER BY 生成隐藏列。
fn hidden_aggregation_order_column(
    plan: &dyn logicalop::LogicalPlan,
    node: &crate::ast::ExprNode,
) -> Option<expression::ExprBox> {
    let crate::ast::ExprKind::Column(name) = &node.Kind else {
        return None;
    };
    let projection = plan
        .as_any()
        .downcast_ref::<logicalop::LogicalProjection>()?;
    if let Some((index, _)) = projection
        .OutputNames()
        .0
        .iter()
        .enumerate()
        .find(|(_, output)| {
            output.as_ref().is_some_and(|output| {
                output.ColName.L == name.Name.L
                    && (name.Table.L.is_empty() || output.TblName.L == name.Table.L)
                    && (name.Schema.L.is_empty() || output.DBName.L == name.Schema.L)
            })
        })
        && let Some(column) = projection.Schema().Columns.get(index)
    {
        return Some(Box::new(column.Clone()));
    }
    let aggregation = projection
        .Children()
        .first()?
        .as_any()
        .downcast_ref::<logicalop::LogicalAggregation>()?;
    aggregation
        .OutputNames()
        .0
        .iter()
        .enumerate()
        .find(|(_, output)| {
            output.as_ref().is_some_and(|output| {
                output.ColName.L == name.Name.L
                    && (name.Table.L.is_empty() || output.TblName.L == name.Table.L)
                    && (name.Schema.L.is_empty() || output.DBName.L == name.Schema.L)
            })
        })
        .and_then(|(index, _)| aggregation.Schema().Columns.get(index))
        .map(|column| Box::new(column.Clone()) as expression::ExprBox)
}

/// 从窗口规格提取排序项。
fn window_sort_items(
    builder: &PlanBuilder,
    items: &[crate::ast::ByItem],
    source: &dyn logicalop::LogicalPlan,
    table_info: Option<&expression::model::TableInfo>,
) -> Result<Vec<property_dependency::SortItem>, expression::Error> {
    items
        .iter()
        .map(|item| {
            let expression = rewrite_for_plan(builder, &item.Expr, source, table_info)?;
            let column = expression.as_column().cloned().ok_or_else(|| {
                expression::errors::New(
                    "window PARTITION BY and ORDER BY expressions must resolve to columns",
                )
            })?;
            Ok(property_dependency::SortItem {
                Col: column,
                Desc: item.Desc,
            })
        })
        .collect()
}

/// 向窗口前投影追加表达式。
fn append_window_projection_expr(
    builder: &PlanBuilder,
    ast_expression: &crate::ast::ExprNode,
    source: &dyn logicalop::LogicalPlan,
    table_info: Option<&expression::model::TableInfo>,
    keep_constant: bool,
    expressions: &mut Vec<expression::ExprBox>,
    schema: &mut expression::Schema,
    names: &mut expression::types::NameSlice,
    aggregate_mapper: &crate::expression_rewriter::AggregateMapper,
) -> Result<expression::ExprBox, expression::Error> {
    let rewritten = rewrite_window_input_for_plan(
        builder,
        ast_expression,
        source,
        table_info,
        aggregate_mapper,
    )?;
    append_rewritten_window_projection_expr(
        builder,
        rewritten,
        keep_constant,
        expressions,
        schema,
        names,
    )
}

/// 向窗口前投影追加已改写的表达式。
fn append_rewritten_window_projection_expr(
    builder: &PlanBuilder,
    rewritten: expression::ExprBox,
    keep_constant: bool,
    expressions: &mut Vec<expression::ExprBox>,
    schema: &mut expression::Schema,
    names: &mut expression::types::NameSlice,
) -> Result<expression::ExprBox, expression::Error> {
    if rewritten.as_column().is_some()
        || (keep_constant && rewritten.as_any().is::<expression::Constant>())
    {
        return Ok(rewritten);
    }
    let column = expression::Column::new(
        rewritten
            .GetType(builder.ctx.GetExprCtx().GetEvalCtx())
            .clone(),
        0,
        builder.ctx.GetExprCtx().AllocPlanColumnID(),
        schema.Len() as isize,
    );
    expressions.push(rewritten);
    schema.Append([column.Clone()]);
    names
        .0
        .push(Some(Arc::clone(&expression::types::EmptyName)));
    Ok(Box::new(column))
}

/// 校验窗口函数参数合法性。
fn validate_window_function_args(
    builder: &mut PlanBuilder,
    select: &crate::ast::SelectStmt,
    source: &dyn logicalop::LogicalPlan,
    table_info: Option<&expression::model::TableInfo>,
    aggregate_mapper: &crate::expression_rewriter::AggregateMapper,
) -> Result<(), expression::Error> {
    builder.optFlag |= rule::FLAG_ELIMINATE_PROJECTION;
    for field in &select.Fields.Fields {
        let Some(crate::ast::ExprNode {
            Kind: crate::ast::ExprKind::WindowFunction { Name, Args, .. },
            ..
        }) = field.Expr.as_ref()
        else {
            continue;
        };
        if Name.eq_ignore_ascii_case("group_concat") {
            return Err(plannererrors::ErrNotSupportedYet
                .GenWithStackByArgs(&["group_concat as window function".into()])
                .into());
        }
        let mut validated_args = Vec::with_capacity(Args.len());
        for (argument_index, argument) in Args.iter().enumerate() {
            let rewritten = rewrite_window_argument_for_plan(
                builder,
                Name,
                argument_index,
                argument,
                source,
                table_info,
                aggregate_mapper,
            )
            .map_err(|error| {
                expression::errors::New(
                    error
                        .to_string()
                        .replace("in 'expression'", "in 'field list'"),
                )
            })?;
            if rewritten.as_column().is_some() || rewritten.as_any().is::<expression::Constant>() {
                validated_args.push(rewritten);
                continue;
            }
            validated_args.push(Box::new(expression::Column::new(
                rewritten
                    .GetType(builder.ctx.GetExprCtx().GetEvalCtx())
                    .clone(),
                0,
                builder.ctx.GetExprCtx().AllocPlanColumnID(),
                0,
            )) as expression::ExprBox);
        }
        if aggregation::NewWindowFuncDesc(builder.ctx.GetExprCtx(), Name, validated_args, false)?
            .is_none()
        {
            return Err(plannererrors::ErrWrongArguments
                .GenWithStackByArgs(&[Name.to_ascii_lowercase().into()])
                .into());
        }
    }
    Ok(())
}

/// 按窗口分组构建投影。
fn build_window_group_projection(
    builder: &PlanBuilder,
    source: logicalop::LogicalPlanRef,
    spec: &crate::ast::WindowSpec,
    functions: &[(usize, &crate::ast::ExprNode)],
    table_info: Option<&expression::model::TableInfo>,
    query_block: i32,
    aggregate_mapper: &crate::expression_rewriter::AggregateMapper,
) -> Result<
    (
        logicalop::LogicalPlanRef,
        Vec<property_dependency::SortItem>,
        Vec<property_dependency::SortItem>,
        Vec<Vec<expression::ExprBox>>,
    ),
    expression::Error,
> {
    // Go initializes the projection before expression rewriting. Rewrites may
    // build subplans, so this preserves the canonical plan-ID lifecycle.
    let mut projection =
        logicalop::LogicalProjection::default().Init(builder.ctx.clone(), query_block);
    let mut expressions = source
        .Schema()
        .Columns
        .iter()
        .map(|column| Box::new(column.Clone()) as expression::ExprBox)
        .collect::<Vec<_>>();
    let mut schema = source.Schema().Clone();
    let mut names = source.OutputNames().Shallow();
    let mut build_items = |items: &[crate::ast::ByItem]| {
        items
            .iter()
            .map(|item| {
                let projected = append_window_projection_expr(
                    builder,
                    &item.Expr,
                    source.as_ref(),
                    table_info,
                    false,
                    &mut expressions,
                    &mut schema,
                    &mut names,
                    aggregate_mapper,
                )?;
                Ok(property_dependency::SortItem {
                    Col: projected
                        .as_column()
                        .expect("window by-item projection returns a column")
                        .Clone(),
                    Desc: item.Desc,
                })
            })
            .collect::<Result<Vec<_>, expression::Error>>()
    };
    let partition_by = build_items(&spec.PartitionBy)?;
    let order_by = build_items(&spec.OrderBy)?;
    let mut projected_args = Vec::with_capacity(functions.len());
    for (_, ast_expression) in functions {
        let crate::ast::ExprKind::WindowFunction { Name, Args, .. } = &ast_expression.Kind else {
            unreachable!("window groups contain only window functions")
        };
        projected_args.push(
            Args.iter()
                .enumerate()
                .map(|(argument_index, argument)| {
                    let normalized = rewrite_window_argument_for_plan(
                        builder,
                        Name,
                        argument_index,
                        argument,
                        source.as_ref(),
                        table_info,
                        aggregate_mapper,
                    )?;
                    append_rewritten_window_projection_expr(
                        builder,
                        normalized,
                        true,
                        &mut expressions,
                        &mut schema,
                        &mut names,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?,
        );
    }
    projection.Exprs = expressions;
    projection.SetSchema(schema);
    projection.SetOutputNames(names);
    projection.SetChildren(vec![source]);
    Ok((Box::new(projection), partition_by, order_by, projected_args))
}

/// 推导窗口 RANGE 比较的数据类型。
fn window_range_type(column: Option<&expression::Column>) -> logicalop::RangeCmpDataType {
    let Some(field) = column.and_then(|column| column.RetType.as_ref()) else {
        return logicalop::RangeCmpDataType::Unsupported;
    };
    match field.EvalType() {
        expression::types::ETInt => logicalop::RangeCmpDataType::Int,
        expression::types::ETReal => logicalop::RangeCmpDataType::Real,
        expression::types::ETDecimal => logicalop::RangeCmpDataType::Decimal,
        expression::types::ETDatetime | expression::types::ETTimestamp => {
            logicalop::RangeCmpDataType::Time
        }
        expression::types::ETDuration => logicalop::RangeCmpDataType::Duration,
        _ => logicalop::RangeCmpDataType::Unsupported,
    }
}

/// 规范化窗口名称。
fn window_name(name: &str) -> String {
    if name.is_empty() {
        "<unnamed window>".to_owned()
    } else {
        name.to_owned()
    }
}

/// 时间单位枚举转名称。
fn window_time_unit_name(unit: crate::ast::TimeUnitType) -> &'static str {
    use crate::ast::TimeUnitType;
    match unit {
        TimeUnitType::Invalid => "",
        TimeUnitType::Microsecond => "MICROSECOND",
        TimeUnitType::Second => "SECOND",
        TimeUnitType::Minute => "MINUTE",
        TimeUnitType::Hour => "HOUR",
        TimeUnitType::Day => "DAY",
        TimeUnitType::Week => "WEEK",
        TimeUnitType::Month => "MONTH",
        TimeUnitType::Quarter => "QUARTER",
        TimeUnitType::Year => "YEAR",
        TimeUnitType::SecondMicrosecond => "SECOND_MICROSECOND",
        TimeUnitType::MinuteMicrosecond => "MINUTE_MICROSECOND",
        TimeUnitType::MinuteSecond => "MINUTE_SECOND",
        TimeUnitType::HourMicrosecond => "HOUR_MICROSECOND",
        TimeUnitType::HourSecond => "HOUR_SECOND",
        TimeUnitType::HourMinute => "HOUR_MINUTE",
        TimeUnitType::DayMicrosecond => "DAY_MICROSECOND",
        TimeUnitType::DaySecond => "DAY_SECOND",
        TimeUnitType::DayMinute => "DAY_MINUTE",
        TimeUnitType::DayHour => "DAY_HOUR",
        TimeUnitType::YearMonth => "YEAR_MONTH",
    }
}

/// ROWS 边界是否为无符号整数。
fn window_rows_bound_is_unsigned_integer(bound: &crate::ast::FrameBound) -> bool {
    let Some(expression) = bound.Expr.as_ref() else {
        return false;
    };
    match &expression.Kind {
        crate::ast::ExprKind::Value(value) => match &value.Datum {
            crate::ast::ValueDatum::Uint64(_) => true,
            crate::ast::ValueDatum::Int64(value) => *value >= 0,
            // The Rust parser retains integer token text in these variants.
            crate::ast::ValueDatum::String(value) | crate::ast::ValueDatum::Decimal(value) => {
                value.parse::<u64>().is_ok()
            }
            _ => false,
        },
        crate::ast::ExprKind::ParamMarker { .. } => true,
        _ => false,
    }
}

/// 校验原始窗口帧边界。
fn check_origin_window_frame_bound(
    bound: &crate::ast::FrameBound,
    spec: &crate::ast::WindowSpec,
    order_by: &[property_dependency::SortItem],
) -> Result<(), expression::Error> {
    if bound.Type == crate::ast::BoundType::CurrentRow || bound.UnBounded {
        return Ok(());
    }
    let name = window_name(&spec.Name.O);
    let frame = spec
        .Frame
        .as_deref()
        .expect("a frame bound belongs to a frame");
    if frame.Type == crate::ast::FrameType::Rows {
        if bound.Unit != crate::ast::TimeUnitType::Invalid {
            return Err(plannererrors::ErrWindowRowsIntervalUse
                .GenWithStackByArgs(&[name.into()])
                .into());
        }
        if !window_rows_bound_is_unsigned_integer(bound) {
            return Err(plannererrors::ErrWindowFrameIllegal
                .GenWithStackByArgs(&[name.into()])
                .into());
        }
        return Ok(());
    }
    if order_by.len() != 1 {
        return Err(plannererrors::ErrWindowRangeFrameOrderType
            .GenWithStackByArgs(&[name.into()])
            .into());
    }
    let range_type = window_range_type(Some(&order_by[0].Col));
    let numeric = matches!(
        range_type,
        logicalop::RangeCmpDataType::Int
            | logicalop::RangeCmpDataType::Real
            | logicalop::RangeCmpDataType::Decimal
    );
    let temporal = matches!(
        range_type,
        logicalop::RangeCmpDataType::Time | logicalop::RangeCmpDataType::Duration
    );
    if !numeric && !temporal {
        return Err(plannererrors::ErrWindowRangeFrameOrderType
            .GenWithStackByArgs(&[name.into()])
            .into());
    }
    if bound.Unit != crate::ast::TimeUnitType::Invalid && !temporal {
        return Err(plannererrors::ErrWindowRangeFrameNumericType
            .GenWithStackByArgs(&[name.into()])
            .into());
    }
    if bound.Unit == crate::ast::TimeUnitType::Invalid && !numeric {
        return Err(plannererrors::ErrWindowRangeFrameTemporalType
            .GenWithStackByArgs(&[name.into()])
            .into());
    }
    let Some(offset) = bound.Expr.as_ref() else {
        return Err(plannererrors::ErrWindowRangeBoundNotConstant
            .GenWithStackByArgs(&[name.into()])
            .into());
    };
    let valid_constant = match &offset.Kind {
        crate::ast::ExprKind::Value(value) => match &value.Datum {
            crate::ast::ValueDatum::Null => {
                return Err(plannererrors::ErrWindowFrameIllegal
                    .GenWithStackByArgs(&[name.into()])
                    .into());
            }
            crate::ast::ValueDatum::Int64(value) => *value >= 0,
            crate::ast::ValueDatum::Uint64(_) => true,
            crate::ast::ValueDatum::Float32(bits) => f32::from_bits(*bits) >= 0.0,
            crate::ast::ValueDatum::Float64(bits) => f64::from_bits(*bits) >= 0.0,
            crate::ast::ValueDatum::Decimal(value) | crate::ast::ValueDatum::String(value) => value
                .parse::<f64>()
                .is_ok_and(|value| value.is_finite() && value >= 0.0),
            _ => false,
        },
        crate::ast::ExprKind::ParamMarker { .. } => true,
        crate::ast::ExprKind::Unary { Op, V } if Op == "-" => {
            if matches!(V.Kind, crate::ast::ExprKind::Value(_)) {
                return Err(plannererrors::ErrWindowFrameIllegal
                    .GenWithStackByArgs(&[name.into()])
                    .into());
            }
            false
        }
        _ => false,
    };
    if !valid_constant {
        return Err(plannererrors::ErrWindowRangeBoundNotConstant
            .GenWithStackByArgs(&[name.into()])
            .into());
    }
    Ok(())
}

/// 校验原始窗口规格。
fn check_origin_window_spec(
    spec: &crate::ast::WindowSpec,
    order_by: &[property_dependency::SortItem],
) -> Result<(), expression::Error> {
    let Some(frame) = spec.Frame.as_deref() else {
        return Ok(());
    };
    let name = window_name(&spec.Name.O);
    if frame.Type == crate::ast::FrameType::Groups {
        return Err(plannererrors::ErrNotSupportedYet
            .GenWithStackByArgs(&["GROUPS".into()])
            .into());
    }
    let start = &frame.Extent.Start;
    let end = &frame.Extent.End;
    if start.Type == crate::ast::BoundType::Following && start.UnBounded {
        return Err(plannererrors::ErrWindowFrameStartIllegal
            .GenWithStackByArgs(&[name.into()])
            .into());
    }
    if end.Type == crate::ast::BoundType::Preceding && end.UnBounded {
        return Err(plannererrors::ErrWindowFrameEndIllegal
            .GenWithStackByArgs(&[name.into()])
            .into());
    }
    if start.Type == crate::ast::BoundType::Following
        && matches!(
            end.Type,
            crate::ast::BoundType::Preceding | crate::ast::BoundType::CurrentRow
        )
    {
        return Err(plannererrors::ErrWindowFrameIllegal
            .GenWithStackByArgs(&[name.into()])
            .into());
    }
    if matches!(
        start.Type,
        crate::ast::BoundType::Following | crate::ast::BoundType::CurrentRow
    ) && end.Type == crate::ast::BoundType::Preceding
    {
        return Err(plannererrors::ErrWindowFrameIllegal
            .GenWithStackByArgs(&[name.into()])
            .into());
    }
    check_origin_window_frame_bound(start, spec, order_by)?;
    check_origin_window_frame_bound(end, spec, order_by)
}

/// 构建窗口帧边界。
fn build_window_bound(
    builder: &PlanBuilder,
    bound: &crate::ast::FrameBound,
    frame_type: logicalop::FrameType,
    order_by: &[property_dependency::SortItem],
    source: &dyn logicalop::LogicalPlan,
    table_info: Option<&expression::model::TableInfo>,
) -> Result<logicalop::FrameBound, expression::Error> {
    let mut result = logicalop::FrameBound {
        Type: match bound.Type {
            crate::ast::BoundType::Preceding => logicalop::BoundType::Preceding,
            crate::ast::BoundType::Following => logicalop::BoundType::Following,
            crate::ast::BoundType::CurrentRow => logicalop::BoundType::CurrentRow,
        },
        UnBounded: bound.UnBounded,
        ..Default::default()
    };
    if let Some(offset) = &bound.Expr {
        if frame_type == logicalop::FrameType::Rows {
            result.Num = read_limit_value(Some(offset), 0)?;
        } else {
            let order = order_by
                .first()
                .expect("validated explicit RANGE frame has one ORDER BY item");
            let value = if let crate::ast::ExprKind::Value(crate::ast::ValueExpr {
                Datum: crate::ast::ValueDatum::String(text),
                ..
            }) = &offset.Kind
            {
                if let Ok(value) = text.parse::<u64>() {
                    Box::new(expression::Constant::with_type(
                        expression::types::NewUintDatum(value),
                        *expression::types::NewFieldType(expression::mysql::TypeLonglong),
                    )) as expression::ExprBox
                } else {
                    let mut value = expression::types::MyDecimal::default();
                    value
                        .FromString(text.as_bytes())
                        .map_err(|error| expression::errors::New(error.to_string()))?;
                    Box::new(expression::Constant::with_type(
                        expression::types::NewDecimalDatum(value),
                        *expression::types::NewFieldType(expression::mysql::TypeNewDecimal),
                    )) as expression::ExprBox
                }
            } else {
                rewrite_for_plan(builder, offset, source, table_info)?
            };
            let return_type = order.Col.RetType.clone().unwrap_or_else(|| {
                *expression::types::NewFieldType(expression::mysql::TypeUnspecified)
            });
            let subtract = (!order.Desc && result.Type == logicalop::BoundType::Preceding)
                || (order.Desc && result.Type == logicalop::BoundType::Following);
            let mut arguments = vec![Box::new(order.Col.Clone()) as expression::ExprBox, value];
            let function_name = if bound.Unit == crate::ast::TimeUnitType::Invalid {
                if subtract {
                    expression::ast::Minus
                } else {
                    expression::ast::Plus
                }
            } else {
                arguments.push(Box::new(expression::Constant::with_type(
                    expression::types::NewStringDatum(window_time_unit_name(bound.Unit).to_owned()),
                    *expression::types::NewFieldType(expression::mysql::TypeVarchar),
                )));
                if subtract {
                    expression::ast::DateSub
                } else {
                    expression::ast::DateAdd
                }
            };
            result.CalcFuncs = vec![expression::NewFunctionBase(
                builder.ctx.GetExprCtx(),
                function_name,
                return_type,
                arguments,
            )?];
            result.IsExplicitRange = true;
            result.CompareCols = vec![Box::new(order.Col.Clone())];
            result.UpdateCmpFuncsAndCmpDataType(window_range_type(Some(&order.Col)));
        }
    }
    Ok(result)
}

/// 构建窗口帧。
fn build_window_frame(
    builder: &PlanBuilder,
    frame: Option<&crate::ast::FrameClause>,
    order_by: &[property_dependency::SortItem],
    source: &dyn logicalop::LogicalPlan,
    table_info: Option<&expression::model::TableInfo>,
) -> Result<Option<logicalop::WindowFrame>, expression::Error> {
    let Some(frame) = frame else {
        return Ok(None);
    };
    let frame_type = match frame.Type {
        crate::ast::FrameType::Rows => logicalop::FrameType::Rows,
        crate::ast::FrameType::Ranges => logicalop::FrameType::Range,
        crate::ast::FrameType::Groups => logicalop::FrameType::Groups,
    };
    Ok(Some(logicalop::WindowFrame {
        Type: frame_type,
        Start: Some(build_window_bound(
            builder,
            &frame.Extent.Start,
            frame_type,
            order_by,
            source,
            table_info,
        )?),
        End: Some(build_window_bound(
            builder,
            &frame.Extent.End,
            frame_type,
            order_by,
            source,
            table_info,
        )?),
    }))
}

/// 解析并合并后的窗口规格。
fn resolved_window_spec(
    select: &crate::ast::SelectStmt,
    spec: &crate::ast::WindowSpec,
) -> Result<crate::ast::WindowSpec, expression::Error> {
    if !spec.Name.L.is_empty() {
        return select
            .WindowSpecs
            .iter()
            .find(|candidate| candidate.Name.L == spec.Name.L)
            .cloned()
            .ok_or_else(|| -> expression::Error {
                plannererrors::ErrWindowNoSuchWindow
                    .GenWithStackByArgs(&[spec.Name.O.clone().into()])
                    .into()
            });
    }
    let mut resolved = spec.clone();
    if !spec.Ref.L.is_empty() {
        let reference = select
            .WindowSpecs
            .iter()
            .find(|candidate| candidate.Name.L == spec.Ref.L)
            .ok_or_else(|| -> expression::Error {
                plannererrors::ErrWindowNoSuchWindow
                    .GenWithStackByArgs(&[spec.Ref.O.clone().into()])
                    .into()
            })?;
        merge_window_spec(&mut resolved, reference)?;
    }
    Ok(resolved)
}

/// 构建全部窗口规格。
fn build_window_specs(
    specs: &[crate::ast::WindowSpec],
) -> Result<Vec<crate::ast::WindowSpec>, expression::Error> {
    let mut definitions = HashMap::with_capacity(specs.len());
    for spec in specs {
        if definitions
            .insert(spec.Name.L.clone(), spec.clone())
            .is_some()
        {
            return Err(plannererrors::ErrWindowDuplicateName
                .GenWithStackByArgs(&[spec.Name.O.clone().into()])
                .into());
        }
    }
    let mut resolved = HashMap::with_capacity(definitions.len());
    let names = specs
        .iter()
        .map(|spec| spec.Name.L.clone())
        .collect::<Vec<_>>();
    for name in &names {
        resolve_window_spec(name, &definitions, &mut resolved, &mut Vec::new())?;
    }
    Ok(names
        .into_iter()
        .filter_map(|name| resolved.remove(&name))
        .collect())
}

/// 解析单个窗口规格引用。
fn resolve_window_spec(
    name: &str,
    definitions: &HashMap<String, crate::ast::WindowSpec>,
    resolved: &mut HashMap<String, crate::ast::WindowSpec>,
    stack: &mut Vec<String>,
) -> Result<crate::ast::WindowSpec, expression::Error> {
    if let Some(spec) = resolved.get(name) {
        return Ok(spec.clone());
    }
    if stack.iter().any(|item| item == name) {
        return Err(plannererrors::ErrWindowCircularityInWindowGraph
            .GenWithStackByArgs(&[])
            .into());
    }
    let mut spec = definitions
        .get(name)
        .cloned()
        .ok_or_else(|| expression::errors::New(format!("unknown window '{name}'")))?;
    if !spec.Ref.L.is_empty() {
        let Some(reference) = definitions.get(&spec.Ref.L) else {
            return Err(plannererrors::ErrWindowNoSuchWindow
                .GenWithStackByArgs(&[spec.Ref.O.clone().into()])
                .into());
        };
        stack.push(name.to_owned());
        let reference = resolve_window_spec(&reference.Name.L, definitions, resolved, stack)?;
        stack.pop();
        merge_window_spec(&mut spec, &reference)?;
    }
    resolved.insert(name.to_owned(), spec.clone());
    Ok(spec)
}

/// 合并引用窗口与当前规格。
fn merge_window_spec(
    spec: &mut crate::ast::WindowSpec,
    reference: &crate::ast::WindowSpec,
) -> Result<(), expression::Error> {
    if reference.Frame.is_some() {
        return Err(plannererrors::ErrWindowNoInherentFrame
            .GenWithStackByArgs(&[reference.Name.O.clone().into()])
            .into());
    }
    if !spec.PartitionBy.is_empty() {
        return Err(plannererrors::ErrWindowNoChildPartitioning
            .GenWithStackByArgs(&[])
            .into());
    }
    if !reference.OrderBy.is_empty() {
        if !spec.OrderBy.is_empty() {
            let name = if spec.Name.O.is_empty() {
                "<unnamed window>".to_owned()
            } else {
                spec.Name.O.clone()
            };
            return Err(plannererrors::ErrWindowNoRedefineOrderBy
                .GenWithStackByArgs(&[name.into(), reference.Name.O.clone().into()])
                .into());
        }
        spec.OrderBy = reference.OrderBy.clone();
    }
    spec.PartitionBy = reference.PartitionBy.clone();
    spec.Ref = crate::ast::CIStr::default();
    Ok(())
}

/// 构建窗口函数计划链。
fn build_select_windows(
    builder: &mut PlanBuilder,
    select: &crate::ast::SelectStmt,
    mut source: logicalop::LogicalPlanRef,
    table_info: Option<&expression::model::TableInfo>,
    query_block: i32,
    aggregate_mapper: &crate::expression_rewriter::AggregateMapper,
) -> Result<
    (
        logicalop::LogicalPlanRef,
        HashMap<usize, expression::Column>,
    ),
    expression::Error,
> {
    validate_window_function_args(
        builder,
        select,
        source.as_ref(),
        table_info,
        aggregate_mapper,
    )?;
    let mut output_columns = HashMap::new();
    let mut groups: Vec<(crate::ast::WindowSpec, Vec<(usize, &crate::ast::ExprNode)>)> = Vec::new();
    for (field_index, field) in select.Fields.Fields.iter().enumerate() {
        let Some(ast_expression) = field.Expr.as_ref() else {
            continue;
        };
        let crate::ast::ExprKind::WindowFunction { Name, Spec, .. } = &ast_expression.Kind else {
            continue;
        };
        let mut spec = resolved_window_spec(select, Spec)?;
        if aggregation::NeedFrame(Name) {
            if spec.Frame.is_none() && !spec.OrderBy.is_empty() {
                spec.Frame = Some(Box::new(crate::ast::FrameClause {
                    Type: crate::ast::FrameType::Ranges,
                    Extent: crate::ast::FrameExtent {
                        Start: crate::ast::FrameBound {
                            Type: crate::ast::BoundType::Preceding,
                            UnBounded: true,
                            ..Default::default()
                        },
                        End: crate::ast::FrameBound {
                            Type: crate::ast::BoundType::CurrentRow,
                            ..Default::default()
                        },
                    },
                }));
            } else if spec
                .Frame
                .as_ref()
                .is_some_and(|frame| frame.Extent.Start.UnBounded && frame.Extent.End.UnBounded)
            {
                spec.Frame = None;
            }
        } else {
            if spec.Frame.is_some() {
                builder.ctx.GetSessionVars().StmtCtx.AppendNote(
                    plannererrors::ErrWindowFunctionIgnoresFrame.FastGenByArgs(&[
                        Name.to_ascii_lowercase().into(),
                        window_name(&spec.Name.O).into(),
                    ]),
                );
                spec.Frame = None;
            }
            let pipelined_window_enabled = builder
                .ctx
                .GetSessionVars()
                .GetSystemVar(vardef_dependency::TiDBEnablePipelinedWindowFunction)
                // Match Go's session default when this context has not loaded
                // the system variable yet.
                .map_or(
                    vardef_dependency::DefEnablePipelinedWindowFunction,
                    |value| matches!(value.to_ascii_lowercase().as_str(), "1" | "on" | "true"),
                );
            if pipelined_window_enabled {
                let (use_default, default_frame) = aggregation::UseDefaultFrame(Name);
                if use_default {
                    spec.Frame = Some(Box::new(default_frame));
                }
            }
        }
        if let Some((_, functions)) = groups.iter_mut().find(|(candidate, _)| *candidate == spec) {
            functions.push((field_index, ast_expression));
        } else {
            groups.push((spec, vec![(field_index, ast_expression)]));
        }
    }

    let by_items_key = |spec: &crate::ast::WindowSpec| {
        spec.PartitionBy
            .iter()
            .chain(&spec.OrderBy)
            .map(|item| (format!("{:?}", item.Expr.Kind), item.Desc))
            .collect::<Vec<_>>()
    };
    // Go sorts window specifications in reverse lexicographical order.  This
    // places compatible prefixes next to each other and minimizes Sort nodes.
    groups.sort_by(|(left, _), (right, _)| by_items_key(right).cmp(&by_items_key(left)));
    let mut start = 0;
    while start < groups.len() {
        let key = by_items_key(&groups[start].0);
        let mut end = start + 1;
        while end < groups.len() && by_items_key(&groups[end].0) == key {
            end += 1;
        }
        // Go's comparator returns true for equal by-item lists, reversing
        // otherwise identical specs such as rank and row_number defaults.
        groups[start..end].reverse();
        start = end;
    }

    for (spec, functions) in groups {
        let (projected_source, partition_by, order_by, projected_args) =
            build_window_group_projection(
                builder,
                source,
                &spec,
                &functions,
                table_info,
                query_block,
                aggregate_mapper,
            )?;
        source = projected_source;
        for (_, expression) in &functions {
            let crate::ast::ExprKind::WindowFunction {
                Distinct,
                IgnoreNull,
                FromLast,
                Spec,
                ..
            } = &expression.Kind
            else {
                unreachable!("window groups contain only window functions")
            };
            if *IgnoreNull {
                return Err(plannererrors::ErrNotSupportedYet
                    .GenWithStackByArgs(&["IGNORE NULLS".into()])
                    .into());
            }
            if *Distinct {
                return Err(plannererrors::ErrNotSupportedYet
                    .GenWithStackByArgs(&["<window function>(DISTINCT ..)".into()])
                    .into());
            }
            if *FromLast {
                return Err(plannererrors::ErrNotSupportedYet
                    .GenWithStackByArgs(&["FROM LAST".into()])
                    .into());
            }
            let original = resolved_window_spec(select, Spec)?;
            check_origin_window_spec(&original, &order_by)?;
        }
        let frame = build_window_frame(
            builder,
            spec.Frame.as_deref(),
            &order_by,
            source.as_ref(),
            table_info,
        )?;
        let base_index = source.Schema().Len();
        let mut descriptors = Vec::with_capacity(functions.len());
        let mut schema = source.Schema().Clone();
        let mut names = source.OutputNames().Shallow();
        for (offset, ((field_index, ast_expression), args)) in
            functions.into_iter().zip(projected_args).enumerate()
        {
            let crate::ast::ExprKind::WindowFunction {
                Name,
                Args,
                Distinct,
                IgnoreNull,
                FromLast,
                ..
            } = &ast_expression.Kind
            else {
                unreachable!("window groups contain only window functions")
            };
            if *Distinct || *IgnoreNull || *FromLast {
                return Err(expression::errors::New(format!(
                    "unsupported window modifier for {}",
                    Name.to_ascii_lowercase()
                )));
            }
            let mut descriptor =
                aggregation::NewWindowFuncDesc(builder.ctx.GetExprCtx(), Name, args, false)?
                    .ok_or_else(|| -> expression::Error {
                        plannererrors::ErrWrongArguments
                            .GenWithStackByArgs(&[Name.to_ascii_lowercase().into()])
                            .into()
                    })?;
            descriptor.WrapCastForAggArgs(builder.ctx.GetExprCtx());
            let return_type = descriptor.RetTp.clone().unwrap_or_else(|| {
                *expression::types::NewFieldType(expression::mysql::TypeLonglong)
            });
            let result_column = expression::Column::new(
                return_type,
                0,
                builder.ctx.GetExprCtx().AllocPlanColumnID(),
                (base_index + offset) as isize,
            );
            schema.Append([result_column.Clone()]);
            names
                .0
                .push(Some(Arc::clone(&expression::types::EmptyName)));
            descriptors.push(logicalop::WindowFuncDesc {
                Name: descriptor.Name.clone(),
                Args: descriptor.Args.iter().map(|arg| arg.CloneExpr()).collect(),
            });
            output_columns.insert(field_index, result_column);
        }
        let mut window = logicalop::LogicalWindow {
            WindowFuncDescs: descriptors,
            PartitionBy: partition_by,
            OrderBy: order_by,
            Frame: frame,
            ..Default::default()
        }
        .Init(builder.ctx.clone(), query_block);
        window.SetSchema(schema);
        window.SetOutputNames(names);
        window.SetChildren(vec![source]);
        source = Box::new(window);
    }
    builder.optFlag |= rule::FLAG_BUILD_KEY_INFO;
    Ok((source, output_columns))
}

/// 窗口前投影。
fn build_pre_window_projection(
    builder: &mut PlanBuilder,
    select: &crate::ast::SelectStmt,
    source: logicalop::LogicalPlanRef,
    _table_info: Option<&expression::model::TableInfo>,
    query_block: i32,
) -> Result<logicalop::LogicalPlanRef, expression::Error> {
    // Go's buildProjectionForWindow starts with every child column and only
    // appends computed PARTITION BY, ORDER BY and argument expressions.  The
    // previous Rust path projected SELECT fields here and represented a window
    // expression with zero, which discarded columns used exclusively by the
    // window specification before logical column pruning ran.
    let mut expressions = source
        .Schema()
        .Columns
        .iter()
        .map(|column| Box::new(column.Clone()) as expression::ExprBox)
        .collect::<Vec<_>>();
    let mut schema = source.Schema().Clone();
    let mut names = source.OutputNames().Shallow();
    // Go first builds the SELECT projection with `considerWindow=false`.
    // Window fields are real zero placeholders at that stage, while columns
    // needed by the later window projection remain available as auxiliaries.
    for field in &select.Fields.Fields {
        if field.Expr.as_ref().is_some_and(|expression| {
            matches!(expression.Kind, crate::ast::ExprKind::WindowFunction { .. })
        }) {
            let zero = Box::new(expression::NewZero()) as expression::ExprBox;
            let column = expression::Column::new(
                zero.GetType(builder.ctx.GetExprCtx().GetEvalCtx()).clone(),
                0,
                builder.ctx.GetExprCtx().AllocPlanColumnID(),
                schema.Len() as isize,
            );
            expressions.push(zero);
            schema.Append([column]);
            names.0.push(Some(if field.AsName.O.is_empty() {
                Arc::clone(&expression::types::EmptyName)
            } else {
                Arc::new(expression::types::FieldName {
                    ColName: field.AsName.clone(),
                    ..Default::default()
                })
            }));
        }
    }
    let mut projection = logicalop::LogicalProjection {
        Exprs: expressions,
        ..Default::default()
    }
    .Init(builder.ctx.clone(), query_block);
    projection.SetSchema(schema);
    projection.SetOutputNames(names);
    projection.SetChildren(vec![source]);
    builder.optFlag |= rule::FLAG_ELIMINATE_PROJECTION | rule::FLAG_BUILD_KEY_INFO;
    Ok(Box::new(projection))
}

/// 窗口输入投影。
fn build_window_input_projection(
    builder: &PlanBuilder,
    source: logicalop::LogicalPlanRef,
    query_block: i32,
) -> logicalop::LogicalPlanRef {
    let expressions = source
        .Schema()
        .Columns
        .iter()
        .map(|column| Box::new(column.Clone()) as expression::ExprBox)
        .collect();
    let schema = source.Schema().Clone();
    let names = source.OutputNames().Shallow();
    let mut projection = logicalop::LogicalProjection {
        Exprs: expressions,
        ..Default::default()
    }
    .Init(builder.ctx.clone(), query_block);
    projection.SetSchema(schema);
    projection.SetOutputNames(names);
    projection.SetChildren(vec![source]);
    Box::new(projection)
}

/// 含窗口的 SELECT 投影。
fn build_select_projection_with_windows(
    builder: &mut PlanBuilder,
    select: &crate::ast::SelectStmt,
    mut source: logicalop::LogicalPlanRef,
    table_info: Option<&expression::model::TableInfo>,
    query_block: i32,
    window_columns: &HashMap<usize, expression::Column>,
    aggregate_mapper: &crate::expression_rewriter::AggregateMapper,
    window_mapper: &crate::expression_rewriter::WindowMapper,
    input_column_count: usize,
) -> Result<logicalop::LogicalPlanRef, expression::Error> {
    let mut expressions = Vec::with_capacity(select.Fields.Fields.len());
    let mut names = Vec::with_capacity(select.Fields.Fields.len());
    let mut visible_len = 0;
    for (index, field) in select.Fields.Fields.iter().enumerate() {
        if let Some(wildcard) = &field.WildCard {
            let start = expressions.len();
            for (column_index, column) in source
                .Schema()
                .Columns
                .iter()
                .take(input_column_count)
                .enumerate()
            {
                let name = source
                    .OutputNames()
                    .0
                    .get(column_index)
                    .and_then(Option::as_ref);
                let matches_schema = wildcard.Schema.O.is_empty()
                    || name.is_some_and(|name| name.DBName.L == wildcard.Schema.L);
                let matches_table = wildcard.Table.O.is_empty()
                    || name.is_some_and(|name| name.TblName.L == wildcard.Table.L);
                if matches_schema && matches_table {
                    expressions.push(Box::new(column.Clone()) as expression::ExprBox);
                    names.push(name.map(|name| Arc::new(name.Clone())));
                }
            }
            if expressions.len() == start {
                return Err(expression::errors::New("wildcard has no matching columns"));
            }
            if !field.Auxiliary {
                visible_len = expressions.len();
            }
            continue;
        }
        let expression = if let Some(column) = window_columns.get(&index) {
            Box::new(column.Clone()) as expression::ExprBox
        } else {
            let ast_expression = field
                .Expr
                .as_ref()
                .ok_or_else(|| expression::errors::New("SELECT field has no expression"))?;
            let (rewritten, rewritten_source) = crate::expression_rewriter::rewriteWithPreprocess(
                builder,
                crate::context::TODOArc(),
                ast_expression,
                source,
                aggregate_mapper.clone(),
                Some(window_mapper.clone()),
                true,
                None,
            )?;
            source = rewritten_source;
            rewritten.ok_or_else(|| {
                expression::errors::New("window SELECT expression did not produce a scalar value")
            })?
        };
        names.push(select_field_output_name(
            field,
            expression.as_ref(),
            source.as_ref(),
        ));
        expressions.push(expression);
        if !field.Auxiliary {
            visible_len = expressions.len();
        }
    }
    for item in &select.OrderBy {
        let resolved = resolve_select_order_aliases(select, &item.Expr);
        let order_columns = rewrite_for_plan(builder, &resolved, source.as_ref(), table_info)
            .map(|order_expression| {
                expression::ExtractColumns(order_expression.as_ref())
                    .into_iter()
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|_| {
                ast_columns_outside_subqueries(&resolved)
                    .into_iter()
                    .filter_map(|name| {
                        rewrite_for_plan(
                            builder,
                            &crate::ast::ExprNode::Column(name),
                            source.as_ref(),
                            table_info,
                        )
                        .ok()
                    })
                    .flat_map(|order_expression| {
                        expression::ExtractColumns(order_expression.as_ref())
                            .into_iter()
                            .cloned()
                            .collect::<Vec<_>>()
                    })
                    .collect()
            });
        for column in order_columns {
            if expressions.iter().any(|expression| {
                expression
                    .as_column()
                    .is_some_and(|existing| existing.UniqueID == column.UniqueID)
            }) {
                continue;
            }
            let Some(index) = source
                .Schema()
                .Columns
                .iter()
                .position(|candidate| candidate.UniqueID == column.UniqueID)
            else {
                continue;
            };
            expressions.push(Box::new(column));
            names.push(source.OutputNames().0.get(index).cloned().flatten());
        }
    }
    let columns = expressions
        .iter()
        .enumerate()
        .map(|(index, expression)| {
            if let Some(input) = expression.as_column() {
                let mut column = input.Clone();
                column.Index = index as isize;
                column.IsHidden = index >= visible_len || column.IsHidden;
                return column;
            }
            let mut column = expression::Column::new(
                expression
                    .GetType(builder.ctx.GetExprCtx().GetEvalCtx())
                    .clone(),
                0,
                builder.ctx.GetExprCtx().AllocPlanColumnID(),
                index as isize,
            );
            column.IsHidden = index >= visible_len;
            column
        })
        .collect();
    let mut projection = logicalop::LogicalProjection {
        Exprs: expressions,
        ..Default::default()
    }
    .Init(builder.ctx.clone(), query_block);
    projection.SetSchema(expression::NewSchema(columns));
    projection.SetOutputNames(expression::types::NameSlice(names));
    projection.SetChildren(vec![source]);
    builder.optFlag |= rule::FLAG_ELIMINATE_PROJECTION | rule::FLAG_BUILD_KEY_INFO;
    Ok(Box::new(projection))
}

pub(crate) fn collect_window_expressions(
    node: &crate::ast::ExprNode,
    windows: &mut Vec<crate::ast::ExprNode>,
) {
    struct Collector<'a> {
        windows: &'a mut Vec<crate::ast::ExprNode>,
    }

    impl crate::ast::ExprNodeVisitor for Collector<'_> {
        fn Enter(&mut self, input: &crate::ast::ExprNode) -> (crate::ast::ExprNode, bool) {
            match &input.Kind {
                crate::ast::ExprKind::WindowFunction { .. } => {
                    self.windows.push(input.clone());
                    (input.clone(), true)
                }
                crate::ast::ExprKind::Subquery { .. }
                | crate::ast::ExprKind::ExistsSubquery { .. } => (input.clone(), true),
                _ => (input.clone(), false),
            }
        }

        fn Leave(&mut self, input: &crate::ast::ExprNode) -> (crate::ast::ExprNode, bool) {
            (input.clone(), true)
        }
    }

    let _ = node.Accept(&mut Collector { windows });
}

/// 收集 AST 中的聚合节点。
fn collect_aggregate_nodes<'a>(
    node: &'a crate::ast::ExprNode,
    nodes: &mut Vec<&'a crate::ast::ExprNode>,
) {
    use crate::ast::ExprKind;
    match &node.Kind {
        ExprKind::AggregateFunction { .. } => {
            let key = crate::expression_rewriter::AggregateMapperKey(node);
            if !nodes
                .iter()
                .any(|candidate| crate::expression_rewriter::AggregateMapperKey(candidate) == key)
            {
                nodes.push(node);
            }
        }
        ExprKind::Function { Args, .. } | ExprKind::Row(Args) => {
            for argument in Args {
                collect_aggregate_nodes(argument, nodes);
            }
        }
        ExprKind::Binary { L, R, .. } | ExprKind::CompareSubquery { L, R, .. } => {
            collect_aggregate_nodes(L, nodes);
            collect_aggregate_nodes(R, nodes);
        }
        ExprKind::Unary { V, .. }
        | ExprKind::IsTruth { Expr: V, .. }
        | ExprKind::IsNull { Expr: V, .. }
        | ExprKind::Collate { Expr: V, .. }
        | ExprKind::Parentheses(V)
        | ExprKind::Cast { Expr: V, .. }
        | ExprKind::JSONSumCrc32 { Expr: V, .. } => collect_aggregate_nodes(V, nodes),
        ExprKind::InList { Expr, List, .. } => {
            collect_aggregate_nodes(Expr, nodes);
            for item in List {
                collect_aggregate_nodes(item, nodes);
            }
        }
        ExprKind::Between {
            Expr, Left, Right, ..
        } => {
            collect_aggregate_nodes(Expr, nodes);
            collect_aggregate_nodes(Left, nodes);
            collect_aggregate_nodes(Right, nodes);
        }
        ExprKind::Like { Expr, Pattern, .. } | ExprKind::Regexp { Expr, Pattern, .. } => {
            collect_aggregate_nodes(Expr, nodes);
            collect_aggregate_nodes(Pattern, nodes);
        }
        ExprKind::Case {
            Value,
            WhenClauses,
            ElseClause,
        } => {
            if let Some(value) = Value {
                collect_aggregate_nodes(value, nodes);
            }
            for clause in WhenClauses {
                collect_aggregate_nodes(&clause.Expr, nodes);
                collect_aggregate_nodes(&clause.Result, nodes);
            }
            if let Some(value) = ElseClause {
                collect_aggregate_nodes(value, nodes);
            }
        }
        ExprKind::Variable { Value, .. } => {
            if let Some(value) = Value {
                collect_aggregate_nodes(value, nodes);
            }
        }
        ExprKind::InSubquery { Expr, .. } => collect_aggregate_nodes(Expr, nodes),
        ExprKind::MatchAgainst { Against, .. } => collect_aggregate_nodes(Against, nodes),
        ExprKind::WindowFunction { Args, .. } => {
            for argument in Args {
                collect_aggregate_nodes(argument, nodes);
            }
        }
        ExprKind::Value(_)
        | ExprKind::IntroducedValue { .. }
        | ExprKind::Column(_)
        | ExprKind::NamedDefault(_)
        | ExprKind::MaxValue
        | ExprKind::TimeUnit(_)
        | ExprKind::GetFormatSelector(_)
        | ExprKind::TrimDirection(_)
        | ExprKind::TableName(_)
        | ExprKind::ParamMarker { .. }
        | ExprKind::DefaultValue
        | ExprKind::Subquery { .. }
        | ExprKind::ExistsSubquery { .. } => {}
    }
}

/// SELECT 语句中的聚合节点。
fn select_aggregate_nodes(select: &crate::ast::SelectStmt) -> Vec<&crate::ast::ExprNode> {
    let mut nodes = Vec::new();
    for field in &select.Fields.Fields {
        if let Some(expression) = &field.Expr {
            collect_aggregate_nodes(expression, &mut nodes);
        }
    }
    if let Some(having) = &select.Having {
        collect_aggregate_nodes(having, &mut nodes);
    }
    for item in &select.OrderBy {
        collect_aggregate_nodes(&item.Expr, &mut nodes);
    }
    nodes
}

/// HAVING 中聚合外的列引用。
fn having_columns_outside_aggregates(having: &crate::ast::ExprNode) -> Vec<crate::ast::ColumnName> {
    #[derive(Default)]
    struct Collector {
        columns: Vec<crate::ast::ColumnName>,
    }

    impl crate::ast::ExprNodeVisitor for Collector {
        fn Enter(&mut self, input: &crate::ast::ExprNode) -> (crate::ast::ExprNode, bool) {
            match &input.Kind {
                crate::ast::ExprKind::AggregateFunction { .. }
                | crate::ast::ExprKind::Subquery { .. }
                | crate::ast::ExprKind::ExistsSubquery { .. } => (input.clone(), true),
                crate::ast::ExprKind::Column(column) => {
                    self.columns.push(column.clone());
                    (input.clone(), true)
                }
                _ => (input.clone(), false),
            }
        }

        fn Leave(&mut self, input: &crate::ast::ExprNode) -> (crate::ast::ExprNode, bool) {
            (input.clone(), true)
        }
    }

    let mut collector = Collector::default();
    let _ = having.Accept(&mut collector);
    collector.columns
}

/// 聚合后投影。
fn build_post_aggregation_projection(
    builder: &mut PlanBuilder,
    select: &crate::ast::SelectStmt,
    mut source: logicalop::LogicalPlanRef,
    table_info: Option<&expression::model::TableInfo>,
    query_block: i32,
    aggregate_mapper: crate::expression_rewriter::AggregateMapper,
) -> Result<logicalop::LogicalPlanRef, expression::Error> {
    let visible_len = select.Fields.Fields.len();
    let mut expressions = Vec::with_capacity(select.Fields.Fields.len());
    let mut names = Vec::with_capacity(select.Fields.Fields.len());
    for field in &select.Fields.Fields {
        let ast_expression = field
            .Expr
            .as_ref()
            .ok_or_else(|| expression::errors::New("SELECT field has no expression"))?;
        let (rewritten, rewritten_source) = crate::expression_rewriter::rewrite(
            builder,
            crate::context::TODOArc(),
            ast_expression,
            source,
            aggregate_mapper.clone(),
            true,
        )?;
        source = rewritten_source;
        let expression = rewritten.ok_or_else(|| {
            expression::errors::New("aggregate SELECT expression did not produce a scalar value")
        })?;
        names.push(select_field_output_name(
            field,
            expression.as_ref(),
            source.as_ref(),
        ));
        expressions.push(expression);
    }
    if let Some(having) = &select.Having {
        // Go's pre-HAVING projection carries auxiliary aggregation outputs and
        // grouped columns as hidden fields.  They remain addressable while the
        // HAVING expression is rewritten, then column pruning removes the
        // unused ones.  Without these fields, HAVING-only aggregates and raw
        // grouped columns disappear behind the visible SELECT projection.
        let mut having_aggregates = Vec::new();
        collect_aggregate_nodes(having, &mut having_aggregates);
        let required_aggregate_indexes = having_aggregates
            .into_iter()
            .filter_map(|node| {
                aggregate_mapper
                    .get(&crate::expression_rewriter::AggregateMapperKey(node))
                    .copied()
            })
            .filter_map(|index| usize::try_from(index).ok())
            .collect::<std::collections::HashSet<_>>();
        let unresolved_columns = having_columns_outside_aggregates(having)
            .into_iter()
            .filter(|column| {
                !names.iter().any(|name| {
                    name.as_ref().is_some_and(|name| {
                        name.ColName.L == column.Name.L
                            && (column.Table.L.is_empty() || name.TblName.L == column.Table.L)
                            && (column.Schema.L.is_empty() || name.DBName.L == column.Schema.L)
                    })
                })
            })
            .collect::<Vec<_>>();
        for (index, column) in source.Schema().Columns.iter().enumerate() {
            if column.IsHidden {
                continue;
            }
            let referenced_by_name = source
                .OutputNames()
                .0
                .get(index)
                .and_then(Option::as_ref)
                .is_some_and(|name| {
                    unresolved_columns.iter().any(|column| {
                        name.ColName.L == column.Name.L
                            && (column.Table.L.is_empty() || name.TblName.L == column.Table.L)
                            && (column.Schema.L.is_empty() || name.DBName.L == column.Schema.L)
                    })
                });
            if !required_aggregate_indexes.contains(&index) && !referenced_by_name {
                continue;
            }
            if expressions.iter().any(|expression| {
                expression
                    .as_column()
                    .is_some_and(|existing| existing.EqualColumn(column))
            }) {
                continue;
            }
            expressions.push(Box::new(column.Clone()));
            names.push(source.OutputNames().0.get(index).cloned().flatten());
        }
    }
    let mut order_aggregates = Vec::new();
    for item in &select.OrderBy {
        collect_aggregate_nodes(&item.Expr, &mut order_aggregates);
    }
    let required_order_aggregate_indexes = order_aggregates
        .into_iter()
        .filter_map(|node| {
            aggregate_mapper
                .get(&crate::expression_rewriter::AggregateMapperKey(node))
                .copied()
        })
        .filter_map(|index| usize::try_from(index).ok())
        .collect::<std::collections::HashSet<_>>();
    for (index, column) in source.Schema().Columns.iter().enumerate() {
        if !required_order_aggregate_indexes.contains(&index) {
            continue;
        }
        expressions.push(Box::new(column.Clone()));
        names.push(source.OutputNames().0.get(index).cloned().flatten());
    }
    for item in &select.OrderBy {
        let crate::ast::ExprKind::Column(column_name) = &item.Expr.Kind else {
            continue;
        };
        let Some((index, column)) = source
            .OutputNames()
            .0
            .iter()
            .enumerate()
            .find(|(_, output)| {
                output.as_ref().is_some_and(|output| {
                    output.ColName.L == column_name.Name.L
                        && (column_name.Table.L.is_empty()
                            || output.TblName.L == column_name.Table.L)
                })
            })
            .and_then(|(index, _)| {
                source
                    .Schema()
                    .Columns
                    .get(index)
                    .cloned()
                    .map(|column| (index, column))
            })
        else {
            continue;
        };
        if expressions.iter().any(|expression| {
            expression
                .as_column()
                .is_some_and(|existing| existing.UniqueID == column.UniqueID)
        }) {
            continue;
        }
        source.Schema_mut().Columns[index].IsHidden = true;
        expressions.push(Box::new(column));
        names.push(source.OutputNames().0.get(index).cloned().flatten());
    }
    let columns = expressions
        .iter()
        .enumerate()
        .map(|(index, expression)| {
            // Go buildProjectionField returns a direct input Column unchanged,
            // so identity projections keep the aggregate output ID and can be
            // removed by the strict projection eliminator.
            if let Some(input) = expression.as_column() {
                let mut column = input.Clone();
                column.Index = index as isize;
                column.IsHidden = index >= visible_len || column.IsHidden;
                return column;
            }
            let mut column = expression::Column::new(
                expression
                    .GetType(builder.ctx.GetExprCtx().GetEvalCtx())
                    .clone(),
                0,
                builder.ctx.GetExprCtx().AllocPlanColumnID(),
                index as isize,
            );
            column.IsHidden = index >= visible_len;
            column
        })
        .collect();
    let mut projection = logicalop::LogicalProjection {
        Exprs: expressions,
        ..Default::default()
    }
    .Init(builder.ctx.clone(), query_block);
    projection.SetSchema(expression::NewSchema(columns));
    projection.SetOutputNames(expression::types::NameSlice(names));
    projection.SetChildren(vec![source]);
    builder.optFlag |= rule::FLAG_ELIMINATE_PROJECTION | rule::FLAG_BUILD_KEY_INFO;
    let _ = table_info;
    Ok(Box::new(projection))
}

/// 构建 SELECT 聚合计划。
fn build_select_aggregation(
    builder: &mut PlanBuilder,
    select: &crate::ast::SelectStmt,
    mut source: logicalop::LogicalPlanRef,
    table_info: Option<&expression::model::TableInfo>,
    query_block: i32,
) -> Result<logicalop::LogicalPlanRef, expression::Error> {
    let mut group_by = Vec::with_capacity(select.GroupBy.len());
    for item in &select.GroupBy {
        let (expression, rewritten_source) = crate::expression_rewriter::rewrite(
            builder,
            crate::context::TODOArc(),
            &item.Expr,
            source,
            crate::expression_rewriter::AggregateMapper::default(),
            true,
        )?;
        source = rewritten_source;
        group_by.push(expression.ok_or_else(|| {
            expression::errors::New("GROUP BY expression did not produce a scalar value")
        })?);
    }
    let aggregate_nodes = select_aggregate_nodes(select);
    let mut functions = Vec::with_capacity(aggregate_nodes.len() + group_by.len());
    let mut output_names = Vec::with_capacity(aggregate_nodes.len() + group_by.len());
    for ast_expression in aggregate_nodes {
        let crate::ast::ExprKind::AggregateFunction {
            Name,
            Args,
            Distinct,
            Order,
        } = &ast_expression.Kind
        else {
            continue;
        };
        let count_star = Name.eq_ignore_ascii_case(crate::ast::AggFuncCount)
            && matches!(
                Args.as_slice(),
                [crate::ast::ExprNode {
                    Kind: crate::ast::ExprKind::Value(crate::ast::ValueExpr {
                        Datum: crate::ast::ValueDatum::String(value),
                        ..
                    }),
                    ..
                }] if value == "1"
            );
        let arguments = if Args.is_empty() || count_star {
            let mut field_type = expression::types::NewFieldType(expression::mysql::TypeLonglong);
            field_type.SetFlen(expression::mysql::MaxIntWidth as isize);
            field_type.SetDecimal(0);
            vec![Box::new(expression::Constant::with_type(
                expression::types::NewIntDatum(1),
                *field_type,
            )) as expression::ExprBox]
        } else {
            // Go buildAggregation rewrites each argument against the current
            // plan so nested aggregate arguments can resolve outer scopes.
            let mut rewritten_arguments = Vec::with_capacity(Args.len());
            for argument in Args {
                let (expression, rewritten_source) = crate::expression_rewriter::rewrite(
                    builder,
                    crate::context::TODOArc(),
                    argument,
                    source,
                    crate::expression_rewriter::AggregateMapper::default(),
                    true,
                )?;
                source = rewritten_source;
                rewritten_arguments.push(expression.ok_or_else(|| {
                    expression::errors::New("aggregate argument did not produce a scalar value")
                })?);
            }
            rewritten_arguments
        };
        let mut function =
            aggregation::NewAggFuncDesc(builder.ctx.GetExprCtx(), Name, arguments, *Distinct)?;
        function.OrderByItems = Order
            .iter()
            .map(|item| {
                Ok(planner_util_dependency::ByItems {
                    Expr: rewrite_for_plan(builder, &item.Expr, source.as_ref(), table_info)?,
                    Desc: item.Desc,
                })
            })
            .collect::<Result<Vec<_>, expression::Error>>()?;
        functions.push(function);
        let alias = select.Fields.Fields.iter().find_map(|field| {
            field.Expr.as_ref().and_then(|field_expression| {
                std::ptr::eq(field_expression, ast_expression)
                    .then(|| {
                        (!field.AsName.O.is_empty()).then(|| {
                            Arc::new(expression::types::FieldName {
                                ColName: field.AsName.clone(),
                                ..Default::default()
                            })
                        })
                    })
                    .flatten()
            })
        });
        output_names.push(alias.or_else(|| Some(Arc::clone(&expression::types::EmptyName))));
    }
    // TiDB appends firstrow for every input schema column and lets column
    // pruning remove the unused ones.  This is deliberately independent of
    // the GROUP BY expressions: a MySQL-compatible query may project `a`
    // while grouping by `a + 1`, and the post-aggregation projection still
    // needs an output carrying `a`.
    let input_first_rows = source
        .Schema()
        .Columns
        .iter()
        .enumerate()
        .map(|(index, column)| {
            let input = Box::new(column.Clone()) as expression::ExprBox;
            (
                resolve_group_input_expression(&input, source.as_ref()),
                source.OutputNames().0.get(index).cloned().flatten(),
                column.Clone(),
            )
        })
        .collect::<Vec<_>>();
    for function in &mut functions {
        for argument in &mut function.Args {
            *argument = resolve_group_input_expression(argument, source.as_ref());
        }
        for item in &mut function.OrderByItems {
            item.Expr = resolve_group_input_expression(&item.Expr, source.as_ref());
        }
    }
    for item in &mut group_by {
        *item = resolve_projection_input_expression(item, source.as_ref());
    }
    let scalar_constant_aggregate = group_by.is_empty()
        && functions.iter().all(|function| {
            function
                .Args
                .iter()
                .all(|argument| expression::ExtractColumns(argument.as_ref()).is_empty())
        });
    if scalar_constant_aggregate && source.as_any().is::<logicalop::LogicalProjection>() {
        // A scalar aggregate over constants does not consume the derived
        // projection values.  Go eliminates that projection and aggregates
        // directly over its cardinality-producing child (for example LIMIT).
        source = source.TakeChildren().remove(0);
    } else if scalar_constant_aggregate
        && source.as_any().is::<logicalop::LogicalLimit>()
        && source
            .Children()
            .first()
            .is_some_and(|child| child.as_any().is::<logicalop::LogicalProjection>())
    {
        // A derived SELECT applies LIMIT after its projection.  Preserve the
        // cardinality boundary while bypassing the unused constant projection.
        let mut projection = source.TakeChildren().remove(0);
        source.SetChildren(vec![projection.TakeChildren().remove(0)]);
    }
    let explicit_function_count = functions.len();
    let first_row_output_columns = input_first_rows
        .iter()
        .map(|(_, _, column)| column.Clone())
        .collect::<Vec<_>>();
    for (first_row_arg, name, _) in input_first_rows {
        functions.push(aggregation::NewAggFuncDesc(
            builder.ctx.GetExprCtx(),
            crate::ast::AggFuncFirstRow,
            vec![first_row_arg],
            false,
        )?);
        output_names.push(name);
    }
    let columns = functions
        .iter()
        .enumerate()
        .map(|(index, function)| {
            // Go clones the input column for the synthetic FIRST_ROW entries
            // appended for every child column. Retaining its UniqueID is what
            // carries PK/UK information through grouped derived tables.
            if let Some(input) = index
                .checked_sub(explicit_function_count)
                .and_then(|offset| first_row_output_columns.get(offset))
            {
                let mut column = input.Clone();
                column.RetType = function.RetTp.clone();
                column.Index = index as isize;
                return column;
            }
            let mut column = expression::Column::new(
                function.RetTp.clone().unwrap_or_else(|| {
                    *expression::types::NewFieldType(expression::mysql::TypeLonglong)
                }),
                0,
                builder.ctx.GetExprCtx().AllocPlanColumnID(),
                index as isize,
            );
            column
        })
        .collect::<Vec<_>>();
    let mut aggregate = logicalop::LogicalAggregation {
        AggFuncs: functions,
        GroupByItems: group_by,
        ..Default::default()
    }
    .Init(builder.ctx.clone(), query_block);
    if let Some(hints) = builder.tableHintInfo.last() {
        aggregate.PreferAggType = u64::from(hints.PreferAggType);
        aggregate.PreferAggToCop = hints.PreferAggToCop;
    }
    aggregate.SetSchema(expression::NewSchema(columns));
    aggregate.SetOutputNames(expression::types::NameSlice(output_names));
    aggregate.SetChildren(vec![source]);
    // Go buildAggregation enables the complete rule dependency set: max/min
    // elimination may introduce TopN and IS NOT NULL, and ordinary aggregate
    // elimination needs key/projection information.
    builder.optFlag |= rule::FLAG_BUILD_KEY_INFO
        | rule::FLAG_PUSH_DOWN_AGG
        | rule::FLAG_MAX_MIN_ELIMINATE
        | rule::FLAG_PUSH_DOWN_TOP_N
        | rule::FLAG_PREDICATE_PUSH_DOWN
        | rule::FLAG_ELIMINATE_AGG
        | rule::FLAG_ELIMINATE_PROJECTION;
    Ok(Box::new(aggregate))
}

/// 构建 SELECT 的 FROM 源。
fn build_select_source(
    builder: &mut PlanBuilder,
    ctx: &dyn crate::context::Context,
    select: &crate::ast::SelectStmt,
    query_block: i32,
    ctes: &mut CteEnvironment,
) -> Result<
    (
        logicalop::LogicalPlanRef,
        Option<expression::model::TableInfo>,
    ),
    expression::Error,
> {
    let Some(from) = &select.From else {
        let mut dual = logicalop::LogicalTableDual {
            RowCount: 1,
            ..Default::default()
        }
        .Init(builder.ctx.clone(), query_block);
        dual.SetSchema(expression::NewSchema(Vec::new()));
        dual.SetOutputNames(expression::types::NameSlice(Vec::new()));
        return Ok((Box::new(dual), None));
    };

    build_join_runtime(builder, ctx, select, &from.TableRefs, query_block, 0, ctes)
}

/// AST 结果集是否含 LATERAL。
fn contains_lateral_table_source(node: &crate::ast::ResultSetNode) -> bool {
    match node {
        crate::ast::ResultSetNode::TableSource(source) => source.Lateral,
        crate::ast::ResultSetNode::Join(join) => {
            join.Left
                .as_deref()
                .is_some_and(contains_lateral_table_source)
                || join
                    .Right
                    .as_deref()
                    .is_some_and(contains_lateral_table_source)
        }
    }
}

/// 查找 Join 完整 Schema。
fn find_join_full_schema(
    mut plan: &dyn logicalop::LogicalPlan,
) -> Option<(expression::Schema, expression::types::NameSlice)> {
    loop {
        if let Some(join) = plan.as_any().downcast_ref::<logicalop::LogicalJoin>() {
            return join
                .FullSchema
                .as_ref()
                .map(|schema| (schema.Clone(), join.FullNames.Shallow()));
        }
        if let Some(apply) = plan.as_any().downcast_ref::<logicalop::LogicalApply>() {
            return apply
                .LogicalJoin
                .FullSchema
                .as_ref()
                .map(|schema| (schema.Clone(), apply.LogicalJoin.FullNames.Shallow()));
        }
        if plan.as_any().is::<logicalop::LogicalSelection>() && plan.Children().len() == 1 {
            plan = plan.Children()[0].as_ref();
            continue;
        }
        return None;
    }
}

/// 节点是否为立即 LATERAL 表源。
fn is_immediate_lateral_table_source(node: &crate::ast::ResultSetNode) -> bool {
    match node {
        crate::ast::ResultSetNode::TableSource(source) => source.Lateral,
        crate::ast::ResultSetNode::Join(join) if join.Right.is_none() => join
            .Left
            .as_deref()
            .is_some_and(is_immediate_lateral_table_source),
        crate::ast::ResultSetNode::Join(_) => false,
    }
}

/// 运行时按 AST 结果集节点分派。
fn build_result_set_runtime(
    builder: &mut PlanBuilder,
    ctx: &dyn crate::context::Context,
    select: &crate::ast::SelectStmt,
    node: &crate::ast::ResultSetNode,
    query_block: i32,
    lateral_outer_count: usize,
    ctes: &mut CteEnvironment,
) -> Result<
    (
        logicalop::LogicalPlanRef,
        Option<expression::model::TableInfo>,
    ),
    expression::Error,
> {
    match node {
        crate::ast::ResultSetNode::TableSource(source) => build_table_source_runtime(
            builder,
            ctx,
            select,
            source,
            query_block,
            lateral_outer_count,
            ctes,
        ),
        crate::ast::ResultSetNode::Join(join) => build_join_runtime(
            builder,
            ctx,
            select,
            join,
            query_block,
            lateral_outer_count,
            ctes,
        ),
    }
}

/// 运行时构建 Join / LATERAL Apply。
fn build_join_runtime(
    builder: &mut PlanBuilder,
    ctx: &dyn crate::context::Context,
    select: &crate::ast::SelectStmt,
    join: &crate::ast::Join,
    query_block: i32,
    lateral_outer_count: usize,
    ctes: &mut CteEnvironment,
) -> Result<
    (
        logicalop::LogicalPlanRef,
        Option<expression::model::TableInfo>,
    ),
    expression::Error,
> {
    let left_node = join
        .Left
        .as_deref()
        .ok_or_else(|| expression::errors::New("JOIN has no left result set"))?;
    let (left, left_table) = build_result_set_runtime(
        builder,
        ctx,
        select,
        left_node,
        query_block,
        lateral_outer_count,
        ctes,
    )?;
    let Some(right_node) = join.Right.as_deref() else {
        return Ok((left, left_table));
    };
    let has_lateral = contains_lateral_table_source(right_node);
    if join.Tp == crate::ast::JoinType::FullJoin {
        let session_vars = builder.ctx.GetSessionVars();
        if !session_vars.EnableFullOuterJoin {
            return Err(plannererrors::ErrNotSupportedYet
                .GenWithStackByArgs(&["FULL OUTER JOIN".into()])
                .into());
        }
        if session_vars
            .GetSystemVar(vardef_dependency::TiDBEnableCascadesPlanner)
            .is_some_and(|value| matches!(value.to_ascii_lowercase().as_str(), "on" | "1" | "true"))
        {
            return Err(plannererrors::ErrNotSupportedYet
                .GenWithStackByArgs(&["FULL OUTER JOIN with cascades planner".into()])
                .into());
        }
        if join.NaturalJoin || !join.Using.is_empty() || join.On.is_none() || has_lateral {
            return Err(plannererrors::ErrNotSupportedYet
                .GenWithStackByArgs(&["FULL OUTER JOIN".into()])
                .into());
        }
    }
    let lateral_outer = find_join_full_schema(left.as_ref())
        .unwrap_or_else(|| (left.Schema().Clone(), left.OutputNames().Shallow()));
    if has_lateral {
        builder.outerSchemas.push(lateral_outer.0.Clone());
        builder.outerNames.push(lateral_outer.1.Shallow());
    }
    let right_result = build_result_set_runtime(
        builder,
        ctx,
        select,
        right_node,
        query_block,
        lateral_outer_count + usize::from(has_lateral),
        ctes,
    );
    if has_lateral {
        builder.outerSchemas.pop();
        builder.outerNames.pop();
    }
    let (right, _) = right_result?;
    let correlated_to_left = has_lateral
        && !coreusage::ExtractCorColumnsBySchema4LogicalPlan(right.as_ref(), &lateral_outer.0)
            .is_empty();
    if has_lateral && (is_immediate_lateral_table_source(right_node) || correlated_to_left) {
        return build_lateral_join_runtime(builder, join, left, right, query_block);
    }
    let left_full = find_join_full_schema(left.as_ref())
        .unwrap_or_else(|| (left.Schema().Clone(), left.OutputNames().Shallow()));
    let right_full = find_join_full_schema(right.as_ref())
        .unwrap_or_else(|| (right.Schema().Clone(), right.OutputNames().Shallow()));
    let mut full_schema = merged_schema(&left_full.0, &right_full.0);
    let left_full_len = left_full.0.Len();
    let mut full_names = left_full.1.Shallow();
    full_names.0.extend(right_full.1.0.iter().cloned());
    let left_len = left.Schema().Len();
    let mut schema = merged_schema(left.Schema(), right.Schema());
    let join_type = match join.Tp {
        crate::ast::JoinType::CrossJoin => base::JoinType::InnerJoin,
        crate::ast::JoinType::LeftJoin => {
            builder.optFlag |= rule::FLAG_ELIMINATE_OUTER_JOIN | rule::FLAG_OUTER_JOIN_TO_SEMI_JOIN;
            reset_not_null(&mut schema, left_len);
            reset_not_null(&mut full_schema, left_full_len);
            base::JoinType::LeftOuterJoin
        }
        crate::ast::JoinType::RightJoin => {
            builder.optFlag |= rule::FLAG_ELIMINATE_OUTER_JOIN | rule::FLAG_OUTER_JOIN_TO_SEMI_JOIN;
            reset_not_null(&mut schema, 0);
            reset_not_null(&mut full_schema, 0);
            base::JoinType::RightOuterJoin
        }
        crate::ast::JoinType::FullJoin => {
            builder.optFlag |= rule::FLAG_ELIMINATE_OUTER_JOIN;
            reset_not_null(&mut schema, 0);
            reset_not_null(&mut full_schema, 0);
            base::JoinType::FullOuterJoin
        }
    };
    let mut names = left.OutputNames().Shallow();
    names.0.extend(right.OutputNames().0.iter().cloned());
    let mut logical_join = logicalop::LogicalJoin {
        JoinType: join_type,
        StraightJoin: join.StraightJoin || builder.inStraightJoin,
        FullSchema: Some(full_schema),
        FullNames: full_names,
        ..Default::default()
    }
    .Init(builder.ctx.clone(), query_block);
    logical_join.SetSchema(schema);
    logical_join.SetOutputNames(names);
    logical_join.SetChildren(vec![left, right]);
    let (mut prefer, order) = builder.joinHintPreferenceFor(
        logical_join.Children()[0].as_ref(),
        logical_join.Children()[1].as_ref(),
    );
    if join_type == base::JoinType::FullOuterJoin {
        fn hinted_tables(plan: &dyn logicalop::LogicalPlan) -> Vec<hint::HintedTable> {
            if let Some(source) = plan.as_any().downcast_ref::<logicalop::DataSource>() {
                return vec![hint::HintedTable {
                    DBName: source.DBName.clone(),
                    TblName: source
                        .TableAsName
                        .as_ref()
                        .unwrap_or(&source.TableInfo.Name)
                        .clone(),
                    SelectOffset: source.QueryBlockOffset(),
                    ..Default::default()
                }];
            }
            plan.Children()
                .iter()
                .flat_map(|child| hinted_tables(child.as_ref()))
                .collect()
        }
        if let Some(hints) = builder.tableHintInfo.last_mut() {
            for (side, build_bit, probe_bit) in [
                (0, hint::PreferLeftAsHJBuild, hint::PreferLeftAsHJProbe),
                (1, hint::PreferRightAsHJBuild, hint::PreferRightAsHJProbe),
            ] {
                let tables = hinted_tables(logical_join.Children()[side].as_ref());
                if hints.IfPreferHJBuild(tables.clone()) {
                    prefer |= u64::from(build_bit);
                }
                if hints.IfPreferHJProbe(tables) {
                    prefer |= u64::from(probe_bit);
                }
            }
        }
        for (bit, name) in [(1 << 1, "MERGE_JOIN"), (1 << 2, "INL_JOIN")] {
            if prefer & bit != 0 {
                builder.ctx.GetSessionVars().StmtCtx.AppendWarning(
                    stmtctx_dependency::errors::NewNoStackError(format!(
                        "Optimizer Hint {name} is inapplicable to FULL OUTER JOIN"
                    )),
                );
            }
        }
    }
    logical_join.SetPreferredJoinTypeAndOrder(
        if join_type == base::JoinType::FullOuterJoin {
            prefer & 1
        } else {
            prefer
        },
        order,
    );
    if join_type == base::JoinType::FullOuterJoin {
        // The shared algorithm-hint setter accepts only method bits. Preserve
        // full-join build/probe preferences for the canonical hash enumerator.
        logical_join.PreferJoinType |= prefer
            & u64::from(
                hint::PreferLeftAsHJBuild
                    | hint::PreferRightAsHJBuild
                    | hint::PreferLeftAsHJProbe
                    | hint::PreferRightAsHJProbe,
            );
    }
    let (left_prefer, right_prefer) = builder.joinHintSidePreference(
        logical_join.Children()[0].as_ref(),
        logical_join.Children()[1].as_ref(),
    );
    logical_join.LeftPreferJoinType = left_prefer;
    logical_join.RightPreferJoinType = right_prefer;

    let mut using_names = join
        .Using
        .iter()
        .map(|column| column.Name.L.clone())
        .collect::<Vec<_>>();
    if join.NaturalJoin {
        for (left_index, left_name) in logical_join.Children()[0]
            .OutputNames()
            .0
            .iter()
            .enumerate()
            .filter_map(|(index, name)| name.as_ref().map(|name| (index, name)))
        {
            let name = &left_name.ColName.L;
            if !name.is_empty()
                && name != &expression::model::ExtraHandleName.L
                && name != &expression::model::ExtraCommitTSName.L
                && name != &expression::model::ExtraPhysTblIDName.L
                && !logical_join.Children()[0].Schema().Columns[left_index].IsHidden
                && logical_join.Children()[1]
                    .OutputNames()
                    .0
                    .iter()
                    .enumerate()
                    .any(|(right_index, right_name)| {
                        right_name
                            .as_ref()
                            .is_some_and(|right_name| right_name.ColName.L == *name)
                            && !logical_join.Children()[1].Schema().Columns[right_index].IsHidden
                    })
                && !using_names.contains(name)
            {
                using_names.push(name.clone());
            }
        }
    }
    let mut conditions = if using_names.is_empty() {
        Vec::new()
    } else {
        coalesce_common_columns(builder, &mut logical_join, &using_names, join.Tp)?
    };
    if let Some(on) = &join.On {
        let (on_condition, rewritten) = crate::expression_rewriter::rewrite(
            builder,
            crate::context::TODOArc(),
            on,
            Box::new(logical_join),
            crate::expression_rewriter::AggregateMapper::default(),
            false,
        )?;
        if let Some(on_condition) = on_condition {
            conditions.extend(expression::SplitCNFItems(on_condition.as_ref()));
        }
        let mut rewritten = rewritten;
        let join_type = rewritten
            .as_any()
            .downcast_ref::<logicalop::LogicalJoin>()
            .ok_or_else(|| expression::errors::New("JOIN ON rewrite changed the join root"))?
            .JoinType;
        if join_type == base::JoinType::InnerJoin {
            // Go keeps an inner JOIN's ON clause as a Selection first.  PPD then
            // classifies its predicates into join/access conditions; the real
            // node lifecycle also preserves the plan-ID sequence used by tests.
            let mut selection = logicalop::LogicalSelection {
                Conditions: conditions,
                ..Default::default()
            }
            .Init(builder.ctx.clone(), query_block);
            selection.SetSchema(rewritten.Schema().Clone());
            selection.SetOutputNames(rewritten.OutputNames().Shallow());
            selection.SetChildren(vec![rewritten]);
            builder.optFlag |= rule::FLAG_PREDICATE_PUSH_DOWN
                | rule::FLAG_BUILD_KEY_INFO
                | rule::FLAG_JOIN_REORDER;
            return Ok((Box::new(selection), None));
        }
        rewritten
            .as_any_mut()
            .downcast_mut::<logicalop::LogicalJoin>()
            .expect("JOIN root checked above")
            .AttachOnConds(conditions);
        builder.optFlag |=
            rule::FLAG_PREDICATE_PUSH_DOWN | rule::FLAG_BUILD_KEY_INFO | rule::FLAG_JOIN_REORDER;
        return Ok((rewritten, None));
    }
    if using_names.is_empty() {
        logical_join.AttachOnConds(conditions);
    } else {
        // Go's coalesceCommonColumns deliberately keeps USING/NATURAL
        // predicates in OtherConditions.  They describe redundant output
        // columns and must not be promoted to ordinary hash-join keys here.
        logical_join.OtherConditions.extend(conditions);
    }
    builder.optFlag |=
        rule::FLAG_PREDICATE_PUSH_DOWN | rule::FLAG_BUILD_KEY_INFO | rule::FLAG_JOIN_REORDER;
    Ok((Box::new(logical_join), None))
}

/// 运行时构建 LATERAL 为 Apply。
fn build_lateral_join_runtime(
    builder: &mut PlanBuilder,
    join: &crate::ast::Join,
    left: logicalop::LogicalPlanRef,
    right: logicalop::LogicalPlanRef,
    query_block: i32,
) -> Result<
    (
        logicalop::LogicalPlanRef,
        Option<expression::model::TableInfo>,
    ),
    expression::Error,
> {
    if join.NaturalJoin || !join.Using.is_empty() {
        return Err(expression::errors::New(
            "NATURAL JOIN and USING are not supported with LATERAL",
        ));
    }
    let join_type = match join.Tp {
        crate::ast::JoinType::LeftJoin => {
            builder.optFlag |= rule::FLAG_ELIMINATE_OUTER_JOIN | rule::FLAG_OUTER_JOIN_TO_SEMI_JOIN;
            base::JoinType::LeftOuterJoin
        }
        crate::ast::JoinType::RightJoin => {
            return Err(expression::errors::New(
                "RIGHT JOIN is not supported with LATERAL",
            ));
        }
        crate::ast::JoinType::FullJoin => {
            return Err(expression::errors::New(
                "FULL JOIN is not supported with LATERAL",
            ));
        }
        crate::ast::JoinType::CrossJoin => base::JoinType::InnerJoin,
    };
    let left_full = find_join_full_schema(left.as_ref())
        .unwrap_or_else(|| (left.Schema().Clone(), left.OutputNames().Shallow()));
    let right_full = find_join_full_schema(right.as_ref())
        .unwrap_or_else(|| (right.Schema().Clone(), right.OutputNames().Shallow()));
    let correlated = coreusage::ExtractCorColumnsBySchema4LogicalPlan(right.as_ref(), &left_full.0);
    let mut names = left.OutputNames().Shallow();
    names.0.extend(right.OutputNames().0.iter().cloned());
    let mut visible_schema = merged_schema(left.Schema(), right.Schema());
    let mut full_schema = merged_schema(&left_full.0, &right_full.0);
    if join_type == base::JoinType::LeftOuterJoin {
        reset_not_null(&mut visible_schema, left.Schema().Len());
        reset_not_null(&mut full_schema, left_full.0.Len());
    }
    let mut full_names = left_full.1.Shallow();
    full_names.0.extend(right_full.1.0.iter().cloned());
    let mut apply = logicalop::LogicalApply {
        LogicalJoin: logicalop::LogicalJoin {
            JoinType: join_type,
            FullSchema: Some(full_schema.Clone()),
            FullNames: full_names,
            ..Default::default()
        },
        CorCols: correlated,
        NoDecorrelate: false,
        IsLateral: true,
        PrunedToLeft: false,
    }
    .Init(builder.ctx.clone(), query_block);
    apply.SetSchema(visible_schema);
    apply.SetOutputNames(names);
    apply.SetChildren(vec![left, right]);
    if let Some(on) = &join.On {
        let (condition, rewritten) = crate::expression_rewriter::rewrite(
            builder,
            crate::context::TODOArc(),
            on,
            Box::new(apply),
            crate::expression_rewriter::AggregateMapper::default(),
            false,
        )?;
        let mut rewritten = rewritten;
        let apply = rewritten
            .as_any_mut()
            .downcast_mut::<logicalop::LogicalApply>()
            .ok_or_else(|| expression::errors::New("LATERAL ON rewrite changed Apply root"))?;
        if let Some(condition) = condition {
            apply
                .LogicalJoin
                .AttachOnConds(expression::SplitCNFItems(condition.as_ref()));
        }
        builder.optFlag |= rule::FLAG_PREDICATE_PUSH_DOWN
            | rule::FLAG_BUILD_KEY_INFO
            | rule::FLAG_DECORRELATE
            | rule::FLAG_CONSTANT_PROPAGATION;
        return Ok((rewritten, None));
    }
    builder.optFlag |= rule::FLAG_PREDICATE_PUSH_DOWN
        | rule::FLAG_BUILD_KEY_INFO
        | rule::FLAG_DECORRELATE
        | rule::FLAG_CONSTANT_PROPAGATION;
    Ok((Box::new(apply), None))
}

/// 合并 NATURAL/USING 公共列。
fn coalesce_common_columns(
    builder: &PlanBuilder,
    join: &mut logicalop::LogicalJoin,
    using_names: &[String],
    join_type: crate::ast::JoinType,
) -> Result<Vec<expression::ExprBox>, expression::Error> {
    let [left, right] = join.Children() else {
        return Ok(Vec::new());
    };
    let left_schema = left.Schema().Clone();
    let right_schema = right.Schema().Clone();
    let left_names = left.OutputNames().Shallow();
    let right_names = right.OutputNames().Shallow();
    let full_schema = merged_schema(&left_schema, &right_schema);
    let mut full_names = left_names.Shallow();
    full_names.0.extend(right_names.0.iter().cloned());

    let locate = |names: &expression::types::NameSlice,
                  columns: &[expression::Column],
                  wanted: &str|
     -> Result<usize, expression::Error> {
        let matches = names
            .0
            .iter()
            .enumerate()
            .filter(|(index, name)| {
                !columns[*index].IsHidden
                    && name.as_ref().is_some_and(|name| name.ColName.L == wanted)
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [index] => Ok(*index),
            [] => Err(expression::errors::New(format!(
                "unknown column {wanted} in USING"
            ))),
            _ => Err(expression::errors::New(format!(
                "column {wanted} in USING is ambiguous"
            ))),
        }
    };

    let mut pairs = Vec::with_capacity(using_names.len());
    for name in using_names {
        pairs.push((
            locate(&left_names, &left_schema.Columns, name)?,
            locate(&right_names, &right_schema.Columns, name)?,
        ));
    }

    let right_is_canonical = matches!(join_type, crate::ast::JoinType::RightJoin);
    let mut output_columns = Vec::with_capacity(full_schema.Len() - pairs.len());
    let mut output_names =
        expression::types::NameSlice(Vec::with_capacity(full_names.0.len() - pairs.len()));
    let mut conditions = Vec::with_capacity(pairs.len());

    for (left_index, right_index) in &pairs {
        let left_column = left_schema.Columns[*left_index].clone();
        let right_column = right_schema.Columns[*right_index].clone();
        let (output_column, output_name, redundant) = if right_is_canonical {
            (
                right_column.clone(),
                right_names.0[*right_index].clone(),
                left_column.UniqueID,
            )
        } else {
            (
                left_column.clone(),
                left_names.0[*left_index].clone(),
                right_column.UniqueID,
            )
        };
        let output_index = output_columns.len();
        output_columns.push(output_column);
        output_names.0.push(output_name);
        join.RegisterRedundantColumnMapping(redundant, output_index);
        conditions.push(expression::NewFunction(
            builder.ctx.GetExprCtx(),
            "eq",
            *expression::types::NewFieldType(expression::mysql::TypeTiny),
            vec![Box::new(left_column), Box::new(right_column)],
        )?);
    }

    for (index, column) in left_schema.Columns.iter().enumerate() {
        if !pairs.iter().any(|(left_index, _)| *left_index == index) {
            output_columns.push(column.clone());
            output_names.0.push(left_names.0[index].clone());
        }
    }
    for (index, column) in right_schema.Columns.iter().enumerate() {
        if !pairs.iter().any(|(_, right_index)| *right_index == index) {
            output_columns.push(column.clone());
            output_names.0.push(right_names.0[index].clone());
        }
    }

    join.FullSchema = Some(full_schema);
    join.FullNames = full_names;
    join.SetSchema(expression::NewSchema(output_columns));
    join.SetOutputNames(output_names);
    Ok(conditions)
}

/// 运行时构建表数据源（含索引路径与 hint）。
fn build_table_source_runtime(
    builder: &mut PlanBuilder,
    ctx: &dyn crate::context::Context,
    select: &crate::ast::SelectStmt,
    table_source: &crate::ast::TableSource,
    query_block: i32,
    lateral_outer_count: usize,
    ctes: &mut CteEnvironment,
) -> Result<
    (
        logicalop::LogicalPlanRef,
        Option<expression::model::TableInfo>,
    ),
    expression::Error,
> {
    if let Some(query_source) = &table_source.QuerySource {
        builder.handleHelper.pushMap();
        let hidden_schemas = if !table_source.Lateral && lateral_outer_count > 0 {
            builder
                .outerSchemas
                .split_off(builder.outerSchemas.len() - lateral_outer_count)
        } else {
            Vec::new()
        };
        let hidden_names = if !table_source.Lateral && lateral_outer_count > 0 {
            builder
                .outerNames
                .split_off(builder.outerNames.len() - lateral_outer_count)
        } else {
            Vec::new()
        };
        let result = query_source
            .with_node(|node| build_query_node_with_ctes(builder, ctx, node, ctes))
            .ok_or_else(|| expression::errors::New("derived table AST was consumed"))?;
        builder.outerSchemas.extend(hidden_schemas);
        builder.outerNames.extend(hidden_names);
        builder.handleHelper.popMap();
        let mut plan = result?;
        let mut names = plan.OutputNames().Shallow();
        for (index, name) in names.0.iter_mut().enumerate() {
            let mut field = name.as_ref().map(|name| name.Clone()).unwrap_or_default();
            field.DBName = crate::ast::CIStr::default();
            field.TblName = table_source.AsName.clone();
            field.OrigTblName = table_source.AsName.clone();
            if let Some(column_name) = table_source.ColumnNames.get(index) {
                field.ColName = column_name.clone();
                field.OrigColName = column_name.clone();
            }
            *name = Some(Arc::new(field));
        }
        plan.SetOutputNames(names);
        return Ok((plan, None));
    }
    if table_source.Lateral {
        return Err(expression::errors::New(
            "lateral tables require an Apply plan",
        ));
    }

    if table_source.Source.Schema.O.is_empty()
        && let Some(binding) = ctes.get(&table_source.Source.Name.L).cloned()
    {
        let effective_name = if table_source.AsName.O.is_empty() {
            binding.name.clone()
        } else {
            table_source.AsName.clone()
        };
        let mut names = binding.names.Shallow();
        for name in names.0.iter_mut().flatten() {
            let mut field = name.Clone();
            field.DBName = crate::ast::CIStr::default();
            field.TblName = effective_name.clone();
            field.OrigTblName = binding.name.clone();
            *name = Arc::new(field);
        }
        let schema = cte_result_schema(builder, &binding.schema);
        // Go's LogicalCTE keeps a per-reference map from the visible CTE
        // columns back to the seed columns.  Predicate push-down receives the
        // former and the seed optimizer must see the latter.
        {
            let mut class = binding.class.borrow_mut();
            for (visible, seed) in schema.Columns.iter().zip(binding.schema.Columns.iter()) {
                class.ColumnMap.insert(visible.UniqueID, seed.Clone());
            }
        }
        let mut plan: logicalop::LogicalPlanRef = if binding.recursive_reference {
            Box::new(
                logicalop::LogicalCTETable {
                    SeedStat: binding.seed_stat,
                    Name: binding.name.O.clone(),
                    IDForStorage: binding.storage_id,
                    SeedSchema: binding.schema,
                    ..Default::default()
                }
                .Init(builder.ctx.clone(), query_block),
            )
        } else {
            Box::new(
                logicalop::LogicalCTE {
                    Cte: binding.class,
                    CteAsName: effective_name,
                    CteName: binding.name,
                    SeedStat: binding.seed_stat,
                    ..Default::default()
                }
                .Init(builder.ctx.clone(), query_block),
            )
        };
        plan.SetSchema(schema);
        plan.SetOutputNames(names);
        builder.optFlag |= rule::FLAG_PRUNE_COLUMNS | rule::FLAG_BUILD_KEY_INFO;
        return Ok((plan, None));
    }

    let schema_name = if table_source.Source.Schema.O.is_empty() {
        builder.ctx.GetSessionVars().CurrentDB()
    } else {
        table_source.Source.Schema.O.clone()
    };
    if schema_name.is_empty() {
        return Err(expression::errors::New("No database selected"));
    }
    let schema_key = infoschema_dependency::infoschema::CiString::from(schema_name.as_str());
    let table_key =
        infoschema_dependency::infoschema::CiString::from(table_source.Source.Name.O.as_str());
    let table_info = builder
        .is
        .ModelTableInfoByName(&schema_key, &table_key)
        .map_err(|error| expression::errors::New(error.to_string()))?;
    let effective_table_name = if table_source.AsName.O.is_empty() {
        table_info.Name.clone()
    } else {
        table_source.AsName.clone()
    };
    let db_name = crate::ast::NewCIStr(&schema_name);
    let (mut columns, mut names) = expression::ColumnInfos2ColumnsAndNames(
        builder.ctx.GetExprCtx(),
        db_name.clone(),
        table_info.Name.clone(),
        &table_info.Columns,
        table_info.as_ref(),
    )?;
    if !table_source.AsName.O.is_empty() {
        for name in names.0.iter_mut().flatten() {
            let mut aliased = name.Clone();
            aliased.TblName = effective_table_name.clone();
            *name = Arc::new(aliased);
        }
    }
    let mut schema = expression::NewSchema(columns.clone());
    // Match Go statistics.PseudoRowCount for an unanalyzed table.  Using one
    // here makes every pseudo table look equally tiny and causes the greedy
    // join reorderer to resolve large tie sets by input order instead of by
    // the same selectivity estimates as TiDB.
    let row_count = 10_000.0;
    let table_path = planner_util_dependency::AccessPath {
        CountAfterAccess: row_count,
        MinCountAfterAccess: row_count,
        MaxCountAfterAccess: row_count,
        CountAfterIndex: row_count,
        IsIntHandlePath: !table_info.IsCommonHandle,
        IsCommonHandlePath: table_info.IsCommonHandle,
        IsSingleScan: true,
        ..Default::default()
    };
    let pseudo_histogram =
        logicalop::BuildPseudoHistColl(table_info.as_ref(), table_info.ID, &columns);
    let mut source = logicalop::DataSource {
        TableInfo: table_info.as_ref().clone(),
        Columns: table_info.Columns.clone(),
        DBName: db_name,
        TableAsName: (!table_source.AsName.O.is_empty()).then(|| table_source.AsName.clone()),
        TableStats: logicalop::StatsInfo {
            RowCount: row_count,
            ColNDVs: columns
                .iter()
                // Go `EstimateColumnNDV` uses `PseudoRowCount *
                // distinctFactor` (0.8) when a column histogram is absent.
                .map(|column| (column.UniqueID, row_count * 0.8))
                .collect(),
            HistColl: Some(std::sync::Arc::new(pseudo_histogram)),
            ..Default::default()
        },
        AllPossibleAccessPaths: vec![table_path.clone()],
        PossibleAccessPaths: vec![table_path],
        PhysicalTableID: table_info.ID,
        PartitionNames: table_source.Source.PartitionNames.clone(),
        IsForUpdateRead: builder.GetIsForUpdateRead(),
        ..Default::default()
    }
    .Init(builder.ctx.clone(), query_block);
    if table_info.Partition.is_some() {
        builder.optFlag |= rule::FLAG_PARTITION_PROCESSOR;
    }
    let handle = if table_info.PKIsHandle
        && let Some(handle_column) = table_info
            .Columns
            .iter()
            .find(|column| expression::mysql::HasPriKeyFlag(column.GetFlag()))
            .and_then(|primary| {
                columns
                    .iter()
                    .find(|column| column.ID == primary.ID)
                    .cloned()
            }) {
        planner_util_dependency::NewIntHandleCols(handle_column)
    } else if table_info.IsCommonHandle {
        let primary = table_info
            .Indices
            .iter()
            .find(|index| index.Primary)
            .ok_or_else(|| {
                expression::errors::New(format!(
                    "common-handle table {} has no primary index",
                    table_info.Name.O
                ))
            })?;
        let (common_columns, common_lens) =
            planner_util_dependency::IndexInfo2FullCols(&source.Columns, &schema.Columns, primary);
        let common_columns = common_columns
            .into_iter()
            .zip(common_lens)
            .map(|(column, length)| {
                column
                    .map(|column| (column, length))
                    .ok_or_else(|| expression::errors::New("common-handle column is absent"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        source.CommonHandleCols = common_columns
            .iter()
            .map(|(column, _)| column.Clone())
            .collect();
        source.CommonHandleLens = common_columns.iter().map(|(_, length)| *length).collect();
        Box::new(planner_util_dependency::NewCommonHandleCols(
            table_info.as_ref().clone(),
            primary.clone(),
            &columns,
        )) as Box<dyn planner_util_dependency::HandleCols>
    } else {
        // Go exposes a real extra row handle in every non-clustered
        // DataSource. DELETE consumes it even though ordinary SELECT wildcard
        // expansion keeps the pseudo column hidden.
        let handle_column = source.NewExtraHandleSchemaCol();
        schema.Append([handle_column.Clone()]);
        columns.push(handle_column.Clone());
        source
            .Columns
            .push(expression::model::NewExtraHandleColInfo());
        names.0.push(Some(Arc::new(expression::types::FieldName {
            DBName: crate::ast::NewCIStr(&schema_name),
            TblName: effective_table_name.clone(),
            ColName: expression::model::ExtraHandleName.clone(),
            OrigTblName: table_info.Name.clone(),
            OrigColName: expression::model::ExtraHandleName.clone(),
            ..Default::default()
        })));
        planner_util_dependency::NewIntHandleCols(handle_column)
    };
    source.UnMutableHandleCols = Some(handle.CloneHandleCols());
    source.HandleCols = Some(handle);

    // Go appends the MVCC commit-ts pseudo column after the physical handle. It is
    // hidden and normally pruned, but its real unique-ID allocation is
    // observable by later scalar-subquery marker columns.
    let commit_ts = source.NewExtraCommitTSSchemaCol();
    schema.Append([commit_ts.CloneColumn()]);
    columns.push(commit_ts.CloneColumn());
    source
        .Columns
        .push(expression::model::NewExtraCommitTSColInfo());
    names.0.push(Some(Arc::new(expression::types::FieldName {
        DBName: crate::ast::NewCIStr(&schema_name),
        TblName: effective_table_name.clone(),
        ColName: expression::model::ExtraCommitTSName.clone(),
        OrigTblName: table_info.Name.clone(),
        OrigColName: expression::model::ExtraCommitTSName.clone(),
        Hidden: true,
        ..Default::default()
    })));
    source.SetSchema(schema);
    source.SetOutputNames(names);
    for column in columns {
        source.AppendTableCol(column);
    }
    builder.populateDataSource(ctx, &table_source.Source, &mut source)?;
    install_declared_index_paths(builder, table_source, &mut source);
    install_hypothetical_indexes(builder, select, table_source, &mut source)?;
    let effective_table_name = source
        .TableAsName
        .as_ref()
        .unwrap_or(&source.TableInfo.Name)
        .clone();
    let hinted_table = hint::HintedTable {
        DBName: source.DBName.clone(),
        TblName: effective_table_name,
        SelectOffset: source.QueryBlockOffset(),
        ..Default::default()
    };
    let (force_tikv, force_tiflash) =
        builder
            .tableHintInfo
            .last_mut()
            .map_or((false, false), |plan_hints| {
                let force_tikv = plan_hints.IfPreferTiKV(&hinted_table).is_some();
                let force_tiflash = plan_hints.IfPreferTiFlash(&hinted_table).is_some();
                (force_tikv, force_tiflash)
            });
    if force_tiflash && force_tikv {
        set_hint_warning_once(
            builder,
            format!(
                "Storage hints are conflict, you can only specify one storage type of table {}.{}",
                source.DBName.L, source.TableInfo.Name.L
            ),
        );
    }
    if force_tiflash {
        source.PreferStoreType |= hint::PreferTiFlash as i32;
        source.ForceTiFlashPath();
    } else if force_tikv {
        source.PreferStoreType |= hint::PreferTiKV as i32;
        source.ForceTiKVPath();
    }
    if source.PreferStoreType != 0 {
        builder
            .ctx
            .GetSessionVars()
            .StmtCtx
            .MarkAlternativeLogicalPlanHasStoreTypeHint();
    }
    if builder.ctx.GetSessionVars().EnableAlternativeLogicalPlans
        && (source.IsForUpdateRead
            || !source
                .PossibleAccessPaths
                .iter()
                .any(|path| path.StoreType == kv_dependency::StoreType::TiFlash))
    {
        builder
            .ctx
            .GetSessionVars()
            .StmtCtx
            .MarkAlternativeLogicalPlanMissingTiFlashPath();
    }
    let result_table_info = source.TableInfo.Clone();
    if source.TableInfo.TableCacheStatusType == expression::model::TableCacheStatusEnable {
        if source.HandleCols.is_none() && source.UnMutableHandleCols.is_none() {
            let mut handle_type = expression::types::NewFieldType(expression::mysql::TypeLonglong);
            handle_type.AddFlag(expression::mysql::NotNullFlag | expression::mysql::PriKeyFlag);
            let mut handle = expression::Column::new(
                *handle_type,
                expression::model::ExtraHandleID,
                builder.ctx.GetExprCtx().AllocPlanColumnID(),
                source.Schema().Len() as isize,
            );
            handle.OrigName = format!(
                "{}.{}.{}",
                source.DBName.O,
                source.TableInfo.Name.O,
                expression::model::ExtraHandleName.O
            );
            handle.IsHidden = true;
            source.Schema_mut().Append([handle.Clone()]);
            source
                .Columns
                .push(expression::model::NewExtraHandleColInfo());
            let mut names = source.OutputNames().Shallow();
            names.0.push(Some(Arc::new(expression::types::FieldName {
                OrigTblName: source.TableInfo.Name.clone(),
                OrigColName: expression::model::ExtraHandleName.clone(),
                DBName: source.DBName.clone(),
                TblName: source
                    .TableAsName
                    .as_ref()
                    .unwrap_or(&source.TableInfo.Name)
                    .clone(),
                ColName: expression::model::ExtraHandleName.clone(),
                Hidden: true,
                ..Default::default()
            })));
            source.SetOutputNames(names);
            source.HandleCols = Some(planner_util_dependency::NewIntHandleCols(handle));
        }
        let schema = source.Schema().Clone();
        let names = source.OutputNames().Shallow();
        let stats = source.TableStats.clone();
        let handle = source
            .HandleCols
            .as_ref()
            .or(source.UnMutableHandleCols.as_ref())
            .map(|handle| handle.CloneHandleCols())
            .expect("cached table must expose a physical or extra handle");
        let mut union_scan = logicalop::LogicalUnionScan {
            HandleCols: handle,
            ..Default::default()
        }
        .Init(builder.ctx.clone(), query_block);
        union_scan.SetSchema(schema);
        union_scan.SetOutputNames(names);
        union_scan.SetStats(stats);
        union_scan.SetChildren(vec![Box::new(source)]);
        return Ok((Box::new(union_scan), Some(result_table_info)));
    }
    Ok((Box::new(source), Some(result_table_info)))
}

/// 安装已声明索引路径。
fn install_declared_index_paths(
    builder: &PlanBuilder,
    table_source: &crate::ast::TableSource,
    source: &mut logicalop::DataSource,
) {
    let effective_name = source
        .TableAsName
        .as_ref()
        .unwrap_or(&source.TableInfo.Name)
        .clone();
    // Pruning runs after building: preserve explicit merge indexes even when
    // they provide no marginal coverage beyond the clustered-key prefix.
    source.IndexMergeHints = builder
        .TableHints()
        .into_iter()
        .flat_map(|hints| hints.IndexMergeHintList.iter())
        .filter(|hint| hint.Match(source.DBName.clone(), effective_name.clone()))
        .map(|hint| {
            hint.IndexHint
                .as_ref()
                .map(|index| index.IndexNames.iter().map(|name| name.O.clone()).collect())
                .unwrap_or_default()
        })
        .collect();
    let matching_hints = builder
        .TableHints()
        .into_iter()
        .flat_map(|hints| hints.IndexHintList.iter())
        .filter(|hint| hint.Match(source.DBName.clone(), effective_name.clone()))
        .filter_map(|hint| hint.IndexHint.as_ref())
        .chain(table_source.Source.IndexHints.iter())
        .collect::<Vec<_>>();
    let force_names = matching_hints
        .iter()
        .filter(|hint| hint.HintType == crate::ast::HintForce)
        .flat_map(|hint| hint.IndexNames.iter().map(|name| name.L.as_str()))
        .collect::<Vec<_>>();
    let ignore_names = matching_hints
        .iter()
        .filter(|hint| hint.HintType == crate::ast::HintIgnore)
        .flat_map(|hint| hint.IndexNames.iter().map(|name| name.L.as_str()))
        .collect::<Vec<_>>();
    let use_names = matching_hints
        .iter()
        .filter(|hint| hint.HintType == crate::ast::HintUse)
        .flat_map(|hint| hint.IndexNames.iter().map(|name| name.L.as_str()))
        .collect::<Vec<_>>();
    let path_is_allowed = |path: &planner_util_dependency::AccessPath| {
        let Some(index) = path.Index.as_ref() else {
            return force_names.is_empty();
        };
        !ignore_names.contains(&index.Name.L.as_str())
            && (force_names.is_empty() || force_names.contains(&index.Name.L.as_str()))
            && (use_names.is_empty()
                || use_names.contains(&index.Name.L.as_str())
                || force_names.contains(&index.Name.L.as_str()))
    };
    for existing_paths in [
        &mut source.AllPossibleAccessPaths,
        &mut source.PossibleAccessPaths,
    ] {
        existing_paths.retain(&path_is_allowed);
        for path in existing_paths
            .iter_mut()
            .filter(|path| path.Index.is_some())
        {
            path.Forced = !force_names.is_empty() || !use_names.is_empty();
        }
    }
    let existing_index_ids = source
        .AllPossibleAccessPaths
        .iter()
        .filter_map(|path| path.Index.as_ref().map(|index| index.ID))
        .collect::<std::collections::HashSet<_>>();
    let mut paths = source
        .TableInfo
        .Indices
        .iter()
        .filter(|index| {
            !index.Invisible
                && index.State == expression::model::StatePublic
                && !ignore_names.contains(&index.Name.L.as_str())
                && (force_names.is_empty() || force_names.contains(&index.Name.L.as_str()))
                && (use_names.is_empty()
                    || use_names.contains(&index.Name.L.as_str())
                    || force_names.contains(&index.Name.L.as_str()))
                && !existing_index_ids.contains(&index.ID)
        })
        .map(|index| planner_util_dependency::AccessPath {
            Index: Some(index.Clone()),
            CountAfterAccess: (source.TableStats.RowCount * 0.1).max(0.01),
            MinCountAfterAccess: 0.0,
            MaxCountAfterAccess: source.TableStats.RowCount,
            CountAfterIndex: (source.TableStats.RowCount * 0.1).max(0.01),
            IsSingleScan: false,
            Forced: !force_names.is_empty() || !use_names.is_empty(),
            ..Default::default()
        })
        .collect::<Vec<_>>();
    source.AllPossibleAccessPaths.extend(paths.iter().cloned());
    source.PossibleAccessPaths.extend(paths);
    let _ = table_source;
}

/// 安装假设索引。
fn install_hypothetical_indexes(
    builder: &PlanBuilder,
    select: &crate::ast::SelectStmt,
    table_source: &crate::ast::TableSource,
    source: &mut logicalop::DataSource,
) -> Result<(), expression::Error> {
    let current_db = builder.ctx.GetSessionVars().CurrentDB();
    for table_hint in &select.TableHints {
        if table_hint.HintName.L != "hypo_index" {
            continue;
        }
        if table_hint.Tables.len() < 3 {
            set_hint_warning_once(
                builder,
                "Invalid HYPO_INDEX hint, valid usage: HYPO_INDEX(tableName, indexName, cols...)",
            );
            continue;
        }
        let target = &table_hint.Tables[0];
        let target_db = if target.DBName.L.is_empty() {
            current_db.as_str()
        } else {
            target.DBName.L.as_str()
        };
        let schema_key = infoschema_dependency::infoschema::CiString::from(target_db);
        let table_key =
            infoschema_dependency::infoschema::CiString::from(target.TableName.O.as_str());
        if builder
            .is
            .ModelTableInfoByName(&schema_key, &table_key)
            .is_err()
        {
            set_hint_warning_once(
                builder,
                format!(
                    "invalid HYPO_INDEX hint: table '{}.{}' doesn't exist",
                    target_db, target.TableName.O
                ),
            );
            continue;
        }
        if target_db != source.DBName.L
            || (target.TableName.L != table_source.Source.Name.L
                && target.TableName.L
                    != source
                        .TableAsName
                        .as_ref()
                        .map_or("", |alias| alias.L.as_str()))
        {
            continue;
        }
        let mut index = expression::model::IndexInfo {
            ID: -(source.TableInfo.Indices.len() as i64 + 1),
            Name: table_hint.Tables[1].TableName.clone(),
            Table: source.TableInfo.Name.clone(),
            State: expression::model::StatePublic,
            Tp: expression::model::ast::IndexType::Hypo,
            ..Default::default()
        };
        let mut invalid = false;
        for column_hint in table_hint.Tables.iter().skip(2) {
            let Some(offset) = source
                .Columns
                .iter()
                .position(|column| column.Name.L == column_hint.TableName.L)
            else {
                set_hint_warning_once(
                    builder,
                    format!(
                        "invalid HYPO_INDEX hint: can't find column {} in table {}.{}",
                        column_hint.TableName.O, source.DBName.O, source.TableInfo.Name.O
                    ),
                );
                invalid = true;
                break;
            };
            index.Columns.push(expression::model::IndexColumn {
                Name: column_hint.TableName.clone(),
                Offset: offset as isize,
                Length: expression::types::UnspecifiedLength as isize,
                ..Default::default()
            });
        }
        if invalid {
            continue;
        }
        source.TableInfo.Indices.push(index.Clone());
        let path = planner_util_dependency::AccessPath {
            Index: Some(index),
            CountAfterAccess: (source.TableStats.RowCount * 0.1).max(0.01),
            MinCountAfterAccess: 0.0,
            MaxCountAfterAccess: source.TableStats.RowCount,
            CountAfterIndex: (source.TableStats.RowCount * 0.1).max(0.01),
            IsSingleScan: false,
            ..Default::default()
        };
        source.AllPossibleAccessPaths.insert(0, path.clone());
        source.PossibleAccessPaths.insert(0, path);
    }
    Ok(())
}

/// 幂等设置 hint 警告。
fn set_hint_warning_once(builder: &PlanBuilder, warning: impl Into<String>) {
    let warning = warning.into();
    let statement_context = &builder.ctx.GetSessionVars().StmtCtx;
    if statement_context.GetWarnings().iter().any(|existing| {
        existing
            .Err
            .as_ref()
            .is_some_and(|error| error.to_string() == warning)
    }) {
        return;
    }
    statement_context.SetHintWarning(warning);
}

/// 按计划 Schema 改写表达式。
fn rewrite_for_plan(
    builder: &PlanBuilder,
    node: &crate::ast::ExprNode,
    input: &dyn logicalop::LogicalPlan,
    table_info: Option<&expression::model::TableInfo>,
) -> Result<expression::ExprBox, expression::Error> {
    if let crate::ast::ExprKind::Binary { Op, L, R } = &node.Kind
        && Op.eq_ignore_ascii_case("and")
    {
        if matches!(&L.Kind, crate::ast::ExprKind::Value(value) if value == "1") {
            return rewrite_for_plan(builder, R, input, table_info);
        }
        if matches!(&R.Kind, crate::ast::ExprKind::Value(value) if value == "1") {
            return rewrite_for_plan(builder, L, input, table_info);
        }
    }
    crate::PlannerBuildSimpleExpr(
        builder.ctx.GetExprCtx(),
        node,
        vec![expression::WithInputSchemaAndNames(
            input.Schema(),
            input.OutputNames().Shallow(),
            table_info,
        )],
    )
}

/// 为窗口输入改写表达式。
fn rewrite_window_input_for_plan(
    builder: &PlanBuilder,
    node: &crate::ast::ExprNode,
    input: &dyn logicalop::LogicalPlan,
    table_info: Option<&expression::model::TableInfo>,
    aggregate_mapper: &crate::expression_rewriter::AggregateMapper,
) -> Result<expression::ExprBox, expression::Error> {
    if matches!(node.Kind, crate::ast::ExprKind::AggregateFunction { .. })
        && let Some(index) = aggregate_mapper
            .get(&crate::expression_rewriter::AggregateMapperKey(node))
            .and_then(|index| usize::try_from(*index).ok())
        && let Some(column) = input.Schema().Columns.get(index)
    {
        return Ok(Box::new(column.Clone()));
    }
    rewrite_for_plan(builder, node, input, table_info)
}

/// Go parser emits LEAD/LAG offsets as integer ValueExpr datums.  The Rust
/// parser currently retains an integer token's text in String/Decimal, so
/// restore the Go datum shape before descriptor validation and projection.
fn rewrite_window_argument_for_plan(
    builder: &PlanBuilder,
    function_name: &str,
    argument_index: usize,
    node: &crate::ast::ExprNode,
    input: &dyn logicalop::LogicalPlan,
    table_info: Option<&expression::model::TableInfo>,
    aggregate_mapper: &crate::expression_rewriter::AggregateMapper,
) -> Result<expression::ExprBox, expression::Error> {
    let mut normalized;
    let node = if argument_index == 1
        && matches!(function_name.to_ascii_lowercase().as_str(), "lead" | "lag")
        && let crate::ast::ExprKind::Value(value) = &node.Kind
        && let crate::ast::ValueDatum::String(text) | crate::ast::ValueDatum::Decimal(text) =
            &value.Datum
        && let Ok(offset) = text.parse::<u64>()
    {
        normalized = node.clone();
        let crate::ast::ExprKind::Value(value) = &mut normalized.Kind else {
            unreachable!("the cloned node is a value expression")
        };
        value.Datum = i64::try_from(offset).map_or(
            crate::ast::ValueDatum::Uint64(offset),
            crate::ast::ValueDatum::Int64,
        );
        &normalized
    } else {
        node
    };
    rewrite_window_input_for_plan(builder, node, input, table_info, aggregate_mapper)
}

/// 推导 SELECT 字段输出名。
fn select_field_output_name(
    field: &crate::ast::SelectField,
    rewritten: &dyn expression::Expression,
    source: &dyn logicalop::LogicalPlan,
) -> Option<Arc<expression::types::FieldName>> {
    if !field.AsName.O.is_empty() {
        return Some(Arc::new(expression::types::FieldName {
            ColName: field.AsName.clone(),
            ..Default::default()
        }));
    }
    if let Some(column) = rewritten.as_column()
        && let Some(name) = source
            .Schema()
            .Columns
            .iter()
            .position(|candidate| candidate.UniqueID == column.UniqueID)
            .and_then(|index| source.OutputNames().0.get(index))
            .and_then(Option::as_ref)
    {
        return Some(Arc::new(name.Clone()));
    }
    if let Some(crate::ast::ExprNode {
        Kind: crate::ast::ExprKind::Column(column),
        ..
    }) = field.Expr.as_ref()
    {
        return Some(Arc::new(expression::types::FieldName {
            DBName: column.Schema.clone(),
            TblName: column.Table.clone(),
            ColName: column.Name.clone(),
            OrigTblName: column.Table.clone(),
            OrigColName: column.Name.clone(),
            ..Default::default()
        }));
    }
    Some(Arc::clone(&expression::types::EmptyName))
}

/// 解析 GROUP BY 输入表达式。
fn resolve_group_input_expression(
    expression: &expression::ExprBox,
    input: &dyn logicalop::LogicalPlan,
) -> expression::ExprBox {
    if let Some(column) = expression.as_column() {
        if let Some(projection) = input
            .as_any()
            .downcast_ref::<logicalop::LogicalProjection>()
            && let Some(index) = projection.Schema().ColumnIndex(column)
            && let Some(child) = projection.Children().first()
        {
            return resolve_group_input_expression(&projection.Exprs[index], child.as_ref());
        }
        if let Some(aggregation) = input
            .as_any()
            .downcast_ref::<logicalop::LogicalAggregation>()
            && let Some(index) = aggregation.Schema().ColumnIndex(column)
            && let Some(function) = aggregation.AggFuncs.get(index)
            && function.Name.eq_ignore_ascii_case("firstrow")
            && let [argument] = function.Args.as_slice()
            && let Some(child) = aggregation.Children().first()
        {
            return resolve_group_input_expression(argument, child.as_ref());
        }
        return expression.CloneExpr();
    }
    let Some(function) = expression.as_scalar_function() else {
        return expression.CloneExpr();
    };
    let mut function = function.clone_scalar();
    for argument in function.GetArgsMut() {
        *argument = resolve_group_input_expression(argument, input);
    }
    function.CleanHashCode();
    Box::new(function)
}

/// 解析投影输入表达式。
fn resolve_projection_input_expression(
    expression: &expression::ExprBox,
    input: &dyn logicalop::LogicalPlan,
) -> expression::ExprBox {
    if let Some(column) = expression.as_column() {
        if let Some(projection) = input
            .as_any()
            .downcast_ref::<logicalop::LogicalProjection>()
            && let Some(index) = projection.Schema().ColumnIndex(column)
        {
            return projection.Exprs[index].CloneExpr();
        }
        return expression.CloneExpr();
    }
    let Some(function) = expression.as_scalar_function() else {
        return expression.CloneExpr();
    };
    let mut function = function.clone_scalar();
    for argument in function.GetArgsMut() {
        *argument = resolve_projection_input_expression(argument, input);
    }
    function.CleanHashCode();
    Box::new(function)
}

/// 构建 SELECT 列表投影。
fn build_select_projection(
    builder: &mut PlanBuilder,
    select: &crate::ast::SelectStmt,
    mut source: logicalop::LogicalPlanRef,
    _table_info: Option<&expression::model::TableInfo>,
    query_block: i32,
) -> Result<logicalop::LogicalPlanRef, expression::Error> {
    // Go initializes the SELECT Projection before rewriting its fields.  Field
    // rewriting may build scalar-subquery plans, so this ordering is also the
    // stable logical plan-ID contract observed by the column-pruning suite.
    let mut projection =
        logicalop::LogicalProjection::default().Init(builder.ctx.clone(), query_block);
    let mut expressions = Vec::new();
    let mut output_names = Vec::new();
    let mut projected_column_ids = Vec::new();
    for field in &select.Fields.Fields {
        if let Some(wildcard) = &field.WildCard {
            let start = expressions.len();
            for (index, column) in source.Schema().Columns.iter().enumerate() {
                if column.IsHidden
                    && !(builder.inDeleteStmt && column.ID == expression::model::ExtraHandleID)
                {
                    continue;
                }
                let name = source.OutputNames().0.get(index).and_then(Option::as_ref);
                let matches_schema = wildcard.Schema.O.is_empty()
                    || name.is_some_and(|name| name.DBName.L == wildcard.Schema.L);
                let matches_table = wildcard.Table.O.is_empty()
                    || name.is_some_and(|name| name.TblName.L == wildcard.Table.L);
                if matches_schema && matches_table {
                    expressions.push(Box::new(column.Clone()) as expression::ExprBox);
                    output_names.push(name.map(|name| Arc::new(name.Clone())));
                    projected_column_ids.push(None);
                }
            }
            if expressions.len() == start {
                return Err(expression::errors::New("wildcard has no matching columns"));
            }
            continue;
        }

        let ast_expression = field
            .Expr
            .as_ref()
            .ok_or_else(|| expression::errors::New("SELECT field has no expression"))?;
        let (expression, rewritten_source) = crate::expression_rewriter::rewrite(
            builder,
            crate::context::TODOArc(),
            ast_expression,
            source,
            crate::expression_rewriter::AggregateMapper::default(),
            true,
        )?;
        source = rewritten_source;
        let expression = expression.ok_or_else(|| {
            expression::errors::New("SELECT expression did not produce a scalar value")
        })?;
        let name = if !field.AsName.O.is_empty() {
            Some(Arc::new(expression::types::FieldName {
                ColName: field.AsName.clone(),
                ..Default::default()
            }))
        } else if matches!(
            ast_expression.Kind,
            crate::ast::ExprKind::Subquery { .. }
                | crate::ast::ExprKind::ExistsSubquery { .. }
                | crate::ast::ExprKind::CompareSubquery { .. }
                | crate::ast::ExprKind::InSubquery { .. }
        ) {
            Some(Arc::clone(&expression::types::EmptyName))
        } else if let Some(column) = expression.as_any().downcast_ref::<expression::Column>() {
            source
                .Schema()
                .Columns
                .iter()
                .position(|candidate| candidate.UniqueID == column.UniqueID)
                .and_then(|index| source.OutputNames().0.get(index))
                .and_then(Option::as_ref)
                .map(|name| Arc::new(name.Clone()))
        } else if let Some(correlated) = expression.as_correlated_column() {
            builder
                .outerSchemas
                .iter()
                .zip(&builder.outerNames)
                .rev()
                .find_map(|(schema, names)| {
                    schema
                        .Columns
                        .iter()
                        .position(|column| column.UniqueID == correlated.column.UniqueID)
                        .and_then(|index| names.0.get(index))
                        .and_then(Option::as_ref)
                        .map(|name| Arc::new(name.Clone()))
                })
        } else {
            Some(Arc::clone(&expression::types::EmptyName))
        };
        let projected_column_id = expression
            .as_column()
            .is_none()
            .then(|| builder.ctx.GetExprCtx().AllocPlanColumnID());
        expressions.push(expression);
        output_names.push(name);
        projected_column_ids.push(projected_column_id);
    }

    let eval_context = builder.ctx.GetExprCtx().GetEvalCtx();
    let visible_len = expressions.len();
    for item in &select.OrderBy {
        let resolved_order = resolve_select_order_aliases(select, &item.Expr);
        let selected_computed_order = select.Fields.Fields.iter().any(|field| {
            field.WildCard.is_none()
                && field
                    .Expr
                    .as_ref()
                    .is_some_and(|expression| ast_expressions_equal(expression, &resolved_order))
        });
        let rewritten_order = rewrite_for_plan(builder, &item.Expr, source.as_ref(), None);
        if let Ok(order_expression) = &rewritten_order
            && order_expression.as_column().is_none()
        {
            if selected_computed_order
                || !expressions
                    .iter()
                    .any(|existing| existing.Equal(eval_context, order_expression.as_ref()))
            {
                expressions.push(order_expression.CloneExpr());
                output_names.push(Some(Arc::clone(&expression::types::EmptyName)));
                projected_column_ids.push(Some(builder.ctx.GetExprCtx().AllocPlanColumnID()));
            }
            continue;
        }
        let order_columns = rewritten_order
            .map(|order_expression| {
                expression::ExtractColumns(order_expression.as_ref())
                    .into_iter()
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|_| {
                // Go resolves SELECT aliases inside ORDER BY before projection
                // construction.  If the mixed expression cannot be rewritten
                // against the source as a whole (for example c1+c2+c), retain
                // every non-alias source column that is independently valid.
                ast_columns_outside_subqueries(&item.Expr)
                    .into_iter()
                    .filter_map(|name| {
                        rewrite_for_plan(
                            builder,
                            &crate::ast::ExprNode::Column(name),
                            source.as_ref(),
                            None,
                        )
                        .ok()
                    })
                    .flat_map(|order_expression| {
                        expression::ExtractColumns(order_expression.as_ref())
                            .into_iter()
                            .cloned()
                            .collect::<Vec<_>>()
                    })
                    .collect()
            });
        for column in &order_columns {
            if expressions.iter().any(|expression| {
                expression
                    .as_column()
                    .is_some_and(|existing| existing.UniqueID == column.UniqueID)
            }) {
                continue;
            }
            let Some(index) = source
                .Schema()
                .Columns
                .iter()
                .position(|candidate| candidate.UniqueID == column.UniqueID)
            else {
                continue;
            };
            expressions.push(Box::new(column.clone()));
            output_names.push(source.OutputNames().0.get(index).cloned().flatten());
            projected_column_ids.push(None);
        }
    }

    let columns = expressions
        .iter()
        .enumerate()
        .map(|(index, expression)| {
            // Go buildProjectionField reuses a direct input Column.  Only
            // computed and correlated expressions receive a fresh output ID.
            if let Some(input) = expression.as_column() {
                let mut column = input.clone();
                column.Index = index as isize;
                column.IsHidden = index >= visible_len || column.IsHidden;
                if builder.inDeleteStmt && column.ID == expression::model::ExtraHandleID {
                    column.IsHidden = false;
                }
                return column;
            }
            let mut column = expression::Column::new(
                expression.GetType(eval_context).clone(),
                0,
                projected_column_ids[index]
                    .unwrap_or_else(|| builder.ctx.GetExprCtx().AllocPlanColumnID()),
                index as isize,
            );
            column.IsHidden = index >= visible_len;
            if column.IsHidden {
                column.OrigName = expression
                    .StringWithCtx(Some(eval_context), expression::errors::RedactLogDisable);
            }
            column
        })
        .collect();
    projection.Exprs = expressions;
    projection.SetSchema(expression::NewSchema(columns));
    projection.SetOutputNames(expression::types::NameSlice(output_names));
    projection.SetChildren(vec![source]);
    builder.optFlag |= rule::FLAG_ELIMINATE_PROJECTION | rule::FLAG_BUILD_KEY_INFO;
    Ok(Box::new(projection))
}

/// 子查询外的 AST 列引用。
fn ast_columns_outside_subqueries(
    expression: &crate::ast::ExprNode,
) -> Vec<crate::ast::ColumnName> {
    #[derive(Default)]
    struct Collector {
        columns: Vec<crate::ast::ColumnName>,
    }

    impl crate::ast::ExprNodeVisitor for Collector {
        fn Enter(&mut self, input: &crate::ast::ExprNode) -> (crate::ast::ExprNode, bool) {
            match &input.Kind {
                crate::ast::ExprKind::Subquery { .. }
                | crate::ast::ExprKind::ExistsSubquery { .. } => (input.clone(), true),
                crate::ast::ExprKind::Column(column) => {
                    self.columns.push(column.clone());
                    (input.clone(), true)
                }
                _ => (input.clone(), false),
            }
        }

        fn Leave(&mut self, input: &crate::ast::ExprNode) -> (crate::ast::ExprNode, bool) {
            (input.clone(), true)
        }
    }

    let mut collector = Collector::default();
    let _ = expression.Accept(&mut collector);
    collector.columns
}

/// 校验 HAVING/窗口别名引用。
fn validate_having_window_alias(select: &crate::ast::SelectStmt) -> Result<(), expression::Error> {
    let aliases = select
        .Fields
        .Fields
        .iter()
        .filter(|field| !field.AsName.L.is_empty())
        .filter_map(|field| {
            let expression = field.Expr.as_ref()?;
            let mut windows = Vec::new();
            collect_window_expressions(expression, &mut windows);
            (!windows.is_empty()).then_some((field.AsName.L.clone(), field.AsName.O.clone()))
        })
        .collect::<HashMap<_, _>>();
    for item in &select.GroupBy {
        for column in ast_columns_outside_subqueries(&item.Expr) {
            if column.Schema.L.is_empty()
                && column.Table.L.is_empty()
                && let Some(alias) = aliases.get(&column.Name.L)
            {
                return Err(plannererrors::ErrIllegalReference
                    .GenWithStackByArgs(&[
                        alias.clone().into(),
                        "reference to window function".into(),
                    ])
                    .into());
            }
        }
    }
    let Some(having) = select.Having.as_ref() else {
        return Ok(());
    };
    for column in ast_columns_outside_subqueries(having) {
        if column.Schema.L.is_empty()
            && column.Table.L.is_empty()
            && let Some(alias) = aliases.get(&column.Name.L)
        {
            return Err(plannererrors::ErrWindowInvalidWindowFuncAliasUse
                .GenWithStackByArgs(&[alias.clone().into()])
                .into());
        }
    }
    Ok(())
}

/// 解析 ORDER BY 对 SELECT 别名的引用。
fn resolve_select_order_aliases(
    select: &crate::ast::SelectStmt,
    expression: &crate::ast::ExprNode,
) -> crate::ast::ExprNode {
    struct Resolver {
        aliases: std::collections::HashMap<String, crate::ast::ExprNode>,
    }

    impl crate::ast::ExprNodeVisitor for Resolver {
        fn Enter(&mut self, input: &crate::ast::ExprNode) -> (crate::ast::ExprNode, bool) {
            let crate::ast::ExprKind::Column(column) = &input.Kind else {
                return (input.clone(), false);
            };
            if column.Schema.L.is_empty()
                && column.Table.L.is_empty()
                && let Some(replacement) = self.aliases.get(&column.Name.L)
            {
                return (replacement.clone(), true);
            }
            (input.clone(), true)
        }

        fn Leave(&mut self, input: &crate::ast::ExprNode) -> (crate::ast::ExprNode, bool) {
            (input.clone(), true)
        }
    }

    let aliases = select
        .Fields
        .Fields
        .iter()
        .filter(|field| !field.AsName.L.is_empty())
        .filter_map(|field| {
            field
                .Expr
                .as_ref()
                .map(|expression| (field.AsName.L.clone(), expression.clone()))
        })
        .collect();
    let mut resolver = Resolver { aliases };
    expression.Accept(&mut resolver).0
}

/// 比较两个 AST 表达式是否相等。
fn ast_expressions_equal(left: &crate::ast::ExprNode, right: &crate::ast::ExprNode) -> bool {
    struct ClearPositions;

    impl crate::ast::ExprNodeVisitor for ClearPositions {
        fn Enter(&mut self, input: &crate::ast::ExprNode) -> (crate::ast::ExprNode, bool) {
            (input.clone(), false)
        }

        fn Leave(&mut self, input: &crate::ast::ExprNode) -> (crate::ast::ExprNode, bool) {
            let mut normalized = input.clone();
            normalized.OriginTextPosition = 0;
            (normalized, true)
        }
    }

    left.Accept(&mut ClearPositions) == right.Accept(&mut ClearPositions)
}

/// 为相关聚合预构建子查询源。
fn prebuild_subquery_sources_for_correlated_aggregates(
    builder: &mut PlanBuilder,
    ctx: &dyn crate::context::Context,
    select: &crate::ast::SelectStmt,
    outer_plan: &dyn logicalop::LogicalPlan,
    ctes: &CteEnvironment,
) -> Result<(), expression::Error> {
    struct Resolver<'a> {
        builder: &'a mut PlanBuilder,
        ctx: &'a dyn crate::context::Context,
        outer_schema: expression::Schema,
        outer_names: expression::types::NameSlice,
        ctes: CteEnvironment,
        error: Option<expression::Error>,
    }

    impl crate::ast::ExprNodeVisitor for Resolver<'_> {
        fn Enter(&mut self, input: &crate::ast::ExprNode) -> (crate::ast::ExprNode, bool) {
            let crate::ast::ExprKind::Subquery { Query, .. } = &input.Kind else {
                return (input.clone(), false);
            };
            if self.error.is_some() {
                return (input.clone(), true);
            }
            let result = Query.with_node(|node| {
                let Some(inner) = node.as_any().downcast_ref::<crate::ast::SelectStmt>() else {
                    return Ok(());
                };
                self.builder.outerSchemas.push(self.outer_schema.Clone());
                self.builder.outerNames.push(self.outer_names.Shallow());
                let mut inner_ctes = self.ctes.clone();
                let built = build_select_source(
                    self.builder,
                    self.ctx,
                    inner,
                    inner.QueryBlockOffset as i32,
                    &mut inner_ctes,
                );
                self.builder.outerSchemas.pop();
                self.builder.outerNames.pop();
                built.map(|_| ())
            });
            self.error = match result {
                Some(Err(error)) => Some(error),
                None => Some(expression::errors::New(
                    "correlated-aggregate subquery AST was consumed",
                )),
                Some(Ok(())) => None,
            };
            (input.clone(), true)
        }

        fn Leave(&mut self, input: &crate::ast::ExprNode) -> (crate::ast::ExprNode, bool) {
            (input.clone(), self.error.is_none())
        }
    }

    let mut resolver = Resolver {
        builder,
        ctx,
        outer_schema: outer_plan.Schema().Clone(),
        outer_names: outer_plan.OutputNames().Shallow(),
        ctes: ctes.clone(),
        error: None,
    };
    for expression in select
        .Fields
        .Fields
        .iter()
        .filter_map(|field| field.Expr.as_ref())
        .chain(select.Having.iter())
        .chain(select.OrderBy.iter().map(|item| &item.Expr))
    {
        let _ = expression.Accept(&mut resolver);
        if let Some(error) = resolver.error.take() {
            return Err(error);
        }
    }
    Ok(())
}

/// 构造指定长度的空输出名切片。
fn empty_output_names(length: usize) -> expression::types::NameSlice {
    expression::types::NameSlice(vec![
        Some(Arc::clone(&expression::types::EmptyName));
        length
    ])
}

/// 从指定下标起清除 Schema NOT NULL。
fn reset_not_null(schema: &mut expression::Schema, start: usize) {
    let end = schema.Columns.len();
    for column in &mut schema.Columns[start..end] {
        let mut cloned = column.clone();
        if let Some(field_type) = &mut cloned.RetType {
            field_type.DelFlag(expression::mysql::NotNullFlag);
        }
        *column = cloned;
    }
}

/// 合并左右 Schema。
fn merged_schema(left: &expression::Schema, right: &expression::Schema) -> expression::Schema {
    let mut columns = left.Clone().Columns;
    columns.extend(right.Clone().Columns);
    expression::NewSchema(columns)
}

/// 从 AST Limit 构建 LogicalLimit，零行则 Dual。
pub(crate) fn build_limit_runtime(
    ctx: base::ContextRef,
    opt_flag: &mut u64,
    hints: Option<&hint::PlanHints>,
    source: logicalop::LogicalPlanRef,
    limit: &crate::ast::Limit,
    target_query_block: i32,
) -> Result<logicalop::LogicalPlanRef, expression::Error> {
    *opt_flag |= rule::FLAG_PUSH_DOWN_TOP_N;
    let offset = read_bound_limit_value(&ctx, limit.Offset.as_ref(), 0)?;
    let count =
        read_bound_limit_value(&ctx, limit.Count.as_ref(), 0)?.min(u64::MAX.saturating_sub(offset));
    let schema = source.Schema().Clone();
    let names = source.OutputNames().Shallow();
    if offset.saturating_add(count) == 0 {
        let mut dual = logicalop::LogicalTableDual {
            RowCount: 0,
            ..Default::default()
        }
        .Init(ctx, target_query_block);
        dual.SetSchema(schema);
        dual.SetOutputNames(names);
        return Ok(Box::new(dual));
    }

    let mut logical_limit = logicalop::LogicalLimit {
        Offset: offset,
        Count: count,
        OffsetParam: limit.Offset.as_ref().and_then(|expr| match &expr.Kind {
            crate::ast::ExprKind::ParamMarker { Offset } => ctx.prepared_param_index(*Offset),
            _ => None,
        }),
        CountParam: limit.Count.as_ref().and_then(|expr| match &expr.Kind {
            crate::ast::ExprKind::ParamMarker { Offset } => ctx.prepared_param_index(*Offset),
            _ => None,
        }),
        ..Default::default()
    }
    .Init(ctx, target_query_block);
    if let Some(hints) = hints {
        logical_limit.PreferLimitToCop = hints.PreferLimitToCop;
    }
    logical_limit.SetSchema(schema);
    logical_limit.SetOutputNames(names);
    logical_limit.SetChildren(vec![source]);
    Ok(Box::new(logical_limit))
}

/// Resolve prepared LIMIT markers by SQL offset before parsing a literal.
fn read_bound_limit_value(
    ctx: &base::ContextRef,
    value: Option<&crate::ast::ExprNode>,
    default: u64,
) -> Result<u64, expression::Error> {
    if let Some(crate::ast::ExprNode {
        Kind: crate::ast::ExprKind::ParamMarker { Offset },
        ..
    }) = value
    {
        let index = ctx
            .prepared_param_index(*Offset)
            .ok_or_else(|| expression::errors::New("Incorrect arguments to LIMIT"))?;
        return ctx
            .prepared_limit_value(index)
            .map_err(expression::errors::New);
    }
    read_limit_value(value, default)
}

/// 解析 LIMIT 字面量为 u64。
pub(crate) fn read_limit_value(
    value: Option<&crate::ast::ExprNode>,
    default: u64,
) -> Result<u64, expression::Error> {
    let Some(value) = value else {
        return Ok(default);
    };
    let crate::ast::ExprKind::Value(value) = &value.Kind else {
        return Err(expression::errors::New(
            "LIMIT count and offset must be constant values",
        ));
    };
    match &value.Datum {
        crate::ast::ValueDatum::Uint64(value) => Ok(*value),
        crate::ast::ValueDatum::Int64(value) if *value >= 0 => Ok(*value as u64),
        // The Rust parser currently preserves LIMIT integer token text as a
        // typed string.  Accept the same unsigned decimal domain as Go's
        // parser-produced integer ValueExpr; reject signs/fractions below.
        crate::ast::ValueDatum::String(value) | crate::ast::ValueDatum::Decimal(value) => {
            value.parse::<u64>().map_err(|_| {
                expression::errors::New("LIMIT count and offset must be non-negative integers")
            })
        }
        _ => Err(expression::errors::New(
            "LIMIT count and offset must be non-negative integers",
        )),
    }
}

/// DISTINCT 转为按键前缀分组的 FirstRow 聚合。
pub(crate) fn build_distinct_runtime(
    ctx: base::ContextRef,
    opt_flag: &mut u64,
    hints: Option<&hint::PlanHints>,
    child: logicalop::LogicalPlanRef,
    length: usize,
) -> Result<logicalop::LogicalPlanRef, expression::Error> {
    *opt_flag |= rule::FLAG_BUILD_KEY_INFO | rule::FLAG_PUSH_DOWN_AGG;
    if length > child.Schema().Len() {
        return Err(expression::errors::New(
            "DISTINCT key length exceeds child schema",
        ));
    }
    let offset = child.QueryBlockOffset();
    let schema = child.Schema().Clone();
    let names = child.OutputNames().Shallow();
    let group_by = expression::Column2Exprs(&schema.Columns[..length]);
    let aggregate_functions = schema
        .Columns
        .iter()
        .map(|column| {
            aggregation::NewAggFuncDesc(
                ctx.GetExprCtx(),
                crate::ast::AggFuncFirstRow,
                vec![Box::new(column.Clone())],
                false,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut aggregate = logicalop::LogicalAggregation {
        AggFuncs: aggregate_functions,
        GroupByItems: group_by,
        ..Default::default()
    }
    .Init(ctx, offset);
    if let Some(hints) = hints {
        aggregate.PreferAggType = u64::from(hints.PreferAggType);
        aggregate.PreferAggToCop = hints.PreferAggToCop;
    }
    aggregate.SetSchema(schema);
    aggregate.SetOutputNames(names);
    aggregate.SetChildren(vec![child]);
    Ok(Box::new(aggregate))
}

impl PlanBuilder {
    /// PlanBuilder：委托 build_limit_runtime。
    pub fn buildLimit(
        &mut self,
        source: logicalop::LogicalPlanRef,
        limit: &crate::ast::Limit,
        target_query_block: i32,
    ) -> Result<logicalop::LogicalPlanRef, expression::Error> {
        let hints = self.TableHints().cloned();
        build_limit_runtime(
            self.ctx.clone(),
            &mut self.optFlag,
            hints.as_ref(),
            source,
            limit,
            target_query_block,
        )
    }

    /// PlanBuilder：委托 build_distinct_runtime。
    pub fn buildDistinct(
        &mut self,
        child: logicalop::LogicalPlanRef,
        length: usize,
    ) -> Result<logicalop::LogicalPlanRef, expression::Error> {
        let hints = self.TableHints().cloned();
        build_distinct_runtime(
            self.ctx.clone(),
            &mut self.optFlag,
            hints.as_ref(),
            child,
            length,
        )
    }

    /// Go `buildApplyWithJoinType`: constructs a real LogicalApply and enables
    /// every logical rule required to simplify it later.
    pub fn buildApplyWithJoinType(
        &mut self,
        outer_plan: logicalop::LogicalPlanRef,
        inner_plan: logicalop::LogicalPlanRef,
        join_type: base::JoinType,
        mark_no_decorrelate: bool,
    ) -> logicalop::LogicalPlanRef {
        self.optFlag |= rule::FLAG_PREDICATE_PUSH_DOWN
            | rule::FLAG_BUILD_KEY_INFO
            | rule::FLAG_DECORRELATE
            | rule::FLAG_CONSTANT_PROPAGATION;

        let correlated = coreusage::ExtractCorColumnsBySchema4LogicalPlan(
            inner_plan.as_ref(),
            outer_plan.Schema(),
        );
        let outer_len = outer_plan.Schema().Len();
        let mut schema = merged_schema(outer_plan.Schema(), inner_plan.Schema());
        let mut names = empty_output_names(schema.Len());
        names.0[..outer_len].clone_from_slice(&outer_plan.OutputNames().0[..outer_len]);
        if join_type == base::JoinType::LeftOuterJoin {
            self.optFlag |= rule::FLAG_ELIMINATE_OUTER_JOIN;
            reset_not_null(&mut schema, outer_len);
        }

        let mut apply = logicalop::LogicalApply {
            LogicalJoin: logicalop::LogicalJoin {
                JoinType: join_type,
                ..Default::default()
            },
            CorCols: correlated,
            NoDecorrelate: mark_no_decorrelate,
            ..Default::default()
        }
        .Init(self.ctx.clone(), self.getSelectOffset());
        let (prefer, order) = self.joinHintPreference();
        apply
            .LogicalJoin
            .SetPreferredJoinTypeAndOrder(prefer, order);
        apply.SetSchema(schema);
        apply.SetOutputNames(names);
        apply.SetChildren(vec![outer_plan, inner_plan]);
        Box::new(apply)
    }

    /// Go `buildSemiApply`: preserves semi/anti scalar shape and wraps the
    /// resulting join in a real LogicalApply rather than returning the join.
    pub fn buildSemiApply(
        &mut self,
        outer_plan: logicalop::LogicalPlanRef,
        inner_plan: logicalop::LogicalPlanRef,
        conditions: Vec<expression::ExprBox>,
        as_scalar: bool,
        not: bool,
        consider_rewrite: bool,
        mark_no_decorrelate: bool,
    ) -> Result<logicalop::LogicalPlanRef, expression::Error> {
        self.optFlag |=
            rule::FLAG_PREDICATE_PUSH_DOWN | rule::FLAG_BUILD_KEY_INFO | rule::FLAG_DECORRELATE;

        let mut join = self.buildSemiJoin(
            outer_plan,
            inner_plan,
            conditions,
            as_scalar,
            not,
            consider_rewrite,
        );
        let schema = join.Schema().Clone();
        let names = join.OutputNames().Shallow();
        let children = join.TakeChildren();
        let mut apply = logicalop::LogicalApply {
            LogicalJoin: join,
            NoDecorrelate: mark_no_decorrelate,
            ..Default::default()
        }
        .Init(self.ctx.clone(), self.getSelectOffset());
        apply.SetSchema(schema);
        apply.SetOutputNames(names);
        apply.SetChildren(children);
        Ok(Box::new(apply))
    }

    /// 构造半/反半连接 LogicalJoin。
    fn buildSemiJoin(
        &mut self,
        outer_plan: logicalop::LogicalPlanRef,
        inner_plan: logicalop::LogicalPlanRef,
        mut conditions: Vec<expression::ExprBox>,
        as_scalar: bool,
        not: bool,
        force_rewrite: bool,
    ) -> logicalop::LogicalJoin {
        for condition in &mut conditions {
            *condition = condition.Decorrelate(outer_plan.Schema());
        }

        let outer_schema = outer_plan.Schema().Clone();
        let outer_names = outer_plan.OutputNames().Shallow();
        let mut join =
            logicalop::LogicalJoin::default().Init(self.ctx.clone(), self.getSelectOffset());
        join.SetChildren(vec![outer_plan, inner_plan]);
        join.AttachOnConds(conditions);

        if as_scalar {
            let mut schema = outer_schema;
            schema.Append([expression::Column::new(
                *expression::types::NewFieldType(expression::mysql::TypeTiny),
                0,
                self.ctx.GetExprCtx().AllocPlanColumnID(),
                0,
            )]);
            let mut names = outer_names;
            names
                .0
                .push(Some(Arc::clone(&expression::types::EmptyName)));
            join.SetSchema(schema);
            join.SetOutputNames(names);
            join.JoinType = if not {
                base::JoinType::AntiLeftOuterSemiJoin
            } else {
                base::JoinType::LeftOuterSemiJoin
            };
        } else {
            join.SetSchema(outer_schema);
            join.SetOutputNames(outer_names);
            join.JoinType = if not {
                base::JoinType::AntiSemiJoin
            } else {
                base::JoinType::SemiJoin
            };
        }

        if not {
            let (null_aware, regular): (Vec<_>, Vec<_>) = std::mem::take(&mut join.EqualConditions)
                .into_iter()
                .partition(|condition| {
                    condition
                        .as_any()
                        .downcast_ref::<expression::ScalarFunction>()
                        .is_some_and(|function| {
                            function.GetArgs().iter().any(|argument| {
                                expression::ExtractColumns(argument.as_ref())
                                    .iter()
                                    .any(|column| column.InOperand)
                            })
                        })
                });
            join.EqualConditions = regular;
            join.NAEQConditions.extend(null_aware);
        }

        let (prefer, order) = self.joinHintPreference();
        join.SetPreferredJoinTypeAndOrder(prefer, order);
        if force_rewrite || self.enableSemiJoinRewrite {
            join.PreferJoinType |= u64::from(hint::PreferRewriteSemiJoin);
            self.optFlag |= rule::FLAG_SEMI_JOIN_REWRITE;
        }
        join
    }

    /// The MaxOneRow query-block offset belongs to its child, not to the
    /// expression rewriter's current block.
    pub fn buildMaxOneRow(&self, plan: logicalop::LogicalPlanRef) -> logicalop::LogicalPlanRef {
        let offset = plan.QueryBlockOffset();
        let mut schema = plan.Schema().Clone();
        reset_not_null(&mut schema, 0);
        let names = plan.OutputNames().Shallow();
        let mut max_one_row = logicalop::LogicalMaxOneRow::default().Init(self.ctx.clone(), offset);
        max_one_row.SetSchema(schema);
        max_one_row.SetOutputNames(names);
        max_one_row.SetChildren(vec![plan]);
        max_one_row.SetMaxOneRow(true);
        Box::new(max_one_row)
    }
}

/// Restores only the CTE entries changed by `prepareCTECheckForSubQuery`.
pub fn resetCTECheckForSubQuery(entries: Vec<CteInfoRef>) {
    for entry in entries {
        entry.borrow_mut().enterSubquery = false;
    }
}

/// Appends one dynamic-privilege check while preserving existing checks and
/// their order, matching Go's slice append behavior.
pub fn appendDynamicVisitInfo(
    mut visit_info: Vec<VisitInfo>,
    privileges: Vec<String>,
    with_grant: bool,
    error: expression::Error,
) -> Vec<VisitInfo> {
    visit_info.push(VisitInfo::dynamic(privileges, with_grant, error));
    visit_info
}

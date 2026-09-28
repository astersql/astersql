// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

// 列裁剪（Column Pruning）逻辑优化规则。
//
// 自顶向下根据上层所需列集合，删除逻辑计划（Logical Plan，优化器内部的代数算子树）
// 中多余的输出列，减少后续算子处理与传输的数据量。

// 列裁剪规则；LogicalPlan 等类型由后续模块接线。
//
// ColumnPruner 对应 Go 类型，负责删除逻辑计划中不再需要的列。
// pub struct ColumnPruner;
//
// impl ColumnPruner {
// Optimize 先复制 schema 列，再调用 Go 的 PruneColumns；错误会立即返回。
//     pub fn Optimize(&self, mut plan: base::LogicalPlan) -> (Option<base::LogicalPlan>, bool, Error) {
//         let plan_changed = false;
//         let columns = plan.schema().columns().to_vec();
//         if let Err(err) = plan.prune_columns(columns) {
//             return (None, plan_changed, err);
//         }
// 断言是开发期不变量检查：除特殊节点外，裁剪后不应产生零列 schema。
//         intest::assert_fn(|| noUnexpectedZeroColumnSchema(&plan),
//             "After column pruning, some operator got an unexpected zero-column output schema. Please fix it.");
//         (Some(plan), plan_changed, Error::none())
//     }
//
// Name 返回优化规则的稳定注册名。
//     pub fn Name(&self) -> &'static str { "column_prune" }
// }
//
// noUnexpectedZeroColumnSchema 保留 Go 的递归不变量检查。
// fn noUnexpectedZeroColumnSchema(plan: &base::LogicalPlan) -> bool {
//     for child in plan.children() {
//         if !noUnexpectedZeroColumnSchema(child) { return false; }
//     }
//     if plan.schema().len() == 0 {
// 某些算子复用第一个子节点的 schema，此时不能把空 schema 当作错误。
//         if !plan.children().is_empty() && plan.schema().same_as(plan.children()[0].schema()) { return true; }
// LogicalTableDual 合法地没有输出列；其他算子都必须保留至少一列。
//         if !plan.is_logical_table_dual() { return false; }
//     }
//     true
// }
//
// base、logicalop、intest 为尚未接通的外部依赖；这里不声称可以独立编译。
// */
use crate::rule_init::{LogicalRule, Plan, PlanKind};
use std::collections::BTreeSet;

/// 列裁剪规则求解器：按上层需要的列向下裁剪各算子输出 schema。
pub struct ColumnPruner;
impl LogicalRule for ColumnPruner {
    fn name(&self) -> &'static str {
        "column_prune"
    }
    fn optimize(&self, mut plan: Plan) -> Result<(Plan, bool), String> {
        // 根节点输出列即为整棵计划树最初的「必需列」集合。
        let required: BTreeSet<_> = plan.schema.iter().copied().collect();
        prune(&mut plan, &required)?;
        // 与 Go ColumnPruner 保持一致：该规则不通过布尔返回值报告变更。
        Ok((plan, false))
    }
}

/// 递归裁剪：先收紧本节点 schema，再按算子类型推导子节点所需列并下推。
fn prune(plan: &mut Plan, required: &BTreeSet<i64>) -> Result<bool, String> {
    let original_schema = plan.schema.clone();
    let old = plan.schema.len();
    plan.schema.retain(|column| required.contains(column));
    // TableDual 允许零列；其它算子若裁空则回填一列，避免破坏 schema 不变量。
    if plan.schema.is_empty() && !matches!(plan.kind, PlanKind::TableDual { .. }) {
        if let Some(column) = required
            .iter()
            .next()
            .or_else(|| plan.children.iter().flat_map(|child| &child.schema).next())
        {
            plan.schema.push(*column);
        }
    }
    // 谓词引用的列也必须从子树保留，否则过滤条件无法求值。
    let mut child_required: BTreeSet<i64> = plan
        .predicates
        .iter()
        .flat_map(|predicate| predicate.columns())
        .collect();
    child_required.extend(required);
    match &plan.kind {
        // Projection：仅保留仍被上层需要的投影表达式的输入列。
        PlanKind::Projection { expressions } => {
            child_required.clear();
            for (offset, expression) in expressions.iter().enumerate() {
                if original_schema
                    .get(offset)
                    .is_some_and(|column| required.contains(column))
                {
                    child_required.extend(expression.columns());
                }
            }
        }
        // Aggregation：分组键与聚合参数列都必须下推保留。
        PlanKind::Aggregation {
            aggregates,
            group_by,
        } => {
            child_required.extend(group_by.iter().flat_map(|expression| expression.columns()));
            child_required.extend(aggregates.iter().flat_map(|aggregate| {
                aggregate
                    .args
                    .iter()
                    .flat_map(|expression| expression.columns())
            }));
        }
        PlanKind::Sort { by } => {
            child_required.extend(by.iter().flat_map(|expression| expression.columns()))
        }
        // Join：等值条件与其它连接条件引用的列都需要两侧子树提供。
        PlanKind::Join {
            equal_conditions,
            other_conditions,
            ..
        } => child_required.extend(
            equal_conditions
                .iter()
                .chain(other_conditions)
                .flat_map(|expression| expression.columns()),
        ),
        _ => {}
    }
    let mut changed = old != plan.schema.len();
    // 对每个子节点只请求其自身 schema 与 child_required 的交集。
    for child in &mut plan.children {
        let child_columns = child.all_columns();
        let requested = child_required
            .intersection(&child_columns)
            .copied()
            .collect();
        changed |= prune(child, &requested)?;
    }
    Ok(changed)
}

// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 逻辑计划杂项工具函数。
//
// 提供 Selection 挂接、TopN 下推、排序项去重、TiFlash 标记探测、
// 计划树内存估算、计划 ID 哈希，以及重复无关聚合列抽取等共享辅助逻辑。

use crate::{Expression, LogicalPlan, LogicalPlanRef, Result};
use std::collections::HashSet;

/// 将子计划挂到 parent 的指定 child 槽位，并返回过滤条件（供 Selection 使用）。
pub fn AddSelection(
    parent: &mut dyn LogicalPlan,
    child_index: usize,
    mut child: LogicalPlanRef,
    mut conditions: Vec<Expression>,
) -> Result<()> {
    if child_index >= parent.Children().len() {
        return Err(crate::PlannerError(format!(
            "child index {child_index} out of bounds"
        )));
    }
    if conditions.is_empty() {
        parent.Children_mut()[child_index] = child;
        return Ok(());
    }
    let context = parent
        .SCtx()
        .cloned()
        .ok_or_else(|| crate::PlannerError("logical parent has no plan context".into()))?;
    conditions = rule_util::ApplyPredicateSimplification(context.clone(), conditions, true, None);
    if conditions.is_empty()
        || child
            .as_any()
            .downcast_ref::<crate::LogicalTableDual>()
            .is_some_and(|dual| dual.RowCount == 0)
    {
        parent.Children_mut()[child_index] = child;
        return Ok(());
    }
    if crate::Conds2TableDual(&conditions) {
        let mut dual = crate::LogicalTableDual {
            RowCount: 0,
            ..Default::default()
        }
        .Init(context, parent.QueryBlockOffset());
        dual.SetSchema(child.Schema().Clone());
        dual.SetOutputNames(child.OutputNames().Shallow());
        child = Box::new(dual);
    } else {
        let mut selection = crate::LogicalSelection {
            Conditions: conditions,
            ..Default::default()
        }
        .Init(context, parent.QueryBlockOffset());
        selection.SetSchema(child.Schema().Clone());
        selection.SetOutputNames(child.OutputNames().Shallow());
        selection.SetChildren(vec![child]);
        child = Box::new(selection);
    }
    parent.Children_mut()[child_index] = child;
    Ok(())
}

/// 对任意逻辑计划走基类 TopN 下推路径。
pub fn pushDownTopNForBaseLogicalPlan(
    plan: &mut dyn LogicalPlan,
    top_n: Option<LogicalPlanRef>,
) -> Option<LogicalPlanRef> {
    plan.base_mut().PushDownTopN(top_n)
}

/// Removes duplicate ordering expressions while retaining their stable order.
/// 按表达式 HashCode 去重排序项，保持首次出现的稳定顺序。
pub fn pruneByItems(items: Vec<Expression>) -> Vec<Expression> {
    let mut seen = HashSet::<Vec<u8>>::new();
    items
        .into_iter()
        .filter(|item| seen.insert(item.HashCode()))
        .collect()
}

/// 探测计划是否标记可能含 TiFlash 访问路径。
pub fn GetHasTiFlash(plan: Option<&dyn LogicalPlan>) -> bool {
    plan.is_some_and(|plan| plan.base().PreparePossiblePropertiesValue())
}

/// 递归估算计划树内存占用（自身 size_of_val + 子节点）。
pub fn RecursiveMemoryUsage(plan: &dyn LogicalPlan) -> i64 {
    let own = std::mem::size_of_val(plan) as i64;
    own + plan
        .Children()
        .iter()
        .map(|child| RecursiveMemoryUsage(child.as_ref()))
        .sum::<i64>()
}

/// 前序遍历收集计划节点 ID 到 output。
pub fn FlattenTreePlan(plan: &dyn LogicalPlan, output: &mut Vec<i32>) {
    output.push(plan.ID());
    for child in plan.Children() {
        FlattenTreePlan(child.as_ref(), output);
    }
}

/// 用 FNV-1a 风格哈希整棵计划树的节点 ID 序列，作计划指纹。
pub fn GetPlanIDsHash(plan: &dyn LogicalPlan) -> u64 {
    let mut ids = Vec::new();
    FlattenTreePlan(plan, &mut ids);
    // FNV-1a 64-bit：offset basis 与 prime 与 Go 侧一致。
    ids.into_iter().fold(1469598103934665603_u64, |hash, id| {
        (hash ^ id as u64).wrapping_mul(1099511628211)
    })
}

/// 若聚合对重复行无关（duplicate-agnostic），抽取参数中的列集合。
pub fn GetDupAgnosticAggCols(
    plan: &dyn LogicalPlan,
    mut old_agg_cols: Vec<crate::Column>,
) -> (bool, Vec<crate::Column>) {
    let Some(aggregate) = plan.as_any().downcast_ref::<crate::LogicalAggregation>() else {
        return (false, Vec::new());
    };
    old_agg_cols.clear();
    for descriptor in &aggregate.AggFuncs {
        if !descriptor.HasDistinct
            && descriptor.Name != aggregation::ast::AggFuncFirstRow
            && descriptor.Name != aggregation::ast::AggFuncMax
            && descriptor.Name != aggregation::ast::AggFuncMin
            && descriptor.Name != aggregation::ast::AggFuncApproxCountDistinct
        {
            old_agg_cols.clear();
            return (true, old_agg_cols);
        }
        for argument in &descriptor.Args {
            old_agg_cols.extend(
                expression::ExtractColumns(argument.as_ref())
                    .into_iter()
                    .cloned(),
            );
        }
    }
    (true, old_agg_cols)
}

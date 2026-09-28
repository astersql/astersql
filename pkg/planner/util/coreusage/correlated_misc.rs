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

// 从逻辑/物理计划中抽取相关列（correlated column）。
//
// 相关列指外层查询列被内层子查询引用的列；解相关（decorrelate）前需
// 先按前序遍历收集，再按 Schema 去重并共享 datum 槽位（对齐 Go 指针语义）。

use base::PhysicalPlan;
use expression::{CorrelatedColumn, NewCorrelatedDatum, Schema};
use logicalop::LogicalPlan;

/// 前序递归：先收集当前节点自身相关列，再合并子节点结果。
fn extract_correlated_cols_recursive<T, Own, Children>(
    plan: &T,
    own: &Own,
    children: &Children,
) -> Vec<CorrelatedColumn>
where
    T: ?Sized,
    Own: Fn(&T) -> Vec<CorrelatedColumn>,
    Children: for<'a> Fn(&'a T) -> Vec<&'a T>,
{
    let mut result = own(plan);
    for child in children(plan) {
        result.extend(extract_correlated_cols_recursive(child, own, children));
    }
    result
}

/// CTE 的相关列来自其种子与递归计划；逻辑算子 crate 不能反向依赖本 crate，
/// 因此这里补上 Go `LogicalCTE.ExtractCorrelatedCols` 的跨 crate 递归接线。
fn extract_correlated_cols_from_cte(cte: &logicalop::LogicalCTE) -> Vec<CorrelatedColumn> {
    let cte = cte.Cte.borrow();
    [&cte.SeedPartLogicalPlan, &cte.RecursivePartLogicalPlan]
        .into_iter()
        .flatten()
        .flat_map(|plan| ExtractCorrelatedCols4LogicalPlan(plan.as_ref()))
        .collect()
}

/// Recursively collects correlated columns from a logical plan in pre-order.
///
/// 从前序遍历逻辑计划，递归收集相关列。
pub fn ExtractCorrelatedCols4LogicalPlan(plan: &dyn LogicalPlan) -> Vec<CorrelatedColumn> {
    // 按具体逻辑算子类型派发 ExtractCorrelatedCols；未命中则走基类。
    fn own(plan: &dyn LogicalPlan) -> Vec<CorrelatedColumn> {
        macro_rules! extract {
            ($type:ty) => {
                if let Some(plan) = plan.as_any().downcast_ref::<$type>() {
                    return plan.ExtractCorrelatedCols();
                }
            };
        }
        if let Some(cte) = plan.as_any().downcast_ref::<logicalop::LogicalCTE>() {
            return extract_correlated_cols_from_cte(cte);
        }
        extract!(logicalop::LogicalAggregation);
        extract!(logicalop::LogicalApply);
        extract!(logicalop::DataSource);
        extract!(logicalop::LogicalExpand);
        extract!(logicalop::LogicalJoin);
        extract!(logicalop::LogicalProjection);
        extract!(logicalop::LogicalSelection);
        extract!(logicalop::LogicalSort);
        extract!(logicalop::LogicalTopN);
        extract!(logicalop::LogicalWindow);
        plan.base().ExtractCorrelatedCols()
    }

    extract_correlated_cols_recursive(plan, &own, &|plan| {
        plan.Children().iter().map(|child| child.as_ref()).collect()
    })
}

/// Recursively collects correlated columns from a physical plan in pre-order.
///
/// 从前序遍历物理计划，递归收集相关列。
pub fn ExtractCorrelatedCols4PhysicalPlan(plan: &dyn PhysicalPlan) -> Vec<CorrelatedColumn> {
    extract_correlated_cols_recursive(plan, &|plan| plan.extract_correlated_cols(), &|plan| {
        plan.children()
    })
}

/// 测试钩子：注入自有列与子节点访问器，复用同一递归实现。
#[cfg(test)]
pub(crate) fn extract_correlated_cols_for_test<T>(
    plan: &T,
    own: impl Fn(&T) -> Vec<CorrelatedColumn>,
    children: impl for<'a> Fn(&'a T) -> Vec<&'a T>,
) -> Vec<CorrelatedColumn> {
    extract_correlated_cols_recursive(plan, &own, &children)
}

/// 抽取逻辑计划相关列，再按 Schema 去重（不解析物理列下标）。
pub fn ExtractCorColumnsBySchema4LogicalPlan(
    plan: &dyn LogicalPlan,
    schema: &Schema,
) -> Vec<CorrelatedColumn> {
    let mut columns = ExtractCorrelatedCols4LogicalPlan(plan);
    ExtractCorColumnsBySchema(&mut columns, schema, false)
}

/// 抽取物理计划相关列，再按 Schema 去重并解析物理列下标。
pub fn ExtractCorColumnsBySchema4PhysicalPlan(
    plan: &dyn PhysicalPlan,
    schema: &Schema,
) -> Vec<CorrelatedColumn> {
    let mut columns = ExtractCorrelatedCols4PhysicalPlan(plan);
    ExtractCorColumnsBySchema(&mut columns, schema, true)
}

/// Deduplicates correlated columns in schema order and shares one datum slot
/// between every occurrence of the same schema column, matching Go pointer semantics.
///
/// 按 Schema 列顺序去重相关列，同一 Schema 列的多次出现共享一个 datum 槽位，
/// 以对齐 Go 中指针共享语义。`resolve_index` 为真时写入物理列下标。
pub fn ExtractCorColumnsBySchema(
    correlated: &mut [CorrelatedColumn],
    schema: &Schema,
    resolve_index: bool,
) -> Vec<CorrelatedColumn> {
    // 按 Schema 长度预分配槽位；仅保留出现在 Schema 中的相关列。
    let mut result: Vec<Option<CorrelatedColumn>> =
        std::iter::repeat_with(|| None).take(schema.Len()).collect();
    for correlated_column in correlated {
        let Some(index) = schema.ColumnIndex(&correlated_column.column) else {
            continue;
        };
        // 首次出现时新建槽位；后续出现复用同一 Arc datum。
        let entry = result[index].get_or_insert_with(|| CorrelatedColumn {
            column: schema.Columns[index].Clone(),
            data: Some(NewCorrelatedDatum(types::datum::Datum::default())),
        });
        correlated_column.data = entry.data.clone();
    }

    result
        .into_iter()
        .enumerate()
        .filter_map(|(index, column)| {
            column.map(|mut column| {
                if resolve_index {
                    column.column.Index = index as isize;
                }
                column
            })
        })
        .collect()
}

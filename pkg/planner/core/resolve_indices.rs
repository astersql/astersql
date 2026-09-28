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

// 物理计划列索引解析（resolve indices）。
//
// 在物理执行计划（Physical Plan）中，将表达式里的列引用从 UniqueID
// 解析为子节点 Schema 中的下标（index），使执行器能按偏移读取列。
// 覆盖 Projection、UnionScan、IndexLookUp、Selection、TopN、Limit 等算子。

use std::collections::BTreeMap;

/// Schema 中的一列：以 UniqueID 标识，并持有在子 Schema 中的下标。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IndexedColumn {
    /// 全局唯一列 ID，用于跨算子匹配同一逻辑列。
    pub unique_id: i64,
    /// 在子节点 Schema 中的列下标。
    pub index: usize,
    /// 虚拟生成列表达式文本；用于按表达式匹配虚拟列。
    pub virtual_expr: Option<String>,
}
/// 可索引解析的表达式：列引用或标量函数树。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IndexedExpr {
    /// 单列引用。
    Column(IndexedColumn),
    /// 标量函数：名称 + 参数表达式列表。
    Scalar {
        name: String,
        args: Vec<IndexedExpr>,
    },
}
/// 列 Schema：按输出顺序排列的 `IndexedColumn` 列表。
pub type IndexSchema = Vec<IndexedColumn>;

/// 在子 Schema 中按 UniqueID 查找列，并写回其下标。
fn resolve_column(column: &IndexedColumn, schema: &IndexSchema) -> Result<IndexedColumn, String> {
    schema
        .iter()
        .position(|candidate| candidate.unique_id == column.unique_id)
        .map(|index| {
            let mut result = column.clone();
            result.index = index;
            result
        })
        .ok_or_else(|| {
            format!(
                "column {} cannot find the reference from its child",
                column.unique_id
            )
        })
}

/// 递归解析表达式树中所有列引用的下标。
fn resolve_expr(expression: &IndexedExpr, schema: &IndexSchema) -> Result<IndexedExpr, String> {
    match expression {
        IndexedExpr::Column(column) => resolve_column(column, schema).map(IndexedExpr::Column),
        IndexedExpr::Scalar { name, args } => Ok(IndexedExpr::Scalar {
            name: name.clone(),
            args: args
                .iter()
                .map(|arg| resolve_expr(arg, schema))
                .collect::<Result<Vec<_>, _>>()?,
        }),
    }
}

/// 按虚拟列表达式文本在子 Schema 中匹配并递归解析下标。
fn resolve_virtual_expr(expression: &IndexedExpr, schema: &IndexSchema) -> Option<IndexedExpr> {
    match expression {
        IndexedExpr::Column(column) => {
            if let Some(index) = schema
                .iter()
                .position(|candidate| candidate.unique_id == column.unique_id)
            {
                return Some(IndexedExpr::Column(IndexedColumn {
                    index,
                    ..column.clone()
                }));
            }
            let virtual_expr = column.virtual_expr.as_ref()?;
            schema
                .iter()
                .position(|candidate| candidate.virtual_expr.as_ref() == Some(virtual_expr))
                .map(|index| {
                    IndexedExpr::Column(IndexedColumn {
                        index,
                        ..column.clone()
                    })
                })
        }
        IndexedExpr::Scalar { name, args } => Some(IndexedExpr::Scalar {
            name: name.clone(),
            args: args
                .iter()
                .map(|arg| resolve_virtual_expr(arg, schema))
                .collect::<Option<Vec<_>>>()?,
        }),
    }
}

/// 物理 Projection 算子的简化计划结构，用于索引解析。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProjectionPlan {
    /// 投影表达式列表。
    pub exprs: Vec<IndexedExpr>,
    /// 本算子输出 Schema。
    pub schema: IndexSchema,
    /// 子节点输出 Schema。
    pub child_schema: IndexSchema,
    /// 可选的子 Projection，用于相邻投影下标精炼。
    pub child_projection: Option<Box<ProjectionPlan>>,
}

/// 仅解析本层投影表达式相对 child_schema 的下标，并可选精炼相邻投影。
pub fn resolveIndicesItself4PhysicalProjection(plan: &mut ProjectionPlan) -> Result<(), String> {
    for expression in &mut plan.exprs {
        *expression = resolve_expr(expression, &plan.child_schema)?;
    }
    if let Some(child) = plan.child_projection.as_deref().cloned() {
        refine4NeighbourProj(plan, &child);
    }
    Ok(())
}

/// 先解析输出 Schema 相对子 Schema 的 inline 投影下标，再解析表达式本身。
pub fn resolveIndices4PhysicalProjection(plan: &mut ProjectionPlan) -> Result<(), String> {
    resolveIndexForInlineProjection(&mut plan.schema, &plan.child_schema)?;
    resolveIndicesItself4PhysicalProjection(plan)
}

/// 并查集找根（路径压缩），用于合并同源输出列。
fn find_root(parent: &mut [usize], index: usize) -> usize {
    if parent[index] != index {
        parent[index] = find_root(parent, parent[index]);
    }
    parent[index]
}
/// 相邻两层 Projection：将父层列下标重定向到子层同源输出的代表下标。
fn refine4NeighbourProj(plan: &mut ProjectionPlan, child: &ProjectionPlan) {
    // 子投影：同一输入列可能映射到多个输出下标，建立并查集合并。
    let mut input_to_outputs: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (output, expression) in child.exprs.iter().enumerate() {
        if let IndexedExpr::Column(column) = expression {
            input_to_outputs
                .entry(column.index)
                .or_default()
                .push(output);
        }
    }
    let mut parent: Vec<usize> = (0..child.schema.len()).collect();
    for outputs in input_to_outputs.values() {
        for output in outputs.iter().skip(1) {
            let root = find_root(&mut parent, outputs[0]);
            parent[*output] = root;
        }
    }
    for expression in &mut plan.exprs {
        if let IndexedExpr::Column(column) = expression {
            column.index = find_root(&mut parent, column.index);
        }
    }
}

/// 物理 UnionScan：合并内存写缓冲与底层扫描结果时使用的过滤与 handle 列。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UnionScanPlan {
    /// 过滤条件表达式。
    pub conditions: Vec<IndexedExpr>,
    /// 行句柄（handle）列，用于定位行版本。
    pub handle_columns: Vec<IndexedColumn>,
    /// 子节点 Schema。
    pub child_schema: IndexSchema,
}
/// 解析 UnionScan 的条件与 handle 列相对子 Schema 的下标。
pub fn resolveIndices4PhysicalUnionScan(plan: &mut UnionScanPlan) -> Result<(), String> {
    for condition in &mut plan.conditions {
        *condition = resolve_expr(condition, &plan.child_schema)?;
    }
    for column in &mut plan.handle_columns {
        *column = resolve_column(column, &plan.child_schema)?;
    }
    Ok(())
}

/// 物理 IndexLookUpReader：先扫索引再回表读取的双阶段读算子。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IndexLookUpReaderPlan {
    /// 最终输出 Schema。
    pub schema: IndexSchema,
    /// 表侧 Schema（回表结果列）。
    pub table_schema: IndexSchema,
    /// 索引侧 Schema。
    pub index_schema: IndexSchema,
    /// 额外附加的 handle 列（如隐式 _tidb_rowid）。
    pub extra_handle_col: Option<IndexedColumn>,
    /// 聚簇索引（common handle）组成列。
    pub common_handle_cols: Vec<IndexedColumn>,
}
/// 校验虚拟列存在性，并解析 handle 相关列相对 table_schema 的下标。
pub fn resolveIndices4PhysicalIndexLookUpReader(
    plan: &mut IndexLookUpReaderPlan,
) -> Result<(), String> {
    // 输出 Schema 中的虚拟列必须能在 table_schema 中按表达式找到。
    for column in plan.schema.clone() {
        if column.virtual_expr.is_some()
            && !plan
                .table_schema
                .iter()
                .any(|candidate| candidate.virtual_expr == column.virtual_expr)
        {
            return Err(format!("virtual column {} is missing", column.unique_id));
        }
    }
    if let Some(column) = plan.extra_handle_col.clone() {
        plan.extra_handle_col = Some(resolve_column(&column, &plan.table_schema)?);
    }
    for column in &mut plan.common_handle_cols {
        *column = resolve_column(column, &plan.table_schema)?;
    }
    Ok(())
}

/// 物理 Selection（过滤）算子计划。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SelectionPlan {
    /// WHERE/HAVING 等过滤条件。
    pub conditions: Vec<IndexedExpr>,
    /// 子节点 Schema。
    pub child_schema: IndexSchema,
}
/// 解析 Selection 条件；UniqueID 找不到时回退到按虚拟列表达式匹配。
pub fn resolveIndices4PhysicalSelection(plan: &mut SelectionPlan) -> Result<(), String> {
    for condition in &mut plan.conditions {
        *condition = match resolve_expr(condition, &plan.child_schema) {
            Ok(resolved) => resolved,
            Err(error) => resolve_virtual_expr(condition, &plan.child_schema).ok_or(error)?,
        };
    }
    Ok(())
}

/// 按 UniqueID 顺序在子 Schema 中单调扫描，为 inline 投影的输出列写回下标。
fn resolveIndexForInlineProjection(
    schema: &mut IndexSchema,
    child_schema: &IndexSchema,
) -> Result<(), String> {
    let mut child_index = 0;
    for column in schema.iter_mut() {
        while child_index < child_schema.len()
            && child_schema[child_index].unique_id != column.unique_id
        {
            child_index += 1;
        }
        if child_index == child_schema.len() {
            return Err("some columns cannot find the reference from its child(ren)".into());
        }
        column.index = child_index;
        child_index += 1;
    }
    Ok(())
}

/// ORDER BY / TopN 中的排序项。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ByItem {
    pub expr: IndexedExpr,
}
/// 分区排序（PARTITION BY）中的分区列。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PartitionByItem {
    pub col: IndexedColumn,
}
/// 物理 TopN（排序截断）算子计划。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TopNPlan {
    pub schema: IndexSchema,
    pub child_schema: IndexSchema,
    /// 排序键表达式。
    pub by_items: Vec<ByItem>,
    /// 分区列（窗口/分区 TopN）。
    pub partition_by: Vec<PartitionByItem>,
    /// 可选前缀列，用于优化局部有序。
    pub prefix_col: Option<IndexedColumn>,
}

/// 解析 TopN 的排序键、分区列、输出 Schema 与前缀列下标。
pub fn resolveIndices4PhysicalTopN(plan: &mut TopNPlan) -> Result<(), String> {
    for item in &mut plan.by_items {
        item.expr = resolve_expr(&item.expr, &plan.child_schema)?;
    }
    for item in &mut plan.partition_by {
        item.col = resolve_column(&item.col, &plan.child_schema)?;
    }
    resolveIndexForInlineProjection(&mut plan.schema, &plan.child_schema)?;
    if let Some(column) = plan.prefix_col.clone() {
        plan.prefix_col = Some(resolve_column(&column, &plan.child_schema)?);
    }
    Ok(())
}

/// 物理 Limit 算子计划（可带分区与前缀列）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LimitPlan {
    pub schema: IndexSchema,
    pub child_schema: IndexSchema,
    pub partition_by: Vec<PartitionByItem>,
    pub prefix_col: Option<IndexedColumn>,
}
/// 解析 Limit 的分区列、输出 Schema 与前缀列下标。
pub fn resolveIndices4PhysicalLimit(plan: &mut LimitPlan) -> Result<(), String> {
    for item in &mut plan.partition_by {
        item.col = resolve_column(&item.col, &plan.child_schema)?;
    }
    resolveIndexForInlineProjection(&mut plan.schema, &plan.child_schema)?;
    if let Some(column) = plan.prefix_col.clone() {
        plan.prefix_col = Some(resolve_column(&column, &plan.child_schema)?);
    }
    Ok(())
}

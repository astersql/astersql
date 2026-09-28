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

// 物理计划辅助工具：克隆、下推树展开、分区访问对象、唯一索引键编码等。
//
// 供 Reader 构造、Plan Cache、虚拟列展开与子节点期望行数计算等路径复用。

use std::collections::HashMap;

use base::{ContextRef, PhysicalPlan};
use expression::{Column, Constant, Schema};
use model::{ColumnInfo, IndexInfo, TableInfo};
use property::{HistCollRef, PhysicalProperty};
use types::datum::Datum;

use crate::PhysPlanPartInfo;

/// 批量克隆物理计划树（换绑同一 Context）。
pub fn ClonePhysicalPlan(
    context: ContextRef,
    plans: &[Box<dyn PhysicalPlan>],
) -> Result<Vec<Box<dyn PhysicalPlan>>, expression::Error> {
    plans
        .iter()
        .map(|plan| plan.clone_physical(context.clone()))
        .collect()
}

/// 前序遍历收集节点（根在前）。
fn flatten_preorder<'a>(plan: &'a dyn PhysicalPlan, output: &mut Vec<&'a dyn PhysicalPlan>) {
    output.push(plan);
    for child in plan.children() {
        flatten_preorder(child, output);
    }
}

/// 将一元下推链展平为叶子→根顺序。
/// Flattens a unary push-down plan leaf-first.
pub fn FlattenListPushDownPlan(plan: &dyn PhysicalPlan) -> Vec<&dyn PhysicalPlan> {
    let mut output = Vec::with_capacity(5);
    flatten_preorder(plan, &mut output);
    output.reverse();
    output
}

/// 后序遍历；多孩子时记录“非相邻”的 child→parent 边。
fn flatten_postorder<'a>(
    plan: &'a dyn PhysicalPlan,
    output: &mut Vec<&'a dyn PhysicalPlan>,
    unnatural: &mut HashMap<usize, usize>,
) {
    let children = plan.children();
    let branching = children.len() > 1;
    let mut child_indices = Vec::with_capacity(children.len());
    for child in children {
        flatten_postorder(child, output, unnatural);
        child_indices.push(output.len() - 1);
    }
    output.push(plan);
    if branching {
        // 多孩子时，非“紧邻父节点”的孩子记入 unnatural，供下推重组使用。
        let parent = output.len() - 1;
        for child in child_indices {
            if child + 1 != parent {
                unnatural.insert(child, parent);
            }
        }
    }
}

/// 后序展开树形下推计划，并返回非自然相邻的父子下标映射。
/// Returns postorder nodes and child-index to non-adjacent parent-index edges.
pub fn FlattenTreePushDownPlan(
    plan: &dyn PhysicalPlan,
) -> (Vec<&dyn PhysicalPlan>, HashMap<usize, usize>) {
    let mut output = Vec::with_capacity(5);
    let mut unnatural = HashMap::new();
    flatten_postorder(plan, &mut output, &mut unnatural);
    (output, unnatural)
}

/// 沿子树向下取第一个带 HistColl 的表统计。
pub fn GetTblStats(plan: Option<&dyn PhysicalPlan>) -> Option<HistCollRef> {
    let plan = plan?;
    if let Some(scan) = plan.as_any().downcast_ref::<crate::PhysicalTableScan>() {
        return scan.TblColHists.clone();
    }
    if let Some(scan) = plan.as_any().downcast_ref::<crate::PhysicalIndexScan>() {
        return scan.TblColHists.clone();
    }
    plan.children()
        .first()
        .and_then(|child| GetTblStats(Some(*child)))
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 动态剪枝后的分区访问对象（库/表/分区名列表）。
pub struct DynamicPartitionAccessObject {
    /// 数据库名。
    pub Database: String,
    /// 表名或别名。
    pub Table: String,
    /// 命中的分区名列表。
    pub Partitions: Vec<String>,
    /// 是否访问全部分区。
    pub AllPartitions: bool,
    /// 剪枝失败时的错误信息（非空表示失败）。
    pub Err: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 分区剪枝结果：全范围或分区下标列表。
pub enum PartitionPruningResult {
    /// 无法缩小，扫描全部分区。
    FullRange,
    /// 命中的分区定义下标。
    Partitions(Vec<usize>),
}

/// infoschema/分区剪枝集成边界。
/// Boundary supplied by infoschema/partitionpruning integration.
pub trait DynamicPartitionResolver {
    fn dynamic_pruning_enabled(&self) -> bool;
    fn database_name(&self, table: &TableInfo) -> Option<String>;
    fn partition_indices(
        &self,
        table: &TableInfo,
        info: &PhysPlanPartInfo,
    ) -> Result<PartitionPruningResult, expression::Error>;
}

/// 在动态剪枝开启时构造 Explain 用的分区访问对象。
pub fn GetDynamicAccessPartition(
    resolver: &dyn DynamicPartitionResolver,
    table: &TableInfo,
    info: &PhysPlanPartInfo,
    alias: &str,
) -> Option<DynamicPartitionAccessObject> {
    let partition = table.Partition.as_ref()?;
    if !resolver.dynamic_pruning_enabled() {
        return None;
    }
    let mut result = DynamicPartitionAccessObject {
        Database: resolver.database_name(table).unwrap_or_default(),
        Table: if alias.is_empty() {
            table.Name.O.clone()
        } else {
            alias.to_owned()
        },
        ..DynamicPartitionAccessObject::default()
    };
    match resolver.partition_indices(table, info) {
        Ok(PartitionPruningResult::FullRange) => result.AllPartitions = true,
        Ok(PartitionPruningResult::Partitions(indices)) => {
            for index in indices {
                let Some(definition) = partition.Definitions.get(index) else {
                    result.Err = format!("partition index {index} out of range");
                    break;
                };
                result.Partitions.push(definition.Name.O.clone());
            }
        }
        Err(error) => result.Err = format!("partition pruning error:{error}"),
    }
    Some(result)
}

/// 解析生成列（虚拟列）表达式中的列下标。
pub fn ResolveIndicesForVirtualColumn(
    columns: &mut [Column],
    schema: &Schema,
) -> Result<(), expression::Error> {
    for column in columns {
        if let Some(expression) = &column.VirtualExpr {
            column.VirtualExpr = Some(expression.ResolveIndices(schema)?);
        }
    }
    Ok(())
}

/// Plan Cache 专用克隆；失败时返回 (None, false)。
/// Cache cloning remains an explicit physical-plan operation, avoiding an invalid Plan downcast.
pub fn ClonePhysicalPlansForPlanCache(
    context: ContextRef,
    plans: &[Box<dyn PhysicalPlan>],
) -> (Option<Vec<Box<dyn PhysicalPlan>>>, bool) {
    let mut cloned = Vec::with_capacity(plans.len());
    for plan in plans {
        let (_, cacheable) = plan.clone_for_plan_cache(context.clone());
        if !cacheable {
            return (None, false);
        }
        let Ok(plan) = plan.clone_physical(context.clone()) else {
            return (None, false);
        };
        cloned.push(plan);
    }
    (Some(cloned), true)
}

/// 唯一索引键编码适配：归一化 Datum 并编码为字节。
/// Session/table adapter for Go's CastValue plus statement-context key encoding.
pub trait UniqueIndexValueEncoder {
    fn normalize_value(
        &self,
        column: &ColumnInfo,
        value: &Datum,
    ) -> Result<Datum, expression::Error>;
    fn encode_values(&self, values: &[Datum]) -> Result<Vec<u8>, expression::Error>;
}

/// 编码完整唯一索引键：`t{table_id}_i{index_id}` + 列值。
pub fn EncodeUniqueIndexKey(
    encoder: &dyn UniqueIndexValueEncoder,
    table: &TableInfo,
    index: &IndexInfo,
    values: &[Datum],
    table_id: i64,
) -> Result<Vec<u8>, expression::Error> {
    let encoded = EncodeUniqueIndexValuesForKey(encoder, table, index, values)?;
    let mut key = Vec::with_capacity(19 + encoded.len());
    key.push(b't');
    encode_comparable_i64(&mut key, table_id);
    key.extend_from_slice(b"_i");
    encode_comparable_i64(&mut key, index.ID);
    key.extend(encoded);
    Ok(key)
}

/// 可比较序的 i64 大端编码（符号位翻转）。
fn encode_comparable_i64(output: &mut Vec<u8>, value: i64) {
    output.extend_from_slice(&(value as u64 ^ (1_u64 << 63)).to_be_bytes());
}

/// 按索引列顺序归一化并编码索引值部分。
pub fn EncodeUniqueIndexValuesForKey(
    encoder: &dyn UniqueIndexValueEncoder,
    table: &TableInfo,
    index: &IndexInfo,
    values: &[Datum],
) -> Result<Vec<u8>, expression::Error> {
    if values.len() != index.Columns.len() {
        return Err(expression::errors::New(format!(
            "index {} expects {} values, got {}",
            index.Name.O,
            index.Columns.len(),
            values.len()
        )));
    }
    let mut normalized = Vec::with_capacity(values.len());
    for (value, index_column) in values.iter().zip(&index.Columns) {
        let offset = usize::try_from(index_column.Offset)
            .map_err(|_| expression::errors::New("negative index column offset"))?;
        let column = table.Columns.get(offset).ok_or_else(|| {
            expression::errors::New(format!("index column offset {offset} out of range"))
        })?;
        normalized.push(encoder.normalize_value(column, value)?);
    }
    encoder.encode_values(&normalized)
}

/// Plan Cache：二维常量列表浅层克隆。
pub fn CloneConstant2DForPlanCache(constants: &[Vec<Constant>]) -> Vec<Vec<Constant>> {
    constants.iter().map(|row| row.clone()).collect()
}

/// 展开 Schema 中虚拟列依赖的基列；保留末尾 ExtraHandle/PhysTblID。
pub fn ExpandVirtualColumn(
    columns: &[ColumnInfo],
    schema: &mut Schema,
    all_columns: &[ColumnInfo],
) -> Vec<ColumnInfo> {
    let mut copied = columns.to_vec();
    let old_len = schema.Columns.len();
    let extra_count = schema
        .Columns
        .iter()
        .rev()
        .take_while(|column| {
            column.ID == model::ExtraHandleID || column.ID == model::ExtraPhysTblID
        })
        .count();
    if old_len > extra_count && extra_count > 0 {
        // 先摘掉尾部额外列，展开虚拟列依赖后再拼回，保持 Extra* 列在末尾。
        let extra_schema = schema.Columns.split_off(old_len - extra_count);
        let extra_models = copied.split_off(copied.len() - extra_count);
        expand_virtual_column(schema, &mut copied, all_columns);
        schema.Columns.extend(extra_schema);
        copied.extend(extra_models);
    } else {
        expand_virtual_column(schema, &mut copied, all_columns);
    }
    copied
}

/// 收集虚拟表达式依赖列并追加到 Schema/Columns。
fn expand_virtual_column(
    schema: &mut Schema,
    columns: &mut Vec<ColumnInfo>,
    all_columns: &[ColumnInfo],
) {
    let mut dependent = Vec::new();
    for column in &schema.Columns {
        let Some(expression) = &column.VirtualExpr else {
            continue;
        };
        for base_column in expression::ExtractDependentColumns(expression.as_ref()) {
            if !schema.Contains(base_column)
                && !dependent
                    .iter()
                    .any(|existing: &Column| existing.UniqueID == base_column.UniqueID)
            {
                dependent.push(base_column.Clone());
            }
        }
    }
    for column in dependent {
        if let Some(info) = model::FindColumnInfoByID(all_columns, column.ID) {
            columns.push(info.Clone());
            schema.Columns.push(column);
        }
    }
}

/// 按父期望行数与有序选择率，估算下推给子节点的 ExpectedCnt。
pub fn CalcChildExpectedCnt(
    context: &dyn base::PlanContext,
    property: &PhysicalProperty,
    child_row_count: f64,
    estimated_row_count: f64,
) -> f64 {
    let has_order = !property.IsSortItemEmpty();
    let order_ratio = if has_order {
        context
            .GetSessionVars()
            .RecordRelevantOptVar(vardef::TiDBOptOrderingIdxSelRatio);
        context.GetSessionVars().OptOrderingIdxSelRatio
    } else {
        0.0
    };
    if property.ExpectedCnt < estimated_row_count
        // 父层 Limit/TopN 收紧期望行数，或有序扫描需额外探测行时收紧子期望。
        || (has_order
            && order_ratio > 0.0
            && child_row_count > estimated_row_count
            && property.ExpectedCnt < child_row_count
            && estimated_row_count > 0.0)
    {
        let rows_to_meet_first = if has_order && order_ratio > 0.0 {
            ((child_row_count - estimated_row_count) * order_ratio).max(0.0)
        } else {
            0.0
        };
        child_row_count * property.ExpectedCnt / estimated_row_count + rows_to_meet_first
    } else {
        f64::MAX
    }
}

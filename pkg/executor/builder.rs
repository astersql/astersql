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

// 执行器构建器：将物理计划（Physical Plan）树编译为可运行的 Executor。
//
// `executorBuilder` 按 `Plan` / `ExecutorKind` 分发到各类 `buildXxx`，组装
// TableReader、IndexJoin、HashJoin、聚合、DML、DDL/Admin、Analyze 等算子。
// 依赖通过 `ExecutorBuilderDependencies` 注入（会话、快照、遥测、CTE 存储等）。
// 构建入口保持 Go 的“按计划节点逐层分发”形状，便于逐类对照 `buildXxx` 语义。
#![allow(dead_code, non_camel_case_types, non_snake_case)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use astersql_util_memory::tracker::Tracker;

/// Builder results must expose the same typed Chunk lifecycle consumed by the
/// statement adapter; a marker-only executor cannot be opened or drained.
pub trait Executor: crate::adapter::ExecExecutor {}

impl<T: crate::adapter::ExecExecutor + ?Sized> Executor for T {}
/// 执行器装箱类型别名。
pub type ExecutorBox = Box<dyn Executor>;

#[derive(Clone)]
pub struct TypedScanBinding {
    pub table_id: i64,
    pub retriever: Arc<dyn astersql_kv::Retriever + Send + Sync>,
    pub ranges: Vec<crate::typed_kv_scan::KeyRange>,
}

/// Build an owned typed scan from a canonical physical table-scan node and
/// the exact encoded key intervals selected by the planner.
pub fn BuildTypedTableScan(
    scan: &astersql_planner_core_operator_physicalop::PhysicalTableScan,
    retriever: Arc<dyn astersql_kv::Retriever + Send + Sync>,
    ranges: Vec<crate::typed_kv_scan::KeyRange>,
    initial_capacity: usize,
    maximum_chunk_size: usize,
) -> Result<ExecutorBox, BuildError> {
    let table = scan
        .Table
        .as_ref()
        .ok_or_else(|| BuildError::new("PhysicalTableScan has no TableInfo"))?;
    let table_id = if scan.PhysicalTableID != 0 {
        scan.PhysicalTableID
    } else {
        table.ID
    };
    Ok(Box::new(crate::typed_kv_scan::TypedKVScan::new(
        retriever,
        table_id,
        table.PKIsHandle,
        scan.Desc,
        scan.Columns.clone(),
        ranges,
        initial_capacity,
        maximum_chunk_size,
    )))
}

pub fn BuildTypedPointGet(
    point: &astersql_planner_core_operator_physicalop::PointGetPlan,
    retriever: Arc<dyn astersql_kv::Retriever + Send + Sync>,
    initial_capacity: usize,
    maximum_chunk_size: usize,
) -> Result<crate::typed_point_get::TypedPointGet, BuildError> {
    let table = point
        .TblInfo
        .as_ref()
        .ok_or_else(|| BuildError::new("PointGet has no TableInfo"))?;
    let physical_table_id = if let Some(index) = point.PartitionIdx {
        table
            .Partition
            .as_ref()
            .and_then(|partition| partition.Definitions.get(index))
            .map(|definition| definition.ID)
            .ok_or_else(|| BuildError::new("PointGet partition index is invalid"))?
    } else {
        table.ID
    };
    let (index_id, index_columns) = if let Some(index) = point.IndexInfo.as_ref() {
        if !index.Unique || index.Columns.len() != point.IndexValues.len() {
            return Err(BuildError::new(
                "PointGet requires a complete unique index key",
            ));
        }
        (Some(index.ID), index.Columns.len())
    } else {
        (None, 0)
    };
    let point_reader = crate::typed_point_get::TypedPointGet::new(
        retriever,
        table.ID,
        physical_table_id,
        table.PKIsHandle,
        point.Columns.clone(),
        point.Handle,
        index_id,
        point.IndexValues.clone(),
        index_columns,
        point.IsTableDual,
        initial_capacity,
        maximum_chunk_size,
    );
    Ok(if let Some(index) = point.IndexInfo.as_ref() {
        point_reader.WithIndexMetadata(table.clone(), index.clone())
    } else {
        point_reader
    }
    .WithLockPlan(point.Lock, point.LockWaitTime))
}

/// Compile the supported canonical physical tree into a typed, lazy executor.
/// The planner supplies the already encoded interval for the single table scan.
/// Unsupported operators fail explicitly so prepared parameters cannot be
/// executed against an incomplete or silently simplified plan.
pub fn BuildTypedPhysicalPlan(
    plan: &dyn astersql_planner_core_base::PhysicalPlan,
    retriever: Arc<dyn astersql_kv::Retriever + Send + Sync>,
    ranges: Vec<crate::typed_kv_scan::KeyRange>,
    initial_capacity: usize,
    maximum_chunk_size: usize,
) -> Result<ExecutorBox, BuildError> {
    let table_id = find_typed_table_scan(plan)
        .and_then(|scan| {
            scan.Table.as_ref().map(|table| {
                if scan.PhysicalTableID != 0 {
                    scan.PhysicalTableID
                } else {
                    table.ID
                }
            })
        })
        .unwrap_or_default();
    BuildTypedPhysicalPlanWithBindings(
        plan,
        vec![TypedScanBinding {
            table_id,
            retriever,
            ranges,
        }],
        initial_capacity,
        maximum_chunk_size,
    )
}

pub fn BuildTypedPhysicalPlanWithBindings(
    plan: &dyn astersql_planner_core_base::PhysicalPlan,
    bindings: Vec<TypedScanBinding>,
    initial_capacity: usize,
    maximum_chunk_size: usize,
) -> Result<ExecutorBox, BuildError> {
    let mut binding_index = 0;
    build_typed_physical_plan(
        plan,
        &bindings,
        &mut binding_index,
        initial_capacity,
        maximum_chunk_size,
        false,
    )
}

/// Compile a SelectLock tree only for the adapter's locking execution path.
/// Its wrapper is drained by ExecStmt, which acquires returned record keys
/// before exposing any buffered row to the client.
pub fn BuildTypedPhysicalSelectLockPlan(
    plan: &dyn astersql_planner_core_base::PhysicalPlan,
    retriever: Arc<dyn astersql_kv::Retriever + Send + Sync>,
    ranges: Vec<crate::typed_kv_scan::KeyRange>,
    initial_capacity: usize,
    maximum_chunk_size: usize,
) -> Result<ExecutorBox, BuildError> {
    let table_id = find_typed_table_scan(plan)
        .and_then(|scan| {
            scan.Table.as_ref().map(|table| {
                if scan.PhysicalTableID != 0 {
                    scan.PhysicalTableID
                } else {
                    table.ID
                }
            })
        })
        .unwrap_or_default();
    let bindings = vec![TypedScanBinding {
        table_id,
        retriever,
        ranges,
    }];
    let mut binding_index = 0;
    build_typed_physical_plan(
        plan,
        &bindings,
        &mut binding_index,
        initial_capacity,
        maximum_chunk_size,
        true,
    )
}

fn find_typed_index_scan(
    plan: &dyn astersql_planner_core_base::PhysicalPlan,
) -> Option<&astersql_planner_core_operator_physicalop::PhysicalIndexScan> {
    if let Some(scan) = plan
        .as_any()
        .downcast_ref::<astersql_planner_core_operator_physicalop::PhysicalIndexScan>()
    {
        return Some(scan);
    }
    let children = plan.children();
    (children.len() == 1)
        .then(|| find_typed_index_scan(children[0]))
        .flatten()
}

fn find_typed_table_scan(
    plan: &dyn astersql_planner_core_base::PhysicalPlan,
) -> Option<&astersql_planner_core_operator_physicalop::PhysicalTableScan> {
    if let Some(scan) = plan
        .as_any()
        .downcast_ref::<astersql_planner_core_operator_physicalop::PhysicalTableScan>()
    {
        return Some(scan);
    }
    let children = plan.children();
    (children.len() == 1)
        .then(|| find_typed_table_scan(children[0]))
        .flatten()
}

fn typed_scan_binding<'a>(
    bindings: &'a [TypedScanBinding],
    table_id: i64,
    binding_index: &mut usize,
) -> Result<&'a TypedScanBinding, BuildError> {
    let index = bindings[*binding_index..]
        .iter()
        .position(|binding| binding.table_id == table_id)
        .map(|offset| *binding_index + offset)
        .or_else(|| (bindings.len() == 1).then_some(0))
        .ok_or_else(|| {
            BuildError::new(format!("typed scan binding is absent for table {table_id}"))
        })?;
    *binding_index = index + 1;
    Ok(&bindings[index])
}

fn wrap_typed_pushdown_plan(
    plan: &dyn astersql_planner_core_base::PhysicalPlan,
    executor: ExecutorBox,
) -> Result<ExecutorBox, BuildError> {
    use astersql_planner_core_operator_physicalop::{
        PhysicalIndexScan, PhysicalLimit, PhysicalProjection, PhysicalSelection, PhysicalTableScan,
    };

    if plan.as_any().is::<PhysicalIndexScan>() || plan.as_any().is::<PhysicalTableScan>() {
        return Ok(executor);
    }
    let children = plan.children();
    if children.len() != 1 {
        return Err(BuildError::new(
            "typed index pushdown requires a unary canonical subtree",
        ));
    }
    let child = wrap_typed_pushdown_plan(children[0], executor)?;
    if let Some(selection) = plan.as_any().downcast_ref::<PhysicalSelection>() {
        return Ok(Box::new(crate::typed_selection::TypedSelection::new(
            child,
            selection.Conditions.clone(),
            plan.s_ctx().clone(),
        )));
    }
    if let Some(projection) = plan.as_any().downcast_ref::<PhysicalProjection>() {
        return Ok(Box::new(crate::typed_projection::TypedProjection::new(
            child,
            projection.Exprs.clone(),
            plan.s_ctx().clone(),
            projection.CalculateNoDelay,
        )));
    }
    if let Some(limit) = plan.as_any().downcast_ref::<PhysicalLimit>() {
        return Ok(Box::new(crate::typed_limit::TypedLimit::new(
            child,
            limit.Offset,
            limit.Count,
        )));
    }
    Err(BuildError::new("unsupported typed index pushdown operator"))
}

fn build_typed_physical_plan(
    plan: &dyn astersql_planner_core_base::PhysicalPlan,
    bindings: &[TypedScanBinding],
    binding_index: &mut usize,
    initial_capacity: usize,
    maximum_chunk_size: usize,
    locking: bool,
) -> Result<ExecutorBox, BuildError> {
    use astersql_planner_core_operator_physicalop::{
        LegacyPhysicalLock, PhysicalHashAgg, PhysicalHashJoin, PhysicalIndexLookUpReader,
        PhysicalIndexReader, PhysicalIndexScan, PhysicalLimit, PhysicalProjection,
        PhysicalSelection, PhysicalTableDual, PhysicalTableReader, PhysicalTableScan, PointGetPlan,
    };

    if let Some(dual) = plan.as_any().downcast_ref::<PhysicalTableDual>() {
        if !(0..=1).contains(&dual.RowCount) {
            return Err(BuildError::new("invalid row count for dual table"));
        }
        return Ok(Box::new(crate::typed_projection::TypedTableDual::new(
            dual.RowCount as usize,
            plan.schema()
                .Columns
                .iter()
                .map(|column| crate::adapter::SchemaColumn {
                    field_type: column.RetType.clone().unwrap_or_default(),
                })
                .collect(),
            initial_capacity,
            maximum_chunk_size,
        )));
    }
    if let Some(point) = plan.as_any().downcast_ref::<PointGetPlan>() {
        if locking {
            return Err(BuildError::new(
                "locking PointGet requires the canonical point lock runtime",
            ));
        }
        return Ok(Box::new(BuildTypedPointGet(
            point,
            bindings
                .first()
                .ok_or_else(|| BuildError::new("PointGet has no scan binding"))?
                .retriever
                .clone(),
            initial_capacity,
            maximum_chunk_size,
        )?));
    }

    if let Some(lock) = plan.as_any().downcast_ref::<LegacyPhysicalLock>() {
        if !locking || lock.LockType != "for update" {
            return Err(BuildError::new(
                "typed SelectLock requires the adapter's plain FOR UPDATE path",
            ));
        }
        let children = plan.children();
        if children.len() != 1 {
            return Err(BuildError::new("SelectLock requires one child"));
        }
        return build_typed_physical_plan(
            children[0],
            bindings,
            binding_index,
            initial_capacity,
            maximum_chunk_size,
            locking,
        );
    }

    if let Some(join) = plan.as_any().downcast_ref::<PhysicalHashJoin>() {
        if locking {
            return Err(BuildError::new(
                "typed HashJoin does not support SelectLock",
            ));
        }
        if !join.NAEqualConditions.is_empty()
            || !join.BasePhysicalJoin.LeftNAJoinKeys.is_empty()
            || !join.BasePhysicalJoin.RightNAJoinKeys.is_empty()
        {
            return Err(BuildError::new(
                "typed HashJoin does not support null-aware anti join keys",
            ));
        }
        let children = plan.children();
        if children.len() != 2 {
            return Err(BuildError::new("PhysicalHashJoin requires two children"));
        }
        let mut left = build_typed_physical_plan(
            children[0],
            bindings,
            binding_index,
            initial_capacity,
            maximum_chunk_size,
            false,
        )?;
        let mut right = build_typed_physical_plan(
            children[1],
            bindings,
            binding_index,
            initial_capacity,
            maximum_chunk_size,
            false,
        )?;
        if join.BasePhysicalJoin.JoinType != astersql_planner_core_base::JoinType::FullOuterJoin
            && !join.BasePhysicalJoin.LeftConditions.is_empty()
        {
            left = Box::new(crate::typed_selection::TypedSelection::new(
                left,
                join.BasePhysicalJoin.LeftConditions.clone(),
                plan.s_ctx().clone(),
            ));
        }
        if join.BasePhysicalJoin.JoinType != astersql_planner_core_base::JoinType::FullOuterJoin
            && !join.BasePhysicalJoin.RightConditions.is_empty()
        {
            right = Box::new(crate::typed_selection::TypedSelection::new(
                right,
                join.BasePhysicalJoin.RightConditions.clone(),
                plan.s_ctx().clone(),
            ));
        }
        let (left_conditions, right_conditions) = if join.BasePhysicalJoin.JoinType
            == astersql_planner_core_base::JoinType::FullOuterJoin
        {
            (
                join.BasePhysicalJoin.LeftConditions.clone(),
                join.BasePhysicalJoin.RightConditions.clone(),
            )
        } else {
            (Vec::new(), Vec::new())
        };
        return crate::typed_hash_join::TypedHashJoin::new(
            left,
            right,
            join.BasePhysicalJoin.LeftJoinKeys.clone(),
            join.BasePhysicalJoin.RightJoinKeys.clone(),
            join.BasePhysicalJoin.IsNullEQ.clone(),
            left_conditions,
            right_conditions,
            join.BasePhysicalJoin.OtherConditions.clone(),
            plan.s_ctx().clone(),
            join.BasePhysicalJoin.JoinType,
        )
        .map(|executor| Box::new(executor) as ExecutorBox)
        .map_err(BuildError::new);
    }

    if let Some(reader) = plan.as_any().downcast_ref::<PhysicalIndexReader>() {
        let index_plan = reader
            .IndexPlan
            .as_deref()
            .ok_or_else(|| BuildError::new("typed IndexReader requires an IndexPlan"))?;
        let scan = find_typed_index_scan(index_plan)
            .ok_or_else(|| BuildError::new("typed IndexReader requires one PhysicalIndexScan"))?;
        let table = scan
            .Table
            .as_ref()
            .ok_or_else(|| BuildError::new("PhysicalIndexScan has no TableInfo"))?;
        let index = scan
            .Index
            .as_ref()
            .ok_or_else(|| BuildError::new("PhysicalIndexScan has no IndexInfo"))?;
        let table_id = if scan.PhysicalTableID != 0 {
            scan.PhysicalTableID
        } else {
            table.ID
        };
        let binding = typed_scan_binding(bindings, table_id, binding_index)?;
        let index_column_ids = index
            .Columns
            .iter()
            .map(|column| {
                usize::try_from(column.Offset)
                    .ok()
                    .and_then(|offset| table.Columns.get(offset))
                    .map(|column| column.ID)
                    .ok_or_else(|| BuildError::new("index column offset is outside TableInfo"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let output_columns = if reader.OutputColumns.is_empty() {
            scan.Columns.clone()
        } else {
            reader
                .OutputColumns
                .iter()
                .map(|output| {
                    scan.Columns
                        .iter()
                        .find(|column| column.ID == output.ID)
                        .cloned()
                        .ok_or_else(|| {
                            BuildError::new("IndexReader output column is not in IndexScan")
                        })
                })
                .collect::<Result<Vec<_>, _>>()?
        };
        for column in &output_columns {
            if !index_column_ids.contains(&column.ID)
                && !astersql_parser_mysql::r#type::HasPriKeyFlag(column.GetFlag())
            {
                return Err(BuildError::new(format!(
                    "column {} is not covered by index",
                    column.Name.O
                )));
            }
        }
        let mut executor: ExecutorBox = Box::new(crate::typed_index_reader::TypedIndexReader::new(
            binding.retriever.clone(),
            table_id,
            index_column_ids,
            output_columns,
            scan.Desc,
            binding.ranges.clone(),
            initial_capacity,
            maximum_chunk_size,
        ));
        if !scan.FilterCondition.is_empty() {
            executor = Box::new(crate::typed_selection::TypedSelection::new(
                executor,
                scan.FilterCondition.clone(),
                plan.s_ctx().clone(),
            ));
        }
        executor = wrap_typed_pushdown_plan(index_plan, executor)?;
        return Ok(executor);
    }

    if let Some(reader) = plan.as_any().downcast_ref::<PhysicalIndexLookUpReader>() {
        let index_plan = reader
            .IndexPlan
            .as_deref()
            .ok_or_else(|| BuildError::new("typed index lookup requires an IndexPlan"))?;
        let index = find_typed_index_scan(index_plan)
            .ok_or_else(|| BuildError::new("typed index lookup requires one PhysicalIndexScan"))?;
        let table_plan = reader
            .TablePlan
            .as_deref()
            .ok_or_else(|| BuildError::new("typed index lookup requires a TablePlan"))?;
        let table = find_typed_table_scan(table_plan)
            .ok_or_else(|| BuildError::new("typed index lookup requires one PhysicalTableScan"))?;
        let index_info = index
            .Index
            .as_ref()
            .ok_or_else(|| BuildError::new("PhysicalIndexScan has no IndexInfo"))?;
        let table_info = table
            .Table
            .as_ref()
            .ok_or_else(|| BuildError::new("PhysicalTableScan has no TableInfo"))?;
        let table_id = if table.PhysicalTableID != 0 {
            table.PhysicalTableID
        } else {
            table_info.ID
        };
        let binding = typed_scan_binding(bindings, table_id, binding_index)?;
        let record_decoder = crate::typed_kv_scan::TypedKVScan::new(
            binding.retriever.clone(),
            table_id,
            table_info.PKIsHandle,
            table.Desc,
            table.Columns.clone(),
            Vec::new(),
            initial_capacity,
            maximum_chunk_size,
        );
        let mut executor: ExecutorBox = Box::new(crate::typed_index_lookup::TypedIndexLookUp::new(
            binding.retriever.clone(),
            record_decoder,
            index_info.Columns.len(),
            table_id,
            index.Desc,
            binding.ranges.clone(),
        ));
        if !index.FilterCondition.is_empty() {
            executor = Box::new(crate::typed_selection::TypedSelection::new(
                executor,
                index.FilterCondition.clone(),
                plan.s_ctx().clone(),
            ));
        }
        executor = wrap_typed_pushdown_plan(index_plan, executor)?;
        if !table.FilterCondition.is_empty() {
            executor = Box::new(crate::typed_selection::TypedSelection::new(
                executor,
                table.FilterCondition.clone(),
                plan.s_ctx().clone(),
            ));
        }
        executor = wrap_typed_pushdown_plan(table_plan, executor)?;
        if let Some(limit) = reader.PushedLimit {
            executor = Box::new(crate::typed_limit::TypedLimit::new(
                executor,
                limit.Offset,
                limit.Count,
            ));
        }
        return Ok(executor);
    }

    if let Some(reader) = plan.as_any().downcast_ref::<PhysicalTableReader>() {
        let child = reader
            .GetTablePlan()
            .ok_or_else(|| BuildError::new("PhysicalTableReader has no TablePlan"))?;
        return build_typed_physical_plan(
            child,
            bindings,
            binding_index,
            initial_capacity,
            maximum_chunk_size,
            locking,
        );
    }
    if let Some(aggregate) = plan.as_any().downcast_ref::<PhysicalHashAgg>() {
        let children = plan.children();
        if children.len() != 1 {
            return Err(BuildError::new("PhysicalHashAgg requires one child"));
        }
        let child = build_typed_physical_plan(
            children[0],
            bindings,
            binding_index,
            initial_capacity,
            maximum_chunk_size,
            locking,
        )?;
        return crate::typed_hash_agg::TypedHashAgg::new(
            child,
            aggregate.BasePhysicalAgg.AggFuncs.clone(),
            aggregate.BasePhysicalAgg.GroupByItems.clone(),
            plan.s_ctx().clone(),
        )
        .map(|executor| Box::new(executor) as ExecutorBox)
        .map_err(BuildError::new);
    }
    if let Some(limit) = plan.as_any().downcast_ref::<PhysicalLimit>() {
        let children = plan.children();
        if children.len() != 1 {
            return Err(BuildError::new("PhysicalLimit requires one child"));
        }
        let child = build_typed_physical_plan(
            children[0],
            bindings,
            binding_index,
            initial_capacity,
            maximum_chunk_size,
            locking,
        )?;
        return Ok(Box::new(crate::typed_limit::TypedLimit::new(
            child,
            limit.Offset,
            limit.Count,
        )));
    }
    if let Some(selection) = plan.as_any().downcast_ref::<PhysicalSelection>() {
        let children = plan.children();
        if children.len() != 1 {
            return Err(BuildError::new("PhysicalSelection requires one child"));
        }
        let child = build_typed_physical_plan(
            children[0],
            bindings,
            binding_index,
            initial_capacity,
            maximum_chunk_size,
            locking,
        )?;
        return Ok(Box::new(crate::typed_selection::TypedSelection::new(
            child,
            selection.Conditions.clone(),
            plan.s_ctx().clone(),
        )));
    }
    if let Some(projection) = plan.as_any().downcast_ref::<PhysicalProjection>() {
        let children = plan.children();
        if children.len() != 1 {
            return Err(BuildError::new("PhysicalProjection requires one child"));
        }
        let child = build_typed_physical_plan(
            children[0],
            bindings,
            binding_index,
            initial_capacity,
            maximum_chunk_size,
            locking,
        )?;
        return Ok(Box::new(crate::typed_projection::TypedProjection::new(
            child,
            projection.Exprs.clone(),
            plan.s_ctx().clone(),
            projection.CalculateNoDelay,
        )));
    }
    if let Some(scan) = plan.as_any().downcast_ref::<PhysicalTableScan>() {
        let table = scan
            .Table
            .as_ref()
            .ok_or_else(|| BuildError::new("PhysicalTableScan has no TableInfo"))?;
        let table_id = if scan.PhysicalTableID != 0 {
            scan.PhysicalTableID
        } else {
            table.ID
        };
        let binding = typed_scan_binding(bindings, table_id, binding_index)?;
        let child = BuildTypedTableScan(
            scan,
            binding.retriever.clone(),
            binding.ranges.clone(),
            initial_capacity,
            maximum_chunk_size,
        )?;
        if scan.FilterCondition.is_empty() {
            return Ok(child);
        }
        return Ok(Box::new(crate::typed_selection::TypedSelection::new(
            child,
            scan.FilterCondition.clone(),
            plan.s_ctx().clone(),
        )));
    }
    Err(BuildError::new(format!(
        "typed physical builder does not support {}",
        plan.tp(&[])
    )))
}

#[derive(Debug, Clone, Eq, PartialEq)]
/// 构建执行器失败时的错误类型。
pub struct BuildError {
    message: String,
}

impl BuildError {
    /// 构造。
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for BuildError {
    /// fmt。
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for BuildError {}

/// 语句上下文抽象接口。
pub trait StatementContext: Send + Sync {}
/// 会话上下文抽象接口。
pub trait SessionContext: Send + Sync {}
/// 信息模式/列集合抽象接口。
pub trait InfoSchema: Send + Sync {}
/// 遥测抽象接口。
pub trait Telemetry: Send + Sync {}
/// 公用表表达式 CTE存储抽象接口。
pub trait CteStorage: Send + Sync {}
/// 公用表表达式 CTE生产者抽象接口。
pub trait CteProducer: Send + Sync {}
/// 系统会话抽象接口。
pub trait SystemSession: Send {}
/// Ddl信息抽象接口。
pub trait DdlInfo: Send {}
/// 已解析语句抽象接口。
pub trait ParsedStatement: Send {}
/// 计划的构建数据接口。
pub trait PlanData: Send + Sync {}
/// 快照抽象接口。
pub trait Snapshot: Send {
    /// 设置option。
    fn set_option(&mut self, option: SnapshotOption) -> Result<(), BuildError>;
}

#[derive(Clone, Debug, PartialEq)]
/// 快照选项枚举。
pub enum SnapshotOption {
    ReadReplicaScope(String),
    TaskId(u64),
    ReadTimeoutMillis(u64),
    ResourceGroupName(String),
    ExplicitRequestSourceType(String),
    MatchStoreLabel { key: String, value: String },
    ReplicaReadAdjuster(ReplicaReadAdjuster),
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 快照会话选项s。
pub struct SnapshotSessionOptions {
    pub transaction_read_replica_scope: String,
    pub replica_read_is_closest: bool,
    pub global_transaction_scope: String,
    pub dc_label_key: String,
    pub task_id: u64,
    pub read_timeout_millis: u64,
    pub resource_group_name: String,
    pub explicit_request_source_type: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 已解析表。
pub struct ResolvedTable {
    pub id: i64,
    pub name: String,
    pub is_base_table: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 哈希连接构建配置。
pub struct HashJoinBuildConfig {
    pub right_as_build_side: bool,
    pub left_as_build_side: bool,
}

/// 公用表表达式 CTE存储s。
pub struct CTEStorages {
    pub res_tbl: Mutex<Option<Arc<dyn CteStorage>>>,
    pub iter_in_tbl: Mutex<Option<Arc<dyn CteStorage>>>,
    pub producer: Mutex<Option<Arc<dyn CteProducer>>>,
    pub init_result: OnceLock<Result<(), BuildError>>,
}

impl CTEStorages {
    /// Remove partially initialized CTE resources after producer construction fails.
    pub fn clear_after_build_error(&self) {
        *self
            .res_tbl
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        *self
            .iter_in_tbl
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        *self
            .producer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }

    /// Report whether every producer-owned CTE resource has been released.
    pub fn is_cleared(&self) -> bool {
        self.res_tbl
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_none()
            && self
                .iter_in_tbl
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_none()
            && self
                .producer
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_none()
    }
}

#[derive(Debug, Default)]
/// 账户锁遥测信息。
pub struct AccountLockTelemetryInfo {
    pub create_or_alter_user: u64,
    pub lock_user: u64,
    pub unlock_user: u64,
}

#[derive(Debug, Default)]
/// 遥测信息。
pub struct TelemetryInfo {
    pub account_lock: Option<AccountLockTelemetryInfo>,
    pub use_multi_schema_change: bool,
    pub use_exchange_partition: bool,
    pub use_flashback_to_cluster: bool,
    pub partition: Option<PartitionTelemetryInfo>,
    pub use_non_recursive_cte: bool,
    pub use_recursive_cte: bool,
}

#[derive(Debug, Default)]
/// 分区遥测信息。
pub struct PartitionTelemetryInfo {
    pub use_drop_interval_partition: bool,
    pub use_add_interval_partition: bool,
    pub use_reorganize_partition: bool,
    pub table_partition_max_partitions_num: u64,
    pub use_table_partition: bool,
    pub use_table_partition_range: bool,
    pub use_table_partition_range_columns: bool,
    pub use_table_partition_range_columns_gt_1: bool,
    pub use_table_partition_range_columns_gt_2: bool,
    pub use_table_partition_range_columns_gt_3: bool,
    pub use_create_interval_partition: bool,
    pub use_table_partition_hash: bool,
    pub use_table_partition_list: bool,
    pub use_table_partition_list_columns: bool,
    pub use_compact_table_partition: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 执行器种类：对应各类物理/管理算子的构建入口。
pub enum ExecutorKind {
    CancelDdlJobs,
    PauseDdlJobs,
    ResumeDdlJobs,
    AlterDdlJob,
    ShowNextRowId,
    ShowDdlJobs,
    ShowDdlJobQueries,
    ShowDdlJobQueriesWithRange,
    ShowSlow,
    IndexLookupChecker,
    FastCheckTable,
    CheckTable,
    RecoverIndex,
    CleanupIndex,
    CheckIndexRange,
    ChecksumTable,
    ReloadExprPushdownBlacklist,
    ReloadOptRuleBlacklist,
    AdminPlugins,
    Deallocate,
    SelectLock,
    Limit,
    Prepare,
    Execute,
    Show,
    Grant,
    Revoke,
    Brie,
    CalibrateResource,
    AddQueryWatch,
    ImportIntoAction,
    CancelDistributionJob,
    Simple,
    Set,
    SetConfig,
    Replace,
    Insert,
    LoadStats,
    LockStats,
    UnlockStats,
    GrantDdl,
    RevokeDdl,
    Ddl,
    Trace,
    TraceSorted,
    Explain,
    SelectInto,
    UnionScan,
    MergeJoin,
    HashJoinV1,
    HashJoinV2,
    HashAgg,
    StreamAgg,
    Selection,
    Expand,
    Projection,
    TableDual,
    MemTable,
    Sort,
    TopN,
    ApplySerial,
    ApplyParallel,
    MaxOneRow,
    UnionAll,
    DistributeTable,
    SplitIndexRegion,
    SplitTableRegion,
    Update,
    Delete,
    Analyze,
    IndexLookupJoin,
    IndexLookupMergeJoin,
    IndexNestedLoopHashJoin,
    MppGather,
    TableReader,
    IndexReader,
    IndexLookupReader,
    IndexMergeReader,
    Window,
    Shuffle,
    ShuffleReceiver,
    SqlBind,
    BatchPointGet,
    TableSample,
    Cte,
    CteTable,
    CompactTable,
    AdminShowBdrRole,
    RecommendIndex,
    WorkloadRepoCreate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 延迟计划种类枚举。
pub enum DeferredPlanKind {
    PointGet,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 简单语句语句种类枚举。
pub enum SimpleStatementKind {
    Grant,
    Revoke,
    Brie,
    CreateUser,
    AlterUser,
    CalibrateResource,
    AddQueryWatch,
    ImportIntoAction,
    CancelDistributionJob,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 账户锁选项枚举。
pub enum AccountLockOption {
    Lock,
    Unlock,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 计划重放器模式枚举。
pub enum PlanReplayerMode {
    Load,
    Capture,
    Remove,
    Dump,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 流量操作枚举。
pub enum TrafficOperation {
    Capture,
    Replay,
    Cancel,
    Show,
}

#[derive(Clone, Debug, PartialEq)]
/// 流量选项枚举。
pub enum TrafficOption {
    Duration(String),
    EncryptionMethod(String),
    Compress(bool),
    Username(String),
    Password(String),
    Speed(Option<String>),
    ReadOnly(bool),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 变更表遥测规格枚举。
pub enum AlterTableTelemetrySpec {
    DropFirstPartition,
    AddLastPartition,
    ExchangePartition,
    ReorganizePartition,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 分区遥测种类枚举。
pub enum PartitionTelemetryKind {
    Range,
    Hash,
    List,
    Other,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 创建表分区遥测。
pub struct CreateTablePartitionTelemetry {
    pub kind: PartitionTelemetryKind,
    pub declared_partitions: u64,
    pub definition_count: usize,
    pub column_count: usize,
    pub has_subpartition: bool,
    pub has_interval: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Ddl遥测事件枚举。
pub enum DdlTelemetryEvent {
    AlterTable(Vec<AlterTableTelemetrySpec>),
    CreateTable(Option<CreateTablePartitionTelemetry>),
    FlashbackToTimestamp,
    Other,
}

/// CHECK TABLE计划的构建数据接口。
pub trait CheckTablePlanData: PlanData {
    /// fastcheckenabled。
    fn fast_check_enabled(&self) -> bool;
    /// indexessupportfastcheck。
    fn indexes_support_fast_check(&self) -> bool;
}

/// SELECT FOR UPDATE 锁计划的构建数据接口。
pub trait SelectLockPlanData: PlanData {
    /// childplan。
    fn child_plan(&self) -> Plan<'_>;
    /// pessimisticlockeligible。
    fn pessimistic_lock_eligible(&self) -> bool;
}

/// 限制计划的构建数据接口。
pub trait LimitPlanData: PlanData {
    /// childplan。
    fn child_plan(&self) -> Plan<'_>;
    /// count。
    fn count(&self) -> u64;
    /// maxchunksize。
    fn max_chunk_size(&self) -> usize;
    /// childcolumncount。
    fn child_column_count(&self) -> usize;
    /// usedchildcolumns。
    fn used_child_columns(&self) -> &[usize];
}

/// EXECUTE计划的构建数据接口。
pub trait ExecutePlanData: PlanData {}

/// SHOW计划的构建数据接口。
pub trait ShowPlanData: PlanData {
    /// needsstartts。
    fn needs_start_ts(&self) -> bool;
}

/// 简单语句计划的构建数据接口。
pub trait SimplePlanData: PlanData {
    /// statementkind。
    fn statement_kind(&self) -> SimpleStatementKind;
    /// accountlockoptions。
    fn account_lock_options(&self) -> &[AccountLockOption];
}

/// 插入计划的构建数据接口。
pub trait InsertPlanData: PlanData {
    /// selectplan。
    fn select_plan(&self) -> Option<Plan<'_>>;
    /// isreplace。
    fn is_replace(&self) -> bool;
}

/// IMPORT INTO计划的构建数据接口。
pub trait ImportIntoPlanData: PlanData {
    /// targettableid。
    fn target_table_id(&self) -> i64;
    /// selectplan。
    fn select_plan(&self) -> Option<Plan<'_>>;
}

/// LOAD DATA计划的构建数据接口。
pub trait LoadDataPlanData: PlanData {
    /// targettableid。
    fn target_table_id(&self) -> i64;
}

/// 计划重放器计划的构建数据接口。
pub trait PlanReplayerPlanData: PlanData {
    /// mode。
    fn mode(&self) -> PlanReplayerMode;
    /// statementsql。
    fn statement_sql(&self) -> &[String];
    /// hassinglestatement。
    fn has_single_statement(&self) -> bool;
}

/// 流量计划的构建数据接口。
pub trait TrafficPlanData: PlanData {
    /// operation。
    fn operation(&self) -> TrafficOperation;
    /// directory。
    fn directory(&self) -> &str;
    /// options。
    fn options(&self) -> &[TrafficOption];
}

/// Ddl计划的构建数据接口。
pub trait DdlPlanData: PlanData {
    /// telemetryevent。
    fn telemetry_event(&self) -> DdlTelemetryEvent;
}

/// TRACE计划的构建数据接口。
pub trait TracePlanData: PlanData {
    /// logformat。
    fn log_format(&self) -> bool;
    /// optimizertrace。
    fn optimizer_trace(&self) -> bool;
}

/// EXPLAIN计划的构建数据接口。
pub trait ExplainPlanData: PlanData {
    /// analyze。
    fn analyze(&self) -> bool;
    /// hasbriefbinaryplan。
    fn has_brief_binary_plan(&self) -> bool;
    /// targetplan。
    fn target_plan(&self) -> Option<Plan<'_>>;
}

/// SELECT INTO计划的构建数据接口。
pub trait SelectIntoPlanData: PlanData {
    /// targetplan。
    fn target_plan(&self) -> Plan<'_>;
}

/// 联合扫描（含未提交写）计划的构建数据接口。
pub trait UnionScanPlanData: PlanData {
    /// childplan。
    fn child_plan(&self) -> Plan<'_>;
}

/// 归并连接计划的构建数据接口。
pub trait MergeJoinPlanData: PlanData {
    /// leftplan。
    fn left_plan(&self) -> Plan<'_>;
    /// rightplan。
    fn right_plan(&self) -> Plan<'_>;
    /// innerfiltercount。
    fn inner_filter_count(&self) -> usize;
}

/// 哈希连接计划的构建数据接口。
pub trait HashJoinPlanData: PlanData {
    /// leftplan。
    fn left_plan(&self) -> Plan<'_>;
    /// rightplan。
    fn right_plan(&self) -> Plan<'_>;
    /// sessionusesv2。
    fn session_uses_v2(&self) -> bool;
    /// v2supported。
    fn v2_supported(&self) -> bool;
    /// canusev2。
    fn can_use_v2(&self) -> bool;
    /// innerchildindex。
    fn inner_child_index(&self) -> usize;
    /// useoutertobuild。
    fn use_outer_to_build(&self) -> bool;
    /// leftconditioncount。
    fn left_condition_count(&self) -> usize;
    /// rightconditioncount。
    fn right_condition_count(&self) -> usize;
    /// childrenusedcolumnsavailable。
    fn children_used_columns_available(&self) -> bool;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 聚合函数种类枚举。
pub enum AggregateFunctionKind {
    Average,
    GroupConcat,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 聚合函数规格。
pub struct AggregateFunctionSpec {
    pub kind: AggregateFunctionKind,
    pub order_by_count: usize,
    pub distinct: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 哈希聚合构建配置。
pub struct HashAggBuildConfig {
    pub allocate_default_row: bool,
    pub unparallel: bool,
    pub has_distinct: bool,
    pub partial_ordinals: Vec<Vec<usize>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 流式聚合构建配置。
pub struct StreamAggBuildConfig {
    pub allocate_default_row: bool,
}

/// 哈希聚合计划的构建数据接口。
pub trait HashAggPlanData: PlanData {
    /// childplan。
    fn child_plan(&self) -> Plan<'_>;
    /// 分组bycount。
    fn group_by_count(&self) -> usize;
    /// allfirstrow。
    fn all_first_row(&self) -> bool;
    /// isfinalaggregation。
    fn is_final_aggregation(&self) -> bool;
    /// functions。
    fn functions(&self) -> &[AggregateFunctionSpec];
    /// finalconcurrency。
    fn final_concurrency(&self) -> isize;
    /// partialconcurrency。
    fn partial_concurrency(&self) -> isize;
}

/// 流式聚合计划的构建数据接口。
pub trait StreamAggPlanData: PlanData {
    /// childplan。
    fn child_plan(&self) -> Plan<'_>;
    /// 分组bycount。
    fn group_by_count(&self) -> usize;
    /// allfirstrow。
    fn all_first_row(&self) -> bool;
    /// isfinalaggregation。
    fn is_final_aggregation(&self) -> bool;
}

/// 一元计划的构建数据接口。
pub trait UnaryPlanData: PlanData {
    /// childplan。
    fn child_plan(&self) -> Plan<'_>;
}

/// 并行一元计划的构建数据接口。
pub trait ParallelUnaryPlanData: UnaryPlanData {
    /// configuredworkers。
    fn configured_workers(&self) -> i64;
    /// estimatedrows。
    fn estimated_rows(&self) -> i64;
    /// maxchunksize。
    fn max_chunk_size(&self) -> i64;
}

/// TableDual计划的构建数据接口。
pub trait TableDualPlanData: PlanData {
    /// rowcount。
    fn row_count(&self) -> usize;
}

/// TopN计划的构建数据接口。
pub trait TopNPlanData: UnaryPlanData {
    /// childprojection。
    fn child_projection(&self) -> Option<TopNProjection>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// TopN投影。
pub struct TopNProjection {
    pub used_child_columns: Vec<usize>,
    pub column_missing: bool,
}

/// 相关子查询 Apply计划的构建数据接口。
pub trait ApplyPlanData: PlanData {
    /// leftplan。
    fn left_plan(&self) -> Plan<'_>;
    /// rightplan。
    fn right_plan(&self) -> Plan<'_>;
    /// concurrency。
    fn concurrency(&self) -> usize;
}

/// UNION ALL计划的构建数据接口。
pub trait UnionAllPlanData: PlanData {
    /// childcount。
    fn child_count(&self) -> usize;
    /// childplan。
    fn child_plan(&self, index: usize) -> Plan<'_>;
}

/// 分裂 Region计划的构建数据接口。
pub trait SplitRegionPlanData: PlanData {
    /// hasindex。
    fn has_index(&self) -> bool;
    /// hasvaluelists。
    fn has_value_lists(&self) -> bool;
    /// tableinfo。
    fn table_info(&self) -> &TableInfo;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 赋值规格。
pub struct AssignmentSpec {
    pub column_index: usize,
    pub column_id: i64,
    pub table_index: Option<usize>,
}

/// 更新计划的构建数据接口。
pub trait UpdatePlanData: PlanData {
    /// selectplan。
    fn select_plan(&self) -> Plan<'_>;
    /// schemalen。
    fn schema_len(&self) -> usize;
    /// assignments。
    fn assignments(&self) -> &[AssignmentSpec];
    /// allowwriterowid。
    fn allow_write_row_id(&self) -> bool;
}

/// 删除计划的构建数据接口。
pub trait DeletePlanData: PlanData {
    /// selectplan。
    fn select_plan(&self) -> Plan<'_>;
}

/// 统计信息收集索引任务数据抽象接口。
pub trait AnalyzeIndexTaskData: Send + Sync {}

/// 统计信息收集列s任务数据抽象接口。
pub trait AnalyzeColumnsTaskData: Send + Sync {
    /// configuredsamplerate。
    fn configured_sample_rate(&self) -> f64;
}

/// 统计信息收集计划的构建数据接口。
pub trait AnalyzePlanData: PlanData {
    /// columntaskcount。
    fn column_task_count(&self) -> usize;
    /// columntask。
    fn column_task(&self, index: usize) -> &dyn AnalyzeColumnsTaskData;
    /// indextaskcount。
    fn index_task_count(&self) -> usize;
    /// indextask。
    fn index_task(&self, index: usize) -> &dyn AnalyzeIndexTaskData;
    /// autoanalyze。
    fn auto_analyze(&self) -> bool;
}

/// 统计信息收集任务抽象接口。
pub trait AnalyzeTask: Send {}

#[derive(Clone, Copy, Debug, PartialEq)]
/// 统计信息收集表计数。
pub struct AnalyzeTableCounts {
    pub stats_meta_count: Option<i64>,
    pub approximate_storage_count: Option<f64>,
    pub base_count: i64,
    pub base_modify_count: i64,
}

#[derive(Clone, Debug, PartialEq)]
/// 统计信息收集采样配置。
pub struct AnalyzeSamplingConfig {
    pub snapshot_ts: u64,
    pub sample_rate: f64,
    pub sample_rate_reason: String,
    pub base_count: i64,
    pub base_modify_count: i64,
}

/// 分布式计划的构建数据接口。
pub trait DistributedPlanData: PlanData {
    /// hascorrelatedcolumninsupportedexpression。
    fn has_correlated_column_in_supported_expression(&self) -> bool;
    /// hascorrelatedcolumninaccesscondition。
    fn has_correlated_column_in_access_condition(&self) -> bool;
}

/// 数据读取器构建器。
pub struct DataReaderBuilder {
    pub snapshot_ts: u64,
    pub index_join_key_unique_ids: Vec<i64>,
    pub statement_context_lock: Arc<Mutex<()>>,
    dependencies: Arc<dyn ExecutorBuilderDependencies>,
    partition_pruning_result: Arc<OnceLock<Result<Vec<i64>, BuildError>>>,
}

/// 索引连接计划的构建数据接口。
pub trait IndexJoinPlanData: PlanData {
    /// outerplan。
    fn outer_plan(&self) -> Plan<'_>;
    /// innerplan。
    fn inner_plan(&self) -> &dyn DistributedPlanData;
    /// innerchildindex。
    fn inner_child_index(&self) -> usize;
    /// innerfiltercount。
    fn inner_filter_count(&self) -> usize;
}

/// 表读取计划的构建数据接口。
pub trait TableReaderPlanData: DistributedPlanData {
    /// usempp。
    fn use_mpp(&self) -> bool;
    /// istiflashbatchcop。
    fn is_tiflash_batch_cop(&self) -> bool;
    /// hasvirtualcolumns。
    fn has_virtual_columns(&self) -> bool;
    /// tablescancount。
    fn table_scan_count(&self) -> usize;
    /// byitems。
    fn by_items(&self) -> &[OrderByExpression];
    /// dynamicpartitionpruning。
    fn dynamic_partition_pruning(&self) -> bool;
    /// alreadypartitionreader。
    fn already_partition_reader(&self) -> bool;
    /// haspartitioninfo。
    fn has_partition_info(&self) -> bool;
}

/// 表读取草稿草稿接口。
pub trait TableReaderDraft: Send {}
/// 旁路数据源执行器抽象接口。
pub trait bypassDataSourceExecutor: Executor {}
/// table统计预加载抽象接口。
pub trait tableStatsPreloader: DistributedPlanData {}
/// 索引读取草稿草稿接口。
pub trait IndexReaderDraft: Send {}
/// 索引回表读取器草稿草稿接口。
pub trait IndexLookupReaderDraft: Send {}
/// 索引合并读取器草稿草稿接口。
pub trait IndexMergeReaderDraft: Send {}
/// DAG 请求请求抽象接口。
pub trait DagRequest: Send {}
/// 索引使用情况报告器抽象接口。
pub trait IndexUsageReporter: Send {}

/// 索引读取计划的构建数据接口。
pub trait IndexReaderPlanData: DistributedPlanData {
    /// byitems。
    fn by_items(&self) -> &[OrderByExpression];
}

/// 索引回表读取器计划的构建数据接口。
pub trait IndexLookupReaderPlanData: DistributedPlanData {}
/// 索引合并读取器计划的构建数据接口。
pub trait IndexMergeReaderPlanData: DistributedPlanData {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 数据读取器计划种类枚举。
pub enum DataReaderPlanKind {
    TableReader,
    IndexReader,
    IndexLookupReader,
    UnionScan,
    Projection,
    HashJoin,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 数据读取器构建种类枚举。
pub enum DataReaderBuildKind {
    Generic,
    HashJoin,
    UnionScan,
    TableReader,
    IndexReader,
    IndexLookupReader,
    Projection,
}

/// 数据读取器计划的构建数据接口。
pub trait DataReaderPlanData: DistributedPlanData {
    /// kind。
    fn kind(&self) -> DataReaderPlanKind;
    /// schemauniqueids。
    fn schema_unique_ids(&self) -> &[i64];
    /// lookupchildindex。
    fn lookup_child_index(&self) -> Option<usize>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 索引连接回表内容。
/// Index Join 单次探测内容：分区、行句柄与键值。
pub struct IndexJoinLookUpContent {
    pub partition_id: i64,
    pub handle: Vec<u8>,
    pub key_values: Vec<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 键范围。
pub struct KeyRange {
    pub start_key: Vec<u8>,
    pub end_key: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 逻辑范围。
pub struct LogicalRange {
    pub low: Vec<i64>,
    pub high: Vec<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 索引连接分区范围s。
pub struct IndexJoinPartitionRanges {
    pub partition_ids: Vec<i64>,
    pub key_ranges: Vec<Vec<KeyRange>>,
    pub lookup_contents: Vec<Vec<IndexJoinLookUpContent>>,
}

/// index连接分区范围s类型别名。
pub type indexJoinPartitionRanges = IndexJoinPartitionRanges;
/// data读取器构建器类型别名。
pub type dataReaderBuilder = DataReaderBuilder;

/// data读取器构建器一次性。
pub struct dataReaderBuilderOnce {
    pub partition_pruning_result: OnceLock<Result<Vec<i64>, BuildError>>,
}

/// mock物理索引读取。
pub struct mockPhysicalIndexReader;

#[derive(Clone, Copy, Debug, PartialEq)]
/// 就近读调节器。
pub struct ClosestReadAdjuster {
    pub net_data_size: f64,
}

/// 窗口函数计划的构建数据接口。
pub trait WindowPlanData: UnaryPlanData {}

/// Shuffle计划的构建数据接口。
pub trait ShufflePlanData: PlanData {
    /// datasourcecount。
    fn data_source_count(&self) -> usize;
    /// datasourceplan。
    fn data_source_plan(&self, index: usize) -> Plan<'_>;
    /// workercount。
    fn worker_count(&self) -> usize;
    /// workerplan。
    fn worker_plan(&self, index: usize) -> Plan<'_>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 临时表种类枚举。
pub enum TemporaryTableKind {
    None,
    Local,
    Global,
}

/// 表读规格抽象接口。
pub trait TableReadSpec: Send + Sync {
    /// tableid。
    fn table_id(&self) -> i64;
    /// tablename。
    fn table_name(&self) -> &str;
    /// cacheenabled。
    fn cache_enabled(&self) -> bool;
    /// temporarytablekind。
    fn temporary_table_kind(&self) -> TemporaryTableKind;
    /// sessionsnapshotts。
    fn session_snapshot_ts(&self) -> u64;
    /// transactionisstaleness。
    fn transaction_is_staleness(&self) -> bool;
    /// inexplainstatement。
    fn in_explain_statement(&self) -> bool;
}

/// 批量点查询裁剪结果。
pub struct BatchPointGetPruneResult {
    pub handles: Vec<Vec<u8>>,
    pub index_value_count: usize,
    pub table_dual: bool,
    pub partition_ids: Vec<i64>,
}

/// 批量点查询计划的构建数据接口。
pub trait BatchPointGetPlanData: PlanData {
    /// table。
    fn table(&self) -> &dyn TableReadSpec;
    /// lock。
    fn lock(&self) -> bool;
    /// averagerowsize。
    fn average_row_size(&self) -> f64;
    /// closestreadadaptive。
    fn closest_read_adaptive(&self) -> bool;
    /// closestreadthresholdbytes。
    fn closest_read_threshold_bytes(&self) -> i64;
    /// transactionscope。
    fn transaction_scope(&self) -> &str;
}

/// 表采样计划的构建数据接口。
pub trait TableSamplePlanData: PlanData {
    /// table。
    fn table(&self) -> &dyn TableReadSpec;
    /// usestidbregionsampling。
    fn uses_tidb_region_sampling(&self) -> bool;
}

/// 公用表表达式 CTE计划的构建数据接口。
pub trait CtePlanData: PlanData {
    /// storageid。
    fn storage_id(&self) -> i64;
    /// seedplan。
    fn seed_plan(&self) -> Option<Plan<'_>>;
    /// recursiveplan。
    fn recursive_plan(&self) -> Option<Plan<'_>>;
}

/// 公用表表达式 CTE表计划的构建数据接口。
pub trait CteTablePlanData: PlanData {
    /// storageid。
    fn storage_id(&self) -> i64;
}

/// 内存缓冲抽象接口。
pub trait MemoryBuffer: Send {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 压缩副本种类枚举。
pub enum CompactReplicaKind {
    TiFlash,
    All,
    Unsupported,
}

/// COMPACT TABLE计划的构建数据接口。
pub trait CompactTablePlanData: PlanData {
    /// replicakind。
    fn replica_kind(&self) -> CompactReplicaKind;
    /// partitionnames。
    fn partition_names(&self) -> Option<&[String]>;
}

/// 分区ed表数据抽象接口。
pub trait PartitionedTableData: Send + Sync {
    /// partitionids。
    fn partition_ids(&self) -> &[i64];
}

/// 分区裁剪数据抽象接口。
pub trait PartitionPruningData: Send + Sync {
    /// selectedindexes。
    fn selected_indexes(&self) -> Result<Vec<isize>, BuildError>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 行解码列。
pub struct RowDecodeColumn {
    pub id: i64,
    pub primary_key: bool,
    pub virtual_generated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 行解码器。
pub struct RowDecoder {
    pub requested_columns: Vec<RowDecodeColumn>,
    pub primary_key_column_ids: Vec<i64>,
}

#[derive(Clone, Debug, PartialEq)]
/// 副本读调节器。
pub struct ReplicaReadAdjuster {
    pub average_row_size: f64,
    pub threshold_bytes: i64,
    pub transaction_scope: String,
}

/// 待构建的计划节点枚举（借用计划数据 trait 对象）。
pub enum Plan<'a> {
    PhysicalWrapper(Box<Plan<'a>>),
    CheckTable(&'a dyn CheckTablePlanData),
    RecoverIndex(&'a dyn PlanData),
    CleanupIndex(&'a dyn PlanData),
    CheckIndexRange(&'a dyn PlanData),
    ChecksumTable(&'a dyn PlanData),
    ReloadExprPushdownBlacklist(&'a dyn PlanData),
    ReloadOptRuleBlacklist(&'a dyn PlanData),
    AdminPlugins(&'a dyn PlanData),
    Deallocate(&'a dyn PlanData),
    Execute(&'a dyn ExecutePlanData),
    Insert(&'a dyn InsertPlanData),
    Limit(&'a dyn LimitPlanData),
    SelectLock(&'a dyn SelectLockPlanData),
    CancelDdlJobs(&'a dyn PlanData),
    PauseDdlJobs(&'a dyn PlanData),
    ResumeDdlJobs(&'a dyn PlanData),
    AlterDdlJob(&'a dyn PlanData),
    ShowNextRowId(&'a dyn PlanData),
    ShowDdl(&'a dyn PlanData),
    ShowDdlJobs(&'a dyn PlanData),
    ShowDdlJobQueries(&'a dyn PlanData),
    ShowDdlJobQueriesWithRange(&'a dyn PlanData),
    ShowSlow(&'a dyn PlanData),
    Show(&'a dyn ShowPlanData),
    Simple(&'a dyn SimplePlanData),
    Set(&'a dyn PlanData),
    SetConfig(&'a dyn PlanData),
    ImportInto(&'a dyn ImportIntoPlanData),
    LoadData(&'a dyn LoadDataPlanData),
    LoadStats(&'a dyn PlanData),
    LockStats(&'a dyn PlanData),
    UnlockStats(&'a dyn PlanData),
    PlanReplayer(&'a dyn PlanReplayerPlanData),
    Traffic(&'a dyn TrafficPlanData),
    Ddl(&'a dyn DdlPlanData),
    Trace(&'a dyn TracePlanData),
    Explain(&'a dyn ExplainPlanData),
    SelectInto(&'a dyn SelectIntoPlanData),
    UnionScan(&'a dyn UnionScanPlanData),
    MergeJoin(&'a dyn MergeJoinPlanData),
    HashJoin(&'a dyn HashJoinPlanData),
    HashAgg(&'a dyn HashAggPlanData),
    StreamAgg(&'a dyn StreamAggPlanData),
    Selection(&'a dyn UnaryPlanData),
    Expand(&'a dyn ParallelUnaryPlanData),
    Projection(&'a dyn ParallelUnaryPlanData),
    TableDual(&'a dyn TableDualPlanData),
    MemTable(&'a dyn PlanData),
    Sort(&'a dyn UnaryPlanData),
    TopN(&'a dyn TopNPlanData),
    Apply(&'a dyn ApplyPlanData),
    MaxOneRow(&'a dyn UnaryPlanData),
    UnionAll(&'a dyn UnionAllPlanData),
    DistributeTable(&'a dyn PlanData),
    SplitRegion(&'a dyn SplitRegionPlanData),
    Update(&'a dyn UpdatePlanData),
    Delete(&'a dyn DeletePlanData),
    Analyze(&'a dyn AnalyzePlanData),
    IndexJoin(&'a dyn IndexJoinPlanData),
    IndexMergeJoin(&'a dyn IndexJoinPlanData),
    IndexHashJoin(&'a dyn IndexJoinPlanData),
    TableReader(&'a dyn TableReaderPlanData),
    IndexReader(&'a dyn IndexReaderPlanData),
    IndexLookupReader(&'a dyn IndexLookupReaderPlanData),
    IndexMergeReader(&'a dyn IndexMergeReaderPlanData),
    Window(&'a dyn WindowPlanData),
    Shuffle(&'a dyn ShufflePlanData),
    ShuffleReceiver(&'a dyn PlanData),
    SqlBind(&'a dyn PlanData),
    BatchPointGet(&'a dyn BatchPointGetPlanData),
    TableSample(&'a dyn TableSamplePlanData),
    Cte(&'a dyn CtePlanData),
    CteTable(&'a dyn CteTablePlanData),
    CompactTable(&'a dyn CompactTablePlanData),
    AdminShowBdrRole(&'a dyn PlanData),
    RecommendIndex(&'a dyn PlanData),
    WorkloadRepoCreate(&'a dyn PlanData),
    Deferred(DeferredPlanKind, &'a dyn PlanData),
    Mock(ExecutorBox),
}

/// 执行器构建器依赖：构建执行器所需的外部能力注入。
pub trait ExecutorBuilderDependencies: Send + Sync {
    /// isstatementstaleness。
    fn is_statement_staleness(&self, session: &dyn SessionContext) -> bool;
    /// transactionscope。
    fn transaction_scope(&self, session: &dyn SessionContext) -> Result<String, BuildError>;
    /// readreplicascope。
    fn read_replica_scope(&self, session: &dyn SessionContext) -> Result<String, BuildError>;

    /// 构建executor执行器。
    fn build_executor(
        &self,
        kind: ExecutorKind,
        plan: &dyn PlanData,
        children: Vec<ExecutorBox>,
    ) -> Result<ExecutorBox, BuildError>;
    /// 构建deferredexecutor执行器。
    fn build_deferred_executor(
        &self,
        kind: DeferredPlanKind,
        plan: &dyn PlanData,
    ) -> Result<ExecutorBox, BuildError>;

    /// ddlownerid。
    fn ddl_owner_id(&self, timeout: Duration) -> Result<String, BuildError>;
    /// ddlselfid。
    fn ddl_self_id(&self) -> String;
    /// acquiresystemsession。
    fn acquire_system_session(&self) -> Result<Box<dyn SystemSession>, BuildError>;
    /// ddlinfowithnewtransaction。
    fn ddl_info_with_new_transaction(
        &self,
        session: &mut dyn SystemSession,
    ) -> Result<Box<dyn DdlInfo>, BuildError>;
    /// releasesystemsession。
    fn release_system_session(&self, session: Box<dyn SystemSession>);
    /// 构建showddlexecutor执行器。
    fn build_show_ddl_executor(
        &self,
        plan: &dyn PlanData,
        owner_id: String,
        ddl_info: Box<dyn DdlInfo>,
        self_id: String,
    ) -> Result<ExecutorBox, BuildError>;

    /// 更新forupdatets。
    fn update_for_update_ts(&self) -> Result<(), BuildError>;
    /// filtertemporarylocktables。
    fn filter_temporary_lock_tables(&self, plan: &dyn SelectLockPlanData)
    -> Result<(), BuildError>;
    /// starttransactionforshow。
    fn start_transaction_for_show(&self, plan: &dyn ShowPlanData) -> Result<(), BuildError>;
    /// verifyexecutestaleness。
    fn verify_execute_staleness(&self, plan: &dyn ExecutePlanData) -> Result<(), BuildError>;

    /// 初始化ializeinsertcolumns。
    fn initialize_insert_columns(&self, plan: &dyn InsertPlanData) -> Result<(), BuildError>;
    /// 构建foreignkeychecks执行器。
    fn build_foreign_key_checks(&self, plan: &dyn InsertPlanData) -> Result<(), BuildError>;
    /// 构建foreignkeycascades执行器。
    fn build_foreign_key_cascades(&self, plan: &dyn InsertPlanData) -> Result<(), BuildError>;

    /// resolvetable。
    fn resolve_table(
        &self,
        table_id: i64,
        use_latest_info_schema: bool,
    ) -> Result<ResolvedTable, BuildError>;
    /// 构建importintoexecutor执行器。
    fn build_import_into_executor(
        &self,
        plan: &dyn ImportIntoPlanData,
        table: &ResolvedTable,
        child: Option<ExecutorBox>,
    ) -> Result<ExecutorBox, BuildError>;
    /// 构建loaddataexecutor执行器。
    fn build_load_data_executor(
        &self,
        plan: &dyn LoadDataPlanData,
        table: &ResolvedTable,
    ) -> Result<ExecutorBox, BuildError>;
    /// parsereplayerstatement。
    fn parse_replayer_statement(&self, sql: &str) -> Result<Box<dyn ParsedStatement>, BuildError>;
    /// 构建planreplayerexecutor执行器。
    fn build_plan_replayer_executor(
        &self,
        plan: &dyn PlanReplayerPlanData,
        parsed_statements: Vec<Box<dyn ParsedStatement>>,
    ) -> Result<ExecutorBox, BuildError>;
    /// 构建trafficexecutor执行器。
    fn build_traffic_executor(
        &self,
        plan: &dyn TrafficPlanData,
        arguments: BTreeMap<String, String>,
    ) -> Result<ExecutorBox, BuildError>;
    /// 初始化ializeruntimestats。
    fn initialize_runtime_stats(&self) -> Result<(), BuildError>;
    /// 构建unionscanfromreader执行器。
    fn build_union_scan_from_reader(
        &self,
        plan: &dyn UnionScanPlanData,
        reader: ExecutorBox,
        in_write_statement: bool,
    ) -> Result<ExecutorBox, BuildError>;
    /// 处理cachedtable。
    fn handle_cached_table(
        &self,
        in_write_statement: bool,
        in_explain_statement: bool,
    ) -> Result<(), BuildError>;
    /// 构建mergejoinexecutor执行器。
    fn build_merge_join_executor(
        &self,
        plan: &dyn MergeJoinPlanData,
        left: ExecutorBox,
        right: ExecutorBox,
    ) -> Result<ExecutorBox, BuildError>;
    /// 构建hashjoinexecutor执行器。
    fn build_hash_join_executor(
        &self,
        kind: ExecutorKind,
        plan: &dyn HashJoinPlanData,
        left: ExecutorBox,
        right: ExecutorBox,
        config: HashJoinBuildConfig,
    ) -> Result<ExecutorBox, BuildError>;
    /// 构建hashaggexecutor执行器。
    fn build_hash_agg_executor(
        &self,
        plan: &dyn HashAggPlanData,
        child: ExecutorBox,
        config: HashAggBuildConfig,
    ) -> Result<ExecutorBox, BuildError>;
    /// 构建streamaggexecutor执行器。
    fn build_stream_agg_executor(
        &self,
        plan: &dyn StreamAggPlanData,
        child: ExecutorBox,
        config: StreamAggBuildConfig,
    ) -> Result<ExecutorBox, BuildError>;
    /// 构建parallelunaryexecutor执行器。
    fn build_parallel_unary_executor(
        &self,
        kind: ExecutorKind,
        plan: &dyn ParallelUnaryPlanData,
        child: ExecutorBox,
        workers: i64,
    ) -> Result<ExecutorBox, BuildError>;
    /// 构建topnexecutor执行器。
    fn build_top_n_executor(
        &self,
        plan: &dyn TopNPlanData,
        child: ExecutorBox,
        projection: Option<Vec<usize>>,
    ) -> Result<ExecutorBox, BuildError>;
    /// 构建applyexecutor执行器。
    fn build_apply_executor(
        &self,
        kind: ExecutorKind,
        plan: &dyn ApplyPlanData,
        left: ExecutorBox,
        right: ExecutorBox,
    ) -> Result<ExecutorBox, BuildError>;
    /// parallelapplyisbuildable。
    fn parallel_apply_is_buildable(&self, plan: &dyn ApplyPlanData) -> bool;
    /// statementforupdatets。
    fn statement_for_update_ts(&self) -> Result<u64, BuildError>;
    /// statementreadts。
    fn statement_read_ts(&self) -> Result<u64, BuildError>;
    /// snapshotwithforupdatets。
    fn snapshot_with_for_update_ts(&self) -> Result<Box<dyn Snapshot>, BuildError>;
    /// snapshotwithreadts。
    fn snapshot_with_read_ts(&self) -> Result<Box<dyn Snapshot>, BuildError>;
    /// snapshotsessionoptions。
    fn snapshot_session_options(&self) -> SnapshotSessionOptions;
    /// 构建memtableexecutor执行器。
    fn build_mem_table_executor(&self, plan: &dyn PlanData) -> Result<ExecutorBox, BuildError>;
    /// 构建updateexecutor执行器。
    fn build_update_executor(
        &self,
        plan: &dyn UpdatePlanData,
        child: ExecutorBox,
        assignment_flags: Vec<isize>,
    ) -> Result<ExecutorBox, BuildError>;
    /// validateupdatelist。
    fn validate_update_list(
        &self,
        plan: &dyn UpdatePlanData,
        assignment_flags: &[isize],
    ) -> Result<(), BuildError>;
    /// 构建updateforeignkeychecks执行器。
    fn build_update_foreign_key_checks(&self, plan: &dyn UpdatePlanData) -> Result<(), BuildError>;
    /// 构建updateforeignkeycascades执行器。
    fn build_update_foreign_key_cascades(
        &self,
        plan: &dyn UpdatePlanData,
    ) -> Result<(), BuildError>;
    /// 构建deleteexecutor执行器。
    fn build_delete_executor(
        &self,
        plan: &dyn DeletePlanData,
        child: ExecutorBox,
    ) -> Result<ExecutorBox, BuildError>;
    /// 构建deleteforeignkeychecks执行器。
    fn build_delete_foreign_key_checks(&self, plan: &dyn DeletePlanData) -> Result<(), BuildError>;
    /// 构建deleteforeignkeycascades执行器。
    fn build_delete_foreign_key_cascades(
        &self,
        plan: &dyn DeletePlanData,
    ) -> Result<(), BuildError>;
    /// flushstatsdeltaforanalyze。
    fn flush_stats_delta_for_analyze(&self, plan: &dyn AnalyzePlanData) -> Result<(), BuildError>;
    /// analyzetablecounts。
    fn analyze_table_counts(
        &self,
        task: &dyn AnalyzeColumnsTaskData,
    ) -> Result<AnalyzeTableCounts, BuildError>;
    /// 构建analyzeindextask执行器。
    fn build_analyze_index_task(
        &self,
        task: &dyn AnalyzeIndexTaskData,
        snapshot_ts: u64,
        auto_analyze: bool,
    ) -> Result<Box<dyn AnalyzeTask>, BuildError>;
    /// 构建analyzecolumnstask执行器。
    fn build_analyze_columns_task(
        &self,
        task: &dyn AnalyzeColumnsTaskData,
        config: AnalyzeSamplingConfig,
    ) -> Result<Box<dyn AnalyzeTask>, BuildError>;
    /// 构建analyzeexecutor执行器。
    fn build_analyze_executor(
        &self,
        plan: &dyn AnalyzePlanData,
        tasks: Vec<Box<dyn AnalyzeTask>>,
    ) -> Result<ExecutorBox, BuildError>;
    /// 构建indexjoinexecutor执行器。
    fn build_index_join_executor(
        &self,
        kind: ExecutorKind,
        plan: &dyn IndexJoinPlanData,
        outer: ExecutorBox,
        reader_builder: DataReaderBuilder,
    ) -> Result<ExecutorBox, BuildError>;
    /// wrapindexnestedloophashjoin。
    fn wrap_index_nested_loop_hash_join(
        &self,
        plan: &dyn IndexJoinPlanData,
        lookup_join: ExecutorBox,
    ) -> Result<ExecutorBox, BuildError>;
    /// validatetablereaderaccess。
    fn validate_table_reader_access(
        &self,
        plan: &dyn TableReaderPlanData,
    ) -> Result<(), BuildError>;
    /// 构建norangetablereader执行器。
    fn build_no_range_table_reader(
        &self,
        plan: &dyn TableReaderPlanData,
        snapshot_ts: u64,
    ) -> Result<Box<dyn TableReaderDraft>, BuildError>;
    /// 构建mppgatherexecutor执行器。
    fn build_mpp_gather_executor(
        &self,
        plan: &dyn TableReaderPlanData,
        snapshot_ts: u64,
    ) -> Result<ExecutorBox, BuildError>;
    /// alignemptytablereaderschema。
    fn align_empty_table_reader_schema(
        &self,
        plan: &dyn TableReaderPlanData,
    ) -> Result<(), BuildError>;
    /// 标记tablereaderstore。
    fn mark_table_reader_store(&self, plan: &dyn TableReaderPlanData);
    /// warnignoredtiflashreplicaread。
    fn warn_ignored_tiflash_replica_read(&self, plan: &dyn TableReaderPlanData);
    /// finalizetablereader。
    fn finalize_table_reader(
        &self,
        plan: &dyn TableReaderPlanData,
        reader: Box<dyn TableReaderDraft>,
        sorted_partition_ids: Option<Vec<i64>>,
    ) -> Result<ExecutorBox, BuildError>;
    /// prunetablereaderpartitions。
    fn prune_table_reader_partitions(
        &self,
        plan: &dyn TableReaderPlanData,
    ) -> Result<Vec<i64>, BuildError>;
    /// 构建partitionindexranges执行器。
    fn build_partition_index_ranges(
        &self,
        partition_id: i64,
        contents: &[IndexJoinLookUpContent],
    ) -> Result<Vec<KeyRange>, BuildError>;
    /// pruneinnerpartitions。
    fn prune_inner_partitions(
        &self,
        contents: &[IndexJoinLookUpContent],
    ) -> Result<Vec<i64>, BuildError>;
    /// 构建norangeindexreader执行器。
    fn build_no_range_index_reader(
        &self,
        plan: &dyn IndexReaderPlanData,
        snapshot_ts: u64,
    ) -> Result<Box<dyn IndexReaderDraft>, BuildError>;
    /// finalizeindexreader。
    fn finalize_index_reader(
        &self,
        plan: &dyn IndexReaderPlanData,
        reader: Box<dyn IndexReaderDraft>,
    ) -> Result<ExecutorBox, BuildError>;
    /// 构建tablerequest执行器。
    fn build_table_request(
        &self,
        schema_len: usize,
        plan: &dyn DistributedPlanData,
    ) -> Result<Box<dyn DagRequest>, BuildError>;
    /// 构建indexrequest执行器。
    fn build_index_request(
        &self,
        column_count: usize,
        handle_len: usize,
        unnatural_order: bool,
    ) -> Result<Box<dyn DagRequest>, BuildError>;
    /// 构建norangeindexlookupreader执行器。
    fn build_no_range_index_lookup_reader(
        &self,
        plan: &dyn IndexLookupReaderPlanData,
        snapshot_ts: u64,
    ) -> Result<Box<dyn IndexLookupReaderDraft>, BuildError>;
    /// finalizeindexlookupreader。
    fn finalize_index_lookup_reader(
        &self,
        plan: &dyn IndexLookupReaderPlanData,
        reader: Box<dyn IndexLookupReaderDraft>,
    ) -> Result<ExecutorBox, BuildError>;
    /// 构建norangeindexmergereader执行器。
    fn build_no_range_index_merge_reader(
        &self,
        plan: &dyn IndexMergeReaderPlanData,
        snapshot_ts: u64,
    ) -> Result<Box<dyn IndexMergeReaderDraft>, BuildError>;
    /// finalizeindexmergereader。
    fn finalize_index_merge_reader(
        &self,
        plan: &dyn IndexMergeReaderPlanData,
        reader: Box<dyn IndexMergeReaderDraft>,
    ) -> Result<ExecutorBox, BuildError>;
    /// 构建indexusagereporter执行器。
    fn build_index_usage_reporter(
        &self,
        plan: &dyn DistributedPlanData,
        load_stats: bool,
    ) -> Result<Box<dyn IndexUsageReporter>, BuildError>;
    /// 构建datareaderexecutor执行器。
    fn build_data_reader_executor(
        &self,
        kind: DataReaderBuildKind,
        plan: &dyn DataReaderPlanData,
        contents: &[IndexJoinLookUpContent],
        snapshot_ts: u64,
        range_mem_tracker: Option<Arc<Tracker>>,
    ) -> Result<ExecutorBox, BuildError>;
    /// openindexjoinhashexecutor。
    fn open_index_join_hash_executor(&self, executor: &mut dyn Executor) -> Result<(), BuildError>;
    /// encodekeyranges。
    fn encode_key_ranges(
        &self,
        partition_id: i64,
        ranges: &[LogicalRange],
    ) -> Result<Vec<KeyRange>, BuildError>;
    /// 构建tablereaderbaseforindexjoin执行器。
    fn build_table_reader_base_for_index_join(
        &self,
        snapshot_ts: u64,
        ranges: &[KeyRange],
    ) -> Result<ExecutorBox, BuildError>;
    /// 构建shuffleexecutor执行器。
    fn build_shuffle_executor(
        &self,
        plan: &dyn ShufflePlanData,
        data_sources: Vec<ExecutorBox>,
        workers: Vec<ExecutorBox>,
    ) -> Result<ExecutorBox, BuildError>;
    /// prunebatchpointget。
    fn prune_batch_point_get(
        &self,
        plan: &dyn BatchPointGetPlanData,
    ) -> Result<BatchPointGetPruneResult, BuildError>;
    /// 构建batchpointgetexecutor执行器。
    fn build_batch_point_get_executor(
        &self,
        plan: &dyn BatchPointGetPlanData,
        snapshot: Box<dyn Snapshot>,
        prune_result: BatchPointGetPruneResult,
        lock: bool,
        capacity: usize,
        cache: Option<Box<dyn MemoryBuffer>>,
    ) -> Result<ExecutorBox, BuildError>;
    /// 构建tablesampleexecutor执行器。
    fn build_table_sample_executor(
        &self,
        plan: &dyn TableSamplePlanData,
        snapshot_ts: u64,
        empty_sampler: bool,
    ) -> Result<ExecutorBox, BuildError>;
    /// loadctestorages。
    fn load_cte_storages(&self, storage_id: i64) -> Result<Option<Arc<CTEStorages>>, BuildError>;
    /// createandstorectestorages。
    fn create_and_store_cte_storages(
        &self,
        storage_id: i64,
    ) -> Result<Arc<CTEStorages>, BuildError>;
    /// 构建ctestorageproducer执行器。
    fn build_cte_storage_producer(
        &self,
        plan: &dyn CtePlanData,
        storages: &Arc<CTEStorages>,
        seed: ExecutorBox,
        recursive: Option<ExecutorBox>,
    ) -> Result<(), BuildError>;
    /// 构建cteexecutor执行器。
    fn build_cte_executor(
        &self,
        plan: &dyn CtePlanData,
        storages: Arc<CTEStorages>,
    ) -> Result<ExecutorBox, BuildError>;
    /// 构建ctetablereaderexecutor执行器。
    fn build_cte_table_reader_executor(
        &self,
        plan: &dyn CteTablePlanData,
        storages: Arc<CTEStorages>,
    ) -> Result<ExecutorBox, BuildError>;
    /// 获取cachetable。
    fn get_cache_table(
        &self,
        table: &dyn TableReadSpec,
        snapshot_ts: u64,
        may_update_read_lock: bool,
    ) -> Result<Option<Box<dyn MemoryBuffer>>, BuildError>;
    /// validatecompactstorage。
    fn validate_compact_storage(&self) -> Result<(), BuildError>;
    /// resolvecompactpartitionids。
    fn resolve_compact_partition_ids(
        &self,
        plan: &dyn CompactTablePlanData,
    ) -> Result<Vec<i64>, BuildError>;
    /// 构建compacttableexecutor执行器。
    fn build_compact_table_executor(
        &self,
        plan: &dyn CompactTablePlanData,
        partition_ids: Vec<i64>,
    ) -> Result<ExecutorBox, BuildError>;
}

/// 执行器构建器：持有会话/依赖并按计划种类分发 buildXxx。
pub struct executorBuilder {
    ctx: Arc<dyn StatementContext>,
    sctx: Arc<dyn SessionContext>,
    is: Arc<dyn InfoSchema>,
    dependencies: Arc<dyn ExecutorBuilderDependencies>,
    err: Option<BuildError>,
    hasLock: bool,
    Ti: TelemetryInfo,
    isStaleness: bool,
    txnScope: String,
    readReplicaScope: String,
    inUpdateStmt: bool,
    inDeleteStmt: bool,
    inInsertStmt: bool,
    inSelectLockStmt: bool,
    forDataReaderBuilder: bool,
    dataReaderTS: u64,
    encounterUnionScan: bool,
    stmtCtxLock: Option<Arc<Mutex<()>>>,
}

/// 构造执行器构建器。
pub fn newExecutorBuilder(
    ctx: Arc<dyn StatementContext>,
    sctx: Arc<dyn SessionContext>,
    is: Arc<dyn InfoSchema>,
    dependencies: Arc<dyn ExecutorBuilderDependencies>,
) -> Result<executorBuilder, BuildError> {
    let is_staleness = dependencies.is_statement_staleness(sctx.as_ref());
    let txn_scope = dependencies.transaction_scope(sctx.as_ref())?;
    let read_replica_scope = dependencies.read_replica_scope(sctx.as_ref())?;

    Ok(executorBuilder {
        ctx,
        sctx,
        is,
        dependencies,
        err: None,
        hasLock: false,
        Ti: TelemetryInfo::default(),
        isStaleness: is_staleness,
        txnScope: txn_scope,
        readReplicaScope: read_replica_scope,
        inUpdateStmt: false,
        inDeleteStmt: false,
        inInsertStmt: false,
        inSelectLockStmt: false,
        forDataReaderBuilder: false,
        dataReaderTS: 0,
        encounterUnionScan: false,
        stmtCtxLock: None,
    })
}

impl executorBuilder {
    /// 在锁保护下执行：StmtCtx锁。
    pub fn withStmtCtxLock<T>(&self, callback: impl FnOnce() -> T) -> T {
        let Some(lock) = &self.stmtCtxLock else {
            return callback();
        };
        let _guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        callback()
    }

    /// 返回构建过程中累计的错误。
    pub fn error(&self) -> Option<&BuildError> {
        self.err.as_ref()
    }

    /// finish。
    fn finish(&mut self, result: Result<ExecutorBox, BuildError>) -> Option<ExecutorBox> {
        match result {
            Ok(executor) => Some(executor),
            Err(error) => {
                self.err = Some(error);
                None
            }
        }
    }

    /// 构建leaf执行器。
    fn build_leaf(&mut self, kind: ExecutorKind, plan: &dyn PlanData) -> Option<ExecutorBox> {
        let result = self.dependencies.build_executor(kind, plan, Vec::new());
        self.finish(result)
    }

    /// 构建执行器。
    pub fn build(&mut self, plan: Option<Plan<'_>>) -> Option<ExecutorBox> {
        // 按 Plan 变体分发到对应 buildXxx。
        let Some(plan) = plan else {
            return None;
        };

        match plan {
            Plan::PhysicalWrapper(inner) => self.build(Some(*inner)),
            Plan::CheckTable(plan) => self.buildCheckTable(plan),
            Plan::RecoverIndex(plan) => self.buildRecoverIndex(plan),
            Plan::CleanupIndex(plan) => self.buildCleanupIndex(plan),
            Plan::CheckIndexRange(plan) => self.buildCheckIndexRange(plan),
            Plan::ChecksumTable(plan) => self.buildChecksumTable(plan),
            Plan::ReloadExprPushdownBlacklist(plan) => self.buildReloadExprPushdownBlacklist(plan),
            Plan::ReloadOptRuleBlacklist(plan) => self.buildReloadOptRuleBlacklist(plan),
            Plan::AdminPlugins(plan) => self.buildAdminPlugins(plan),
            Plan::Deallocate(plan) => self.buildDeallocate(plan),
            Plan::Execute(plan) => self.buildExecute(plan),
            Plan::Insert(plan) => self.buildInsert(plan),
            Plan::Limit(plan) => self.buildLimit(plan),
            Plan::SelectLock(plan) => self.buildSelectLock(plan),
            Plan::CancelDdlJobs(plan) => self.buildCancelDDLJobs(plan),
            Plan::PauseDdlJobs(plan) => self.buildPauseDDLJobs(plan),
            Plan::ResumeDdlJobs(plan) => self.buildResumeDDLJobs(plan),
            Plan::AlterDdlJob(plan) => self.buildAlterDDLJob(plan),
            Plan::ShowNextRowId(plan) => self.buildShowNextRowID(plan),
            Plan::ShowDdl(plan) => self.buildShowDDL(plan),
            Plan::ShowDdlJobs(plan) => self.buildShowDDLJobs(plan),
            Plan::ShowDdlJobQueries(plan) => self.buildShowDDLJobQueries(plan),
            Plan::ShowDdlJobQueriesWithRange(plan) => self.buildShowDDLJobQueriesWithRange(plan),
            Plan::ShowSlow(plan) => self.buildShowSlow(plan),
            Plan::Show(plan) => self.buildShow(plan),
            Plan::Simple(plan) => self.buildSimple(plan),
            Plan::Set(plan) => self.buildSet(plan),
            Plan::SetConfig(plan) => self.buildSetConfig(plan),
            Plan::ImportInto(plan) => self.buildImportInto(plan),
            Plan::LoadData(plan) => self.buildLoadData(plan),
            Plan::LoadStats(plan) => self.buildLoadStats(plan),
            Plan::LockStats(plan) => self.buildLockStats(plan),
            Plan::UnlockStats(plan) => self.buildUnlockStats(plan),
            Plan::PlanReplayer(plan) => self.buildPlanReplayer(plan),
            Plan::Traffic(plan) => self.buildTraffic(plan),
            Plan::Ddl(plan) => self.buildDDL(plan),
            Plan::Trace(plan) => self.buildTrace(plan),
            Plan::Explain(plan) => self.buildExplain(plan),
            Plan::SelectInto(plan) => self.buildSelectInto(plan),
            Plan::UnionScan(plan) => self.buildUnionScanExec(plan),
            Plan::MergeJoin(plan) => self.buildMergeJoin(plan),
            Plan::HashJoin(plan) => self.buildHashJoin(plan),
            Plan::HashAgg(plan) => self.buildHashAgg(plan),
            Plan::StreamAgg(plan) => self.buildStreamAgg(plan),
            Plan::Selection(plan) => self.buildSelection(plan),
            Plan::Expand(plan) => self.buildExpand(plan),
            Plan::Projection(plan) => self.buildProjection(plan),
            Plan::TableDual(plan) => self.buildTableDual(plan),
            Plan::MemTable(plan) => self.buildMemTable(plan),
            Plan::Sort(plan) => self.buildSort(plan),
            Plan::TopN(plan) => self.buildTopN(plan),
            Plan::Apply(plan) => self.buildApply(plan),
            Plan::MaxOneRow(plan) => self.buildMaxOneRow(plan),
            Plan::UnionAll(plan) => self.buildUnionAll(plan),
            Plan::DistributeTable(plan) => self.buildDistributeTable(plan),
            Plan::SplitRegion(plan) => self.buildSplitRegion(plan),
            Plan::Update(plan) => self.buildUpdate(plan),
            Plan::Delete(plan) => self.buildDelete(plan),
            Plan::Analyze(plan) => self.buildAnalyze(plan),
            Plan::IndexJoin(plan) => self.buildIndexLookUpJoin(plan),
            Plan::IndexMergeJoin(plan) => self.buildIndexLookUpMergeJoin(plan),
            Plan::IndexHashJoin(plan) => self.buildIndexNestedLoopHashJoin(plan),
            Plan::TableReader(plan) => self.buildTableReader(plan),
            Plan::IndexReader(plan) => self.buildIndexReader(plan),
            Plan::IndexLookupReader(plan) => self.buildIndexLookUpReader(plan),
            Plan::IndexMergeReader(plan) => self.buildIndexMergeReader(plan),
            Plan::Window(plan) => self.buildWindow(plan),
            Plan::Shuffle(plan) => self.buildShuffle(plan),
            Plan::ShuffleReceiver(plan) => self.buildShuffleReceiverStub(plan),
            Plan::SqlBind(plan) => self.buildSQLBindExec(plan),
            Plan::BatchPointGet(plan) => self.buildBatchPointGet(plan),
            Plan::TableSample(plan) => self.buildTableSample(plan),
            Plan::Cte(plan) => self.buildCTE(plan),
            Plan::CteTable(plan) => self.buildCTETableReader(plan),
            Plan::CompactTable(plan) => self.buildCompactTable(plan),
            Plan::AdminShowBdrRole(plan) => self.buildAdminShowBDRRole(plan),
            Plan::RecommendIndex(plan) => self.buildRecommendIndex(plan),
            Plan::WorkloadRepoCreate(plan) => self.buildWorkloadRepoCreate(plan),
            Plan::Deferred(kind, plan) => {
                let result = self.dependencies.build_deferred_executor(kind, plan);
                self.finish(result)
            }
            Plan::Mock(executor) => Some(executor),
        }
    }

    /// 构建取消DDL作业执行器。
    pub fn buildCancelDDLJobs(&mut self, plan: &dyn PlanData) -> Option<ExecutorBox> {
        self.build_leaf(ExecutorKind::CancelDdlJobs, plan)
    }

    /// 构建暂停DDL作业执行器。
    pub fn buildPauseDDLJobs(&mut self, plan: &dyn PlanData) -> Option<ExecutorBox> {
        self.build_leaf(ExecutorKind::PauseDdlJobs, plan)
    }

    /// 构建恢复DDL作业执行器。
    pub fn buildResumeDDLJobs(&mut self, plan: &dyn PlanData) -> Option<ExecutorBox> {
        self.build_leaf(ExecutorKind::ResumeDdlJobs, plan)
    }

    /// 构建变更DDL作业执行器。
    pub fn buildAlterDDLJob(&mut self, plan: &dyn PlanData) -> Option<ExecutorBox> {
        self.build_leaf(ExecutorKind::AlterDdlJob, plan)
    }

    /// 构建SHOW下一个行 ID执行器。
    pub fn buildShowNextRowID(&mut self, plan: &dyn PlanData) -> Option<ExecutorBox> {
        self.build_leaf(ExecutorKind::ShowNextRowId, plan)
    }

    /// 构建SHOW DDL执行器。
    pub fn buildShowDDL(&mut self, plan: &dyn PlanData) -> Option<ExecutorBox> {
        let owner_id = match self.dependencies.ddl_owner_id(Duration::from_secs(3)) {
            Ok(owner_id) => owner_id,
            Err(error) => {
                self.err = Some(error);
                return None;
            }
        };
        let mut session = match self.dependencies.acquire_system_session() {
            Ok(session) => session,
            Err(error) => {
                self.err = Some(error);
                return None;
            }
        };
        let ddl_info_result = self
            .dependencies
            .ddl_info_with_new_transaction(session.as_mut());
        self.dependencies.release_system_session(session);
        let ddl_info = match ddl_info_result {
            Ok(ddl_info) => ddl_info,
            Err(error) => {
                self.err = Some(error);
                return None;
            }
        };
        let result = self.dependencies.build_show_ddl_executor(
            plan,
            owner_id,
            ddl_info,
            self.dependencies.ddl_self_id(),
        );
        self.finish(result)
    }

    /// 构建SHOW DDL作业执行器。
    pub fn buildShowDDLJobs(&mut self, plan: &dyn PlanData) -> Option<ExecutorBox> {
        self.build_leaf(ExecutorKind::ShowDdlJobs, plan)
    }

    /// 构建SHOW DDL作业查询执行器。
    pub fn buildShowDDLJobQueries(&mut self, plan: &dyn PlanData) -> Option<ExecutorBox> {
        self.build_leaf(ExecutorKind::ShowDdlJobQueries, plan)
    }

    /// 构建SHOW DDL作业查询带范围执行器。
    pub fn buildShowDDLJobQueriesWithRange(&mut self, plan: &dyn PlanData) -> Option<ExecutorBox> {
        self.build_leaf(ExecutorKind::ShowDdlJobQueriesWithRange, plan)
    }

    /// 构建SHOW慢查询执行器。
    pub fn buildShowSlow(&mut self, plan: &dyn PlanData) -> Option<ExecutorBox> {
        self.build_leaf(ExecutorKind::ShowSlow, plan)
    }

    /// 构建CHECK TABLE执行器。
    pub fn buildCheckTable(&mut self, plan: &dyn CheckTablePlanData) -> Option<ExecutorBox> {
        let kind = if plan.fast_check_enabled() && plan.indexes_support_fast_check() {
            ExecutorKind::FastCheckTable
        } else {
            ExecutorKind::CheckTable
        };
        self.build_leaf(kind, plan)
    }

    /// 构建恢复索引执行器。
    pub fn buildRecoverIndex(&mut self, plan: &dyn PlanData) -> Option<ExecutorBox> {
        self.build_leaf(ExecutorKind::RecoverIndex, plan)
    }

    /// 构建清理索引执行器。
    pub fn buildCleanupIndex(&mut self, plan: &dyn PlanData) -> Option<ExecutorBox> {
        self.build_leaf(ExecutorKind::CleanupIndex, plan)
    }

    /// 构建CHECK INDEX范围执行器。
    pub fn buildCheckIndexRange(&mut self, plan: &dyn PlanData) -> Option<ExecutorBox> {
        self.build_leaf(ExecutorKind::CheckIndexRange, plan)
    }

    /// 构建CHECKSUM TABLE执行器。
    pub fn buildChecksumTable(&mut self, plan: &dyn PlanData) -> Option<ExecutorBox> {
        self.build_leaf(ExecutorKind::ChecksumTable, plan)
    }

    /// 构建重载表达式下推黑名单执行器。
    pub fn buildReloadExprPushdownBlacklist(&mut self, plan: &dyn PlanData) -> Option<ExecutorBox> {
        self.build_leaf(ExecutorKind::ReloadExprPushdownBlacklist, plan)
    }

    /// 构建重载优化规则黑名单执行器。
    pub fn buildReloadOptRuleBlacklist(&mut self, plan: &dyn PlanData) -> Option<ExecutorBox> {
        self.build_leaf(ExecutorKind::ReloadOptRuleBlacklist, plan)
    }

    /// 构建ADMIN PLUGINS执行器。
    pub fn buildAdminPlugins(&mut self, plan: &dyn PlanData) -> Option<ExecutorBox> {
        self.build_leaf(ExecutorKind::AdminPlugins, plan)
    }

    /// 构建释放准备语句执行器。
    pub fn buildDeallocate(&mut self, plan: &dyn PlanData) -> Option<ExecutorBox> {
        self.build_leaf(ExecutorKind::Deallocate, plan)
    }

    /// 构建SELECT FOR UPDATE 锁执行器。
    pub fn buildSelectLock(&mut self, plan: &dyn SelectLockPlanData) -> Option<ExecutorBox> {
        let reset_select_lock_flag = !self.inSelectLockStmt;
        if reset_select_lock_flag {
            self.inSelectLockStmt = true;
        }

        if let Err(error) = self.dependencies.update_for_update_ts() {
            self.err = Some(error);
            if reset_select_lock_flag {
                self.inSelectLockStmt = false;
            }
            return None;
        }

        let child = self.build(Some(plan.child_plan()));
        if self.err.is_some() {
            if reset_select_lock_flag {
                self.inSelectLockStmt = false;
            }
            return None;
        }
        let Some(child) = child else {
            self.err = Some(BuildError::new(
                "select lock child did not build an executor",
            ));
            if reset_select_lock_flag {
                self.inSelectLockStmt = false;
            }
            return None;
        };

        let result = if !plan.pessimistic_lock_eligible() {
            Ok(child)
        } else {
            self.hasLock = true;
            self.dependencies
                .filter_temporary_lock_tables(plan)
                .and_then(|_| {
                    self.dependencies
                        .build_executor(ExecutorKind::SelectLock, plan, vec![child])
                })
        };
        if reset_select_lock_flag {
            self.inSelectLockStmt = false;
        }
        self.finish(result)
    }

    /// 构建限制执行器。
    pub fn buildLimit(&mut self, plan: &dyn LimitPlanData) -> Option<ExecutorBox> {
        let child = self.build(Some(plan.child_plan()));
        if self.err.is_some() {
            return None;
        }
        let Some(child) = child else {
            self.err = Some(BuildError::new("limit child did not build an executor"));
            return None;
        };

        let _initial_capacity = usize::min(
            usize::try_from(plan.count()).unwrap_or(usize::MAX),
            plan.max_chunk_size(),
        );
        let used = plan.used_child_columns();
        if used.iter().any(|index| *index >= plan.child_column_count()) {
            self.err = Some(BuildError::new(
                "limit projection references a column outside the child schema",
            ));
            return None;
        }
        let result = self
            .dependencies
            .build_executor(ExecutorKind::Limit, plan, vec![child]);
        self.finish(result)
    }

    /// 构建PREPARE执行器。
    pub fn buildPrepare(&mut self, plan: &dyn PlanData) -> Option<ExecutorBox> {
        self.build_leaf(ExecutorKind::Prepare, plan)
    }

    /// 构建EXECUTE执行器。
    pub fn buildExecute(&mut self, plan: &dyn ExecutePlanData) -> Option<ExecutorBox> {
        if let Err(error) = self.dependencies.verify_execute_staleness(plan) {
            self.err = Some(error);
            return None;
        }
        self.build_leaf(ExecutorKind::Execute, plan)
    }

    /// 构建SHOW执行器。
    pub fn buildShow(&mut self, plan: &dyn ShowPlanData) -> Option<ExecutorBox> {
        if plan.needs_start_ts()
            && let Err(error) = self.dependencies.start_transaction_for_show(plan)
        {
            self.err = Some(error);
            return None;
        }
        self.build_leaf(ExecutorKind::Show, plan)
    }

    /// 构建简单语句执行器。
    pub fn buildSimple(&mut self, plan: &dyn SimplePlanData) -> Option<ExecutorBox> {
        let kind = match plan.statement_kind() {
            SimpleStatementKind::Grant => return self.buildGrant(plan),
            SimpleStatementKind::Revoke => return self.buildRevoke(plan),
            SimpleStatementKind::Brie => ExecutorKind::Brie,
            SimpleStatementKind::CalibrateResource => ExecutorKind::CalibrateResource,
            SimpleStatementKind::AddQueryWatch => ExecutorKind::AddQueryWatch,
            SimpleStatementKind::ImportIntoAction => ExecutorKind::ImportIntoAction,
            SimpleStatementKind::CancelDistributionJob => ExecutorKind::CancelDistributionJob,
            SimpleStatementKind::CreateUser | SimpleStatementKind::AlterUser => {
                let telemetry = self
                    .Ti
                    .account_lock
                    .get_or_insert_with(AccountLockTelemetryInfo::default);
                telemetry.create_or_alter_user += 1;
                if let Some(option) = plan
                    .account_lock_options()
                    .iter()
                    .rev()
                    .find(|option| **option != AccountLockOption::Other)
                {
                    match option {
                        AccountLockOption::Lock => telemetry.lock_user += 1,
                        AccountLockOption::Unlock => telemetry.unlock_user += 1,
                        AccountLockOption::Other => {}
                    }
                }
                ExecutorKind::Simple
            }
            SimpleStatementKind::Other => ExecutorKind::Simple,
        };
        self.build_leaf(kind, plan)
    }

    /// 构建SET执行器。
    pub fn buildSet(&mut self, plan: &dyn PlanData) -> Option<ExecutorBox> {
        self.build_leaf(ExecutorKind::Set, plan)
    }

    /// 构建SET配置执行器。
    pub fn buildSetConfig(&mut self, plan: &dyn PlanData) -> Option<ExecutorBox> {
        self.build_leaf(ExecutorKind::SetConfig, plan)
    }

    /// 构建插入执行器。
    pub fn buildInsert(&mut self, plan: &dyn InsertPlanData) -> Option<ExecutorBox> {
        self.inInsertStmt = true;
        if let Err(error) = self.dependencies.update_for_update_ts() {
            self.err = Some(error);
            return None;
        }

        let mut children = Vec::new();
        if let Some(select_plan) = plan.select_plan() {
            let select_executor = self.build(Some(select_plan));
            if self.err.is_some() {
                return None;
            }
            let Some(select_executor) = select_executor else {
                self.err = Some(BuildError::new(
                    "insert select plan did not build an executor",
                ));
                return None;
            };
            children.push(select_executor);
        }

        for result in [
            self.dependencies.initialize_insert_columns(plan),
            self.dependencies.build_foreign_key_checks(plan),
            self.dependencies.build_foreign_key_cascades(plan),
        ] {
            if let Err(error) = result {
                self.err = Some(error);
                return None;
            }
        }

        if plan.is_replace() {
            return self.buildReplace(plan, children);
        }
        let result = self
            .dependencies
            .build_executor(ExecutorKind::Insert, plan, children);
        self.finish(result)
    }
}

/// 测试用 mock执行器构建器。
pub struct MockExecutorBuilder {
    executor_builder: executorBuilder,
}

impl MockExecutorBuilder {
    /// 构建执行器。
    pub fn Build(&mut self, plan: Option<Plan<'_>>) -> Option<ExecutorBox> {
        self.executor_builder.build(plan)
    }
}

/// 构造测试用 mock执行器构建器用于Test。
pub fn NewMockExecutorBuilderForTest(
    ctx: Arc<dyn StatementContext>,
    session: Arc<dyn SessionContext>,
    info_schema: Arc<dyn InfoSchema>,
    dependencies: Arc<dyn ExecutorBuilderDependencies>,
) -> Result<MockExecutorBuilder, BuildError> {
    Ok(MockExecutorBuilder {
        executor_builder: newExecutorBuilder(ctx, session, info_schema, dependencies)?,
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 索引列。
pub struct IndexColumn {
    pub offset: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 列信息。
pub struct ColumnInfo {
    pub name: String,
    pub offset: usize,
    pub is_generated: bool,
    pub is_extra_handle: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 表信息。
pub struct TableInfo {
    pub columns: Vec<ColumnInfo>,
    pub common_handle: bool,
    pub pk_is_handle: bool,
    pub primary_index_columns: Vec<IndexColumn>,
    pub primary_key_offset: Option<usize>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 索引信息。
pub struct IndexInfo {
    pub columns: Vec<IndexColumn>,
    pub has_condition: bool,
    pub global: bool,
}

/// 构建IdxColsConcat行句柄 handleCols执行器。
pub fn buildIdxColsConcatHandleCols(
    table: &TableInfo,
    index: &IndexInfo,
    has_generated_column_or_partial_index: bool,
) -> Result<Vec<ColumnInfo>, BuildError> {
    let mut columns = if has_generated_column_or_partial_index {
        table.columns.clone()
    } else {
        let mut columns =
            Vec::with_capacity(index.columns.len() + table.primary_index_columns.len() + 1);
        for index_column in &index.columns {
            if table.pk_is_handle && table.primary_key_offset == Some(index_column.offset) {
                continue;
            }
            let column = table.columns.get(index_column.offset).ok_or_else(|| {
                BuildError::new("index column offset is outside the table schema")
            })?;
            columns.push(column.clone());
        }
        columns
    };

    if table.common_handle {
        for primary_column in &table.primary_index_columns {
            let column = table.columns.get(primary_column.offset).ok_or_else(|| {
                BuildError::new("primary index column offset is outside the table schema")
            })?;
            if !columns.iter().any(|existing| existing.name == column.name) {
                columns.push(column.clone());
            }
        }
        return Ok(columns);
    }

    if table.pk_is_handle {
        let primary_key_offset = table
            .primary_key_offset
            .ok_or_else(|| BuildError::new("PK-is-handle table has no primary key column"))?;
        let column = table.columns.get(primary_key_offset).ok_or_else(|| {
            BuildError::new("primary key column offset is outside the table schema")
        })?;
        columns.push(column.clone());
        return Ok(columns);
    }

    columns.push(ColumnInfo {
        name: "_tidb_rowid".to_owned(),
        offset: columns.len(),
        is_generated: false,
        is_extra_handle: true,
    });
    Ok(columns)
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 行句柄 handle列s枚举。
pub enum HandleColumns {
    Integer { column_index: usize },
    Common { column_indexes: Vec<usize> },
}

/// 构建行句柄 handleCols用于Exec执行器。
pub fn buildHandleColsForExec(
    table: &TableInfo,
    all_columns: &[ColumnInfo],
) -> Result<HandleColumns, BuildError> {
    if !table.common_handle {
        let column_index = all_columns
            .len()
            .checked_sub(1)
            .ok_or_else(|| BuildError::new("integer handle requires a handle column"))?;
        return Ok(HandleColumns::Integer { column_index });
    }

    let mut column_indexes = Vec::with_capacity(table.primary_index_columns.len());
    for primary_column in &table.primary_index_columns {
        let primary = table.columns.get(primary_column.offset).ok_or_else(|| {
            BuildError::new("primary index column offset is outside the table schema")
        })?;
        let position = all_columns
            .iter()
            .position(|column| column.name == primary.name)
            .ok_or_else(|| {
                BuildError::new("common handle column is absent from executor columns")
            })?;
        column_indexes.push(position);
    }
    Ok(HandleColumns::Common { column_indexes })
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 索引回表检查输入。
pub struct IndexLookupCheckInput {
    pub index_columns: Vec<ColumnInfo>,
    pub index_column_names: Vec<String>,
    pub common_handle_columns: usize,
    pub common_handle: bool,
    pub global_index: bool,
    pub table_column_names: Vec<String>,
    pub table_handle_index: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 索引回表检查。
pub struct IndexLookupCheck {
    pub output_offsets: Vec<u32>,
    pub handle_index: usize,
    pub full_range: bool,
    pub decoded_index_columns: Vec<ColumnInfo>,
    pub table_column_offsets: Vec<usize>,
}

/// 构建索引回表检查器执行器。
pub fn buildIndexLookUpChecker(
    input: &IndexLookupCheckInput,
) -> Result<IndexLookupCheck, BuildError> {
    let mut full_column_count = input.index_columns.len() + input.common_handle_columns;
    if !input.common_handle {
        full_column_count += 1;
    }
    if input.global_index {
        full_column_count += 1;
    }
    let output_offsets = (0..full_column_count)
        .map(|offset| {
            u32::try_from(offset).map_err(|_| BuildError::new("index output offset exceeds uint32"))
        })
        .collect::<Result<Vec<_>, _>>()?;

    let table_column_offsets = input
        .index_column_names
        .iter()
        .map(|name| {
            input
                .table_column_names
                .iter()
                .position(|column| column.eq_ignore_ascii_case(name))
                .ok_or_else(|| BuildError::new(format!("unknown index column {name}")))
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(IndexLookupCheck {
        output_offsets,
        handle_index: input.table_handle_index,
        full_range: true,
        decoded_index_columns: input.index_columns.clone(),
        table_column_offsets,
    })
}
impl executorBuilder {
    /// 构建IMPORT INTO执行器。
    pub fn buildImportInto(&mut self, plan: &dyn ImportIntoPlanData) -> Option<ExecutorBox> {
        let table = match self
            .dependencies
            .resolve_table(plan.target_table_id(), true)
        {
            Ok(table) => table,
            Err(error) => {
                self.err = Some(error);
                return None;
            }
        };
        if !table.is_base_table {
            self.err = Some(BuildError::new(format!(
                "table {} is not updatable for IMPORT",
                table.name
            )));
            return None;
        }

        let child = match plan.select_plan() {
            Some(select_plan) => {
                let child = self.build(Some(select_plan));
                if self.err.is_some() {
                    return None;
                }
                match child {
                    Some(child) => Some(child),
                    None => {
                        self.err = Some(BuildError::new(
                            "IMPORT SELECT plan did not build an executor",
                        ));
                        return None;
                    }
                }
            }
            None => None,
        };
        let result = self
            .dependencies
            .build_import_into_executor(plan, &table, child);
        self.finish(result)
    }

    /// 构建LOAD DATA执行器。
    pub fn buildLoadData(&mut self, plan: &dyn LoadDataPlanData) -> Option<ExecutorBox> {
        let table = match self
            .dependencies
            .resolve_table(plan.target_table_id(), false)
        {
            Ok(table) => table,
            Err(error) => {
                self.err = Some(error);
                return None;
            }
        };
        if !table.is_base_table {
            self.err = Some(BuildError::new(format!(
                "table {} is not updatable for LOAD",
                table.name
            )));
            return None;
        }
        let result = self.dependencies.build_load_data_executor(plan, &table);
        self.finish(result)
    }

    /// 构建加载统计执行器。
    pub fn buildLoadStats(&mut self, plan: &dyn PlanData) -> Option<ExecutorBox> {
        self.build_leaf(ExecutorKind::LoadStats, plan)
    }

    /// 构建锁定统计执行器。
    pub fn buildLockStats(&mut self, plan: &dyn PlanData) -> Option<ExecutorBox> {
        self.build_leaf(ExecutorKind::LockStats, plan)
    }

    /// 构建解锁统计执行器。
    pub fn buildUnlockStats(&mut self, plan: &dyn PlanData) -> Option<ExecutorBox> {
        self.build_leaf(ExecutorKind::UnlockStats, plan)
    }

    /// 构建计划重放器执行器。
    pub fn buildPlanReplayer(&mut self, plan: &dyn PlanReplayerPlanData) -> Option<ExecutorBox> {
        let mut statements = Vec::new();
        if plan.mode() == PlanReplayerMode::Dump && !plan.statement_sql().is_empty() {
            statements.reserve(plan.statement_sql().len());
            for sql in plan.statement_sql() {
                match self.dependencies.parse_replayer_statement(sql) {
                    Ok(statement) => statements.push(statement),
                    Err(error) => {
                        self.err = Some(BuildError::new(format!(
                            "plan replayer: failed to parse SQL: {sql}, error: {error}"
                        )));
                        return None;
                    }
                }
            }
        }
        let result = self
            .dependencies
            .build_plan_replayer_executor(plan, statements);
        self.finish(result)
    }

    /// 构建流量执行器。
    pub fn buildTraffic(&mut self, plan: &dyn TrafficPlanData) -> Option<ExecutorBox> {
        let mut arguments = BTreeMap::new();
        match plan.operation() {
            TrafficOperation::Capture => {
                arguments.insert("output".to_owned(), plan.directory().to_owned());
                for option in plan.options() {
                    match option {
                        TrafficOption::Duration(value) => {
                            arguments.insert("duration".to_owned(), value.clone());
                        }
                        TrafficOption::EncryptionMethod(value) => {
                            arguments.insert("encrypt-method".to_owned(), value.clone());
                        }
                        TrafficOption::Compress(value) => {
                            arguments.insert("compress".to_owned(), value.to_string());
                        }
                        TrafficOption::Username(_)
                        | TrafficOption::Password(_)
                        | TrafficOption::Speed(_)
                        | TrafficOption::ReadOnly(_) => {}
                    }
                }
            }
            TrafficOperation::Replay => {
                arguments.insert("input".to_owned(), plan.directory().to_owned());
                for option in plan.options() {
                    match option {
                        TrafficOption::Username(value) => {
                            arguments.insert("username".to_owned(), value.clone());
                        }
                        TrafficOption::Password(value) => {
                            arguments.insert("password".to_owned(), value.clone());
                        }
                        TrafficOption::Speed(Some(value)) => {
                            arguments.insert("speed".to_owned(), value.clone());
                        }
                        TrafficOption::ReadOnly(value) => {
                            arguments.insert("readonly".to_owned(), value.to_string());
                        }
                        TrafficOption::Duration(_)
                        | TrafficOption::EncryptionMethod(_)
                        | TrafficOption::Compress(_)
                        | TrafficOption::Speed(None) => {}
                    }
                }
            }
            TrafficOperation::Cancel | TrafficOperation::Show => {}
        }
        let result = self.dependencies.build_traffic_executor(plan, arguments);
        self.finish(result)
    }

    /// 构建REPLACE执行器。
    pub fn buildReplace(
        &mut self,
        plan: &dyn InsertPlanData,
        children: Vec<ExecutorBox>,
    ) -> Option<ExecutorBox> {
        let result = self
            .dependencies
            .build_executor(ExecutorKind::Replace, plan, children);
        self.finish(result)
    }

    /// 构建授权执行器。
    pub fn buildGrant(&mut self, plan: &dyn PlanData) -> Option<ExecutorBox> {
        self.build_leaf(ExecutorKind::GrantDdl, plan)
    }

    /// 构建回收权限执行器。
    pub fn buildRevoke(&mut self, plan: &dyn PlanData) -> Option<ExecutorBox> {
        self.build_leaf(ExecutorKind::RevokeDdl, plan)
    }

    /// 设置遥测信息。
    pub fn setTelemetryInfo(&mut self, plan: &dyn DdlPlanData) {
        match plan.telemetry_event() {
            DdlTelemetryEvent::AlterTable(specifications) => {
                if specifications.len() > 1 {
                    self.Ti.use_multi_schema_change = true;
                }
                for specification in specifications {
                    match specification {
                        AlterTableTelemetrySpec::DropFirstPartition => {
                            self.Ti
                                .partition
                                .get_or_insert_with(PartitionTelemetryInfo::default)
                                .use_drop_interval_partition = true;
                        }
                        AlterTableTelemetrySpec::AddLastPartition => {
                            self.Ti
                                .partition
                                .get_or_insert_with(PartitionTelemetryInfo::default)
                                .use_add_interval_partition = true;
                        }
                        AlterTableTelemetrySpec::ExchangePartition => {
                            self.Ti.use_exchange_partition = true;
                        }
                        AlterTableTelemetrySpec::ReorganizePartition => {
                            self.Ti
                                .partition
                                .get_or_insert_with(PartitionTelemetryInfo::default)
                                .use_reorganize_partition = true;
                        }
                        AlterTableTelemetrySpec::Other => {}
                    }
                }
            }
            DdlTelemetryEvent::CreateTable(Some(partition)) => {
                let telemetry = self
                    .Ti
                    .partition
                    .get_or_insert_with(PartitionTelemetryInfo::default);
                telemetry.table_partition_max_partitions_num = partition
                    .declared_partitions
                    .max(partition.definition_count as u64);
                telemetry.use_table_partition = true;
                if !partition.has_subpartition {
                    match partition.kind {
                        PartitionTelemetryKind::Range => {
                            if partition.column_count == 0 {
                                telemetry.use_table_partition_range = true;
                            } else {
                                telemetry.use_table_partition_range_columns = true;
                                telemetry.use_table_partition_range_columns_gt_1 =
                                    partition.column_count > 1;
                                telemetry.use_table_partition_range_columns_gt_2 =
                                    partition.column_count > 2;
                                telemetry.use_table_partition_range_columns_gt_3 =
                                    partition.column_count > 3;
                            }
                            telemetry.use_create_interval_partition = partition.has_interval;
                        }
                        PartitionTelemetryKind::Hash => {
                            telemetry.use_table_partition_hash = true;
                        }
                        PartitionTelemetryKind::List => {
                            if partition.column_count == 0 {
                                telemetry.use_table_partition_list = true;
                            } else {
                                telemetry.use_table_partition_list_columns = true;
                            }
                        }
                        PartitionTelemetryKind::Other => {}
                    }
                }
            }
            DdlTelemetryEvent::FlashbackToTimestamp => {
                self.Ti.use_flashback_to_cluster = true;
            }
            DdlTelemetryEvent::CreateTable(None) | DdlTelemetryEvent::Other => {}
        }
    }

    /// 构建DDL执行器。
    pub fn buildDDL(&mut self, plan: &dyn DdlPlanData) -> Option<ExecutorBox> {
        self.setTelemetryInfo(plan);
        self.build_leaf(ExecutorKind::Ddl, plan)
    }

    /// 构建TRACE执行器。
    pub fn buildTrace(&mut self, plan: &dyn TracePlanData) -> Option<ExecutorBox> {
        let kind = if plan.log_format() && !plan.optimizer_trace() {
            ExecutorKind::TraceSorted
        } else {
            ExecutorKind::Trace
        };
        self.build_leaf(kind, plan)
    }

    /// 构建EXPLAIN执行器。
    pub fn buildExplain(&mut self, plan: &dyn ExplainPlanData) -> Option<ExecutorBox> {
        if plan.analyze()
            && let Err(error) = self.dependencies.initialize_runtime_stats()
        {
            self.err = Some(error);
            return None;
        }

        let mut children = Vec::new();
        if plan.analyze() || !plan.has_brief_binary_plan() {
            if let Some(target_plan) = plan.target_plan() {
                let target = self.build(Some(target_plan));
                if self.err.is_some() {
                    return None;
                }
                if let Some(target) = target {
                    children.push(target);
                }
            }
        }
        let result = self
            .dependencies
            .build_executor(ExecutorKind::Explain, plan, children);
        self.finish(result)
    }

    /// 构建SELECT INTO执行器。
    pub fn buildSelectInto(&mut self, plan: &dyn SelectIntoPlanData) -> Option<ExecutorBox> {
        let child = self.build(Some(plan.target_plan()));
        if self.err.is_some() {
            return None;
        }
        let Some(child) = child else {
            self.err = Some(BuildError::new(
                "SELECT INTO target did not build an executor",
            ));
            return None;
        };
        let result = self
            .dependencies
            .build_executor(ExecutorKind::SelectInto, plan, vec![child]);
        self.finish(result)
    }

    /// 构建联合扫描（含未提交写）Exec执行器。
    pub fn buildUnionScanExec(&mut self, plan: &dyn UnionScanPlanData) -> Option<ExecutorBox> {
        let original_encounter_union_scan = self.encounterUnionScan;
        self.encounterUnionScan = true;
        let reader = self.build(Some(plan.child_plan()));
        self.encounterUnionScan = original_encounter_union_scan;
        if self.err.is_some() {
            return None;
        }
        let Some(reader) = reader else {
            self.err = Some(BuildError::new("UnionScan child did not build an executor"));
            return None;
        };
        self.buildUnionScanFromReader(reader, plan)
    }

    /// 构建联合扫描（含未提交写）From读取器执行器。
    pub fn buildUnionScanFromReader(
        &mut self,
        reader: ExecutorBox,
        plan: &dyn UnionScanPlanData,
    ) -> Option<ExecutorBox> {
        let in_write_statement = self.inUpdateStmt || self.inDeleteStmt || self.inInsertStmt;
        let result =
            self.dependencies
                .build_union_scan_from_reader(plan, reader, in_write_statement);
        self.finish(result)
    }

    /// 构建归并连接执行器。
    pub fn buildMergeJoin(&mut self, plan: &dyn MergeJoinPlanData) -> Option<ExecutorBox> {
        let left = self.build(Some(plan.left_plan()));
        if self.err.is_some() {
            return None;
        }
        let Some(left) = left else {
            self.err = Some(BuildError::new("merge join left child is absent"));
            return None;
        };
        let right = self.build(Some(plan.right_plan()));
        if self.err.is_some() {
            return None;
        }
        let Some(right) = right else {
            self.err = Some(BuildError::new("merge join right child is absent"));
            return None;
        };
        if plan.inner_filter_count() != 0 {
            self.err = Some(BuildError::new("merge join's inner filter should be empty"));
            return None;
        }
        let result = self
            .dependencies
            .build_merge_join_executor(plan, left, right);
        self.finish(result)
    }

    /// 构建哈希连接V2执行器。
    pub fn buildHashJoinV2(&mut self, plan: &dyn HashJoinPlanData) -> Option<ExecutorBox> {
        let (left, right) = self.build_join_children(plan)?;
        self.buildHashJoinV2FromChildExecs(left, right, plan)
    }

    /// 构建哈希连接V2FromChildExecs执行器。
    pub fn buildHashJoinV2FromChildExecs(
        &mut self,
        left: ExecutorBox,
        right: ExecutorBox,
        plan: &dyn HashJoinPlanData,
    ) -> Option<ExecutorBox> {
        if !self.validate_hash_join(plan) {
            return None;
        }
        let right_as_build_side = !((plan.inner_child_index() == 1 && plan.use_outer_to_build())
            || (plan.inner_child_index() == 0 && !plan.use_outer_to_build()));
        let config = HashJoinBuildConfig {
            right_as_build_side,
            left_as_build_side: !right_as_build_side,
        };
        let result = self.dependencies.build_hash_join_executor(
            ExecutorKind::HashJoinV2,
            plan,
            left,
            right,
            config,
        );
        self.finish(result)
    }

    /// 构建哈希连接执行器。
    pub fn buildHashJoin(&mut self, plan: &dyn HashJoinPlanData) -> Option<ExecutorBox> {
        if plan.session_uses_v2() && plan.v2_supported() && plan.can_use_v2() {
            return self.buildHashJoinV2(plan);
        }
        let (left, right) = self.build_join_children(plan)?;
        self.buildHashJoinFromChildExecs(left, right, plan)
    }

    /// 构建哈希连接FromChildExecs执行器。
    pub fn buildHashJoinFromChildExecs(
        &mut self,
        left: ExecutorBox,
        right: ExecutorBox,
        plan: &dyn HashJoinPlanData,
    ) -> Option<ExecutorBox> {
        if !self.validate_hash_join(plan) {
            return None;
        }
        let left_as_build_side = if plan.use_outer_to_build() {
            plan.inner_child_index() == 1
        } else {
            plan.inner_child_index() == 0
        };
        let config = HashJoinBuildConfig {
            right_as_build_side: !left_as_build_side,
            left_as_build_side,
        };
        let result = self.dependencies.build_hash_join_executor(
            ExecutorKind::HashJoinV1,
            plan,
            left,
            right,
            config,
        );
        self.finish(result)
    }

    /// 构建joinchildren执行器。
    fn build_join_children(
        &mut self,
        plan: &dyn HashJoinPlanData,
    ) -> Option<(ExecutorBox, ExecutorBox)> {
        let left = self.build(Some(plan.left_plan()));
        if self.err.is_some() {
            return None;
        }
        let left = match left {
            Some(left) => left,
            None => {
                self.err = Some(BuildError::new("hash join left child is absent"));
                return None;
            }
        };
        let right = self.build(Some(plan.right_plan()));
        if self.err.is_some() {
            return None;
        }
        let right = match right {
            Some(right) => right,
            None => {
                self.err = Some(BuildError::new("hash join right child is absent"));
                return None;
            }
        };
        Some((left, right))
    }

    /// validatehashjoin。
    fn validate_hash_join(&mut self, plan: &dyn HashJoinPlanData) -> bool {
        let inner_condition_count = if plan.inner_child_index() == 1 {
            plan.right_condition_count()
        } else {
            plan.left_condition_count()
        };
        if inner_condition_count != 0 {
            self.err = Some(BuildError::new("join's inner condition should be empty"));
            return false;
        }
        if !plan.children_used_columns_available() {
            self.err = Some(BuildError::new("children used should never be nil"));
            return false;
        }
        true
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 排序按表达式枚举。
pub enum OrderByExpression {
    Column { id: i64 },
    NonColumn,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 排序按项。
pub struct OrderByItem {
    pub expression: OrderByExpression,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 有序列。
pub struct OrderedColumn {
    pub id: i64,
}

/// 收集ColIdxFrom按项s。
pub fn collectColIdxFromByItems(
    items: &[OrderByItem],
    columns: &[OrderedColumn],
) -> Result<Vec<usize>, BuildError> {
    let mut indexes = Vec::new();
    for item in items {
        let OrderByExpression::Column { id } = item.expression else {
            return Err(BuildError::new(
                "Not support non-column in orderBy pushed down",
            ));
        };
        if let Some(index) = columns.iter().position(|column| column.id == id) {
            indexes.push(index);
        }
    }
    Ok(indexes)
}

/// 处理Cached表。
pub fn handleCachedTable(
    dependencies: &dyn ExecutorBuilderDependencies,
    in_update_statement: bool,
    in_delete_statement: bool,
    in_insert_statement: bool,
    in_explain_statement: bool,
) -> Result<(), BuildError> {
    dependencies.handle_cached_table(
        in_update_statement || in_delete_statement || in_insert_statement,
        in_explain_statement,
    )
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 连接表达式枚举。
pub enum JoinExpression {
    Column(usize),
    Constant,
    CorrelatedColumn,
    ScalarFunction(Vec<JoinExpression>),
}

/// 收集列索引From表达式。
pub fn collectColumnIndexFromExpr(
    expression: &JoinExpression,
    left_column_size: usize,
    left_column_indexes: &mut Vec<usize>,
    right_column_indexes: &mut Vec<usize>,
) {
    match expression {
        JoinExpression::Column(index) if *index >= left_column_size => {
            right_column_indexes.push(*index - left_column_size);
        }
        JoinExpression::Column(index) => left_column_indexes.push(*index),
        JoinExpression::Constant | JoinExpression::CorrelatedColumn => {}
        JoinExpression::ScalarFunction(arguments) => {
            for argument in arguments {
                collectColumnIndexFromExpr(
                    argument,
                    left_column_size,
                    left_column_indexes,
                    right_column_indexes,
                );
            }
        }
    }
}

/// 提取Used列sIn连接OtherCondition。
pub fn extractUsedColumnsInJoinOtherCondition(
    expressions: &[JoinExpression],
    left_column_size: usize,
) -> (Vec<usize>, Vec<usize>) {
    let mut left_column_indexes = Vec::with_capacity(1);
    let mut right_column_indexes = Vec::with_capacity(1);
    for expression in expressions {
        collectColumnIndexFromExpr(
            expression,
            left_column_size,
            &mut left_column_indexes,
            &mut right_column_indexes,
        );
    }
    (left_column_indexes, right_column_indexes)
}
impl executorBuilder {
    /// 构建哈希聚合执行器。
    pub fn buildHashAgg(&mut self, plan: &dyn HashAggPlanData) -> Option<ExecutorBox> {
        let child = self.build_required_child(plan.child_plan(), "hash agg")?;
        self.buildHashAggFromChildExec(child, plan)
    }

    /// 构建哈希聚合FromChildExec执行器。
    pub fn buildHashAggFromChildExec(
        &mut self,
        child: ExecutorBox,
        plan: &dyn HashAggPlanData,
    ) -> Option<ExecutorBox> {
        let functions = plan.functions();
        let mut unparallel = functions
            .iter()
            .any(|function| function.order_by_count != 0);
        let has_distinct = functions.iter().any(|function| function.distinct);
        let final_concurrency = plan.final_concurrency();
        let partial_concurrency = plan.partial_concurrency();
        if final_concurrency <= 0
            || partial_concurrency <= 0
            || (final_concurrency == 1 && partial_concurrency == 1)
        {
            unparallel = true;
        }

        let mut partial_ordinal = 0;
        let mut partial_ordinals = Vec::with_capacity(functions.len());
        if !unparallel {
            for function in functions {
                let mut ordinals = vec![partial_ordinal];
                partial_ordinal += 1;
                if function.kind == AggregateFunctionKind::Average {
                    ordinals.push(partial_ordinal + 1);
                    partial_ordinal += 1;
                }
                partial_ordinals.push(ordinals);
            }
        }

        let config = HashAggBuildConfig {
            allocate_default_row: plan.group_by_count() == 0
                && !plan.all_first_row()
                && plan.is_final_aggregation(),
            unparallel,
            has_distinct,
            partial_ordinals,
        };
        let result = self
            .dependencies
            .build_hash_agg_executor(plan, child, config);
        self.finish(result)
    }

    /// 构建流式聚合执行器。
    pub fn buildStreamAgg(&mut self, plan: &dyn StreamAggPlanData) -> Option<ExecutorBox> {
        let child = self.build_required_child(plan.child_plan(), "stream agg")?;
        self.buildStreamAggFromChildExec(child, plan)
    }

    /// 构建流式聚合FromChildExec执行器。
    pub fn buildStreamAggFromChildExec(
        &mut self,
        child: ExecutorBox,
        plan: &dyn StreamAggPlanData,
    ) -> Option<ExecutorBox> {
        let config = StreamAggBuildConfig {
            allocate_default_row: plan.group_by_count() == 0
                && !plan.all_first_row()
                && plan.is_final_aggregation(),
        };
        let result = self
            .dependencies
            .build_stream_agg_executor(plan, child, config);
        self.finish(result)
    }

    /// 构建过滤 Selection执行器。
    pub fn buildSelection(&mut self, plan: &dyn UnaryPlanData) -> Option<ExecutorBox> {
        let child = self.build_required_child(plan.child_plan(), "selection")?;
        let result = self
            .dependencies
            .build_executor(ExecutorKind::Selection, plan, vec![child]);
        self.finish(result)
    }

    /// 构建Expand执行器。
    pub fn buildExpand(&mut self, plan: &dyn ParallelUnaryPlanData) -> Option<ExecutorBox> {
        let child = self.build_required_child(plan.child_plan(), "expand")?;
        let workers = self.parallel_unary_workers(plan);
        let result = self.dependencies.build_parallel_unary_executor(
            ExecutorKind::Expand,
            plan,
            child,
            workers,
        );
        self.finish(result)
    }

    /// 构建投影执行器。
    pub fn buildProjection(&mut self, plan: &dyn ParallelUnaryPlanData) -> Option<ExecutorBox> {
        let child = self.build_required_child(plan.child_plan(), "projection")?;
        self.newProjectionExec(child, plan)
    }

    /// 构造投影Exec。
    pub fn newProjectionExec(
        &mut self,
        child: ExecutorBox,
        plan: &dyn ParallelUnaryPlanData,
    ) -> Option<ExecutorBox> {
        let workers = self.parallel_unary_workers(plan);
        let result = self.dependencies.build_parallel_unary_executor(
            ExecutorKind::Projection,
            plan,
            child,
            workers,
        );
        self.finish(result)
    }

    /// parallelunaryworkers。
    fn parallel_unary_workers(&self, plan: &dyn ParallelUnaryPlanData) -> i64 {
        if plan.estimated_rows() < plan.max_chunk_size()
            || self.inUpdateStmt
            || self.inDeleteStmt
            || self.inInsertStmt
            || self.hasLock
        {
            0
        } else {
            plan.configured_workers()
        }
    }

    /// 构建TableDual执行器。
    pub fn buildTableDual(&mut self, plan: &dyn TableDualPlanData) -> Option<ExecutorBox> {
        if plan.row_count() > 1 {
            self.err = Some(BuildError::new(format!(
                "buildTableDual failed, invalid row count for dual table: {}",
                plan.row_count()
            )));
            return None;
        }
        self.build_leaf(ExecutorKind::TableDual, plan)
    }

    /// 获取快照时间戳。
    pub fn getSnapshotTS(&self) -> Result<u64, BuildError> {
        if self.forDataReaderBuilder {
            return Ok(self.dataReaderTS);
        }
        if self.inInsertStmt || self.inUpdateStmt || self.inDeleteStmt || self.inSelectLockStmt {
            return self.dependencies.statement_for_update_ts();
        }
        self.dependencies.statement_read_ts()
    }

    /// 获取快照。
    pub fn getSnapshot(&self) -> Result<Box<dyn Snapshot>, BuildError> {
        let mut snapshot =
            if self.inInsertStmt || self.inUpdateStmt || self.inDeleteStmt || self.inSelectLockStmt
            {
                self.dependencies.snapshot_with_for_update_ts()?
            } else {
                self.dependencies.snapshot_with_read_ts()?
            };
        InitSnapshotWithSessCtx(
            snapshot.as_mut(),
            &self.dependencies.snapshot_session_options(),
            Some(&self.readReplicaScope),
        )?;
        Ok(snapshot)
    }

    /// 构建内存表执行器。
    pub fn buildMemTable(&mut self, plan: &dyn PlanData) -> Option<ExecutorBox> {
        let result = self.dependencies.build_mem_table_executor(plan);
        self.finish(result)
    }

    /// 构建排序执行器。
    pub fn buildSort(&mut self, plan: &dyn UnaryPlanData) -> Option<ExecutorBox> {
        let child = self.build_required_child(plan.child_plan(), "sort")?;
        let result = self
            .dependencies
            .build_executor(ExecutorKind::Sort, plan, vec![child]);
        self.finish(result)
    }

    /// 构建TopN执行器。
    pub fn buildTopN(&mut self, plan: &dyn TopNPlanData) -> Option<ExecutorBox> {
        let child = self.build_required_child(plan.child_plan(), "top N")?;
        let projection = match plan.child_projection() {
            Some(projection) if projection.column_missing => None,
            Some(projection) => Some(projection.used_child_columns),
            None => None,
        };
        let result = self
            .dependencies
            .build_top_n_executor(plan, child, projection);
        self.finish(result)
    }

    /// 构建相关子查询 Apply执行器。
    pub fn buildApply(&mut self, plan: &dyn ApplyPlanData) -> Option<ExecutorBox> {
        let left = self.build_required_child(plan.left_plan(), "apply left")?;
        let right = self.build_required_child(plan.right_plan(), "apply right")?;
        let kind = if plan.concurrency() > 1 && self.dependencies.parallel_apply_is_buildable(plan)
        {
            ExecutorKind::ApplyParallel
        } else {
            ExecutorKind::ApplySerial
        };
        let result = self
            .dependencies
            .build_apply_executor(kind, plan, left, right);
        self.finish(result)
    }

    /// 构建最多一行执行器。
    pub fn buildMaxOneRow(&mut self, plan: &dyn UnaryPlanData) -> Option<ExecutorBox> {
        let child = self.build_required_child(plan.child_plan(), "max one row")?;
        let result = self
            .dependencies
            .build_executor(ExecutorKind::MaxOneRow, plan, vec![child]);
        self.finish(result)
    }

    /// 构建UNION ALL执行器。
    pub fn buildUnionAll(&mut self, plan: &dyn UnionAllPlanData) -> Option<ExecutorBox> {
        let mut children = Vec::with_capacity(plan.child_count());
        for index in 0..plan.child_count() {
            children.push(self.build_required_child(plan.child_plan(index), "union all")?);
        }
        let result = self
            .dependencies
            .build_executor(ExecutorKind::UnionAll, plan, children);
        self.finish(result)
    }

    /// 构建分布式表执行器。
    pub fn buildDistributeTable(&mut self, plan: &dyn PlanData) -> Option<ExecutorBox> {
        self.build_leaf(ExecutorKind::DistributeTable, plan)
    }

    /// 构建分裂 Region执行器。
    pub fn buildSplitRegion(&mut self, plan: &dyn SplitRegionPlanData) -> Option<ExecutorBox> {
        let kind = if plan.has_index() {
            ExecutorKind::SplitIndexRegion
        } else {
            if let Err(error) = buildHandleColsForSplit(plan.table_info()) {
                self.err = Some(error);
                return None;
            }
            let _uses_value_list_form = plan.has_value_lists();
            ExecutorKind::SplitTableRegion
        };
        self.build_leaf(kind, plan)
    }

    /// 构建更新执行器。
    pub fn buildUpdate(&mut self, plan: &dyn UpdatePlanData) -> Option<ExecutorBox> {
        self.inUpdateStmt = true;
        if let Err(error) = self.updateForUpdateTS() {
            self.err = Some(error);
            return None;
        }

        let child = self.build_required_child(plan.select_plan(), "update select")?;
        let assignment_flags = match getAssignFlag(
            plan.allow_write_row_id(),
            plan.schema_len(),
            plan.assignments(),
        ) {
            Ok(flags) => flags,
            Err(error) => {
                self.err = Some(error);
                return None;
            }
        };
        for result in [
            self.dependencies
                .validate_update_list(plan, &assignment_flags),
            self.dependencies.build_update_foreign_key_checks(plan),
            self.dependencies.build_update_foreign_key_cascades(plan),
        ] {
            if let Err(error) = result {
                self.err = Some(error);
                return None;
            }
        }
        let result = self
            .dependencies
            .build_update_executor(plan, child, assignment_flags);
        self.finish(result)
    }

    /// 构建删除执行器。
    pub fn buildDelete(&mut self, plan: &dyn DeletePlanData) -> Option<ExecutorBox> {
        self.inDeleteStmt = true;
        if let Err(error) = self.updateForUpdateTS() {
            self.err = Some(error);
            return None;
        }

        let child = self.build_required_child(plan.select_plan(), "delete select")?;
        for result in [
            self.dependencies.build_delete_foreign_key_checks(plan),
            self.dependencies.build_delete_foreign_key_cascades(plan),
        ] {
            if let Err(error) = result {
                self.err = Some(error);
                return None;
            }
        }
        let result = self.dependencies.build_delete_executor(plan, child);
        self.finish(result)
    }

    /// 更新SELECT FOR UPDATE时间戳。
    pub fn updateForUpdateTS(&self) -> Result<(), BuildError> {
        self.dependencies.statement_for_update_ts().map(|_| ())
    }

    /// 构建requiredchild执行器。
    fn build_required_child(&mut self, plan: Plan<'_>, operator: &str) -> Option<ExecutorBox> {
        let child = self.build(Some(plan));
        if self.err.is_some() {
            return None;
        }
        match child {
            Some(child) => Some(child),
            None => {
                self.err = Some(BuildError::new(format!(
                    "{operator} child did not build an executor"
                )));
                None
            }
        }
    }
}

/// 初始化快照带SessCtx。
pub fn InitSnapshotWithSessCtx(
    snapshot: &mut dyn Snapshot,
    options: &SnapshotSessionOptions,
    transaction_replica_read_scope: Option<&str>,
) -> Result<(), BuildError> {
    let replica_scope = transaction_replica_read_scope
        .unwrap_or(&options.transaction_read_replica_scope)
        .to_owned();
    snapshot.set_option(SnapshotOption::ReadReplicaScope(replica_scope.clone()))?;
    snapshot.set_option(SnapshotOption::TaskId(options.task_id))?;
    snapshot.set_option(SnapshotOption::ReadTimeoutMillis(
        options.read_timeout_millis,
    ))?;
    snapshot.set_option(SnapshotOption::ResourceGroupName(
        options.resource_group_name.clone(),
    ))?;
    snapshot.set_option(SnapshotOption::ExplicitRequestSourceType(
        options.explicit_request_source_type.clone(),
    ))?;
    if options.replica_read_is_closest && replica_scope != options.global_transaction_scope {
        snapshot.set_option(SnapshotOption::MatchStoreLabel {
            key: options.dc_label_key.clone(),
            value: replica_scope,
        })?;
    }
    Ok(())
}

/// 构建行句柄 handleCols用于分裂执行器。
pub fn buildHandleColsForSplit(table: &TableInfo) -> Result<HandleColumns, BuildError> {
    if table.common_handle {
        let mut indexes = Vec::with_capacity(table.primary_index_columns.len());
        for (index, primary_column) in table.primary_index_columns.iter().enumerate() {
            if primary_column.offset >= table.columns.len() {
                return Err(BuildError::new(
                    "primary index column offset is outside the table schema",
                ));
            }
            indexes.push(index);
        }
        return Ok(HandleColumns::Common {
            column_indexes: indexes,
        });
    }
    Ok(HandleColumns::Integer { column_index: 0 })
}

/// EXTRA_HANDLE_ID常量。
pub const EXTRA_HANDLE_ID: i64 = -1;

/// 获取Assign标志。
pub fn getAssignFlag(
    allow_write_row_id: bool,
    schema_len: usize,
    assignments: &[AssignmentSpec],
) -> Result<Vec<isize>, BuildError> {
    let mut assignment_flags = vec![-1; schema_len];
    for assignment in assignments {
        if !allow_write_row_id && assignment.column_id == EXTRA_HANDLE_ID {
            return Err(BuildError::new(
                "insert, update and replace statements for _tidb_rowid are not supported",
            ));
        }
        if let Some(table_index) = assignment.table_index {
            let flag = assignment_flags
                .get_mut(assignment.column_index)
                .ok_or_else(|| BuildError::new("update assignment column is outside the schema"))?;
            *flag = isize::try_from(table_index)
                .map_err(|_| BuildError::new("update table index exceeds isize"))?;
        }
    }
    Ok(assignment_flags)
}
impl executorBuilder {
    /// 构建统计信息收集索引下推执行器。
    pub fn buildAnalyzeIndexPushdown(
        &mut self,
        task: &dyn AnalyzeIndexTaskData,
        auto_analyze: bool,
    ) -> Option<Box<dyn AnalyzeTask>> {
        let snapshot_ts = match self.getSnapshotTS() {
            Ok(snapshot_ts) => snapshot_ts,
            Err(error) => {
                self.err = Some(error);
                return None;
            }
        };
        match self
            .dependencies
            .build_analyze_index_task(task, snapshot_ts, auto_analyze)
        {
            Ok(task) => Some(task),
            Err(error) => {
                self.err = Some(error);
                None
            }
        }
    }

    /// 构建统计信息收集采样下推执行器。
    pub fn buildAnalyzeSamplingPushdown(
        &mut self,
        task: &dyn AnalyzeColumnsTaskData,
    ) -> Option<Box<dyn AnalyzeTask>> {
        let snapshot_ts = match self.getSnapshotTS() {
            Ok(snapshot_ts) => snapshot_ts,
            Err(error) => {
                self.err = Some(error);
                return None;
            }
        };
        let counts = match self.dependencies.analyze_table_counts(task) {
            Ok(counts) => counts,
            Err(error) => {
                self.err = Some(error);
                return None;
            }
        };

        let mut sample_rate = task.configured_sample_rate();
        let mut sample_rate_reason = String::new();
        if sample_rate < 0.0 {
            match self.getAdjustedSampleRate(task) {
                Ok((adjusted, reason)) => {
                    sample_rate = adjusted;
                    sample_rate_reason = reason;
                }
                Err(error) => {
                    self.err = Some(error);
                    return None;
                }
            }
        }
        let config = AnalyzeSamplingConfig {
            snapshot_ts,
            sample_rate,
            sample_rate_reason,
            base_count: counts.base_count,
            base_modify_count: counts.base_modify_count,
        };
        match self.dependencies.build_analyze_columns_task(task, config) {
            Ok(task) => Some(task),
            Err(error) => {
                self.err = Some(error);
                None
            }
        }
    }

    /// 获取Adjusted采样Rate。
    pub fn getAdjustedSampleRate(
        &self,
        task: &dyn AnalyzeColumnsTaskData,
    ) -> Result<(f64, String), BuildError> {
        const DEFAULT_RATE: f64 = 0.001;
        const DESIRED_ROWS: f64 = 110_000.0;
        let counts = self.dependencies.analyze_table_counts(task)?;
        let approximate = counts.approximate_storage_count;
        let Some(realtime_count) = counts.stats_meta_count else {
            return match approximate {
                Some(approximate_count) if approximate_count > 0.0 => {
                    let rate = (150_000.0 / approximate_count).min(1.0);
                    Ok((
                        rate,
                        format!(
                            "stats_meta is unavailable; use min(1, 150000/{approximate_count}) as the sample-rate={rate}"
                        ),
                    ))
                }
                _ => Ok((
                    DEFAULT_RATE,
                    format!(
                        "TiDB cannot get the row count of the table, use the default-rate={DEFAULT_RATE}"
                    ),
                )),
            };
        };

        if realtime_count == 0 && approximate.is_none() {
            return Ok((
                1.0,
                "TiDB assumes that the table is empty and cannot get row count from PD, use sample-rate=1"
                    .to_owned(),
            ));
        }
        if let Some(approximate_count) = approximate
            && (realtime_count as f64) * 5.0 < approximate_count
        {
            let rate = (150_000.0 / approximate_count).min(1.0);
            return Ok((
                rate,
                format!(
                    "row count in stats_meta is much smaller than PD; use min(1, 150000/{approximate_count}) as the sample-rate={rate}"
                ),
            ));
        }
        if realtime_count == 0 {
            return Ok((
                1.0,
                "TiDB assumes that the table is empty, use sample-rate=1".to_owned(),
            ));
        }

        let rate = (DESIRED_ROWS / realtime_count as f64).min(1.0);
        Ok((
            rate,
            format!("use min(1, {DESIRED_ROWS}/{realtime_count}) as the sample-rate={rate}"),
        ))
    }

    /// 获取Approximate表CountFrom存储。
    pub fn getApproximateTableCountFromStorage(
        &self,
        task: &dyn AnalyzeColumnsTaskData,
    ) -> Result<(f64, bool), BuildError> {
        let approximate = self
            .dependencies
            .analyze_table_counts(task)?
            .approximate_storage_count;
        Ok(match approximate {
            Some(count) => (count, true),
            None => (0.0, false),
        })
    }

    /// 构建统计信息收集执行器。
    pub fn buildAnalyze(&mut self, plan: &dyn AnalyzePlanData) -> Option<ExecutorBox> {
        if let Err(error) = self.dependencies.flush_stats_delta_for_analyze(plan) {
            self.err = Some(error);
            return None;
        }

        let mut tasks = Vec::with_capacity(plan.column_task_count() + plan.index_task_count());
        for index in 0..plan.column_task_count() {
            let task = self.buildAnalyzeSamplingPushdown(plan.column_task(index))?;
            tasks.push(task);
        }
        for index in 0..plan.index_task_count() {
            let task =
                self.buildAnalyzeIndexPushdown(plan.index_task(index), plan.auto_analyze())?;
            tasks.push(task);
        }
        let result = self.dependencies.build_analyze_executor(plan, tasks);
        self.finish(result)
    }

    /// 检测相关列：corColInDist计划。
    pub fn corColInDistPlan(&self, plans: &[&dyn DistributedPlanData]) -> bool {
        plans
            .iter()
            .any(|plan| plan.has_correlated_column_in_supported_expression())
    }

    /// 检测相关列：corColInAccess。
    pub fn corColInAccess(&self, plan: &dyn DistributedPlanData) -> bool {
        plan.has_correlated_column_in_access_condition()
    }

    /// 构造数据读取器构建器。
    pub fn newDataReaderBuilder(
        &self,
        _plan: &dyn DistributedPlanData,
    ) -> Result<DataReaderBuilder, BuildError> {
        Ok(DataReaderBuilder {
            snapshot_ts: self.getSnapshotTS()?,
            index_join_key_unique_ids: Vec::new(),
            statement_context_lock: self
                .stmtCtxLock
                .clone()
                .unwrap_or_else(|| Arc::new(Mutex::new(()))),
            dependencies: self.dependencies.clone(),
            partition_pruning_result: Arc::new(OnceLock::new()),
        })
    }

    /// 构建索引回表连接执行器。
    pub fn buildIndexLookUpJoin(&mut self, plan: &dyn IndexJoinPlanData) -> Option<ExecutorBox> {
        self.build_index_join_kind(ExecutorKind::IndexLookupJoin, plan)
    }

    /// 构建索引回表归并连接执行器。
    pub fn buildIndexLookUpMergeJoin(
        &mut self,
        plan: &dyn IndexJoinPlanData,
    ) -> Option<ExecutorBox> {
        self.build_index_join_kind(ExecutorKind::IndexLookupMergeJoin, plan)
    }

    /// 构建索引嵌套循环哈希连接执行器。
    pub fn buildIndexNestedLoopHashJoin(
        &mut self,
        plan: &dyn IndexJoinPlanData,
    ) -> Option<ExecutorBox> {
        let lookup_join = self.buildIndexLookUpJoin(plan)?;
        let result = self
            .dependencies
            .wrap_index_nested_loop_hash_join(plan, lookup_join);
        self.finish(result)
    }

    /// 构建indexjoinkind执行器。
    fn build_index_join_kind(
        &mut self,
        kind: ExecutorKind,
        plan: &dyn IndexJoinPlanData,
    ) -> Option<ExecutorBox> {
        let outer = self.build_required_child(plan.outer_plan(), "index join outer")?;
        if plan.inner_filter_count() != 0 {
            self.err = Some(BuildError::new("join's inner condition should be empty"));
            return None;
        }
        let reader_builder = match self.newDataReaderBuilder(plan.inner_plan()) {
            Ok(reader_builder) => reader_builder,
            Err(error) => {
                self.err = Some(error);
                return None;
            }
        };
        let result = self
            .dependencies
            .build_index_join_executor(kind, plan, outer, reader_builder);
        self.finish(result)
    }

    /// 构建MPP（大规模并行处理）汇聚执行器。
    pub fn buildMPPGather(&mut self, plan: &dyn TableReaderPlanData) -> Option<ExecutorBox> {
        let snapshot_ts = match self.getSnapshotTS() {
            Ok(snapshot_ts) => snapshot_ts,
            Err(error) => {
                self.err = Some(error);
                return None;
            }
        };
        if plan.table_scan_count() != 1 && (plan.has_virtual_columns() || self.encounterUnionScan) {
            self.err = Some(BuildError::new(format!(
                "should only have one TableScan in MPP fragment(hasVirtualCol: {}, encounterUnionScan: {})",
                plan.has_virtual_columns(),
                self.encounterUnionScan
            )));
            return None;
        }
        let result = self
            .dependencies
            .build_mpp_gather_executor(plan, snapshot_ts);
        self.finish(result)
    }

    /// 构建表读取执行器。
    pub fn buildTableReader(&mut self, plan: &dyn TableReaderPlanData) -> Option<ExecutorBox> {
        if let Err(error) = self.dependencies.align_empty_table_reader_schema(plan) {
            self.err = Some(error);
            return None;
        }
        self.dependencies.mark_table_reader_store(plan);
        if plan.use_mpp() || plan.is_tiflash_batch_cop() {
            self.dependencies.warn_ignored_tiflash_replica_read(plan);
        }
        if plan.use_mpp() {
            return self.buildMPPGather(plan);
        }
        if let Err(error) = assertByItemsAreColumns(plan.by_items()) {
            self.err = Some(error);
            return None;
        }

        let reader = match buildNoRangeTableReader(self, plan) {
            Ok(reader) => reader,
            Err(error) => {
                self.err = Some(error);
                return None;
            }
        };
        if let Err(error) = self.dependencies.validate_table_reader_access(plan) {
            self.err = Some(error);
            return None;
        }

        let partition_ids = if !plan.dynamic_partition_pruning()
            || plan.already_partition_reader()
            || !plan.has_partition_info()
        {
            None
        } else {
            match self.dependencies.prune_table_reader_partitions(plan) {
                Ok(mut partitions) => {
                    partitions.sort_unstable();
                    Some(partitions)
                }
                Err(error) => {
                    self.err = Some(error);
                    return None;
                }
            }
        };
        let result = self
            .dependencies
            .finalize_table_reader(plan, reader, partition_ids);
        self.finish(result)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 模式/列集合列。
pub struct SchemaColumn {
    pub identity: i64,
    pub resolved_index: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 模式/列集合。
pub struct Schema {
    pub columns: Vec<SchemaColumn>,
}

/// 检索列IdxsUsed按Child。
pub fn retrieveColumnIdxsUsedByChild(
    self_schema: &Schema,
    child_schema: &Schema,
) -> (Option<Vec<isize>>, bool) {
    let mut equal_schema = self_schema.columns.len() == child_schema.columns.len();
    let mut column_missing = false;
    let mut indexes = Vec::with_capacity(self_schema.columns.len());
    for (self_index, self_column) in self_schema.columns.iter().enumerate() {
        let child_index = child_schema
            .columns
            .iter()
            .position(|child| child.identity == self_column.identity)
            .map(|index| index as isize)
            .unwrap_or(-1);
        column_missing |= child_index == -1;
        if equal_schema && self_index as isize != child_index {
            equal_schema = false;
        }
        indexes.push(child_index);
    }
    if equal_schema {
        (None, column_missing)
    } else {
        (Some(indexes), column_missing)
    }
}

/// 标记ChildrenUsedCols。
pub fn markChildrenUsedCols(
    output_columns: &[SchemaColumn],
    child_schemas: &[Schema],
) -> Vec<Vec<usize>> {
    let mut marked_offsets = BTreeMap::new();
    for (original_index, column) in output_columns.iter().enumerate() {
        marked_offsets.insert(column.resolved_index, original_index);
    }

    let mut prefix_length = 0;
    let mut result = Vec::with_capacity(child_schemas.len());
    for child_schema in child_schemas {
        let mut pairs = Vec::new();
        for index in 0..child_schema.columns.len() {
            if let Some(original_index) = marked_offsets.get(&(prefix_length + index)) {
                pairs.push((*original_index, index));
            }
        }
        pairs.sort_unstable_by_key(|pair| pair.0);
        result.push(pairs.into_iter().map(|pair| pair.1).collect());
        prefix_length += child_schema.columns.len();
    }
    result
}

/// 构建No范围表读取执行器。
pub fn buildNoRangeTableReader(
    builder: &executorBuilder,
    plan: &dyn TableReaderPlanData,
) -> Result<Box<dyn TableReaderDraft>, BuildError> {
    builder.dependencies.validate_table_reader_access(plan)?;
    let snapshot_ts = builder.getSnapshotTS()?;
    builder
        .dependencies
        .build_no_range_table_reader(plan, snapshot_ts)
}

/// 断言按项sAre列s。
pub fn assertByItemsAreColumns(items: &[OrderByExpression]) -> Result<(), BuildError> {
    if items
        .iter()
        .all(|item| matches!(item, OrderByExpression::Column { .. }))
    {
        Ok(())
    } else {
        Err(BuildError::new(
            "the executor only supports Column type in ByItems",
        ))
    }
}
/// 构建索引范围用于Each分区执行器。
pub fn buildIndexRangeForEachPartition(
    dependencies: &dyn ExecutorBuilderDependencies,
    used_partition_ids: &[i64],
    contents: &[IndexJoinLookUpContent],
) -> Result<Vec<Vec<KeyRange>>, BuildError> {
    let grouped = group_contents_by_partition(contents);
    let mut ranges = Vec::with_capacity(used_partition_ids.len());
    for partition_id in used_partition_ids {
        let partition_contents = grouped.get(partition_id).map(Vec::as_slice).unwrap_or(&[]);
        ranges.push(dependencies.build_partition_index_ranges(*partition_id, partition_contents)?);
    }
    Ok(ranges)
}

/// 获取分区键ColOffsets。
pub fn getPartitionKeyColOffsets(
    key_column_ids: &[i64],
    partition_column_ids: &[i64],
) -> Vec<usize> {
    key_column_ids
        .iter()
        .filter_map(|key_id| {
            partition_column_ids
                .iter()
                .position(|partition_id| partition_id == key_id)
        })
        .collect()
}

/// 排序KV范围s按Start键。
pub fn sortKVRangesByStartKey(ranges: &mut [KeyRange]) {
    ranges.sort_unstable_by(|left, right| left.start_key.cmp(&right.start_key));
}

/// 分组contentsbypartition。
fn group_contents_by_partition(
    contents: &[IndexJoinLookUpContent],
) -> BTreeMap<i64, Vec<IndexJoinLookUpContent>> {
    let mut grouped = BTreeMap::new();
    for content in contents {
        grouped
            .entry(content.partition_id)
            .or_insert_with(Vec::new)
            .push(content.clone());
    }
    grouped
}

impl executorBuilder {
    /// 构建索引读取执行器。
    pub fn buildIndexReader(&mut self, plan: &dyn IndexReaderPlanData) -> Option<ExecutorBox> {
        if let Err(error) = assertByItemsAreColumns(plan.by_items()) {
            self.err = Some(error);
            return None;
        }
        let reader = match buildNoRangeIndexReader(self, plan) {
            Ok(reader) => reader,
            Err(error) => {
                self.err = Some(error);
                return None;
            }
        };
        let result = self.dependencies.finalize_index_reader(plan, reader);
        self.finish(result)
    }

    /// 构建索引回表读取器执行器。
    pub fn buildIndexLookUpReader(
        &mut self,
        plan: &dyn IndexLookupReaderPlanData,
    ) -> Option<ExecutorBox> {
        let reader = match buildNoRangeIndexLookUpReader(self, plan) {
            Ok(reader) => reader,
            Err(error) => {
                self.err = Some(error);
                return None;
            }
        };
        let result = self.dependencies.finalize_index_lookup_reader(plan, reader);
        self.finish(result)
    }

    /// 构建索引使用情况报告器执行器。
    pub fn buildIndexUsageReporter(
        &self,
        plan: &dyn DistributedPlanData,
        load_stats: bool,
    ) -> Result<Box<dyn IndexUsageReporter>, BuildError> {
        buildIndexUsageReporter(self.dependencies.as_ref(), plan, load_stats)
    }

    /// 构建索引合并读取器执行器。
    pub fn buildIndexMergeReader(
        &mut self,
        plan: &dyn IndexMergeReaderPlanData,
    ) -> Option<ExecutorBox> {
        let reader = match buildNoRangeIndexMergeReader(self, plan) {
            Ok(reader) => reader,
            Err(error) => {
                self.err = Some(error);
                return None;
            }
        };
        let result = self.dependencies.finalize_index_merge_reader(plan, reader);
        self.finish(result)
    }
}

/// 构建No范围索引读取执行器。
pub fn buildNoRangeIndexReader(
    builder: &executorBuilder,
    plan: &dyn IndexReaderPlanData,
) -> Result<Box<dyn IndexReaderDraft>, BuildError> {
    let snapshot_ts = builder.getSnapshotTS()?;
    builder
        .dependencies
        .build_no_range_index_reader(plan, snapshot_ts)
}

/// 构建表Req执行器。
pub fn buildTableReq(
    dependencies: &dyn ExecutorBuilderDependencies,
    schema_len: usize,
    plan: &dyn DistributedPlanData,
) -> Result<Box<dyn DagRequest>, BuildError> {
    dependencies.build_table_request(schema_len, plan)
}

/// 构建索引回表PushDownDAGReq执行器。
pub fn buildIndexLookUpPushDownDAGReq(
    dependencies: &dyn ExecutorBuilderDependencies,
    column_count: usize,
    handle_len: usize,
) -> Result<Box<dyn DagRequest>, BuildError> {
    dependencies.build_index_request(column_count, handle_len, true)
}

/// 构建索引Req执行器。
pub fn buildIndexReq(
    dependencies: &dyn ExecutorBuilderDependencies,
    column_count: usize,
    handle_len: usize,
) -> Result<Box<dyn DagRequest>, BuildError> {
    dependencies.build_index_request(column_count, handle_len, false)
}

/// 构建索引扫描OutputOffsets执行器。
pub fn buildIndexScanOutputOffsets(
    by_items: &[OrderByExpression],
    schema_column_ids: &[i64],
    index_column_count: usize,
    handle_len: usize,
    need_extra_output_column: bool,
) -> Result<Vec<u32>, BuildError> {
    let mut output_offsets =
        Vec::with_capacity(by_items.len() + handle_len + usize::from(need_extra_output_column));
    for item in by_items {
        let OrderByExpression::Column { id } = item else {
            return Err(BuildError::new(
                "Not support non-column in orderBy pushed down",
            ));
        };
        let offset = schema_column_ids
            .iter()
            .position(|schema_id| schema_id == id)
            .ok_or_else(|| {
                BuildError::new("Not found order by related columns in indexScan.schema")
            })?;
        output_offsets.push(
            u32::try_from(offset)
                .map_err(|_| BuildError::new("index scan output offset exceeds uint32"))?,
        );
    }
    for handle_offset in 0..handle_len {
        output_offsets.push(
            u32::try_from(index_column_count + handle_offset)
                .map_err(|_| BuildError::new("handle output offset exceeds uint32"))?,
        );
    }
    if need_extra_output_column {
        output_offsets.push(
            u32::try_from(index_column_count + handle_len)
                .map_err(|_| BuildError::new("extra output offset exceeds uint32"))?,
        );
    }
    Ok(output_offsets)
}

/// 构建No范围索引回表读取器执行器。
pub fn buildNoRangeIndexLookUpReader(
    builder: &executorBuilder,
    plan: &dyn IndexLookupReaderPlanData,
) -> Result<Box<dyn IndexLookupReaderDraft>, BuildError> {
    let snapshot_ts = builder.getSnapshotTS()?;
    builder
        .dependencies
        .build_no_range_index_lookup_reader(plan, snapshot_ts)
}

/// 构建No范围索引合并读取器执行器。
pub fn buildNoRangeIndexMergeReader(
    builder: &executorBuilder,
    plan: &dyn IndexMergeReaderPlanData,
) -> Result<Box<dyn IndexMergeReaderDraft>, BuildError> {
    let snapshot_ts = builder.getSnapshotTS()?;
    builder
        .dependencies
        .build_no_range_index_merge_reader(plan, snapshot_ts)
}

/// 构建索引使用情况报告器执行器。
pub fn buildIndexUsageReporter(
    dependencies: &dyn ExecutorBuilderDependencies,
    plan: &dyn DistributedPlanData,
    load_stats: bool,
) -> Result<Box<dyn IndexUsageReporter>, BuildError> {
    dependencies.build_index_usage_reporter(plan, load_stats)
}

impl DataReaderBuilder {
    /// 分组索引连接回表内容按分区。
    pub fn groupIndexJoinLookUpContentsByPartition(
        &self,
        contents: &[IndexJoinLookUpContent],
    ) -> BTreeMap<i64, Vec<IndexJoinLookUpContent>> {
        group_contents_by_partition(contents)
    }

    /// 构建分区ed表读取KV范围s用于索引连接执行器。
    pub fn buildPartitionedTableReaderKVRangesForIndexJoin(
        &self,
        used_partition_ids: &[i64],
        contents: &[IndexJoinLookUpContent],
    ) -> Result<Vec<KeyRange>, BuildError> {
        let mut ranges = buildIndexRangeForEachPartition(
            self.dependencies.as_ref(),
            used_partition_ids,
            contents,
        )?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        sortKVRangesByStartKey(&mut ranges);
        Ok(ranges)
    }

    /// 构建索引连接分区范围s执行器。
    pub fn buildIndexJoinPartitionRanges(
        &self,
        contents: &[IndexJoinLookUpContent],
    ) -> Result<IndexJoinPartitionRanges, BuildError> {
        let partition_ids = self.prunePartitionForInnerExecutor(contents)?;
        let grouped = self.groupIndexJoinLookUpContentsByPartition(contents);
        let mut key_ranges = Vec::with_capacity(partition_ids.len());
        let mut lookup_contents = Vec::with_capacity(partition_ids.len());
        for partition_id in &partition_ids {
            let partition_contents = grouped.get(partition_id).cloned().unwrap_or_default();
            key_ranges.push(
                self.dependencies
                    .build_partition_index_ranges(*partition_id, &partition_contents)?,
            );
            lookup_contents.push(partition_contents);
        }
        Ok(IndexJoinPartitionRanges {
            partition_ids,
            key_ranges,
            lookup_contents,
        })
    }

    /// prune分区用于Inner执行器。
    pub fn prunePartitionForInnerExecutor(
        &self,
        contents: &[IndexJoinLookUpContent],
    ) -> Result<Vec<i64>, BuildError> {
        self.dependencies.prune_inner_partitions(contents)
    }

    /// 内存使用情况。
    pub fn MemoryUsage(&self) -> i64 {
        i64::try_from(std::mem::size_of::<Self>()).unwrap_or(i64::MAX)
    }

    /// clone用于索引连接构建。
    pub fn cloneForIndexJoinBuild(&self) -> DataReaderBuilder {
        DataReaderBuilder {
            snapshot_ts: self.snapshot_ts,
            index_join_key_unique_ids: self.index_join_key_unique_ids.clone(),
            statement_context_lock: self.statement_context_lock.clone(),
            dependencies: self.dependencies.clone(),
            partition_pruning_result: self.partition_pruning_result.clone(),
        }
    }

    /// 构建执行器用于索引连接执行器。
    pub fn BuildExecutorForIndexJoin(
        &self,
        plan: &dyn DataReaderPlanData,
        lookup_contents: &[IndexJoinLookUpContent],
        range_mem_tracker: Arc<Tracker>,
    ) -> Result<ExecutorBox, BuildError> {
        self.cloneForIndexJoinBuild()
            .buildExecutorForIndexJoinInternal(plan, lookup_contents, range_mem_tracker)
    }

    /// 构建执行器用于索引连接Internal执行器。
    pub fn buildExecutorForIndexJoinInternal(
        &self,
        plan: &dyn DataReaderPlanData,
        lookup_contents: &[IndexJoinLookUpContent],
        range_mem_tracker: Arc<Tracker>,
    ) -> Result<ExecutorBox, BuildError> {
        let kind = match plan.kind() {
            DataReaderPlanKind::TableReader => DataReaderBuildKind::TableReader,
            DataReaderPlanKind::IndexReader => DataReaderBuildKind::IndexReader,
            DataReaderPlanKind::IndexLookupReader => DataReaderBuildKind::IndexLookupReader,
            DataReaderPlanKind::UnionScan => DataReaderBuildKind::UnionScan,
            DataReaderPlanKind::Projection => DataReaderBuildKind::Projection,
            DataReaderPlanKind::HashJoin => DataReaderBuildKind::HashJoin,
        };
        self.dependencies.build_data_reader_executor(
            kind,
            plan,
            lookup_contents,
            self.snapshot_ts,
            Some(range_mem_tracker),
        )
    }

    /// 构建哈希连接用于索引连接执行器。
    pub fn buildHashJoinForIndexJoin(
        &self,
        plan: &dyn DataReaderPlanData,
        lookup_contents: &[IndexJoinLookUpContent],
    ) -> Result<ExecutorBox, BuildError> {
        if self.indexJoinLookupChildIdx(plan).is_none() {
            return Err(BuildError::new(
                "index join inner hash join cannot locate lookup child",
            ));
        }
        let mut executor = self.buildIndexJoinHashJoinExecFromChildExecs(plan, lookup_contents)?;
        openIndexJoinHashJoinExec(self.dependencies.as_ref(), executor.as_mut())?;
        Ok(executor)
    }

    /// 构建索引连接哈希连接ExecFromChildExecs执行器。
    pub fn buildIndexJoinHashJoinExecFromChildExecs(
        &self,
        plan: &dyn DataReaderPlanData,
        lookup_contents: &[IndexJoinLookUpContent],
    ) -> Result<ExecutorBox, BuildError> {
        self.dependencies.build_data_reader_executor(
            DataReaderBuildKind::HashJoin,
            plan,
            lookup_contents,
            self.snapshot_ts,
            None,
        )
    }

    /// index连接回表ChildIdx。
    pub fn indexJoinLookupChildIdx(&self, plan: &dyn DataReaderPlanData) -> Option<usize> {
        let child_index = plan.lookup_child_index()?;
        (child_index < 2).then_some(child_index)
    }

    /// 构建联合扫描（含未提交写）用于索引连接执行器。
    pub fn buildUnionScanForIndexJoin(
        &self,
        plan: &dyn DataReaderPlanData,
        lookup_contents: &[IndexJoinLookUpContent],
    ) -> Result<ExecutorBox, BuildError> {
        self.dependencies.build_data_reader_executor(
            DataReaderBuildKind::UnionScan,
            plan,
            lookup_contents,
            self.snapshot_ts,
            None,
        )
    }

    /// 构建表读取用于索引连接执行器。
    pub fn buildTableReaderForIndexJoin(
        &self,
        plan: &dyn DataReaderPlanData,
        lookup_contents: &[IndexJoinLookUpContent],
    ) -> Result<ExecutorBox, BuildError> {
        self.dependencies.build_data_reader_executor(
            DataReaderBuildKind::TableReader,
            plan,
            lookup_contents,
            self.snapshot_ts,
            None,
        )
    }

    /// 构建表读取Base执行器。
    pub fn buildTableReaderBase(&self, ranges: &[KeyRange]) -> Result<ExecutorBox, BuildError> {
        self.dependencies
            .build_table_reader_base_for_index_join(self.snapshot_ts, ranges)
    }

    /// 构建表读取From行句柄 handles执行器。
    pub fn buildTableReaderFromHandles(
        &self,
        handles: &[Vec<u8>],
        can_reorder_handles: bool,
    ) -> Result<ExecutorBox, BuildError> {
        let mut handles = handles.to_vec();
        if can_reorder_handles {
            handles.sort_unstable();
        }
        handles.dedup();
        let ranges = handles
            .into_iter()
            .map(|handle| KeyRange {
                start_key: handle.clone(),
                end_key: handle,
            })
            .collect::<Vec<_>>();
        self.buildTableReaderBase(&ranges)
    }

    /// 构建表读取FromKv范围s执行器。
    pub fn buildTableReaderFromKvRanges(
        &self,
        ranges: &[KeyRange],
    ) -> Result<ExecutorBox, BuildError> {
        self.buildTableReaderBase(ranges)
    }

    /// 构建索引读取用于索引连接执行器。
    pub fn buildIndexReaderForIndexJoin(
        &self,
        plan: &dyn DataReaderPlanData,
        lookup_contents: &[IndexJoinLookUpContent],
        range_mem_tracker: Arc<Tracker>,
    ) -> Result<ExecutorBox, BuildError> {
        self.dependencies.build_data_reader_executor(
            DataReaderBuildKind::IndexReader,
            plan,
            lookup_contents,
            self.snapshot_ts,
            Some(range_mem_tracker),
        )
    }

    /// 构建索引回表读取器用于索引连接执行器。
    pub fn buildIndexLookUpReaderForIndexJoin(
        &self,
        plan: &dyn DataReaderPlanData,
        lookup_contents: &[IndexJoinLookUpContent],
        range_mem_tracker: Arc<Tracker>,
    ) -> Result<ExecutorBox, BuildError> {
        self.dependencies.build_data_reader_executor(
            DataReaderBuildKind::IndexLookupReader,
            plan,
            lookup_contents,
            self.snapshot_ts,
            Some(range_mem_tracker),
        )
    }

    /// 构建投影用于索引连接执行器。
    pub fn buildProjectionForIndexJoin(
        &self,
        plan: &dyn DataReaderPlanData,
        lookup_contents: &[IndexJoinLookUpContent],
    ) -> Result<ExecutorBox, BuildError> {
        self.dependencies.build_data_reader_executor(
            DataReaderBuildKind::Projection,
            plan,
            lookup_contents,
            self.snapshot_ts,
            None,
        )
    }
}

/// open索引连接哈希连接Exec。
pub fn openIndexJoinHashJoinExec(
    dependencies: &dyn ExecutorBuilderDependencies,
    executor: &mut dyn Executor,
) -> Result<(), BuildError> {
    dependencies.open_index_join_hash_executor(executor)
}

/// Build the two HashJoin children in Go order and close every successfully
/// built child if a later child fails. Ownership transfers only after success.
pub fn buildIndexJoinHashJoinChildrenWithCleanup(
    build_lookup_child: impl FnOnce() -> Result<ExecutorBox, BuildError>,
    build_other_child: impl FnOnce() -> Result<ExecutorBox, BuildError>,
) -> Result<[ExecutorBox; 2], BuildError> {
    let lookup_child = build_lookup_child()?;
    match build_other_child() {
        Ok(other_child) => Ok([lookup_child, other_child]),
        Err(error) => {
            let mut lookup_child = lookup_child;
            let _ = lookup_child.Close();
            Err(error)
        }
    }
}

/// schemaContainsUniqueIDs。
/// 检查 schema 列 ID 是否包含全部 required unique IDs。
pub fn schemaContainsUniqueIDs(schema_ids: &[i64], required_ids: &[i64]) -> bool {
    required_ids
        .iter()
        .all(|required| schema_ids.iter().any(|candidate| candidate == required))
}

/// dedup行句柄 handles。
/// 对 Index Join 探测内容按同质键去重，返回 handle 与有效项。
pub fn dedupHandles(
    lookup_contents: &[IndexJoinLookUpContent],
) -> (Vec<Vec<u8>>, Vec<IndexJoinLookUpContent>) {
    let mut handles = Vec::with_capacity(lookup_contents.len());
    let mut valid_contents = Vec::with_capacity(lookup_contents.len());
    for content in lookup_contents {
        let Some(first) = content.key_values.first() else {
            continue;
        };
        if content.key_values.iter().all(|key| key == first) {
            handles.push(content.handle.clone());
            valid_contents.push(content.clone());
        }
    }
    (handles, valid_contents)
}

/// Kv范围构建器From范围And分区。
pub struct KvRangeBuilderFromRangeAndPartition {
    partition_ids: Vec<i64>,
    dependencies: Arc<dyn ExecutorBuilderDependencies>,
}

/// kv范围构建器From范围And分区类型别名。
pub type kvRangeBuilderFromRangeAndPartition = KvRangeBuilderFromRangeAndPartition;

impl KvRangeBuilderFromRangeAndPartition {
    /// 构建键范围Separately执行器。
    pub fn buildKeyRangeSeparately(
        &self,
        ranges: &[LogicalRange],
    ) -> Result<(Vec<i64>, Vec<Vec<KeyRange>>), BuildError> {
        let mut result = Vec::with_capacity(self.partition_ids.len());
        for partition_id in &self.partition_ids {
            result.push(self.dependencies.encode_key_ranges(*partition_id, ranges)?);
        }
        Ok((self.partition_ids.clone(), result))
    }

    /// 构建键范围执行器。
    pub fn buildKeyRange(&self, ranges: &[LogicalRange]) -> Result<Vec<Vec<KeyRange>>, BuildError> {
        self.buildKeyRangeSeparately(ranges)
            .map(|(_, ranges)| ranges)
    }
}

/// 构造就近读调节器。
pub fn newClosestReadAdjuster(net_data_size: f64) -> ClosestReadAdjuster {
    ClosestReadAdjuster { net_data_size }
}

/// 构建范围s用于索引连接执行器。
pub fn buildRangesForIndexJoin(
    lookup_contents: &[IndexJoinLookUpContent],
    template_ranges: &[LogicalRange],
    key_offset_to_index_offset: &[usize],
) -> Result<Vec<LogicalRange>, BuildError> {
    let mut result = Vec::with_capacity(lookup_contents.len() * template_ranges.len());
    for content in lookup_contents {
        for template in template_ranges {
            let mut range = template.clone();
            for (key_offset, index_offset) in key_offset_to_index_offset.iter().copied().enumerate()
            {
                let key = content.key_values.get(key_offset).ok_or_else(|| {
                    BuildError::new("index join lookup key offset is out of bounds")
                })?;
                let low = range.low.get_mut(index_offset).ok_or_else(|| {
                    BuildError::new("index join range index offset is out of bounds")
                })?;
                let high = range.high.get_mut(index_offset).ok_or_else(|| {
                    BuildError::new("index join range index offset is out of bounds")
                })?;
                *low = *key;
                *high = *key;
            }
            result.push(range);
        }
    }
    Ok(result)
}

/// 构建Kv范围s用于索引连接执行器。
pub fn buildKvRangesForIndexJoin(
    dependencies: &dyn ExecutorBuilderDependencies,
    table_id: i64,
    lookup_contents: &[IndexJoinLookUpContent],
    template_ranges: &[LogicalRange],
    key_offset_to_index_offset: &[usize],
) -> Result<Vec<KeyRange>, BuildError> {
    let ranges =
        buildRangesForIndexJoin(lookup_contents, template_ranges, key_offset_to_index_offset)?;
    dependencies.encode_key_ranges(table_id, &ranges)
}
impl executorBuilder {
    /// 构建窗口函数执行器。
    pub fn buildWindow(&mut self, plan: &dyn WindowPlanData) -> Option<ExecutorBox> {
        let child = self.build_required_child(plan.child_plan(), "window")?;
        let result = self
            .dependencies
            .build_executor(ExecutorKind::Window, plan, vec![child]);
        self.finish(result)
    }

    /// 构建Shuffle执行器。
    pub fn buildShuffle(&mut self, plan: &dyn ShufflePlanData) -> Option<ExecutorBox> {
        let mut data_sources = Vec::with_capacity(plan.data_source_count());
        for index in 0..plan.data_source_count() {
            data_sources.push(
                self.build_required_child(plan.data_source_plan(index), "shuffle data source")?,
            );
        }
        let mut workers = Vec::with_capacity(plan.worker_count());
        for index in 0..plan.worker_count() {
            workers.push(self.build_required_child(plan.worker_plan(index), "shuffle worker")?);
        }
        let result = self
            .dependencies
            .build_shuffle_executor(plan, data_sources, workers);
        self.finish(result)
    }

    /// 构建ShuffleReceiverStub执行器。
    pub fn buildShuffleReceiverStub(&mut self, plan: &dyn PlanData) -> Option<ExecutorBox> {
        self.build_leaf(ExecutorKind::ShuffleReceiver, plan)
    }

    /// 构建SQLBindExec执行器。
    pub fn buildSQLBindExec(&mut self, plan: &dyn PlanData) -> Option<ExecutorBox> {
        self.build_leaf(ExecutorKind::SqlBind, plan)
    }

    /// 构建批量点查询执行器。
    pub fn buildBatchPointGet(&mut self, plan: &dyn BatchPointGetPlanData) -> Option<ExecutorBox> {
        if let Err(error) = self.validCanReadTemporaryOrCacheTable(plan.table()) {
            self.err = Some(error);
            return None;
        }

        let reset_select_lock = plan.lock() && !self.inSelectLockStmt;
        if reset_select_lock {
            self.inSelectLockStmt = true;
        }
        let prune_result = match self.dependencies.prune_batch_point_get(plan) {
            Ok(result) => result,
            Err(error) => {
                self.err = Some(error);
                if reset_select_lock {
                    self.inSelectLockStmt = false;
                }
                return None;
            }
        };
        if prune_result.table_dual {
            let result =
                self.dependencies
                    .build_executor(ExecutorKind::TableDual, plan, Vec::new());
            if reset_select_lock {
                self.inSelectLockStmt = false;
            }
            return self.finish(result);
        }

        let mut snapshot = match self.getSnapshot() {
            Ok(snapshot) => snapshot,
            Err(error) => {
                self.err = Some(error);
                if reset_select_lock {
                    self.inSelectLockStmt = false;
                }
                return None;
            }
        };
        if plan.closest_read_adaptive()
            && let Err(error) =
                snapshot.set_option(SnapshotOption::ReplicaReadAdjuster(newReplicaReadAdjuster(
                    plan.average_row_size(),
                    plan.closest_read_threshold_bytes(),
                    plan.transaction_scope(),
                )))
        {
            self.err = Some(error);
            if reset_select_lock {
                self.inSelectLockStmt = false;
            }
            return None;
        }

        let snapshot_ts = match self.getSnapshotTS() {
            Ok(snapshot_ts) => snapshot_ts,
            Err(error) => {
                self.err = Some(error);
                if reset_select_lock {
                    self.inSelectLockStmt = false;
                }
                return None;
            }
        };
        let cache = if plan.table().cache_enabled() {
            match self.getCacheTable(plan.table(), snapshot_ts) {
                Ok(cache) => cache,
                Err(error) => {
                    self.err = Some(error);
                    if reset_select_lock {
                        self.inSelectLockStmt = false;
                    }
                    return None;
                }
            }
        } else {
            None
        };

        let mut lock = plan.lock();
        if plan.table().temporary_table_kind() != TemporaryTableKind::None {
            lock = false;
        }
        if lock {
            self.hasLock = true;
        }
        let capacity = if prune_result.handles.is_empty() {
            prune_result.index_value_count
        } else {
            prune_result.handles.len()
        };
        let result = self.dependencies.build_batch_point_get_executor(
            plan,
            snapshot,
            prune_result,
            lock,
            capacity,
            cache,
        );
        if reset_select_lock {
            self.inSelectLockStmt = false;
        }
        self.finish(result)
    }

    /// 构建表采样执行器。
    pub fn buildTableSample(&mut self, plan: &dyn TableSamplePlanData) -> Option<ExecutorBox> {
        let snapshot_ts = match self.getSnapshotTS() {
            Ok(snapshot_ts) => snapshot_ts,
            Err(error) => {
                self.err = Some(error);
                return None;
            }
        };
        let empty_sampler = match plan.table().temporary_table_kind() {
            TemporaryTableKind::Local => {
                self.err = Some(BuildError::new(
                    "TABLESAMPLE clause can not be applied to local temporary tables",
                ));
                return None;
            }
            TemporaryTableKind::Global => true,
            TemporaryTableKind::None => false,
        };
        let result = self.dependencies.build_table_sample_executor(
            plan,
            snapshot_ts,
            empty_sampler || !plan.uses_tidb_region_sampling(),
        );
        self.finish(result)
    }

    /// 构建公用表表达式 CTE执行器。
    pub fn buildCTE(&mut self, plan: &dyn CtePlanData) -> Option<ExecutorBox> {
        self.withStmtCtxLock(|| {});
        self.Ti.use_non_recursive_cte = true;
        if plan.recursive_plan().is_some() {
            self.Ti.use_recursive_cte = true;
        }

        let storages = match self.loadOrStoreCTEStorages(plan.storage_id()) {
            Ok(storages) => storages,
            Err(error) => {
                self.err = Some(error);
                return None;
            }
        };
        let init_result = storages
            .init_result
            .get_or_init(|| self.buildCTEStorageProducer(plan, &storages));
        if let Err(error) = init_result {
            self.err = Some(error.clone());
            return None;
        }
        let result = self.dependencies.build_cte_executor(plan, storages);
        self.finish(result)
    }

    /// load公用表表达式 CTE存储s。
    pub fn loadCTEStorages(&self, storage_id: i64) -> Result<Option<Arc<CTEStorages>>, BuildError> {
        self.withStmtCtxLock(|| self.dependencies.load_cte_storages(storage_id))
    }

    /// loadOrStore公用表表达式 CTE存储s。
    pub fn loadOrStoreCTEStorages(&self, storage_id: i64) -> Result<Arc<CTEStorages>, BuildError> {
        if let Some(storages) = self.loadCTEStorages(storage_id)? {
            return Ok(storages);
        }
        self.withStmtCtxLock(|| {
            if let Some(storages) = self.dependencies.load_cte_storages(storage_id)? {
                return Ok(storages);
            }
            self.dependencies.create_and_store_cte_storages(storage_id)
        })
    }

    /// 构建公用表表达式 CTE存储生产者执行器。
    pub fn buildCTEStorageProducer(
        &mut self,
        plan: &dyn CtePlanData,
        storages: &Arc<CTEStorages>,
    ) -> Result<(), BuildError> {
        let seed_plan = plan
            .seed_plan()
            .ok_or_else(|| BuildError::new("cte.seedPlan cannot be nil"))?;
        let seed = self
            .build(Some(seed_plan))
            .ok_or_else(|| self.take_or_make_error("CTE seed"))?;
        let recursive = if let Some(recursive_plan) = plan.recursive_plan() {
            let recursive = self
                .build(Some(recursive_plan))
                .ok_or_else(|| self.take_or_make_error("CTE recursive"));
            match recursive {
                Ok(recursive) => Some(recursive),
                Err(error) => {
                    storages.clear_after_build_error();
                    return Err(error);
                }
            }
        } else {
            None
        };
        let result = self
            .dependencies
            .build_cte_storage_producer(plan, storages, seed, recursive);
        if result.is_err() {
            storages.clear_after_build_error();
        }
        result
    }

    /// 构建公用表表达式 CTE表读取执行器。
    pub fn buildCTETableReader(&mut self, plan: &dyn CteTablePlanData) -> Option<ExecutorBox> {
        let storages = match self.loadCTEStorages(plan.storage_id()) {
            Ok(Some(storages)) => storages,
            Ok(None) => {
                self.err = Some(BuildError::new(format!(
                    "iterInTbl should already be set up by CTEExec(id: {})",
                    plan.storage_id()
                )));
                return None;
            }
            Err(error) => {
                self.err = Some(error);
                return None;
            }
        };
        let result = self
            .dependencies
            .build_cte_table_reader_executor(plan, storages);
        self.finish(result)
    }

    /// validCan读临时OrCache表。
    pub fn validCanReadTemporaryOrCacheTable(
        &self,
        table: &dyn TableReadSpec,
    ) -> Result<(), BuildError> {
        self.validCanReadTemporaryTable(table)?;
        self.validCanReadCacheTable(table)
    }

    /// validCan读Cache表。
    pub fn validCanReadCacheTable(&self, table: &dyn TableReadSpec) -> Result<(), BuildError> {
        if table.cache_enabled() && (table.transaction_is_staleness() || self.isStaleness) {
            return Err(BuildError::new("can not stale read cache table"));
        }
        Ok(())
    }

    /// validCan读临时表。
    pub fn validCanReadTemporaryTable(&self, table: &dyn TableReadSpec) -> Result<(), BuildError> {
        if table.temporary_table_kind() == TemporaryTableKind::None {
            return Ok(());
        }
        if table.temporary_table_kind() == TemporaryTableKind::Local
            && table.session_snapshot_ts() != 0
        {
            return Err(BuildError::new(
                "can not read local temporary table when 'tidb_snapshot' is set",
            ));
        }
        if table.transaction_is_staleness() || self.isStaleness {
            return Err(BuildError::new("can not stale read temporary table"));
        }
        Ok(())
    }

    /// 获取Cache表。
    pub fn getCacheTable(
        &self,
        table: &dyn TableReadSpec,
        snapshot_ts: u64,
    ) -> Result<Option<Box<dyn MemoryBuffer>>, BuildError> {
        let may_update_read_lock =
            !table.in_explain_statement() && !self.inDeleteStmt && !self.inUpdateStmt;
        self.dependencies
            .get_cache_table(table, snapshot_ts, may_update_read_lock)
    }

    /// 构建COMPACT TABLE执行器。
    pub fn buildCompactTable(&mut self, plan: &dyn CompactTablePlanData) -> Option<ExecutorBox> {
        if !matches!(
            plan.replica_kind(),
            CompactReplicaKind::TiFlash | CompactReplicaKind::All
        ) {
            self.err = Some(BuildError::new("compact replica kind is not supported"));
            return None;
        }
        if let Err(error) = self.dependencies.validate_compact_storage() {
            self.err = Some(error);
            return None;
        }
        let partition_ids = match self.dependencies.resolve_compact_partition_ids(plan) {
            Ok(partition_ids) => partition_ids,
            Err(error) => {
                self.err = Some(error);
                return None;
            }
        };
        if plan.partition_names().is_some() {
            self.Ti
                .partition
                .get_or_insert_with(PartitionTelemetryInfo::default)
                .use_compact_table_partition = true;
        }
        let result = self
            .dependencies
            .build_compact_table_executor(plan, partition_ids);
        self.finish(result)
    }

    /// 构建管理命令SHOWBDRRole执行器。
    pub fn buildAdminShowBDRRole(&mut self, plan: &dyn PlanData) -> Option<ExecutorBox> {
        self.build_leaf(ExecutorKind::AdminShowBdrRole, plan)
    }

    /// 构建Recommend索引执行器。
    pub fn buildRecommendIndex(&mut self, plan: &dyn PlanData) -> Option<ExecutorBox> {
        self.build_leaf(ExecutorKind::RecommendIndex, plan)
    }

    /// 构建WorkloadRepo创建执行器。
    pub fn buildWorkloadRepoCreate(&mut self, plan: &dyn PlanData) -> Option<ExecutorBox> {
        self.build_leaf(ExecutorKind::WorkloadRepoCreate, plan)
    }

    /// takeormakeerror。
    fn take_or_make_error(&mut self, operator: &str) -> BuildError {
        self.err
            .take()
            .unwrap_or_else(|| BuildError::new(format!("{operator} did not build an executor")))
    }
}

/// 构造行解码器。
pub fn NewRowDecoder(
    schema_columns: &[RowDecodeColumn],
    common_primary_key_column_ids: &[i64],
) -> RowDecoder {
    let mut primary_key_column_ids = schema_columns
        .iter()
        .filter(|column| column.primary_key || column.id == EXTRA_HANDLE_ID)
        .map(|column| column.id)
        .collect::<Vec<_>>();
    if primary_key_column_ids.is_empty() {
        primary_key_column_ids.extend_from_slice(common_primary_key_column_ids);
    }
    if primary_key_column_ids.is_empty() {
        primary_key_column_ids.push(-1);
    }
    RowDecoder {
        requested_columns: schema_columns.to_vec(),
        primary_key_column_ids,
    }
}

/// 构造副本读调节器。
pub fn newReplicaReadAdjuster(
    average_row_size: f64,
    threshold_bytes: i64,
    transaction_scope: &str,
) -> ReplicaReadAdjuster {
    ReplicaReadAdjuster {
        average_row_size,
        threshold_bytes,
        transaction_scope: transaction_scope.to_owned(),
    }
}

/// isCommon行句柄 handle读。
pub fn isCommonHandleRead(table_is_common_handle: bool, index_is_primary: bool) -> bool {
    table_is_common_handle && index_is_primary
}

/// 获取物理表ID。
pub fn getPhysicalTableID(physical_table_id: Option<i64>, logical_table_id: i64) -> i64 {
    physical_table_id.unwrap_or(logical_table_id)
}

impl DataReaderBuilder {
    /// partition裁剪。
    pub fn partitionPruning(
        &self,
        table: &dyn PartitionedTableData,
        pruning: &dyn PartitionPruningData,
    ) -> Result<Vec<i64>, BuildError> {
        self.partition_pruning_result
            .get_or_init(|| partitionPruning(table, pruning))
            .clone()
    }
}

/// partition裁剪。
pub fn partitionPruning(
    table: &dyn PartitionedTableData,
    pruning: &dyn PartitionPruningData,
) -> Result<Vec<i64>, BuildError> {
    let indexes = pruning.selected_indexes()?;
    if fullRangePartition(&indexes) {
        return Ok(table.partition_ids().to_vec());
    }
    indexes
        .into_iter()
        .map(|index| {
            let index = usize::try_from(index)
                .map_err(|_| BuildError::new("partition index cannot be negative"))?;
            table
                .partition_ids()
                .get(index)
                .copied()
                .ok_or_else(|| BuildError::new("partition index is out of bounds"))
        })
        .collect()
}

/// 获取分区IDsAfter裁剪。
pub fn getPartitionIDsAfterPruning(
    table: &dyn PartitionedTableData,
    pruning: Option<&dyn PartitionPruningData>,
) -> Result<BTreeSet<i64>, BuildError> {
    let pruning = pruning.ok_or_else(|| {
        BuildError::new("physPlanPartInfo in getPartitionIDsAfterPruning must not be nil")
    })?;
    Ok(partitionPruning(table, pruning)?.into_iter().collect())
}

/// full范围分区。
pub fn fullRangePartition(indexes: &[isize]) -> bool {
    indexes.len() == 1 && indexes[0] == -1
}

/// empty采样r。
pub struct emptySampler;

impl emptySampler {
    /// writeChunk。
    pub fn writeChunk(&mut self, _rows: &[Vec<u8>]) -> Result<(), BuildError> {
        Ok(())
    }

    /// finished。
    pub fn finished(&self) -> bool {
        true
    }
}

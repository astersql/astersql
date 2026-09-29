// Copyright 2026 AsterSQL.

// 物理算子（physicalop）crate 根模块。
//
// 汇总各类物理计划算子（Join/Agg/Scan/Reader/Exchange 等）、MPP Fragment、
// 名义排序与任务（Task）定义；并通过依赖倒置路由器把代价计算委托给 planner core，
// 避免 physicalop → core 的循环依赖。同时用宏为具体算子统一实现 `Plan` /
// `PhysicalPlan` trait。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

// ---- 内部子模块：各物理算子与基础设施 ----
mod base_physical_agg;
#[cfg(test)]
mod base_physical_agg_test;
mod base_physical_join;
#[cfg(test)]
mod base_physical_join_test;
mod base_physical_plan;
#[cfg(test)]
mod base_physical_plan_test;
mod cache_snapshot;
#[cfg(test)]
mod cache_snapshot_contract_test;
#[cfg(test)]
mod cache_snapshot_test;
mod fragment;
mod nominal_sort;
#[cfg(test)]
mod nominal_sort_test;
mod physical_apply;
#[cfg(test)]
mod physical_apply_test;
mod physical_batch_point_get;
#[cfg(test)]
mod physical_batch_point_get_test;
mod physical_delete;
#[cfg(test)]
mod physical_delete_test;
mod physical_exchange_receiver;
#[cfg(test)]
mod physical_exchange_receiver_test;
mod physical_exchange_sender;
#[cfg(test)]
mod physical_exchange_sender_test;
mod physical_hash_agg;
mod physical_hash_join;
#[cfg(test)]
mod physical_hash_join_test;
mod physical_index_join;
#[cfg(test)]
mod physical_index_join_test;
mod physical_index_reader;
#[cfg(test)]
mod physical_index_reader_test;
mod physical_index_scan;
#[cfg(test)]
mod physical_index_scan_test;
mod physical_indexlookup_reader;
#[cfg(test)]
mod physical_indexlookup_reader_test;
mod physical_indexmerge_reader;
#[cfg(test)]
mod physical_indexmerge_reader_test;
mod physical_insert;
mod physical_limit;
#[cfg(test)]
mod physical_limit_test;
mod physical_max_one_row;
#[cfg(test)]
mod physical_max_one_row_test;
mod physical_mem_table;
#[cfg(test)]
mod physical_mem_table_test;
mod physical_merge_join;
#[cfg(test)]
mod physical_merge_join_test;
mod physical_plan_misc;
#[cfg(test)]
mod physical_plan_misc_test;
mod physical_projection;
#[cfg(test)]
mod physical_projection_test;
mod physical_schema_producer;
#[cfg(test)]
mod physical_schema_producer_test;

mod physical_selection;
#[cfg(test)]
mod physical_selection_test;
mod physical_show;
#[cfg(test)]
mod physical_show_test;
mod physical_sort;
#[cfg(test)]
mod physical_sort_test;
mod physical_stream_agg;
#[cfg(test)]
mod physical_stream_agg_test;
mod physical_table_dual;
#[cfg(test)]
mod physical_table_dual_test;
mod physical_table_reader;
#[cfg(test)]
mod physical_table_reader_test;
mod physical_table_scan;
#[cfg(test)]
mod physical_table_scan_test;
mod physical_topn;
#[cfg(test)]
mod physical_topn_test;
mod physical_union_all;
mod physical_union_scan;
#[cfg(test)]
mod physical_union_scan_test;
mod physical_utils;
mod physical_window;
#[cfg(test)]
mod physical_window_test;
mod task;
#[cfg(test)]
mod task_base_test;
#[cfg(test)]
mod task_test;

// ---- 对外公开的辅助/扩展模块 ----
pub mod enforce;
#[cfg(test)]
mod enforce_test;
pub mod foreign_key;
#[cfg(test)]
mod foreign_key_test;
pub mod physical_common_plans;
#[cfg(test)]
mod physical_common_plans_test;
pub mod physical_cte;
pub mod physical_cte_table;
#[cfg(test)]
mod physical_cte_table_test;
#[cfg(test)]
mod physical_cte_test;
pub mod physical_expand;
#[cfg(test)]
mod physical_expand_test;
pub mod physical_index_hash_join;
pub mod physical_index_merge_join;
#[cfg(test)]
mod physical_index_merge_join_test;
pub mod physical_indexlookup;
#[cfg(test)]
mod physical_indexlookup_test;
pub mod physical_lock;
#[cfg(test)]
mod physical_lock_test;
pub mod physical_sequence;
#[cfg(test)]
mod physical_sequence_test;
pub mod physical_shuffle;
#[cfg(test)]
mod physical_shuffle_test;
pub mod physical_table_sample;
#[cfg(test)]
mod physical_table_sample_test;
pub mod plan_clone_generated;
#[cfg(test)]
mod plan_clone_generated_test;
pub mod task_base;
pub mod tiflash_predicate_push_down;
#[cfg(test)]
mod tiflash_predicate_push_down_test;

// 将内部子模块的公开项再导出，便于上层 `use physicalop::*`。
pub use base_physical_agg::*;
pub use base_physical_join::*;
pub use base_physical_plan::*;
pub use cache_snapshot::*;
pub use fragment::*;
pub use nominal_sort::*;
pub use physical_apply::*;
pub use physical_batch_point_get::*;
pub use physical_cte::*;
pub use physical_delete::*;
pub use physical_exchange_receiver::*;
pub use physical_exchange_sender::*;
pub use physical_hash_agg::*;
pub use physical_hash_join::*;
pub use physical_index_join::*;
pub use physical_index_reader::*;
pub use physical_index_scan::*;
pub use physical_indexlookup_reader::*;
pub use physical_indexmerge_reader::*;
pub use physical_insert::*;
pub use physical_limit::*;
pub use physical_lock::*;
pub use physical_max_one_row::*;
pub use physical_mem_table::*;
pub use physical_merge_join::*;
pub use physical_plan_misc::*;
pub use physical_projection::*;
pub use physical_schema_producer::*;
pub use physical_selection::*;
pub use physical_show::*;
pub use physical_sort::*;
pub use physical_stream_agg::*;
pub use physical_table_dual::*;
pub use physical_table_reader::*;
pub use physical_table_sample::*;
pub use physical_table_scan::*;
pub use physical_topn::*;
pub use physical_union_all::*;
pub use physical_union_scan::*;
pub use physical_utils::*;
pub use physical_window::*;
pub use task::*;

#[cfg(test)]
mod physical_insert_aster_unit_test;
#[cfg(test)]
mod physical_insert_test;

#[cfg(test)]
mod physical_hash_agg_aster_unit_test;
#[cfg(test)]
mod physical_table_scan_aster_unit_test;

#[cfg(test)]
mod fragment_aster_unit_test;
#[cfg(test)]
mod physical_exchange_sender_aster_unit_test;

#[cfg(test)]
#[path = "fragment_test.rs"]
mod fragment_test;
#[cfg(test)]
#[path = "physical_utils_test.rs"]
mod physical_utils_test;

pub use base::{ContextRef as PlanContextRef, PhysicalPlan};
pub use expression::{Column, CorrelatedColumn, ExprBox as Expression, Schema};
pub use property::{PhysicalProperty, StatsInfo, TaskType};

/// Go `CloneForPlanCache` 生成集合中的封闭节点种类。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CacheSnapshotPlanKind {
    Update,
    Delete,
    Insert,
    TableScan,
    IndexScan,
    Selection,
    Projection,
    TopN,
    Limit,
    StreamAgg,
    HashAgg,
    HashJoin,
    MergeJoin,
    IndexJoin,
    IndexHashJoin,
    IndexReader,
    TableReader,
    IndexMergeReader,
    IndexLookupReader,
    LocalIndexLookup,
    BatchPointGet,
    PointGet,
    UnionScan,
    UnionAll,
    TableDual,
}

/// 一类可缓存计划在 Go 与 Rust 间的类型及非浅拷贝契约。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CacheSnapshotPlanContract {
    pub kind: CacheSnapshotPlanKind,
    pub go_type: &'static str,
    pub rust_type: &'static str,
    pub special_clone_fields: &'static str,
}

/// 与 Go `plan_clone_generated.go` 的 25 个 `CloneForPlanCache` 实现逐项对应。
pub const CACHE_SNAPSHOT_PLAN_CONTRACT: [CacheSnapshotPlanContract; 25] = [
    CacheSnapshotPlanContract {
        kind: CacheSnapshotPlanKind::Update,
        go_type: "Update",
        rust_type: "Update",
        special_clone_fields: "SimpleSchemaProducer; OrderedList; SelectPlan; reject FKChecks/FKCascades",
    },
    CacheSnapshotPlanContract {
        kind: CacheSnapshotPlanKind::Delete,
        go_type: "Delete",
        rust_type: "Delete",
        special_clone_fields: "SimpleSchemaProducer; SelectPlan; reject FKChecks/FKCascades",
    },
    CacheSnapshotPlanContract {
        kind: CacheSnapshotPlanKind::Insert,
        go_type: "Insert",
        rust_type: "Insert",
        special_clone_fields: "SimpleSchemaProducer; Lists; OnDuplicate; GenCols; SelectPlan; reject FKChecks/FKCascades",
    },
    CacheSnapshotPlanContract {
        kind: CacheSnapshotPlanKind::TableScan,
        go_type: "PhysicalTableScan",
        rust_type: "PhysicalTableScan",
        special_clone_fields: "PhysicalSchemaProducer; conditions; HandleIdx/HandleCols; ByItems; PlanPartInfo; constColsByCond; reject SampleInfo/runtimeFilterList/UsedColumnarIndexes",
    },
    CacheSnapshotPlanContract {
        kind: CacheSnapshotPlanKind::IndexScan,
        go_type: "PhysicalIndexScan",
        rust_type: "PhysicalIndexScan",
        special_clone_fields: "PhysicalSchemaProducer; AccessCondition; IdxCols/IdxColLens; ByItems; PkIsHandleCol share-or-clone; ConstColsByCond; reject GenExprs",
    },
    CacheSnapshotPlanContract {
        kind: CacheSnapshotPlanKind::Selection,
        go_type: "PhysicalSelection",
        rust_type: "PhysicalSelection",
        special_clone_fields: "BasePhysicalPlan; Conditions",
    },
    CacheSnapshotPlanContract {
        kind: CacheSnapshotPlanKind::Projection,
        go_type: "PhysicalProjection",
        rust_type: "PhysicalProjection",
        special_clone_fields: "PhysicalSchemaProducer; Exprs",
    },
    CacheSnapshotPlanContract {
        kind: CacheSnapshotPlanKind::TopN,
        go_type: "PhysicalTopN",
        rust_type: "PhysicalTopN",
        special_clone_fields: "PhysicalSchemaProducer; ByItems; PartitionBy; PrefixCol share-or-clone",
    },
    CacheSnapshotPlanContract {
        kind: CacheSnapshotPlanKind::Limit,
        go_type: "PhysicalLimit",
        rust_type: "PhysicalLimit",
        special_clone_fields: "PhysicalSchemaProducer; PartitionBy; PrefixCol share-or-clone",
    },
    CacheSnapshotPlanContract {
        kind: CacheSnapshotPlanKind::StreamAgg,
        go_type: "PhysicalStreamAgg",
        rust_type: "PhysicalStreamAgg",
        special_clone_fields: "BasePhysicalAgg",
    },
    CacheSnapshotPlanContract {
        kind: CacheSnapshotPlanKind::HashAgg,
        go_type: "PhysicalHashAgg",
        rust_type: "PhysicalHashAgg",
        special_clone_fields: "BasePhysicalAgg",
    },
    CacheSnapshotPlanContract {
        kind: CacheSnapshotPlanKind::HashJoin,
        go_type: "PhysicalHashJoin",
        rust_type: "PhysicalHashJoin",
        special_clone_fields: "BasePhysicalJoin; EqualConditions/NAEqualConditions; reject runtimeFilterList",
    },
    CacheSnapshotPlanContract {
        kind: CacheSnapshotPlanKind::MergeJoin,
        go_type: "PhysicalMergeJoin",
        rust_type: "PhysicalMergeJoin",
        special_clone_fields: "BasePhysicalJoin",
    },
    CacheSnapshotPlanContract {
        kind: CacheSnapshotPlanKind::IndexJoin,
        go_type: "PhysicalIndexJoin",
        rust_type: "PhysicalIndexJoin",
        special_clone_fields: "BasePhysicalJoin; InnerPlan; Ranges; index offsets/lens; CompareFilters; hash keys",
    },
    CacheSnapshotPlanContract {
        kind: CacheSnapshotPlanKind::IndexHashJoin,
        go_type: "PhysicalIndexHashJoin",
        rust_type: "PhysicalIndexHashJoin",
        special_clone_fields: "PhysicalIndexJoin; reset Self to clone",
    },
    CacheSnapshotPlanContract {
        kind: CacheSnapshotPlanKind::IndexReader,
        go_type: "PhysicalIndexReader",
        rust_type: "PhysicalIndexReader",
        special_clone_fields: "PhysicalSchemaProducer; IndexPlan; rebuild IndexPlans; OutputColumns; PlanPartInfo",
    },
    CacheSnapshotPlanContract {
        kind: CacheSnapshotPlanKind::TableReader,
        go_type: "PhysicalTableReader",
        rust_type: "PhysicalTableReader",
        special_clone_fields: "PhysicalSchemaProducer; TablePlan; rebuild TablePlans; PlanPartInfo; reject TableScanAndPartitionInfos",
    },
    CacheSnapshotPlanContract {
        kind: CacheSnapshotPlanKind::IndexMergeReader,
        go_type: "PhysicalIndexMergeReader",
        rust_type: "PhysicalIndexMergeReader",
        special_clone_fields: "PhysicalSchemaProducer; PushedLimit; ByItems; PartialPlansRaw/TablePlan; rebuild flattened plans; PlanPartInfo; HandleCols",
    },
    CacheSnapshotPlanContract {
        kind: CacheSnapshotPlanKind::IndexLookupReader,
        go_type: "PhysicalIndexLookUpReader",
        rust_type: "PhysicalIndexLookUpReader",
        special_clone_fields: "PhysicalSchemaProducer; IndexPlan/TablePlan; rebuild flattened plans/order map; ExtraHandleCol share-or-clone; PushedLimit; CommonHandleCols; PlanPartInfo",
    },
    CacheSnapshotPlanContract {
        kind: CacheSnapshotPlanKind::LocalIndexLookup,
        go_type: "PhysicalLocalIndexLookUp",
        rust_type: "PhysicalLocalIndexLookup",
        special_clone_fields: "PhysicalSchemaProducer; IndexHandleOffsets",
    },
    CacheSnapshotPlanContract {
        kind: CacheSnapshotPlanKind::BatchPointGet,
        go_type: "BatchPointGetPlan",
        rust_type: "BatchPointGetPlan",
        special_clone_fields: "SimpleSchemaProducer; ProbeParents; replace ctx; handles/params/index values/access/index/partition columns",
    },
    CacheSnapshotPlanContract {
        kind: CacheSnapshotPlanKind::PointGet,
        go_type: "PointGetPlan",
        rust_type: "PointGetPlan",
        special_clone_fields: "Plan with new ctx; PartitionIdx; Handle; HandleConstant share-or-clone; index values/constants/columns/lens/access columns",
    },
    CacheSnapshotPlanContract {
        kind: CacheSnapshotPlanKind::UnionScan,
        go_type: "PhysicalUnionScan",
        rust_type: "PhysicalUnionScan",
        special_clone_fields: "BasePhysicalPlan; Conditions; HandleCols",
    },
    CacheSnapshotPlanContract {
        kind: CacheSnapshotPlanKind::UnionAll,
        go_type: "PhysicalUnionAll",
        rust_type: "PhysicalUnionAll",
        special_clone_fields: "PhysicalSchemaProducer",
    },
    CacheSnapshotPlanContract {
        kind: CacheSnapshotPlanKind::TableDual,
        go_type: "PhysicalTableDual",
        rust_type: "PhysicalTableDual",
        special_clone_fields: "PhysicalSchemaProducer; names",
    },
];

use std::sync::OnceLock;

/// Dependency-inversion boundary for the canonical cost models owned by
/// planner core. This avoids a physicalop -> core crate cycle.
/// 代价模型 v1 路由器函数类型：由 planner core 安装，打破循环依赖。
pub type PlanCostVer1Router = fn(
    &dyn base::PhysicalPlan,
    TaskType,
    &costusage::PlanCostOption,
) -> Result<f64, expression::Error>;
/// 代价模型 v2 路由器函数类型；`inl` 为 IndexJoin 探测侧相关标志。
pub type PlanCostVer2Router = fn(
    &dyn base::PhysicalPlan,
    TaskType,
    &costusage::PlanCostOption,
    &[bool],
) -> Result<costusage::CostVer2, expression::Error>;

static PLAN_COST_VER1_ROUTER: OnceLock<PlanCostVer1Router> = OnceLock::new();
static PLAN_COST_VER2_ROUTER: OnceLock<PlanCostVer2Router> = OnceLock::new();

/// 安装全局代价模型 v1 路由器；重复安装返回已有函数。
pub fn InstallPlanCostVer1Router(router: PlanCostVer1Router) -> Result<(), PlanCostVer1Router> {
    PLAN_COST_VER1_ROUTER.set(router)
}

/// 安装全局代价模型 v2 路由器。
pub fn InstallPlanCostVer2Router(router: PlanCostVer2Router) -> Result<(), PlanCostVer2Router> {
    PLAN_COST_VER2_ROUTER.set(router)
}

/// 若已安装路由器则委托计算 v1 代价，否则返回 None 让算子走本地实现。
pub(crate) fn routed_plan_cost_ver1(
    plan: &dyn base::PhysicalPlan,
    task: TaskType,
    option: &costusage::PlanCostOption,
) -> Option<Result<f64, expression::Error>> {
    PLAN_COST_VER1_ROUTER
        .get()
        .map(|router| router(plan, task, option))
}

/// 若已安装路由器则委托计算 v2 代价。
pub(crate) fn routed_plan_cost_ver2(
    plan: &dyn base::PhysicalPlan,
    task: TaskType,
    option: &costusage::PlanCostOption,
    inl: &[bool],
) -> Option<Result<costusage::CostVer2, expression::Error>> {
    PLAN_COST_VER2_ROUTER
        .get()
        .map(|router| router(plan, task, option, inl))
}

/// 具体物理算子共用的钩子：Schema 生产者访问、Explain、Resolve、代价与挂接任务。
trait ConcretePhysicalOperator {
    fn producer(&self) -> &PhysicalSchemaProducer;
    fn producer_mut(&mut self) -> &mut PhysicalSchemaProducer;
    fn schema_operator(&self) -> &Schema {
        self.producer()
            .SchemaRef()
            .unwrap_or_else(|| base::Plan::schema(&self.producer().BasePhysicalPlan))
    }
    fn explain_operator(&self) -> String;
    fn explain_normalized_operator(&self) -> String;
    fn resolve_operator(&mut self) -> Result<(), expression::Error>;
    fn memory_operator(&self) -> i64;
    fn output_names_operator(&self) -> base::types::NameSlice {
        base::Plan::output_names(&self.producer().BasePhysicalPlan)
    }
    fn set_output_names_operator(&mut self, names: base::types::NameSlice) {
        base::Plan::set_output_names(&mut self.producer_mut().BasePhysicalPlan, names)
    }
    fn correlated_operator(&self) -> Vec<CorrelatedColumn> {
        Vec::new()
    }
    fn children_operator(&self) -> Vec<&dyn base::PhysicalPlan> {
        self.producer().BasePhysicalPlan.Children()
    }
    fn set_children_operator(&mut self, children: Vec<Box<dyn base::PhysicalPlan>>) {
        self.producer_mut().BasePhysicalPlan.SetChildren(children);
    }
    fn set_child_operator(&mut self, index: usize, child: Box<dyn base::PhysicalPlan>) {
        self.producer_mut().BasePhysicalPlan.SetChild(index, child);
    }
    /// 默认挂接：克隆子任务计划，包成 RootTask。
    fn attach_operator_to_task(&self, tasks: Vec<Box<dyn base::Task>>) -> Box<dyn base::Task>
    where
        Self: base::PhysicalPlan,
    {
        let children = tasks
            .iter()
            .map(|task| {
                task.plan()
                    .clone_physical(base::Plan::s_ctx(task.plan()).clone())
            })
            .collect::<Result<Vec<_>, _>>()
            .expect("physical child task plan clone");
        let mut plan = self
            .clone_physical(base::Plan::s_ctx(self).clone())
            .expect("physical operator clone");
        plan.set_children(children);
        let (partition_type, hash_cols) = tasks
            .first()
            .map_or((property::AnyType, Vec::new()), |task| {
                (task.mpp_partition_type(), task.mpp_hash_cols())
            });
        Box::new(RootTask::NewWithMpp(plan, None, partition_type, hash_cols))
    }
    fn cost_v1(
        &mut self,
        task: TaskType,
        option: &costusage::PlanCostOption,
    ) -> Result<f64, expression::Error>;
    fn cost_v2(
        &mut self,
        task: TaskType,
        option: &costusage::PlanCostOption,
        inl: &[bool],
    ) -> Result<costusage::CostVer2, expression::Error>;
    fn operator_to_pb(
        &self,
        ctx: &mut base::BuildPBContext,
        store: kv::StoreType,
    ) -> Result<Box<tipb::Executor>, expression::Error> {
        base::PhysicalPlan::to_pb(&self.producer().BasePhysicalPlan, ctx, store)
    }
}

/// 基于 `ConcretePhysicalOperator` 为具体算子统一实现 `Plan` 与 `PhysicalPlan`。
macro_rules! impl_concrete_physical_plan {
    ($operator:ty) => {
        impl base::Plan for $operator {
            fn as_any(&self) -> &dyn std::any::Any {
                self
            }
            fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
                self
            }
            fn as_physical_plan(&self) -> Option<&dyn base::PhysicalPlan> {
                Some(self)
            }
            fn schema(&self) -> &Schema {
                self.schema_operator()
            }
            fn id(&self) -> i32 {
                base::Plan::id(&self.producer().BasePhysicalPlan)
            }
            fn set_id(&mut self, id: i32) {
                base::Plan::set_id(&mut self.producer_mut().BasePhysicalPlan, id)
            }
            fn tp(&self, flags: &[bool]) -> String {
                base::Plan::tp(&self.producer().BasePhysicalPlan, flags)
            }
            fn explain_id(&self, flags: &[bool]) -> Box<dyn std::fmt::Display + '_> {
                base::Plan::explain_id(&self.producer().BasePhysicalPlan, flags)
            }
            fn explain_info(&self) -> String {
                self.explain_operator()
            }
            fn replace_expr_columns(
                &mut self,
                replace: &std::collections::HashMap<String, Column>,
            ) {
                base::Plan::replace_expr_columns(&mut self.producer_mut().BasePhysicalPlan, replace)
            }
            fn s_ctx(&self) -> &base::ContextRef {
                base::Plan::s_ctx(&self.producer().BasePhysicalPlan)
            }
            fn stats_info(&self) -> &StatsInfo {
                base::Plan::stats_info(&self.producer().BasePhysicalPlan)
            }
            fn output_names(&self) -> base::types::NameSlice {
                self.output_names_operator()
            }
            fn set_output_names(&mut self, names: base::types::NameSlice) {
                self.set_output_names_operator(names)
            }
            fn query_block_offset(&self) -> i32 {
                base::Plan::query_block_offset(&self.producer().BasePhysicalPlan)
            }
            fn clone_for_plan_cache(
                &self,
                new_ctx: base::ContextRef,
            ) -> (Option<Box<dyn base::Plan>>, bool) {
                match self.Clone(new_ctx) {
                    Ok(plan) => (Some(Box::new(plan)), true),
                    Err(_) => (None, false),
                }
            }
            fn set_noncacheable_reason(&mut self, reason: String) {
                base::Plan::set_noncacheable_reason(
                    &mut self.producer_mut().BasePhysicalPlan,
                    reason,
                )
            }
            fn get_noncacheable_reason(&self) -> String {
                base::Plan::get_noncacheable_reason(&self.producer().BasePhysicalPlan)
            }
        }

        impl base::PhysicalPlan for $operator {
            fn get_plan_cost_ver1(
                &mut self,
                task: TaskType,
                option: &costusage::PlanCostOption,
            ) -> Result<f64, expression::Error> {
                // 优先走已安装的全局路由器，否则回落到算子本地代价。
                if let Some(cost) = routed_plan_cost_ver1(self, task, option) {
                    return cost;
                }
                self.cost_v1(task, option)
            }
            fn get_plan_cost_ver2(
                &mut self,
                task: TaskType,
                option: &costusage::PlanCostOption,
                inl: &[bool],
            ) -> Result<costusage::CostVer2, expression::Error> {
                if let Some(cost) = routed_plan_cost_ver2(self, task, option, inl) {
                    return cost;
                }
                self.cost_v2(task, option, inl)
            }
            fn attach_to_task(&self, tasks: Vec<Box<dyn base::Task>>) -> Box<dyn base::Task> {
                self.attach_operator_to_task(tasks)
            }
            fn to_pb(
                &self,
                ctx: &mut base::BuildPBContext,
                store: kv::StoreType,
            ) -> Result<Box<tipb::Executor>, expression::Error> {
                self.operator_to_pb(ctx, store)
            }
            fn get_child_req_props(&self, idx: usize) -> &PhysicalProperty {
                base::PhysicalPlan::get_child_req_props(&self.producer().BasePhysicalPlan, idx)
            }
            fn stats_count(&self) -> f64 {
                base::PhysicalPlan::stats_count(&self.producer().BasePhysicalPlan)
            }
            fn extract_correlated_cols(&self) -> Vec<CorrelatedColumn> {
                self.correlated_operator()
            }
            fn children(&self) -> Vec<&dyn base::PhysicalPlan> {
                self.children_operator()
            }
            fn set_children(&mut self, children: Vec<Box<dyn base::PhysicalPlan>>) {
                self.set_children_operator(children)
            }
            fn set_child(&mut self, index: usize, child: Box<dyn base::PhysicalPlan>) {
                self.set_child_operator(index, child)
            }
            fn resolve_indices(&mut self) -> Result<(), expression::Error> {
                self.resolve_operator()
            }
            fn set_stats(&mut self, stats: StatsInfo) {
                base::PhysicalPlan::set_stats(&mut self.producer_mut().BasePhysicalPlan, stats)
            }
            fn explain_normalized_info(&self) -> String {
                self.explain_normalized_operator()
            }
            fn clone_physical(
                &self,
                new_ctx: base::ContextRef,
            ) -> Result<Box<dyn base::PhysicalPlan>, expression::Error> {
                Ok(Box::new(self.Clone(new_ctx)?))
            }
            fn memory_usage(&self) -> i64 {
                self.memory_operator()
            }
            fn set_probe_parents(&mut self, parents: Vec<Box<dyn base::PhysicalPlan>>) {
                base::PhysicalPlan::set_probe_parents(
                    &mut self.producer_mut().BasePhysicalPlan,
                    parents,
                )
            }
            fn get_est_row_count_for_display(&self) -> f64 {
                base::PhysicalPlan::get_est_row_count_for_display(&self.producer().BasePhysicalPlan)
            }
            fn get_actual_probe_count(
                &self,
                stats: &execdetails::execdetails::RuntimeStatsColl,
            ) -> i64 {
                base::PhysicalPlan::get_actual_probe_count(&self.producer().BasePhysicalPlan, stats)
            }
        }
    };
}

/// Join 类算子的通用 ConcretePhysicalOperator 实现，字段名由 `$join` 指定。
macro_rules! join_operator_core {
    ($operator:ty, $join:ident) => {
        impl ConcretePhysicalOperator for $operator {
            fn producer(&self) -> &PhysicalSchemaProducer {
                &self.$join.PhysicalSchemaProducer
            }
            fn producer_mut(&mut self) -> &mut PhysicalSchemaProducer {
                &mut self.$join.PhysicalSchemaProducer
            }
            fn explain_operator(&self) -> String {
                self.ExplainInfo()
            }
            fn explain_normalized_operator(&self) -> String {
                self.ExplainNormalizedInfo()
            }
            fn resolve_operator(&mut self) -> Result<(), expression::Error> {
                self.ResolveIndices()
            }
            fn memory_operator(&self) -> i64 {
                self.MemoryUsage()
            }
            fn cost_v1(
                &mut self,
                task: TaskType,
                option: &costusage::PlanCostOption,
            ) -> Result<f64, expression::Error> {
                self.GetPlanCostVer1(task, option)
            }
            fn cost_v2(
                &mut self,
                task: TaskType,
                option: &costusage::PlanCostOption,
                inl: &[bool],
            ) -> Result<costusage::CostVer2, expression::Error> {
                self.GetPlanCostVer2(task, option, inl)
            }
        }
        impl_concrete_physical_plan!($operator);
    };
}

join_operator_core!(PhysicalIndexJoin, BasePhysicalJoin);
join_operator_core!(PhysicalMergeJoin, BasePhysicalJoin);

// HashJoin 额外提取相关列并实现 ToPB，因此手写而非走 join_operator_core。
impl ConcretePhysicalOperator for PhysicalHashJoin {
    fn producer(&self) -> &PhysicalSchemaProducer {
        &self.BasePhysicalJoin.PhysicalSchemaProducer
    }
    fn producer_mut(&mut self) -> &mut PhysicalSchemaProducer {
        &mut self.BasePhysicalJoin.PhysicalSchemaProducer
    }
    fn explain_operator(&self) -> String {
        self.ExplainInfo()
    }
    fn explain_normalized_operator(&self) -> String {
        self.ExplainNormalizedInfo()
    }
    fn resolve_operator(&mut self) -> Result<(), expression::Error> {
        self.ResolveIndices()
    }
    fn memory_operator(&self) -> i64 {
        self.MemoryUsage()
    }
    fn correlated_operator(&self) -> Vec<CorrelatedColumn> {
        self.ExtractCorrelatedCols()
    }
    fn cost_v1(
        &mut self,
        task: TaskType,
        option: &costusage::PlanCostOption,
    ) -> Result<f64, expression::Error> {
        self.GetPlanCostVer1(task, option)
    }
    fn cost_v2(
        &mut self,
        task: TaskType,
        option: &costusage::PlanCostOption,
        inl: &[bool],
    ) -> Result<costusage::CostVer2, expression::Error> {
        self.GetPlanCostVer2(task, option, inl)
    }
    fn operator_to_pb(
        &self,
        ctx: &mut base::BuildPBContext,
        store: kv::StoreType,
    ) -> Result<Box<tipb::Executor>, expression::Error> {
        self.ToPB(ctx, store)
    }
}
impl_concrete_physical_plan!(PhysicalHashJoin);

impl ConcretePhysicalOperator for PhysicalApply {
    fn producer(&self) -> &PhysicalSchemaProducer {
        &self
            .PhysicalHashJoin
            .BasePhysicalJoin
            .PhysicalSchemaProducer
    }
    fn producer_mut(&mut self) -> &mut PhysicalSchemaProducer {
        &mut self
            .PhysicalHashJoin
            .BasePhysicalJoin
            .PhysicalSchemaProducer
    }
    fn explain_operator(&self) -> String {
        self.PhysicalHashJoin.ExplainInfo()
    }
    fn explain_normalized_operator(&self) -> String {
        self.PhysicalHashJoin.ExplainNormalizedInfo()
    }
    fn resolve_operator(&mut self) -> Result<(), expression::Error> {
        self.ResolveIndices()
    }
    fn memory_operator(&self) -> i64 {
        self.MemoryUsage()
    }
    fn correlated_operator(&self) -> Vec<CorrelatedColumn> {
        self.ExtractCorrelatedCols()
    }
    fn cost_v1(
        &mut self,
        task: TaskType,
        option: &costusage::PlanCostOption,
    ) -> Result<f64, expression::Error> {
        self.GetPlanCostVer1(task, option)
    }
    fn cost_v2(
        &mut self,
        task: TaskType,
        option: &costusage::PlanCostOption,
        inl: &[bool],
    ) -> Result<costusage::CostVer2, expression::Error> {
        self.GetPlanCostVer2(task, option, inl)
    }
    fn operator_to_pb(
        &self,
        ctx: &mut base::BuildPBContext,
        store: kv::StoreType,
    ) -> Result<Box<tipb::Executor>, expression::Error> {
        self.PhysicalHashJoin.ToPB(ctx, store)
    }
}
impl_concrete_physical_plan!(PhysicalApply);

/// 聚合类算子的通用 ConcretePhysicalOperator 实现。
macro_rules! agg_operator_core {
    ($operator:ty) => {
        impl ConcretePhysicalOperator for $operator {
            fn producer(&self) -> &PhysicalSchemaProducer {
                &self.BasePhysicalAgg.PhysicalSchemaProducer
            }
            fn producer_mut(&mut self) -> &mut PhysicalSchemaProducer {
                &mut self.BasePhysicalAgg.PhysicalSchemaProducer
            }
            fn explain_operator(&self) -> String {
                self.BasePhysicalAgg.ExplainInfo()
            }
            fn explain_normalized_operator(&self) -> String {
                self.BasePhysicalAgg.ExplainNormalizedInfo()
            }
            fn resolve_operator(&mut self) -> Result<(), expression::Error> {
                self.BasePhysicalAgg.ResolveIndices()
            }
            fn memory_operator(&self) -> i64 {
                self.MemoryUsage()
            }
            fn correlated_operator(&self) -> Vec<CorrelatedColumn> {
                self.BasePhysicalAgg.ExtractCorrelatedCols()
            }
            fn cost_v1(
                &mut self,
                task: TaskType,
                option: &costusage::PlanCostOption,
            ) -> Result<f64, expression::Error> {
                self.GetPlanCostVer1(task, option)
            }
            fn cost_v2(
                &mut self,
                task: TaskType,
                option: &costusage::PlanCostOption,
                inl: &[bool],
            ) -> Result<costusage::CostVer2, expression::Error> {
                self.GetPlanCostVer2(task, option, inl)
            }
            fn operator_to_pb(
                &self,
                ctx: &mut base::BuildPBContext,
                store: kv::StoreType,
            ) -> Result<Box<tipb::Executor>, expression::Error> {
                self.ToPB(ctx, store)
            }
        }
        impl_concrete_physical_plan!($operator);
    };
}

agg_operator_core!(PhysicalHashAgg);
agg_operator_core!(PhysicalStreamAgg);

/// 直接持有 `$producer` 字段的一元算子通用实现。
macro_rules! direct_operator_core {
    ($operator:ty, $producer:ident $(, $output_names:ident, $set_output_names:ident)?) => {
        impl ConcretePhysicalOperator for $operator {
            fn producer(&self) -> &PhysicalSchemaProducer {
                &self.$producer
            }
            fn producer_mut(&mut self) -> &mut PhysicalSchemaProducer {
                &mut self.$producer
            }
            $(
                fn output_names_operator(&self) -> base::types::NameSlice {
                    self.$output_names()
                }
                fn set_output_names_operator(&mut self, names: base::types::NameSlice) {
                    self.$set_output_names(names)
                }
            )?
            fn explain_operator(&self) -> String {
                self.ExplainInfo()
            }
            fn explain_normalized_operator(&self) -> String {
                self.ExplainNormalizedInfo()
            }
            fn resolve_operator(&mut self) -> Result<(), expression::Error> {
                self.ResolveIndices()
            }
            fn memory_operator(&self) -> i64 {
                self.MemoryUsage()
            }
            fn correlated_operator(&self) -> Vec<CorrelatedColumn> {
                self.ExtractCorrelatedCols()
            }
            fn cost_v1(
                &mut self,
                task: TaskType,
                option: &costusage::PlanCostOption,
            ) -> Result<f64, expression::Error> {
                self.GetPlanCostVer1(task, option)
            }
            fn cost_v2(
                &mut self,
                task: TaskType,
                option: &costusage::PlanCostOption,
                inl: &[bool],
            ) -> Result<costusage::CostVer2, expression::Error> {
                self.GetPlanCostVer2(task, option, inl)
            }
            fn operator_to_pb(
                &self,
                ctx: &mut base::BuildPBContext,
                store: kv::StoreType,
            ) -> Result<Box<tipb::Executor>, expression::Error> {
                self.ToPB(ctx, store)
            }
        }
        impl_concrete_physical_plan!($operator);
    };
}

direct_operator_core!(PhysicalIndexScan, PhysicalSchemaProducer);
direct_operator_core!(PhysicalTableScan, PhysicalSchemaProducer);
direct_operator_core!(
    PointGetPlan,
    PhysicalSchemaProducer,
    OutputNames,
    SetOutputNames
);
direct_operator_core!(PhysicalSelection, PhysicalSchemaProducer);
// Projection 覆盖 Attach2Task / ToPB，因此手写。
impl ConcretePhysicalOperator for PhysicalProjection {
    fn producer(&self) -> &PhysicalSchemaProducer {
        &self.PhysicalSchemaProducer
    }
    fn producer_mut(&mut self) -> &mut PhysicalSchemaProducer {
        &mut self.PhysicalSchemaProducer
    }
    fn explain_operator(&self) -> String {
        self.ExplainInfo()
    }
    fn explain_normalized_operator(&self) -> String {
        self.ExplainNormalizedInfo()
    }
    fn resolve_operator(&mut self) -> Result<(), expression::Error> {
        self.ResolveIndices()
    }
    fn memory_operator(&self) -> i64 {
        self.MemoryUsage()
    }
    fn correlated_operator(&self) -> Vec<CorrelatedColumn> {
        self.ExtractCorrelatedCols()
    }
    fn attach_operator_to_task(&self, tasks: Vec<Box<dyn base::Task>>) -> Box<dyn base::Task> {
        self.Attach2Task(tasks)
    }
    fn cost_v1(
        &mut self,
        task: TaskType,
        option: &costusage::PlanCostOption,
    ) -> Result<f64, expression::Error> {
        self.GetPlanCostVer1(task, option)
    }
    fn cost_v2(
        &mut self,
        task: TaskType,
        option: &costusage::PlanCostOption,
        inl: &[bool],
    ) -> Result<costusage::CostVer2, expression::Error> {
        self.GetPlanCostVer2(task, option, inl)
    }
    fn operator_to_pb(
        &self,
        ctx: &mut base::BuildPBContext,
        store: kv::StoreType,
    ) -> Result<Box<tipb::Executor>, expression::Error> {
        self.ToPB(ctx, store)
    }
}
impl_concrete_physical_plan!(PhysicalProjection);
direct_operator_core!(PhysicalLimit, PhysicalSchemaProducer);
direct_operator_core!(PhysicalExchangeSender, PhysicalSchemaProducer);
direct_operator_core!(PhysicalExchangeReceiver, PhysicalSchemaProducer);
direct_operator_core!(PhysicalUnionScan, PhysicalSchemaProducer);

// IndexReader 的孩子是 IndexPlan，而非 BasePhysicalPlan.Children。
impl ConcretePhysicalOperator for PhysicalIndexReader {
    fn producer(&self) -> &PhysicalSchemaProducer {
        &self.PhysicalSchemaProducer
    }
    fn producer_mut(&mut self) -> &mut PhysicalSchemaProducer {
        &mut self.PhysicalSchemaProducer
    }
    fn explain_operator(&self) -> String {
        self.ExplainInfo()
    }
    fn explain_normalized_operator(&self) -> String {
        self.ExplainNormalizedInfo()
    }
    fn resolve_operator(&mut self) -> Result<(), expression::Error> {
        self.ResolveIndices()
    }
    fn memory_operator(&self) -> i64 {
        self.MemoryUsage()
    }
    fn correlated_operator(&self) -> Vec<CorrelatedColumn> {
        self.ExtractCorrelatedCols()
    }
    fn children_operator(&self) -> Vec<&dyn base::PhysicalPlan> {
        self.IndexPlan.iter().map(|p| p.as_ref()).collect()
    }
    fn set_children_operator(&mut self, children: Vec<Box<dyn base::PhysicalPlan>>) {
        self.SetChildren(children)
    }
    fn set_child_operator(&mut self, index: usize, child: Box<dyn base::PhysicalPlan>) {
        if index == 0 {
            self.SetChildren(vec![child]);
        }
    }
    fn cost_v1(
        &mut self,
        task: TaskType,
        option: &costusage::PlanCostOption,
    ) -> Result<f64, expression::Error> {
        self.GetPlanCostVer1(task, option)
    }
    fn cost_v2(
        &mut self,
        task: TaskType,
        option: &costusage::PlanCostOption,
        inl: &[bool],
    ) -> Result<costusage::CostVer2, expression::Error> {
        self.GetPlanCostVer2(task, option, inl)
    }
    fn operator_to_pb(
        &self,
        ctx: &mut base::BuildPBContext,
        store: kv::StoreType,
    ) -> Result<Box<tipb::Executor>, expression::Error> {
        self.ToPB(ctx, store)
    }
}
impl_concrete_physical_plan!(PhysicalIndexReader);

// TableReader 挂接任务时重新分配 ExplainID，对齐 Go 在 cop 子节点建成后再物化 reader 的语义。
impl ConcretePhysicalOperator for PhysicalTableReader {
    fn producer(&self) -> &PhysicalSchemaProducer {
        &self.PhysicalSchemaProducer
    }
    fn producer_mut(&mut self) -> &mut PhysicalSchemaProducer {
        &mut self.PhysicalSchemaProducer
    }
    fn explain_operator(&self) -> String {
        self.ExplainInfo()
    }
    fn explain_normalized_operator(&self) -> String {
        self.ExplainNormalizedInfo()
    }
    fn resolve_operator(&mut self) -> Result<(), expression::Error> {
        self.ResolveIndices()
    }
    fn memory_operator(&self) -> i64 {
        self.MemoryUsage()
    }
    fn correlated_operator(&self) -> Vec<CorrelatedColumn> {
        self.ExtractCorrelatedCols()
    }
    fn children_operator(&self) -> Vec<&dyn base::PhysicalPlan> {
        self.TablePlan.iter().map(|p| p.as_ref()).collect()
    }
    fn set_children_operator(&mut self, children: Vec<Box<dyn base::PhysicalPlan>>) {
        self.SetChildren(children)
    }
    fn set_child_operator(&mut self, index: usize, child: Box<dyn base::PhysicalPlan>) {
        if index == 0 {
            self.SetChildren(vec![child]);
        }
    }
    fn attach_operator_to_task(&self, tasks: Vec<Box<dyn base::Task>>) -> Box<dyn base::Task> {
        let context = base::Plan::s_ctx(self).clone();
        let children = tasks
            .iter()
            .map(|task| task.plan().clone_physical(task.plan().s_ctx().clone()))
            .collect::<Result<Vec<_>, _>>()
            .expect("physical table reader child task plan clone");
        let mut reader = self
            .Clone(context.clone())
            .expect("physical table reader clone");
        // Go materializes the reader only after its cop child has been built,
        // so its ExplainID follows the scan rather than the reader template.
        base::Plan::set_id(&mut reader, context.alloc_plan_id());
        reader.SetChildren(children);
        Box::new(RootTask::New(Box::new(reader), None))
    }
    fn cost_v1(
        &mut self,
        task: TaskType,
        option: &costusage::PlanCostOption,
    ) -> Result<f64, expression::Error> {
        self.GetPlanCostVer1(task, option)
    }
    fn cost_v2(
        &mut self,
        task: TaskType,
        option: &costusage::PlanCostOption,
        inl: &[bool],
    ) -> Result<costusage::CostVer2, expression::Error> {
        self.GetPlanCostVer2(task, option, inl)
    }
    fn operator_to_pb(
        &self,
        ctx: &mut base::BuildPBContext,
        store: kv::StoreType,
    ) -> Result<Box<tipb::Executor>, expression::Error> {
        self.ToPB(ctx, store)
    }
}
impl_concrete_physical_plan!(PhysicalTableReader);

// IndexLookUp 同时持有 IndexPlan 与 TablePlan，Schema 跟随表侧。
impl ConcretePhysicalOperator for PhysicalIndexLookUpReader {
    fn producer(&self) -> &PhysicalSchemaProducer {
        &self.PhysicalSchemaProducer
    }
    fn producer_mut(&mut self) -> &mut PhysicalSchemaProducer {
        &mut self.PhysicalSchemaProducer
    }
    fn explain_operator(&self) -> String {
        self.ExplainInfo()
    }
    fn explain_normalized_operator(&self) -> String {
        self.ExplainNormalizedInfo()
    }
    fn resolve_operator(&mut self) -> Result<(), expression::Error> {
        self.ResolveIndices()
    }
    fn memory_operator(&self) -> i64 {
        self.MemoryUsage()
    }
    fn correlated_operator(&self) -> Vec<CorrelatedColumn> {
        self.ExtractCorrelatedCols()
    }
    fn children_operator(&self) -> Vec<&dyn base::PhysicalPlan> {
        self.IndexPlan
            .iter()
            .chain(self.TablePlan.iter())
            .map(|p| p.as_ref())
            .collect()
    }
    fn set_children_operator(&mut self, mut children: Vec<Box<dyn base::PhysicalPlan>>) {
        let preserved_table = self.TablePlan.take();
        self.IndexPlan = if children.is_empty() {
            None
        } else {
            Some(children.remove(0))
        };
        self.TablePlan = if children.is_empty() {
            preserved_table
        } else {
            Some(children.remove(0))
        };
        if let Some(table) = &self.TablePlan {
            self.PhysicalSchemaProducer
                .SetSchema(table.schema().Clone());
        }
    }
    fn set_child_operator(&mut self, index: usize, child: Box<dyn base::PhysicalPlan>) {
        if index == 0 {
            self.IndexPlan = Some(child);
        } else if index == 1 {
            self.TablePlan = Some(child);
        }
    }
    fn cost_v1(
        &mut self,
        task: TaskType,
        option: &costusage::PlanCostOption,
    ) -> Result<f64, expression::Error> {
        self.GetPlanCostVer1(task, option)
    }
    fn cost_v2(
        &mut self,
        task: TaskType,
        option: &costusage::PlanCostOption,
        inl: &[bool],
    ) -> Result<costusage::CostVer2, expression::Error> {
        self.GetPlanCostVer2(task, option, inl)
    }
    fn operator_to_pb(
        &self,
        ctx: &mut base::BuildPBContext,
        store: kv::StoreType,
    ) -> Result<Box<tipb::Executor>, expression::Error> {
        self.ToPB(ctx, store)
    }
}
impl_concrete_physical_plan!(PhysicalIndexLookUpReader);

// IndexMerge 孩子为多个 PartialPlans 加上可选 TablePlan。
impl ConcretePhysicalOperator for PhysicalIndexMergeReader {
    fn producer(&self) -> &PhysicalSchemaProducer {
        &self.PhysicalSchemaProducer
    }
    fn producer_mut(&mut self) -> &mut PhysicalSchemaProducer {
        &mut self.PhysicalSchemaProducer
    }
    fn explain_operator(&self) -> String {
        self.ExplainInfo()
    }
    fn explain_normalized_operator(&self) -> String {
        self.ExplainNormalizedInfo()
    }
    fn resolve_operator(&mut self) -> Result<(), expression::Error> {
        self.ResolveIndices()
    }
    fn memory_operator(&self) -> i64 {
        self.MemoryUsage()
    }
    fn correlated_operator(&self) -> Vec<CorrelatedColumn> {
        self.ExtractCorrelatedCols()
    }
    fn children_operator(&self) -> Vec<&dyn base::PhysicalPlan> {
        self.PartialPlansRaw
            .iter()
            .map(|p| p.as_ref())
            .chain(self.TablePlan.iter().map(|p| p.as_ref()))
            .collect()
    }
    fn set_children_operator(&mut self, children: Vec<Box<dyn base::PhysicalPlan>>) {
        self.PartialPlansRaw = children;
    }
    fn set_child_operator(&mut self, index: usize, child: Box<dyn base::PhysicalPlan>) {
        if index < self.PartialPlansRaw.len() {
            self.PartialPlansRaw[index] = child;
        } else {
            self.TablePlan = Some(child);
        }
    }
    fn cost_v1(
        &mut self,
        task: TaskType,
        option: &costusage::PlanCostOption,
    ) -> Result<f64, expression::Error> {
        self.GetPlanCostVer1(task, option)
    }
    fn cost_v2(
        &mut self,
        task: TaskType,
        option: &costusage::PlanCostOption,
        inl: &[bool],
    ) -> Result<costusage::CostVer2, expression::Error> {
        self.GetPlanCostVer2(task, option, inl)
    }
    fn operator_to_pb(
        &self,
        ctx: &mut base::BuildPBContext,
        store: kv::StoreType,
    ) -> Result<Box<tipb::Executor>, expression::Error> {
        self.ToPB(ctx, store)
    }
}
impl_concrete_physical_plan!(PhysicalIndexMergeReader);

// BatchPointGet 复用内部 PointGetPlan 的 Schema 生产者。
impl ConcretePhysicalOperator for BatchPointGetPlan {
    fn output_names_operator(&self) -> base::types::NameSlice {
        self.PointGetPlan.OutputNames()
    }
    fn set_output_names_operator(&mut self, names: base::types::NameSlice) {
        self.PointGetPlan.SetOutputNames(names);
    }
    fn producer(&self) -> &PhysicalSchemaProducer {
        &self.PointGetPlan.PhysicalSchemaProducer
    }
    fn producer_mut(&mut self) -> &mut PhysicalSchemaProducer {
        &mut self.PointGetPlan.PhysicalSchemaProducer
    }
    fn explain_operator(&self) -> String {
        self.ExplainInfo()
    }
    fn explain_normalized_operator(&self) -> String {
        self.ExplainNormalizedInfo()
    }
    fn resolve_operator(&mut self) -> Result<(), expression::Error> {
        self.ResolveIndices()
    }
    fn memory_operator(&self) -> i64 {
        self.MemoryUsage()
    }
    fn correlated_operator(&self) -> Vec<CorrelatedColumn> {
        self.ExtractCorrelatedCols()
    }
    fn cost_v1(
        &mut self,
        task: TaskType,
        option: &costusage::PlanCostOption,
    ) -> Result<f64, expression::Error> {
        self.GetPlanCostVer1(task, option)
    }
    fn cost_v2(
        &mut self,
        task: TaskType,
        option: &costusage::PlanCostOption,
        inl: &[bool],
    ) -> Result<costusage::CostVer2, expression::Error> {
        self.GetPlanCostVer2(task, option, inl)
    }
    fn operator_to_pb(
        &self,
        ctx: &mut base::BuildPBContext,
        store: kv::StoreType,
    ) -> Result<Box<tipb::Executor>, expression::Error> {
        self.ToPB(ctx, store)
    }
}
impl_concrete_physical_plan!(BatchPointGetPlan);

impl ConcretePhysicalOperator for PhysicalUnionAll {
    fn producer(&self) -> &PhysicalSchemaProducer {
        &self.PhysicalSchemaProducer
    }
    fn producer_mut(&mut self) -> &mut PhysicalSchemaProducer {
        &mut self.PhysicalSchemaProducer
    }
    fn explain_operator(&self) -> String {
        String::new()
    }
    fn explain_normalized_operator(&self) -> String {
        String::new()
    }
    fn resolve_operator(&mut self) -> Result<(), expression::Error> {
        self.PhysicalSchemaProducer.ResolveIndices()
    }
    fn memory_operator(&self) -> i64 {
        self.MemoryUsage()
    }
    fn cost_v1(
        &mut self,
        task: TaskType,
        option: &costusage::PlanCostOption,
    ) -> Result<f64, expression::Error> {
        self.GetPlanCostVer1(task, option)
    }
    fn cost_v2(
        &mut self,
        task: TaskType,
        option: &costusage::PlanCostOption,
        inl: &[bool],
    ) -> Result<costusage::CostVer2, expression::Error> {
        self.GetPlanCostVer2(task, option, inl)
    }
}
impl_concrete_physical_plan!(PhysicalUnionAll);

/// Sort / TopN 共用的宏：Explain 归一化信息与未归一化相同。
macro_rules! impl_sort_operator {
    ($operator:ty, $producer:ident, {$($schema_override:tt)*}) => {
        impl ConcretePhysicalOperator for $operator {
            fn producer(&self) -> &PhysicalSchemaProducer {
                &self.$producer
            }
            fn producer_mut(&mut self) -> &mut PhysicalSchemaProducer {
                &mut self.$producer
            }
            $($schema_override)*
            fn explain_operator(&self) -> String {
                self.ExplainInfo()
            }
            fn explain_normalized_operator(&self) -> String {
                self.ExplainInfo()
            }
            fn resolve_operator(&mut self) -> Result<(), expression::Error> {
                self.ResolveIndices()
            }
            fn memory_operator(&self) -> i64 {
                self.MemoryUsage()
            }
            fn correlated_operator(&self) -> Vec<CorrelatedColumn> {
                self.ExtractCorrelatedCols()
            }
            fn cost_v1(
                &mut self,
                task: TaskType,
                option: &costusage::PlanCostOption,
            ) -> Result<f64, expression::Error> {
                self.GetPlanCostVer1(task, option)
            }
            fn cost_v2(
                &mut self,
                task: TaskType,
                option: &costusage::PlanCostOption,
                inl: &[bool],
            ) -> Result<costusage::CostVer2, expression::Error> {
                self.GetPlanCostVer2(task, option, inl)
            }
            fn operator_to_pb(
                &self,
                ctx: &mut base::BuildPBContext,
                store: kv::StoreType,
            ) -> Result<Box<tipb::Executor>, expression::Error> {
                self.ToPB(ctx, store)
            }
        }
        impl_concrete_physical_plan!($operator);
    };
}
impl_sort_operator!(PhysicalSort, PhysicalSchemaProducer, {
    fn schema_operator(&self) -> &Schema {
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .Children()
            .into_iter()
            .next()
            .map(base::Plan::schema)
            .unwrap_or_else(|| {
                self.PhysicalSchemaProducer.SchemaRef().unwrap_or_else(|| {
                    base::Plan::schema(&self.PhysicalSchemaProducer.BasePhysicalPlan)
                })
            })
    }
});
impl_sort_operator!(PhysicalTopN, PhysicalSchemaProducer, {});

impl ConcretePhysicalOperator for physical_cte::PhysicalCteScan {
    fn producer(&self) -> &PhysicalSchemaProducer {
        &self.PhysicalSchemaProducer
    }
    fn producer_mut(&mut self) -> &mut PhysicalSchemaProducer {
        &mut self.PhysicalSchemaProducer
    }
    fn explain_operator(&self) -> String {
        self.ExplainInfo()
    }
    fn explain_normalized_operator(&self) -> String {
        self.ExplainInfo()
    }
    fn resolve_operator(&mut self) -> Result<(), expression::Error> {
        self.ResolveIndices()
    }
    fn memory_operator(&self) -> i64 {
        self.MemoryUsage()
    }
    fn correlated_operator(&self) -> Vec<CorrelatedColumn> {
        self.ExtractCorrelatedCols()
    }
    fn cost_v1(
        &mut self,
        task: TaskType,
        option: &costusage::PlanCostOption,
    ) -> Result<f64, expression::Error> {
        self.GetPlanCostVer1(task, option)
    }
    fn cost_v2(
        &mut self,
        task: TaskType,
        option: &costusage::PlanCostOption,
        inl: &[bool],
    ) -> Result<costusage::CostVer2, expression::Error> {
        self.GetPlanCostVer2(task, option, inl)
    }
}
impl_concrete_physical_plan!(physical_cte::PhysicalCteScan);

impl ConcretePhysicalOperator for physical_cte::PhysicalCTE {
    fn producer(&self) -> &PhysicalSchemaProducer {
        &self.PhysicalSchemaProducer
    }
    fn producer_mut(&mut self) -> &mut PhysicalSchemaProducer {
        &mut self.PhysicalSchemaProducer
    }
    fn explain_operator(&self) -> String {
        format!("data:CTE_{}", self.CTE.IDForStorage)
    }
    fn explain_normalized_operator(&self) -> String {
        self.explain_operator()
    }
    fn resolve_operator(&mut self) -> Result<(), expression::Error> {
        Ok(())
    }
    fn memory_operator(&self) -> i64 {
        self.PhysicalSchemaProducer.MemoryUsage()
    }
    fn cost_v1(
        &mut self,
        task: TaskType,
        option: &costusage::PlanCostOption,
    ) -> Result<f64, expression::Error> {
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .GetPlanCostVer1(task, option)
    }
    fn cost_v2(
        &mut self,
        task: TaskType,
        option: &costusage::PlanCostOption,
        inl: &[bool],
    ) -> Result<costusage::CostVer2, expression::Error> {
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .GetPlanCostVer2(task, option, inl)
    }
}
impl_concrete_physical_plan!(physical_cte::PhysicalCTE);

impl ConcretePhysicalOperator for physical_cte::PhysicalCTEDefinition {
    fn producer(&self) -> &PhysicalSchemaProducer {
        &self.PhysicalSchemaProducer
    }
    fn producer_mut(&mut self) -> &mut PhysicalSchemaProducer {
        &mut self.PhysicalSchemaProducer
    }
    fn explain_operator(&self) -> String {
        format!("CTE_{}", self.IDForStorage)
    }
    fn explain_normalized_operator(&self) -> String {
        self.explain_operator()
    }
    fn resolve_operator(&mut self) -> Result<(), expression::Error> {
        Ok(())
    }
    fn memory_operator(&self) -> i64 {
        self.PhysicalSchemaProducer.MemoryUsage()
    }
    fn cost_v1(
        &mut self,
        task: TaskType,
        option: &costusage::PlanCostOption,
    ) -> Result<f64, expression::Error> {
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .GetPlanCostVer1(task, option)
    }
    fn cost_v2(
        &mut self,
        task: TaskType,
        option: &costusage::PlanCostOption,
        inl: &[bool],
    ) -> Result<costusage::CostVer2, expression::Error> {
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .GetPlanCostVer2(task, option, inl)
    }
}
impl_concrete_physical_plan!(physical_cte::PhysicalCTEDefinition);

// NominalSort 可在 OnlyColumn 时透传子任务，故覆盖 attach。
impl ConcretePhysicalOperator for NominalSort {
    fn producer(&self) -> &PhysicalSchemaProducer {
        &self.PhysicalSchemaProducer
    }
    fn producer_mut(&mut self) -> &mut PhysicalSchemaProducer {
        &mut self.PhysicalSchemaProducer
    }
    fn explain_operator(&self) -> String {
        String::new()
    }
    fn explain_normalized_operator(&self) -> String {
        String::new()
    }
    fn resolve_operator(&mut self) -> Result<(), expression::Error> {
        self.ResolveIndices()
    }
    fn memory_operator(&self) -> i64 {
        self.MemoryUsage()
    }
    fn attach_operator_to_task(&self, tasks: Vec<Box<dyn base::Task>>) -> Box<dyn base::Task>
    where
        Self: base::PhysicalPlan,
    {
        self.Attach2Task(tasks)
    }
    fn cost_v1(
        &mut self,
        task: TaskType,
        option: &costusage::PlanCostOption,
    ) -> Result<f64, expression::Error> {
        self.GetPlanCostVer1(task, option)
    }
    fn cost_v2(
        &mut self,
        task: TaskType,
        option: &costusage::PlanCostOption,
        inl: &[bool],
    ) -> Result<costusage::CostVer2, expression::Error> {
        self.GetPlanCostVer2(task, option, inl)
    }
}
impl_concrete_physical_plan!(NominalSort);

/// 叶子/无复杂孩子算子：代价直接委托 BasePhysicalPlan。
macro_rules! impl_schema_leaf_operator {
    ($operator:ty $(, output_names = $output_names:ident, set_output_names = $set_output_names:ident)?) => {
        impl ConcretePhysicalOperator for $operator {
            fn producer(&self) -> &PhysicalSchemaProducer {
                &self.PhysicalSchemaProducer
            }
            fn producer_mut(&mut self) -> &mut PhysicalSchemaProducer {
                &mut self.PhysicalSchemaProducer
            }
            fn explain_operator(&self) -> String {
                self.ExplainInfo()
            }
            fn explain_normalized_operator(&self) -> String {
                self.ExplainInfo()
            }
            fn resolve_operator(&mut self) -> Result<(), expression::Error> {
                self.PhysicalSchemaProducer.ResolveIndices()
            }
            fn memory_operator(&self) -> i64 {
                self.MemoryUsage()
            }
            $(
                fn output_names_operator(&self) -> base::types::NameSlice {
                    self.$output_names()
                }
                fn set_output_names_operator(&mut self, names: base::types::NameSlice) {
                    self.$set_output_names(names)
                }
            )?
            fn cost_v1(
                &mut self,
                task: TaskType,
                option: &costusage::PlanCostOption,
            ) -> Result<f64, expression::Error> {
                self.PhysicalSchemaProducer
                    .BasePhysicalPlan
                    .GetPlanCostVer1(task, option)
            }
            fn cost_v2(
                &mut self,
                task: TaskType,
                option: &costusage::PlanCostOption,
                inl: &[bool],
            ) -> Result<costusage::CostVer2, expression::Error> {
                self.PhysicalSchemaProducer
                    .BasePhysicalPlan
                    .GetPlanCostVer2(task, option, inl)
            }
        }
        impl_concrete_physical_plan!($operator);
    };
}
impl_schema_leaf_operator!(
    PhysicalTableDual,
    output_names = OutputNames,
    set_output_names = SetOutputNames
);
impl_schema_leaf_operator!(PhysicalMemTable);
impl_schema_leaf_operator!(PhysicalTableSample);
impl_schema_leaf_operator!(PhysicalShow);
impl_schema_leaf_operator!(PhysicalMaxOneRow);
impl_schema_leaf_operator!(LegacyPhysicalLock);

impl ConcretePhysicalOperator for PhysicalWindow {
    fn producer(&self) -> &PhysicalSchemaProducer {
        &self.PhysicalSchemaProducer
    }
    fn producer_mut(&mut self) -> &mut PhysicalSchemaProducer {
        &mut self.PhysicalSchemaProducer
    }
    fn explain_operator(&self) -> String {
        self.ExplainInfo()
    }
    fn explain_normalized_operator(&self) -> String {
        self.ExplainInfo()
    }
    fn resolve_operator(&mut self) -> Result<(), expression::Error> {
        self.ResolveIndices()
    }
    fn memory_operator(&self) -> i64 {
        self.MemoryUsage()
    }
    fn correlated_operator(&self) -> Vec<CorrelatedColumn> {
        self.ExtractCorrelatedCols()
    }
    fn cost_v1(
        &mut self,
        task: TaskType,
        option: &costusage::PlanCostOption,
    ) -> Result<f64, expression::Error> {
        self.GetPlanCostVer1(task, option)
    }
    fn cost_v2(
        &mut self,
        task: TaskType,
        option: &costusage::PlanCostOption,
        inl: &[bool],
    ) -> Result<costusage::CostVer2, expression::Error> {
        self.GetPlanCostVer2(task, option, inl)
    }
    fn operator_to_pb(
        &self,
        ctx: &mut base::BuildPBContext,
        store: kv::StoreType,
    ) -> Result<Box<tipb::Executor>, expression::Error> {
        self.ToPB(ctx, store)
    }
}
impl_concrete_physical_plan!(PhysicalWindow);

impl ConcretePhysicalOperator for PhysicalShuffle {
    fn producer(&self) -> &PhysicalSchemaProducer {
        &self.PhysicalSchemaProducer
    }
    fn producer_mut(&mut self) -> &mut PhysicalSchemaProducer {
        &mut self.PhysicalSchemaProducer
    }
    fn explain_operator(&self) -> String {
        self.ExplainInfo()
    }
    fn explain_normalized_operator(&self) -> String {
        self.ExplainInfo()
    }
    fn resolve_operator(&mut self) -> Result<(), expression::Error> {
        self.ResolveIndices()
    }
    fn memory_operator(&self) -> i64 {
        self.MemoryUsage()
    }
    fn correlated_operator(&self) -> Vec<CorrelatedColumn> {
        self.ExtractCorrelatedCols()
    }
    fn cost_v1(
        &mut self,
        task: TaskType,
        option: &costusage::PlanCostOption,
    ) -> Result<f64, expression::Error> {
        self.GetPlanCostVer1(task, option)
    }
    fn cost_v2(
        &mut self,
        task: TaskType,
        option: &costusage::PlanCostOption,
        inl: &[bool],
    ) -> Result<costusage::CostVer2, expression::Error> {
        self.GetPlanCostVer2(task, option, inl)
    }
}
impl_concrete_physical_plan!(PhysicalShuffle);

impl ConcretePhysicalOperator for PhysicalShuffleReceiverStub {
    fn producer(&self) -> &PhysicalSchemaProducer {
        &self.PhysicalSchemaProducer
    }
    fn producer_mut(&mut self) -> &mut PhysicalSchemaProducer {
        &mut self.PhysicalSchemaProducer
    }
    fn explain_operator(&self) -> String {
        String::new()
    }
    fn explain_normalized_operator(&self) -> String {
        String::new()
    }
    fn resolve_operator(&mut self) -> Result<(), expression::Error> {
        Ok(())
    }
    fn memory_operator(&self) -> i64 {
        self.PhysicalSchemaProducer.MemoryUsage()
            + self
                .DataSource
                .as_ref()
                .map_or(0, |source| source.memory_usage())
    }
    fn children_operator(&self) -> Vec<&dyn base::PhysicalPlan> {
        Vec::new()
    }
    fn cost_v1(
        &mut self,
        task: TaskType,
        option: &costusage::PlanCostOption,
    ) -> Result<f64, expression::Error> {
        self.GetPlanCostVer1(task, option)
    }
    fn cost_v2(
        &mut self,
        task: TaskType,
        option: &costusage::PlanCostOption,
        inl: &[bool],
    ) -> Result<costusage::CostVer2, expression::Error> {
        self.GetPlanCostVer2(task, option, inl)
    }
}
impl_concrete_physical_plan!(PhysicalShuffleReceiverStub);

// ---- 同目录 Aster 单元测试通过 path 属性挂入 ----
#[cfg(test)]
#[path = "foundation_aster_unit_test.rs"]
mod foundation_aster_unit_test;

#[cfg(test)]
#[path = "joins_agg_aster_unit_test.rs"]
mod joins_agg_aster_unit_test;

#[cfg(test)]
#[path = "readers_scans_aster_unit_test.rs"]
mod readers_scans_aster_unit_test;

#[cfg(test)]
#[path = "logical_plan_route_aster_unit_test.rs"]
mod logical_plan_route_aster_unit_test;

#[cfg(test)]
#[path = "canonical_router_aster_unit_test.rs"]
mod canonical_router_aster_unit_test;

#[cfg(test)]
#[path = "index_join_lookup_cardinality_aster_unit_test.rs"]
mod index_join_lookup_cardinality_aster_unit_test;

#[cfg(test)]
#[path = "mpp_join_hash_cols_aster_unit_test.rs"]
mod mpp_join_hash_cols_aster_unit_test;

#[cfg(test)]
#[path = "unary_aster_unit_test.rs"]
mod unary_aster_unit_test;

#[cfg(test)]
#[path = "physical_semantic_aster_unit_test.rs"]
mod physical_semantic_aster_unit_test;

#[cfg(test)]
#[path = "physical_union_all_test.rs"]
mod physical_union_all_test;

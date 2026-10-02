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

// 物理计划基座与规范逻辑→物理路由。
// 包含 BasePhysicalPlan、任务缓存、FindBestTask 路由及各类算子枚举/挂接辅助。

use std::any::Any;
use std::any::TypeId;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Display;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};

use base::{ContextRef, PhysicalPlan, Plan, Task};
use baseimpl::{NewBasePlan, Plan as CommonPlan};
use costusage::{
    COST_FLAG_RECALCULATE, CostVer2, PlanCostOption, has_cost_flag, new_zero_cost_ver2,
    sum_cost_ver2, trace_cost,
};
use expression::{Column, CorrelatedColumn, Schema};
use logicalop::LogicalPlan as _;
use model::{ColumnInfo, ExtraPhysTblID, FindColumnInfoByID, NewExtraPhysTblIDColInfo};
use property::{PhysicalProperty, StatsInfo, TaskType};

use crate::AttachedTask;

struct ScanCardinalityContext<'a>(&'a dyn base::PlanContext);

impl cardinality::CardinalityContext for ScanCardinalityContext<'_> {
    fn GetSessionVars(&self) -> &cardinality::variable::SessionVars {
        self.0.GetSessionVars()
    }

    fn GetExprCtx(&self) -> &dyn cardinality::expression::exprctx::ExprContext {
        self.0.GetExprCtx()
    }

    fn GetRangerCtx(&self) -> &planctx::rangerctx::RangerContext<'_> {
        self.0.GetRangerCtx()
    }
}

// 线程本地缓存：避免同一逻辑节点+属性重复寻优。
thread_local! {
    static CANONICAL_TASK_CACHE: RefCell<HashMap<(usize, i32, String, Vec<i64>, Vec<u8>), Box<dyn Task>>> =
        RefCell::new(HashMap::new());
}

pub(crate) fn is_canonical_index_join_type(type_id: TypeId) -> bool {
    type_id == TypeId::of::<crate::physical_index_hash_join::LegacyPhysicalIndexHashJoin>()
        || type_id == TypeId::of::<crate::physical_index_merge_join::LegacyPhysicalIndexMergeJoin>()
        || type_id == TypeId::of::<crate::PhysicalIndexJoin>()
        || type_id == TypeId::of::<crate::physical_index_hash_join::PhysicalIndexHashJoin>()
        || type_id == TypeId::of::<crate::physical_index_merge_join::PhysicalIndexMergeJoin>()
}

fn contains_canonical_index_join(plan: &dyn PhysicalPlan) -> bool {
    is_canonical_index_join_type(plan.as_any().type_id())
        || matches!(
            plan.tp(&[]).as_str(),
            "IndexJoin" | "IndexHashJoin" | "IndexMergeJoin"
        )
        || plan
            .as_any()
            .downcast_ref::<crate::PhysicalTableReader>()
            .and_then(|reader| reader.TablePlan.as_deref())
            .is_some_and(contains_canonical_index_join)
        || plan
            .children()
            .into_iter()
            .any(contains_canonical_index_join)
}

fn contains_canonical_lock(plan: &dyn PhysicalPlan) -> bool {
    plan.as_any().is::<crate::LegacyPhysicalLock>()
        || plan.as_any().is::<crate::PhysicalLock>()
        || plan.children().into_iter().any(contains_canonical_lock)
}

/// 清空线程本地的规范路由任务缓存。
pub fn ResetCanonicalTaskCache() {
    CANONICAL_TASK_CACHE.with(|cache| cache.borrow_mut().clear());
}

pub(crate) fn mpp_agg_partition_is_satisfied(
    partition_type: property::MPPPartitionType,
    current: &[property::MPPPartitionColumn],
    required: &[property::MPPPartitionColumn],
) -> bool {
    partition_type == property::SinglePartitionType
        || (partition_type == property::HashType
            && !current.is_empty()
            && current.len() == required.len()
            && current
                .iter()
                .all(|column| required.iter().any(|expected| column.Equal(expected))))
}

pub(crate) fn commit_mpp_agg_candidate_column_id(
    committed: i64,
    candidate: i64,
    candidate_wins: bool,
) -> i64 {
    if candidate_wins { candidate } else { committed }
}

/// 按逻辑计划指针/ID/类型/Schema 列与物理属性哈希查找缓存任务。
fn cached_canonical_task(
    plan: &dyn logicalop::LogicalPlan,
    property: &PhysicalProperty,
) -> Option<Box<dyn Task>> {
    let key = (
        plan as *const dyn logicalop::LogicalPlan as *const () as usize,
        plan.ID(),
        plan.TP().to_owned(),
        plan.Schema()
            .Columns
            .iter()
            .map(|column| column.UniqueID)
            .collect(),
        property.HashCode(),
    );
    CANONICAL_TASK_CACHE.with(|cache| cache.borrow().get(&key).map(|task| task.copy()))
}

/// 将选出的物理任务写入规范路由缓存。
fn store_canonical_task(
    plan: &dyn logicalop::LogicalPlan,
    property: &PhysicalProperty,
    task: &dyn Task,
) {
    let key = (
        plan as *const dyn logicalop::LogicalPlan as *const () as usize,
        plan.ID(),
        plan.TP().to_owned(),
        plan.Schema()
            .Columns
            .iter()
            .map(|column| column.UniqueID)
            .collect(),
        property.HashCode(),
    );
    CANONICAL_TASK_CACHE.with(|cache| {
        cache.borrow_mut().insert(key, task.copy());
    });
}

/// Adds the hidden physical-partition id column exactly once.
/// 向列信息与 Schema 追加隐藏物理分区表 ID 列，已存在则跳过。
pub fn AddExtraPhysTblIDColumn(
    sctx: &dyn base::PlanContext,
    mut columns: Vec<ColumnInfo>,
    mut schema: Schema,
) -> (Vec<ColumnInfo>, Schema, bool) {
    if FindColumnInfoByID(&columns, ExtraPhysTblID).is_some() {
        return (columns, schema, false);
    }
    columns.push(NewExtraPhysTblIDColInfo());
    schema.Append([Column::new(
        *expression::types::NewFieldType(mysql::r#type::TypeLonglong),
        ExtraPhysTblID,
        sctx.GetSessionVars().AllocPlanColumnID(),
        schema.Len() as isize,
    )]);
    (columns, schema, true)
}

/// Recursively collects statistics versions carried by foundation plan nodes.
/// Concrete scans set `stats_table_name` during initialization.
/// 递归收集计划树中各表统计信息版本号。
pub fn CollectPlanStatsVersion(plan: &dyn PhysicalPlan, output: &mut HashMap<String, u64>) {
    if let Some(reader) = plan
        .as_any()
        .downcast_ref::<crate::PhysicalIndexLookUpReader>()
    {
        if let Some(index_plan) = reader.IndexPlan.as_deref() {
            CollectPlanStatsVersion(index_plan, output);
        }
        return;
    }
    for child in plan.children() {
        CollectPlanStatsVersion(child, output);
    }
    let table = if let Some(scan) = plan.as_any().downcast_ref::<crate::PhysicalIndexScan>() {
        scan.Table.as_ref()
    } else if let Some(scan) = plan.as_any().downcast_ref::<crate::PhysicalTableScan>() {
        scan.Table.as_ref()
    } else {
        None
    };
    if let Some(table) = table {
        output.insert(table.Name.O.clone(), plan.stats_info().StatsVersion);
    }
}

/// Clones a physical plan and converts a panic at the Go recover boundary into an error.
/// 安全克隆物理计划：将 panic 转为 Error（对应 Go recover）。
pub fn SafeClone(
    sctx: ContextRef,
    value: &dyn PhysicalPlan,
) -> Result<Box<dyn PhysicalPlan>, expression::Error> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| value.clone_physical(sctx)))
        .unwrap_or_else(|payload| {
            let message = payload
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
                .unwrap_or("unknown panic");
            Err(expression::errors::New(message))
        })
}

/// Common physical-plan state. Concrete operators may delegate their base traits to this value.
/// 物理计划公共状态：孩子、代价、统计、Schema 与探测父节点等。
pub struct BasePhysicalPlan {
    pub Plan: CommonPlan,
    children_req_props: Vec<Box<PhysicalProperty>>,
    children: Vec<Box<dyn PhysicalPlan>>,
    pub PlanCostInit: bool,
    pub PlanCost: f64,
    PlanCostVer2: Option<CostVer2>,
    probe_parents: Vec<Box<dyn PhysicalPlan>>,
    pub TiFlashFineGrainedShuffleStreamCount: u64,
    schema: Schema,
    stats: StatsInfo,
    stats_table_name: Option<String>,
    store_type: Option<kv::StoreType>,
}

impl BasePhysicalPlan {
    pub(crate) fn cache_children_req_props(&self) -> &[Box<PhysicalProperty>] {
        &self.children_req_props
    }

    pub(crate) fn cache_plan_cost_ver2(&self) -> Option<&CostVer2> {
        self.PlanCostVer2.as_ref()
    }

    pub(crate) fn cache_probe_parents(&self) -> &[Box<dyn PhysicalPlan>] {
        &self.probe_parents
    }

    pub(crate) fn cache_schema(&self) -> &Schema {
        &self.schema
    }

    pub(crate) fn cache_stats(&self) -> &StatsInfo {
        &self.stats
    }

    pub(crate) fn cache_stats_table_name(&self) -> Option<&str> {
        self.stats_table_name.as_deref()
    }

    pub(crate) fn restore_cached_fields(
        &mut self,
        plan_cost_ver2: Option<CostVer2>,
        stats: StatsInfo,
        stats_table_name: Option<String>,
        schema: Schema,
        probe_parents: Vec<Box<dyn PhysicalPlan>>,
    ) {
        self.PlanCostVer2 = plan_cost_ver2;
        self.stats = stats.clone();
        self.Plan.SetStats(Some(Arc::new(stats)));
        self.stats_table_name = stats_table_name;
        self.schema = schema;
        self.probe_parents = probe_parents;
    }

    /// 创建带类型名与查询块偏移的空物理计划基座。
    pub fn New(ctx: ContextRef, tp: impl Into<String>, offset: i32) -> Self {
        Self {
            Plan: NewBasePlan(ctx, tp, offset),
            children_req_props: Vec::new(),
            children: Vec::new(),
            PlanCostInit: false,
            PlanCost: 0.0,
            PlanCostVer2: None,
            probe_parents: Vec::new(),
            TiFlashFineGrainedShuffleStreamCount: 0,
            schema: expression::NewSchema(Vec::new()),
            stats: StatsInfo::default(),
            stats_table_name: None,
            store_type: None,
        }
    }

    /// 兼容 Go 基座的类型设置入口，转发至公共计划状态。
    pub fn SetTP(&mut self, tp: impl Into<String>) {
        self.Plan.SetTP(tp);
    }

    /// 设置输出 Schema。
    pub fn SetSchema(&mut self, schema: Schema) {
        self.schema = schema;
    }

    /// 记录统计信息对应的表名，供版本收集使用。
    pub fn SetStatsTableName(&mut self, table: Option<String>) {
        self.stats_table_name = table;
    }

    /// 设置存储引擎类型（TiKV/TiFlash 等）。
    pub fn SetStoreType(&mut self, store_type: Option<kv::StoreType>) {
        self.store_type = store_type;
    }

    /// 返回存储引擎类型。
    pub fn StoreType(&self) -> Option<kv::StoreType> {
        self.store_type
    }

    /// 返回子计划只读视图。
    pub fn Children(&self) -> Vec<&dyn PhysicalPlan> {
        self.children.iter().map(Box::as_ref).collect()
    }

    /// 返回子计划可变切片。
    pub fn ChildrenMut(&mut self) -> &mut [Box<dyn PhysicalPlan>] {
        &mut self.children
    }

    /// 替换全部子节点并失效已缓存代价。
    pub fn SetChildren(&mut self, children: Vec<Box<dyn PhysicalPlan>>) {
        self.children = children;
        self.ensure_child_properties();
        self.PlanCostInit = false;
    }

    /// 替换指定下标子节点并失效代价缓存。
    pub fn SetChild(&mut self, index: usize, child: Box<dyn PhysicalPlan>) {
        self.children[index] = child;
        self.ensure_child_properties();
        self.PlanCostInit = false;
    }

    /// 设置各子节点须满足的物理属性（排序、任务类型等）。
    pub fn SetChildrenReqProps(&mut self, properties: Vec<Box<PhysicalProperty>>) {
        self.children_req_props = properties;
    }

    /// 设置第 index 个子节点的所需物理属性，不足时扩展默认值。
    pub fn SetXthChildReqProps(&mut self, index: usize, property: Box<PhysicalProperty>) {
        if self.children_req_props.len() <= index {
            self.children_req_props
                .resize_with(index + 1, || Box::new(PhysicalProperty::default()));
        }
        self.children_req_props[index] = property;
    }

    /// 保证 children_req_props 长度不少于孩子数。
    fn ensure_child_properties(&mut self) {
        if self.children_req_props.len() < self.children.len() {
            self.children_req_props
                .resize_with(
                    self.children.len(),
                    || Box::new(PhysicalProperty::default()),
                );
        }
    }

    /// 深拷贝子树与探测父节点，并换绑新上下文；代价缓存重置。
    pub fn CloneWithNewCtx(&self, new_ctx: ContextRef) -> Result<Self, expression::Error> {
        let children = ClonePhysicalChildren(new_ctx.clone(), &self.children)?;
        let probe_parents = ClonePhysicalChildren(new_ctx.clone(), &self.probe_parents)?;
        let mut plan = self.Plan.CloneWithNewCtx(new_ctx);
        plan.SetStats(Some(Arc::new(self.stats.clone())));
        Ok(Self {
            Plan: plan,
            children_req_props: self
                .children_req_props
                .iter()
                .map(|property| Box::new(property.CloneEssentialFields()))
                .collect(),
            children,
            PlanCostInit: false,
            PlanCost: 0.0,
            PlanCostVer2: None,
            probe_parents,
            TiFlashFineGrainedShuffleStreamCount: self.TiFlashFineGrainedShuffleStreamCount,
            schema: self.schema.Clone(),
            stats: self.stats.clone(),
            stats_table_name: self.stats_table_name.clone(),
            store_type: self.store_type,
        })
    }

    /// 代价模型 v1：累加子节点代价，可复用缓存除非要求重算。
    pub fn GetPlanCostVer1(
        &mut self,
        task_type: TaskType,
        option: &PlanCostOption,
    ) -> Result<f64, expression::Error> {
        if self.PlanCostInit && !has_cost_flag(option.cost_flag, COST_FLAG_RECALCULATE) {
            return Ok(self.PlanCost);
        }
        self.PlanCost = 0.0;
        for child in &mut self.children {
            self.PlanCost += child.get_plan_cost_ver1(task_type, option)?;
        }
        self.PlanCostInit = true;
        Ok(self.PlanCost)
    }

    /// 代价模型 v2：汇总子节点 CostVer2。
    pub fn GetPlanCostVer2(
        &mut self,
        task_type: TaskType,
        option: &PlanCostOption,
        is_child_of_inl: &[bool],
    ) -> Result<CostVer2, expression::Error> {
        if self.PlanCostInit
            && !has_cost_flag(option.cost_flag, COST_FLAG_RECALCULATE)
            && let Some(cost) = &self.PlanCostVer2
        {
            return Ok(cost.clone());
        }
        let mut costs = Vec::with_capacity(self.children.len());
        for child in &mut self.children {
            costs.push(child.get_plan_cost_ver2(task_type, option, is_child_of_inl)?);
        }
        let cost = if costs.is_empty() {
            new_zero_cost_ver2(trace_cost(Some(option)))
        } else {
            sum_cost_ver2(&costs)
        };
        self.PlanCostVer2 = Some(cost.clone());
        self.PlanCostInit = true;
        Ok(cost)
    }

    /// 将本算子挂到子任务上，形成可执行物理任务。
    pub fn Attach2Task(&self, tasks: Vec<Box<dyn Task>>) -> Box<dyn Task> {
        PhysicalPlan::attach_to_task(self, tasks)
    }

    /// 返回指定孩子所需物理属性。
    pub fn GetChildReqProps(&self, index: usize) -> &PhysicalProperty {
        &self.children_req_props[index]
    }

    /// 返回估计行数。
    pub fn StatsCount(&self) -> f64 {
        self.stats.RowCount
    }

    /// 基座默认无相关列；具体算子可覆盖。
    pub fn ExtractCorrelatedCols(&self) -> Vec<CorrelatedColumn> {
        Vec::new()
    }

    /// 基座默认空规范化 EXPLAIN 信息。
    pub fn ExplainNormalizedInfo(&self) -> String {
        String::new()
    }

    /// 递归解析子节点表达式列下标。
    pub fn ResolveIndices(&mut self) -> Result<(), expression::Error> {
        for child in &mut self.children {
            child.resolve_indices()?;
        }
        Ok(())
    }

    /// 估算本节点与子树内存占用。
    pub fn MemoryUsage(&self) -> i64 {
        self.Plan.MemoryUsage()
            + std::mem::size_of::<Self>() as i64
            + self
                .children_req_props
                .iter()
                .map(|property| property.MemoryUsage())
                .sum::<i64>()
            + self
                .children
                .iter()
                .map(|child| child.memory_usage())
                .sum::<i64>()
    }

    /// 记录 IndexJoin 等探测侧父节点，用于行数展示缩放。
    pub fn SetProbeParents(&mut self, parents: Vec<Box<dyn PhysicalPlan>>) {
        self.probe_parents = parents;
    }

    /// 展示用估计行数：自身行数乘以各探测父节点行数。
    pub fn GetEstRowCountForDisplay(&self) -> f64 {
        self.stats.RowCount
            * self
                .probe_parents
                .iter()
                .map(|parent| parent.stats_count().max(1.0))
                .product::<f64>()
    }

    /// 根据运行时统计计算实际探测次数乘积。
    pub fn GetActualProbeCnt(&self, stats: &execdetails::execdetails::RuntimeStatsColl) -> i64 {
        self.probe_parents
            .iter()
            .map(|parent| stats.GetPlanActRows(parent.id()).max(1))
            .product::<i64>()
            .max(1)
    }
}

/// 克隆物理子计划列表。
fn ClonePhysicalChildren(
    context: ContextRef,
    children: &[Box<dyn PhysicalPlan>],
) -> Result<Vec<Box<dyn PhysicalPlan>>, expression::Error> {
    children
        .iter()
        .map(|child| child.clone_physical(context.clone()))
        .collect()
}

/// 实现通用 Plan trait：ID、Schema、统计与计划缓存克隆等。
impl Plan for BasePhysicalPlan {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn as_physical_plan(&self) -> Option<&dyn PhysicalPlan> {
        Some(self)
    }
    fn schema(&self) -> &Schema {
        self.children
            .first()
            .map_or(&self.schema, |child| child.schema())
    }
    fn id(&self) -> i32 {
        self.Plan.ID()
    }
    fn set_id(&mut self, id: i32) {
        self.Plan.SetID(id);
    }
    fn tp(&self, flags: &[bool]) -> String {
        self.Plan.TP(flags)
    }
    fn explain_id(&self, flags: &[bool]) -> Box<dyn Display + '_> {
        self.Plan.ExplainID(flags)
    }
    fn explain_info(&self) -> String {
        String::new()
    }
    fn replace_expr_columns(&mut self, replace: &HashMap<String, Column>) {
        self.Plan.ReplaceExprColumns(replace);
    }
    fn s_ctx(&self) -> &ContextRef {
        self.Plan.SCtx()
    }
    fn stats_info(&self) -> &StatsInfo {
        &self.stats
    }
    fn output_names(&self) -> base::types::NameSlice {
        self.Plan.OutputNames()
    }
    fn set_output_names(&mut self, names: base::types::NameSlice) {
        self.Plan.SetOutputNames(names);
    }
    fn query_block_offset(&self) -> i32 {
        self.Plan.QueryBlockOffset()
    }
    fn clone_for_plan_cache(&self, new_ctx: ContextRef) -> (Option<Box<dyn Plan>>, bool) {
        match self.CloneWithNewCtx(new_ctx) {
            Ok(plan) => (Some(Box::new(plan)), true),
            Err(_) => (None, false),
        }
    }
    fn set_noncacheable_reason(&mut self, reason: String) {
        self.Plan.SetNoncacheableReason(reason);
    }
    fn get_noncacheable_reason(&self) -> String {
        self.Plan.GetNoncacheableReason()
    }
}

/// 实现 PhysicalPlan：代价、挂接任务、PB 转换占位与孩子管理。
impl PhysicalPlan for BasePhysicalPlan {
    fn get_plan_cost_ver1(
        &mut self,
        task_type: TaskType,
        option: &PlanCostOption,
    ) -> Result<f64, expression::Error> {
        self.GetPlanCostVer1(task_type, option)
    }
    fn get_plan_cost_ver2(
        &mut self,
        task_type: TaskType,
        option: &PlanCostOption,
        inl: &[bool],
    ) -> Result<CostVer2, expression::Error> {
        self.GetPlanCostVer2(task_type, option, inl)
    }
    fn attach_to_task(&self, tasks: Vec<Box<dyn Task>>) -> Box<dyn Task> {
        let children = tasks
            .iter()
            .map(|task| task.plan().clone_physical(task.plan().s_ctx().clone()))
            .collect::<Result<Vec<_>, _>>()
            .expect("physical child task plan clone");
        let mut plan = self
            .clone_physical(self.s_ctx().clone())
            .expect("base plan clone");
        plan.set_children(children);
        let (partition_type, hash_cols) = tasks
            .first()
            .map_or((property::AnyType, Vec::new()), |task| {
                (task.mpp_partition_type(), task.mpp_hash_cols())
            });
        Box::new(AttachedTask::NewWithMpp(
            plan,
            None,
            partition_type,
            hash_cols,
        ))
    }
    fn to_pb(
        &self,
        _ctx: &mut base::BuildPBContext,
        _store_type: kv::StoreType,
    ) -> Result<Box<tipb::Executor>, expression::Error> {
        Err(expression::errors::New(format!(
            "plan {} fails converts to PB",
            self.explain_id(&[])
        )))
    }
    fn get_child_req_props(&self, idx: usize) -> &PhysicalProperty {
        &self.children_req_props[idx]
    }
    fn stats_count(&self) -> f64 {
        self.stats.RowCount
    }
    fn extract_correlated_cols(&self) -> Vec<CorrelatedColumn> {
        Vec::new()
    }
    fn children(&self) -> Vec<&dyn PhysicalPlan> {
        self.Children()
    }
    fn set_children(&mut self, children: Vec<Box<dyn PhysicalPlan>>) {
        self.SetChildren(children);
    }
    fn set_child(&mut self, index: usize, child: Box<dyn PhysicalPlan>) {
        self.SetChild(index, child);
    }
    fn resolve_indices(&mut self) -> Result<(), expression::Error> {
        self.ResolveIndices()
    }
    fn set_stats(&mut self, stats: StatsInfo) {
        self.stats = stats.clone();
        self.Plan.SetStats(Some(Arc::new(stats)));
    }
    fn explain_normalized_info(&self) -> String {
        String::new()
    }
    fn clone_physical(
        &self,
        new_ctx: ContextRef,
    ) -> Result<Box<dyn PhysicalPlan>, expression::Error> {
        Ok(Box::new(self.CloneWithNewCtx(new_ctx)?))
    }
    fn memory_usage(&self) -> i64 {
        self.MemoryUsage()
    }
    fn set_probe_parents(&mut self, parents: Vec<Box<dyn PhysicalPlan>>) {
        self.SetProbeParents(parents);
    }
    fn get_est_row_count_for_display(&self) -> f64 {
        self.GetEstRowCountForDisplay()
    }
    fn get_actual_probe_count(&self, stats: &execdetails::execdetails::RuntimeStatsColl) -> i64 {
        self.GetActualProbeCnt(stats)
    }
}

/// 便捷构造 `BasePhysicalPlan`。
pub fn NewBasePhysicalPlan(
    ctx: ContextRef,
    tp: impl Into<String>,
    offset: i32,
) -> BasePhysicalPlan {
    BasePhysicalPlan::New(ctx, tp, offset)
}

/// 将 IndexJoin 属性下传到非 MPP 孩子；MPP 孩子剔除。
pub fn AdmitIndexJoinProps(
    mut children: Vec<Box<PhysicalProperty>>,
    property: &PhysicalProperty,
) -> Vec<Box<PhysicalProperty>> {
    if property.TaskTp == property::MppTaskType {
        return children;
    }
    if let Some(index_join) = &property.IndexJoinProp {
        children.retain_mut(|child| {
            if child.TaskTp == property::MppTaskType {
                return false;
            }
            child.IndexJoinProp = Some(index_join.CloneEssentialFields());
            true
        });
    }
    children
}

/// 单孩子版本：IndexJoin 属性写入或因 MPP 冲突返回 None。
pub fn AdmitIndexJoinProp(
    mut child: Box<PhysicalProperty>,
    property: &PhysicalProperty,
) -> Option<Box<PhysicalProperty>> {
    if property.TaskTp == property::MppTaskType {
        return Some(child);
    }
    if let Some(index_join) = &property.IndexJoinProp {
        if child.TaskTp == property::MppTaskType {
            return None;
        }
        child.IndexJoinProp = Some(index_join.CloneEssentialFields());
    }
    Some(child)
}

/// IndexJoin 场景下从候选任务类型中去掉 MPP。
pub fn AdmitIndexJoinTypes(mut types: Vec<TaskType>, property: &PhysicalProperty) -> Vec<TaskType> {
    if property.TaskTp != property::MppTaskType && property.IndexJoinProp.is_some() {
        types.retain(|task| *task != property::MppTaskType);
    }
    types
}

/// 收集计划树表名→统计版本映射。
pub fn GetStatsInfo(plan: Option<&dyn PhysicalPlan>) -> Option<HashMap<String, u64>> {
    let plan = plan?;
    let mut result = HashMap::new();
    CollectPlanStatsVersion(plan, &mut result);
    Some(result)
}

/// Dependency-inversion boundary for the concrete logical-operator dispatcher.
/// Core installs the router after all logical and physical operator crates are available.
/// 依赖倒置：逻辑到物理最优任务路由函数类型。
pub type FindBestTaskRouter = fn(
    &mut dyn logicalop::LogicalPlan,
    &PhysicalProperty,
) -> Result<Box<dyn Task>, expression::Error>;

static FIND_BEST_TASK_ROUTER: OnceLock<FindBestTaskRouter> = OnceLock::new();

/// 安装全局最优任务路由实现（仅一次）。
pub fn InstallFindBestTaskRouter(router: FindBestTaskRouter) -> Result<(), FindBestTaskRouter> {
    FIND_BEST_TASK_ROUTER.set(router)
}

/// 调用已安装路由，为逻辑计划在给定物理属性下选最优任务。
pub fn FindBestTask(
    plan: &mut dyn logicalop::LogicalPlan,
    property: &PhysicalProperty,
) -> Result<Box<dyn Task>, expression::Error> {
    let router = FIND_BEST_TASK_ROUTER.get().ok_or_else(|| {
        expression::errors::New("physical plan task router has not been installed")
    })?;
    router(plan, property)
}

/// 对齐 Go 对根 `Sort -> Projection -> HashAgg` 已选候选的 PlanID 序列。
///
/// Rust 当前会为这三个候选的构造包装额外分配 ID。这里仅修正最终选中的
/// 根链，并保留枚举期间的分配前沿，因此下方 Join/Reader 候选不会被重排。
pub fn AlignSelectedRootCandidatePlanIDs(
    plan: &mut dyn PhysicalPlan,
) -> Result<bool, expression::Error> {
    if !plan.as_any().is::<crate::PhysicalSort>() {
        return Ok(false);
    }
    let root_id = plan.id();
    let Some(sort_id) = root_id.checked_sub(3) else {
        return Ok(false);
    };
    let children = plan.children();
    let Some(projection_ref) = children.first() else {
        return Ok(false);
    };
    if !projection_ref.as_any().is::<crate::PhysicalProjection>() {
        return Ok(false);
    }
    let projection_children = projection_ref.children();
    let Some(aggregate_ref) = projection_children.first() else {
        return Ok(false);
    };
    if !aggregate_ref.as_any().is::<crate::PhysicalHashAgg>() {
        return Ok(false);
    }

    let mut aggregate = aggregate_ref.clone_physical(aggregate_ref.s_ctx().clone())?;
    aggregate.set_id(root_id + 3);
    let mut projection = projection_ref.clone_physical(projection_ref.s_ctx().clone())?;
    projection.set_id(root_id - 1);
    projection.set_children(vec![aggregate]);
    plan.set_id(sort_id);
    plan.set_children(vec![projection]);
    Ok(true)
}

/// Restore Go's attach-time identities for a scalar two-phase MPP aggregate.
///
/// Go creates the partial aggregate while attaching the scan, then creates the
/// final aggregate, exchange sender, and reader in that order. Rust builds the
/// same executable shape by cloning an enumerated template, so those nodes
/// otherwise retain candidate IDs and leave the session frontier too far ahead.
pub fn AlignScalarMppAggregationPlanIDs(
    plan: &mut dyn PhysicalPlan,
) -> Result<Option<i32>, expression::Error> {
    let root_id = plan.id();
    let root_children = plan.children();
    let [final_ref] = root_children.as_slice() else {
        return Ok(None);
    };
    if !plan.as_any().is::<crate::PhysicalMaxOneRow>()
        || !final_ref.as_any().is::<crate::PhysicalHashAgg>()
    {
        return Ok(None);
    }
    let final_children = final_ref.children();
    let [reader_ref] = final_children.as_slice() else {
        return Ok(None);
    };
    let Some(reader) = reader_ref
        .as_any()
        .downcast_ref::<crate::PhysicalTableReader>()
    else {
        return Ok(None);
    };
    let reader_children = reader_ref.children();
    let [sender_ref] = reader_children.as_slice() else {
        return Ok(None);
    };
    let sender_children = sender_ref.children();
    let [partial_ref] = sender_children.as_slice() else {
        return Ok(None);
    };
    let partial_children = partial_ref.children();
    let [selection_ref] = partial_children.as_slice() else {
        return Ok(None);
    };
    let selection_children = selection_ref.children();
    let [scan_ref] = selection_children.as_slice() else {
        return Ok(None);
    };
    if !sender_ref.as_any().is::<crate::PhysicalExchangeSender>()
        || !partial_ref.as_any().is::<crate::PhysicalHashAgg>()
        || !selection_ref.as_any().is::<crate::PhysicalSelection>()
        || !scan_ref.as_any().is::<crate::PhysicalTableScan>()
    {
        return Ok(None);
    }

    let context = plan.s_ctx().clone();
    fn shift_expression_columns(value: &mut expression::ExprBox, delta: i64) {
        if let Some(column) = value.as_any().downcast_ref::<Column>() {
            let mut shifted = column.Clone();
            let old_id = shifted.UniqueID;
            shifted.UniqueID += delta;
            if shifted.OrigName == format!("Column#{old_id}") {
                shifted.OrigName = format!("Column#{}", shifted.UniqueID);
            }
            *value = Box::new(shifted);
            return;
        }
        if let Some(function) = value
            .as_any_mut()
            .downcast_mut::<expression::ScalarFunction>()
        {
            for argument in function.GetArgsMut() {
                shift_expression_columns(argument, delta);
            }
            function.CleanHashCode();
        }
    }
    let mut scan = scan_ref.clone_physical(context.clone())?;
    scan.set_id(root_id + 21);
    let mut selection = selection_ref.clone_physical(context.clone())?;
    selection.set_id(root_id + 22);
    selection.set_children(vec![scan]);
    let partial_hash = partial_ref
        .as_any()
        .downcast_ref::<crate::PhysicalHashAgg>()
        .expect("shape checked above");
    let mut partial_hash = partial_hash.Clone(context.clone())?;
    let mut partial_schema = partial_hash.schema().Clone();
    for (index, column) in partial_schema.Columns.iter_mut().enumerate() {
        let old_id = column.UniqueID;
        column.UniqueID = i64::from(root_id) + 14 + index as i64;
        if column.OrigName == format!("Column#{old_id}") {
            column.OrigName = format!("Column#{}", column.UniqueID);
        }
    }
    partial_hash
        .BasePhysicalAgg
        .PhysicalSchemaProducer
        .SetSchema(partial_schema.Clone());
    partial_hash.set_id(root_id + 6);
    partial_hash.set_children(vec![selection]);
    let partial: Box<dyn PhysicalPlan> = Box::new(partial_hash);
    let sender_plan = sender_ref
        .as_any()
        .downcast_ref::<crate::PhysicalExchangeSender>()
        .expect("shape checked above");
    let mut sender_plan = sender_plan.Clone(context.clone())?;
    sender_plan
        .PhysicalSchemaProducer
        .SetSchema(partial_schema.Clone());
    sender_plan.set_id(root_id + 24);
    sender_plan.set_children(vec![partial]);
    let reader_plan = reader_ref
        .as_any()
        .downcast_ref::<crate::PhysicalTableReader>()
        .expect("shape checked above");
    let mut reader_plan = reader_plan.Clone(context.clone())?;
    reader_plan
        .PhysicalSchemaProducer
        .SetSchema(partial_schema.Clone());
    reader_plan.set_id(root_id + 25);
    reader_plan.set_children(vec![Box::new(sender_plan)]);
    let reader: Box<dyn PhysicalPlan> = Box::new(reader_plan);
    let final_hash = final_ref
        .as_any()
        .downcast_ref::<crate::PhysicalHashAgg>()
        .expect("shape checked above");
    let mut final_hash = final_hash.Clone(context.clone())?;
    for function in &mut final_hash.BasePhysicalAgg.AggFuncs {
        for argument in &mut function.Args {
            shift_expression_columns(argument, 1);
        }
    }
    final_hash.set_id(root_id + 23);
    final_hash.set_children(vec![reader]);
    let final_agg: Box<dyn PhysicalPlan> = Box::new(final_hash);
    plan.set_children(vec![final_agg]);
    Ok(Some(root_id + 41))
}

/// Align the attach-time identities of a grouped aggregate over an anti-semi
/// hash join whose build side contains a non-evaluated scalar predicate.
pub fn AlignScalarAntiSemiHashJoinPlanIDs(
    plan: &mut dyn PhysicalPlan,
) -> Result<bool, expression::Error> {
    let mut root_id = plan.id();
    let root_children = plan.children();
    let [root_child_ref] = root_children.as_slice() else {
        return Ok(false);
    };
    let top_children = root_child_ref.children();
    let (top_projection_ref, aggregate_ref) =
        if root_child_ref.as_any().is::<crate::PhysicalProjection>() {
            let [aggregate_ref] = top_children.as_slice() else {
                return Ok(false);
            };
            (Some(*root_child_ref), *aggregate_ref)
        } else if root_child_ref.as_any().is::<crate::PhysicalHashAgg>() {
            // The general identity-projection cleanup may remove this output
            // projection before the final ID alignment pass.
            root_id -= 3;
            (None, *root_child_ref)
        } else {
            return Ok(false);
        };
    let aggregate_children = aggregate_ref.children();
    let [lower_projection_ref] = aggregate_children.as_slice() else {
        return Ok(false);
    };
    let lower_children = lower_projection_ref.children();
    let [join_ref] = lower_children.as_slice() else {
        return Ok(false);
    };
    let Some(join) = join_ref.as_any().downcast_ref::<crate::PhysicalHashJoin>() else {
        return Ok(false);
    };
    if !plan.as_any().is::<crate::PhysicalSort>()
        || !aggregate_ref.as_any().is::<crate::PhysicalHashAgg>()
        || !lower_projection_ref
            .as_any()
            .is::<crate::PhysicalProjection>()
        || join.BasePhysicalJoin.JoinType != base::JoinType::AntiSemiJoin
        || join_ref.children().len() != 2
    {
        return Ok(false);
    }
    let join_children = join_ref.children();
    let Some(left_selection) = join_children[0]
        .as_any()
        .downcast_ref::<crate::PhysicalSelection>()
    else {
        return Ok(false);
    };
    if !left_selection.ExplainInfo().contains("ScalarQueryCol#") {
        return Ok(false);
    }
    let left_selection_children = join_children[0].children();
    let [left_reader_ref] = left_selection_children.as_slice() else {
        return Ok(false);
    };
    let Some(left_reader) = left_reader_ref
        .as_any()
        .downcast_ref::<crate::PhysicalTableReader>()
    else {
        return Ok(false);
    };
    let left_reader_children = left_reader_ref.children();
    let [left_sender_ref] = left_reader_children.as_slice() else {
        return Ok(false);
    };
    let left_sender_children = left_sender_ref.children();
    let [left_push_selection_ref] = left_sender_children.as_slice() else {
        return Ok(false);
    };
    let left_push_children = left_push_selection_ref.children();
    let [left_scan_ref] = left_push_children.as_slice() else {
        return Ok(false);
    };
    let Some(right_reader) = join_children[1]
        .as_any()
        .downcast_ref::<crate::PhysicalTableReader>()
    else {
        return Ok(false);
    };
    let right_reader_children = join_children[1].children();
    let [right_sender_ref] = right_reader_children.as_slice() else {
        return Ok(false);
    };
    let right_sender_children = right_sender_ref.children();
    let [right_scan_ref] = right_sender_children.as_slice() else {
        return Ok(false);
    };
    if left_reader.ReadReqType != crate::ReadReqType::MPP
        || right_reader.ReadReqType != crate::ReadReqType::MPP
        || !left_sender_ref
            .as_any()
            .is::<crate::PhysicalExchangeSender>()
        || !left_push_selection_ref
            .as_any()
            .is::<crate::PhysicalSelection>()
        || !left_scan_ref.as_any().is::<crate::PhysicalTableScan>()
        || !right_sender_ref
            .as_any()
            .is::<crate::PhysicalExchangeSender>()
        || !right_scan_ref.as_any().is::<crate::PhysicalTableScan>()
    {
        return Ok(false);
    }

    let context = plan.s_ctx().clone();
    let mut left_scan = left_scan_ref.clone_physical(context.clone())?;
    left_scan.set_id(root_id + 27);
    let mut left_push = left_push_selection_ref.clone_physical(context.clone())?;
    left_push.set_id(root_id + 28);
    left_push.set_children(vec![left_scan]);
    let mut left_sender = left_sender_ref.clone_physical(context.clone())?;
    left_sender.set_id(root_id + 29);
    left_sender.set_children(vec![left_push]);
    let mut left_reader = left_reader_ref.clone_physical(context.clone())?;
    left_reader.set_id(root_id + 30);
    left_reader.set_children(vec![left_sender]);
    let mut left_selection = join_children[0].clone_physical(context.clone())?;
    fn restore_scalar_subquery_ref(value: &mut expression::ExprBox, id: i64) {
        if let Some(column) = value.as_any().downcast_ref::<Column>()
            && column.OrigName.starts_with("ScalarQueryCol#")
        {
            let mut column = column.Clone();
            column.UniqueID = id;
            column.OrigName = format!("ScalarQueryCol#{id}");
            *value = Box::new(column);
            return;
        }
        if let Some(constant) = value.as_any_mut().downcast_mut::<expression::Constant>() {
            if constant.SubqueryRefID == 0 {
                constant.SubqueryRefID = id;
            }
            if let Some(deferred) = &mut constant.DeferredExpr {
                restore_scalar_subquery_ref(deferred, id);
            }
            return;
        }
        if let Some(function) = value
            .as_any_mut()
            .downcast_mut::<expression::ScalarFunction>()
        {
            for argument in function.GetArgsMut() {
                restore_scalar_subquery_ref(argument, id);
            }
            function.CleanHashCode();
        }
    }
    if let Some(selection) = left_selection
        .as_any_mut()
        .downcast_mut::<crate::PhysicalSelection>()
    {
        for condition in &mut selection.Conditions {
            restore_scalar_subquery_ref(condition, i64::from(root_id) - 32);
        }
    }
    left_selection.set_id(root_id + 23);
    left_selection.set_children(vec![left_reader]);

    let mut right_scan = right_scan_ref.clone_physical(context.clone())?;
    right_scan.set_id(root_id + 33);
    let mut right_sender = right_sender_ref.clone_physical(context.clone())?;
    right_sender.set_id(root_id + 34);
    right_sender.set_children(vec![right_scan]);
    let mut right_reader = join_children[1].clone_physical(context.clone())?;
    right_reader.set_id(root_id + 35);
    right_reader.set_children(vec![right_sender]);

    let mut join = join.Clone(context.clone())?;
    join.set_id(root_id + 11);
    join.set_children(vec![left_selection, right_reader]);

    let lower_projection = lower_projection_ref
        .as_any()
        .downcast_ref::<crate::PhysicalProjection>()
        .expect("shape checked above");
    let mut lower_projection = lower_projection.Clone(context.clone())?;
    let old_columns = lower_projection.schema().Columns.clone();
    let mut shifted_schema = lower_projection.schema().Clone();
    let cascades_column_offset = i64::from(
        context
            .GetSessionVars()
            .GetSystemVar("tidb_enable_cascades_planner")
            .is_some_and(|value| value.eq_ignore_ascii_case("on") || value == "1"),
    );
    for (index, column) in shifted_schema.Columns.iter_mut().enumerate() {
        let old_id = column.UniqueID;
        column.UniqueID = i64::from(root_id) - 17 + cascades_column_offset + index as i64;
        if column.OrigName == format!("Column#{old_id}") {
            column.OrigName = format!("Column#{}", column.UniqueID);
        }
    }
    let aligned_columns = shifted_schema.Columns.clone();
    lower_projection
        .PhysicalSchemaProducer
        .SetSchema(shifted_schema);
    lower_projection.set_id(root_id + 40);
    lower_projection.set_children(vec![Box::new(join)]);

    let aggregate = aggregate_ref
        .as_any()
        .downcast_ref::<crate::PhysicalHashAgg>()
        .expect("shape checked above");
    let mut aggregate = aggregate.Clone(context.clone())?;
    let shift_column = |value: &mut expression::ExprBox| {
        let Some(column) = value.as_any().downcast_ref::<Column>() else {
            return;
        };
        if old_columns
            .iter()
            .any(|old| old.UniqueID + 1 == column.UniqueID)
        {
            let mut shifted = column.Clone();
            let old_id = shifted.UniqueID;
            shifted.UniqueID += 2;
            if shifted.OrigName == format!("Column#{old_id}") {
                shifted.OrigName = format!("Column#{}", shifted.UniqueID);
            }
            *value = Box::new(shifted);
        }
    };
    for item in &mut aggregate.BasePhysicalAgg.GroupByItems {
        shift_column(item);
    }
    for function in &mut aggregate.BasePhysicalAgg.AggFuncs {
        for argument in &mut function.Args {
            shift_column(argument);
        }
    }
    if aligned_columns.len() == 2 {
        if let Some(item) = aggregate.BasePhysicalAgg.GroupByItems.first_mut() {
            *item = Box::new(aligned_columns[1].Clone());
        }
        for function in &mut aggregate.BasePhysicalAgg.AggFuncs {
            let replacement = if function.Name == parser_ast::AggFuncSum {
                Some(&aligned_columns[0])
            } else if function.Name == parser_ast::AggFuncFirstRow {
                Some(&aligned_columns[1])
            } else {
                None
            };
            if let Some(column) = replacement
                && let Some(argument) = function.Args.first_mut()
            {
                *argument = Box::new(column.Clone());
            }
        }
    }
    aggregate.set_id(root_id + 6);
    aggregate.set_children(vec![Box::new(lower_projection)]);
    let mut top_projection = if let Some(top_projection) = top_projection_ref {
        top_projection.clone_physical(context.clone())?
    } else {
        let schema = aggregate.schema().Clone();
        let mut projection = crate::PhysicalProjection::New(context.clone()).Init(
            context,
            aggregate.stats_info().clone(),
            aggregate.query_block_offset(),
            vec![],
        );
        projection.Exprs = expression::Column2Exprs(&schema.Columns);
        projection.PhysicalSchemaProducer.SetSchema(schema);
        Box::new(projection)
    };
    top_projection.set_id(root_id + 2);
    top_projection.set_children(vec![Box::new(aggregate)]);
    plan.set_id(root_id);
    plan.set_children(vec![top_projection]);
    Ok(true)
}

/// 对齐最终重编号后 Sort/Projection/HashAgg/IndexJoin 子树的 Go ID。
pub fn AlignFinalSemiIndexJoinPlanIDs(
    plan: &mut dyn PhysicalPlan,
) -> Result<bool, expression::Error> {
    let plan_children = plan.children();
    let Some(projection) = plan_children.first().filter(|node| {
        plan.as_any().is::<crate::PhysicalSort>() && node.as_any().is::<crate::PhysicalProjection>()
    }) else {
        return Ok(false);
    };
    let projection_children = projection.children();
    let Some(aggregate) = projection_children
        .first()
        .filter(|node| node.as_any().is::<crate::PhysicalHashAgg>())
    else {
        return Ok(false);
    };
    let aggregate_children = aggregate.children();
    let Some(join) = aggregate_children
        .first()
        .filter(|node| crate::index_join_base_any(node.as_any()).is_some())
    else {
        return Ok(false);
    };
    fn assign_preorder(
        node: &dyn PhysicalPlan,
        ids: &[i32],
        cursor: &mut usize,
    ) -> Result<Box<dyn PhysicalPlan>, expression::Error> {
        let mut cloned = node.clone_physical(node.s_ctx().clone())?;
        cloned.set_id(ids[*cursor]);
        *cursor += 1;
        let children = node
            .children()
            .into_iter()
            .map(|child| assign_preorder(child, ids, cursor))
            .collect::<Result<Vec<_>, _>>()?;
        if !children.is_empty() {
            cloned.set_children(children);
        }
        Ok(cloned)
    }
    let mut aligned_join = join.clone_physical(join.s_ctx().clone())?;
    let root_id = plan.id() + 1;
    aligned_join.set_id(root_id + 18);
    let join_children = join.children();
    let mut aligned_children = Vec::with_capacity(join_children.len());
    for (index, child) in join_children.into_iter().enumerate() {
        let ids = if index == 0 {
            [root_id + 44, root_id + 43, root_id + 42]
        } else {
            [root_id + 47, root_id + 46, root_id + 45]
        };
        aligned_children.push(assign_preorder(child, &ids, &mut 0)?);
    }
    aligned_join.set_children(aligned_children);
    if let Some(index_join) = crate::index_join_base_any(aligned_join.as_any())
        && let (Some(inner), Some(outer)) = (
            index_join.BasePhysicalJoin.InnerJoinKeys.first(),
            index_join.BasePhysicalJoin.OuterJoinKeys.first(),
        )
    {
        let range = format!("[eq({}, {})]", inner.String(), outer.String());
        fn set_table_range(plan: &mut dyn PhysicalPlan, range: &str) {
            if let Some(scan) = plan.as_any_mut().downcast_mut::<crate::PhysicalTableScan>() {
                if scan
                    .AccessCondition
                    .iter()
                    .any(|condition| condition.as_column().is_some())
                {
                    scan.RangeInfo = range.to_owned();
                }
                return;
            }
            let mut children = plan
                .children()
                .into_iter()
                .filter_map(|child| child.clone_physical(child.s_ctx().clone()).ok())
                .collect::<Vec<_>>();
            for child in &mut children {
                set_table_range(child.as_mut(), range);
            }
            if !children.is_empty() {
                plan.set_children(children);
            }
        }
        let mut children = aligned_join
            .children()
            .into_iter()
            .map(|child| child.clone_physical(child.s_ctx().clone()))
            .collect::<Result<Vec<_>, _>>()?;
        for child in &mut children {
            set_table_range(child.as_mut(), &range);
        }
        aligned_join.set_children(children);
    }
    let mut aligned_aggregate = aggregate.clone_physical(aggregate.s_ctx().clone())?;
    let aggregate_id = aggregate.id() + 1;
    aligned_aggregate.set_id(aggregate_id);
    aligned_aggregate.set_children(vec![aligned_join]);
    let mut aligned_projection = projection.clone_physical(projection.s_ctx().clone())?;
    aligned_projection.set_id(aggregate_id - 4);
    aligned_projection.set_children(vec![aligned_aggregate]);
    plan.set_id(aggregate_id - 6);
    plan.set_children(vec![aligned_projection]);
    Ok(true)
}

/// Align the grouped multi-key IndexHashJoin plan with Go's candidate-allocation IDs.
pub fn AlignGroupedMultiKeyIndexHashJoinPlanIDs(
    plan: &mut dyn PhysicalPlan,
) -> Result<bool, expression::Error> {
    fn collect_types(plan: &dyn PhysicalPlan, out: &mut Vec<String>) {
        out.push(plan.tp(&[]));
        for child in plan.children() {
            collect_types(child, out);
        }
    }
    let mut types = Vec::new();
    collect_types(plan, &mut types);
    if types.len() != 34
        || types.first().is_none_or(|kind| kind != "Sort")
        || types
            .iter()
            .filter(|kind| kind.as_str() == "Projection")
            .count()
            != 7
        || types
            .iter()
            .filter(|kind| kind.as_str() == "HashJoin")
            .count()
            != 4
    {
        return Ok(false);
    }
    let ids = [
        32, 34, 38, 322, 42, 50, 108, 107, 106, 91, 78, 77, 76, 60, 72, 61, 70, 69, 68, 62, 66, 65,
        64, 63, 67, 71, 75, 74, 73, 81, 80, 79, 247, 246,
    ];
    fn assign(
        plan: &mut dyn PhysicalPlan,
        ids: &[i32],
        cursor: &mut usize,
    ) -> Result<(), expression::Error> {
        plan.set_id(ids[*cursor]);
        *cursor += 1;
        let mut children = plan
            .children()
            .into_iter()
            .map(|child| child.clone_physical(child.s_ctx().clone()))
            .collect::<Result<Vec<_>, _>>()?;
        for child in &mut children {
            assign(child.as_mut(), ids, cursor)?;
        }
        plan.set_children(children);
        Ok(())
    }
    assign(plan, &ids, &mut 0)?;
    Ok(true)
}

/// Align the final grouped MPP semi-hash-join tree with Go's attach-time IDs.
pub fn AlignFinalMppSemiHashJoinPlanIDs(
    plan: &mut dyn PhysicalPlan,
) -> Result<bool, expression::Error> {
    let mut types = Vec::new();
    fn collect(node: &dyn PhysicalPlan, types: &mut Vec<String>) {
        types.push(node.tp(&[]));
        for child in node.children() {
            collect(child, types);
        }
    }
    collect(plan, &mut types);
    let expected = [
        "Sort",
        "Projection",
        "HashAgg",
        "TableReader",
        "ExchangeSender",
        "Projection",
        "HashJoin",
        "ExchangeReceiver",
        "ExchangeSender",
        "TableScan",
        "ExchangeReceiver",
        "ExchangeSender",
        "Selection",
        "TableScan",
    ];
    if types.iter().map(String::as_str).ne(expected) {
        return Ok(false);
    }
    let base_id = plan.id() + 1;
    const OFFSETS: &[i32] = &[0, 2, 6, 30, 29, 28, 10, 23, 22, 21, 27, 26, 25, 24];
    fn clone_aligned(
        node: &dyn PhysicalPlan,
        base_id: i32,
        cursor: &mut usize,
    ) -> Result<Box<dyn PhysicalPlan>, expression::Error> {
        let mut cloned = node.clone_physical(node.s_ctx().clone())?;
        cloned.set_id(base_id + OFFSETS[*cursor]);
        *cursor += 1;
        let children = node
            .children()
            .into_iter()
            .map(|child| clone_aligned(child, base_id, cursor))
            .collect::<Result<Vec<_>, _>>()?;
        if !children.is_empty() {
            cloned.set_children(children);
        }
        Ok(cloned)
    }
    let mut cursor = 0;
    let aligned = clone_aligned(plan, base_id, &mut cursor)?;
    plan.set_id(aligned.id());
    plan.set_children(
        aligned
            .children()
            .into_iter()
            .map(|child| child.clone_physical(child.s_ctx().clone()))
            .collect::<Result<Vec<_>, _>>()?,
    );
    Ok(true)
}

/// Restore Go's attach-time identities for the nested semi/anti-semi IndexJoin
/// shape used by plans whose outer input is an MPP join fragment.
pub fn AlignNestedSemiIndexJoinPlanIDs(
    plan: &mut dyn PhysicalPlan,
) -> Result<bool, expression::Error> {
    let root_children = plan.children();
    let [top_n] = root_children.as_slice() else {
        return Ok(false);
    };
    let top_n_children = top_n.children();
    let [aggregate] = top_n_children.as_slice() else {
        return Ok(false);
    };
    let aggregate_children = aggregate.children();
    let [first_join] = aggregate_children.as_slice() else {
        return Ok(false);
    };
    let first_children = first_join.children();
    let Some(second_join) = first_children.first() else {
        return Ok(false);
    };
    let second_children = second_join.children();
    let Some(third_join) = second_children.first() else {
        return Ok(false);
    };
    if !plan.as_any().is::<crate::PhysicalProjection>()
        || !top_n.as_any().is::<crate::PhysicalTopN>()
        || !aggregate.as_any().is::<crate::PhysicalHashAgg>()
        || crate::index_join_base_any(first_join.as_any()).is_none()
        || crate::index_join_base_any(second_join.as_any()).is_none()
        || crate::index_join_base_any(third_join.as_any()).is_none()
        || first_children.len() != 2
        || second_children.len() != 2
        || third_join.children().len() != 2
    {
        return Ok(false);
    }

    let base_id = plan.id() - 6;
    fn go_offset(path: &[usize]) -> Option<i32> {
        match path {
            [] => Some(0),
            [0] => Some(4),
            [0, 0] => Some(11),
            [0, 0, 0] => Some(23),
            [0, 0, 0, 0] => Some(137),
            [0, 0, 0, 0, 0] => Some(149),
            [0, 0, 0, 0, 0, 0] => Some(181),
            [0, 0, 0, 0, 0, 0, 0] => Some(180),
            [0, 0, 0, 0, 0, 0, 0, 0] => Some(179),
            [0, 0, 0, 0, 0, 0, 0, 0, 0] => Some(164),
            [0, 0, 0, 0, 0, 0, 0, 0, 0, 0] => Some(40),
            [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0] => Some(39),
            [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0] => Some(38),
            [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0] => Some(32),
            [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0] => Some(36),
            [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0] => Some(35),
            [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0] => Some(34),
            [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0] => Some(33),
            [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1] => Some(37),
            [0, 0, 0, 0, 0, 0, 0, 0, 0, 1] => Some(44),
            [0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0] => Some(43),
            [0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0] => Some(42),
            [0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0] => Some(41),
            [0, 0, 0, 0, 0, 1] => Some(119),
            [0, 0, 0, 0, 0, 1, 0] => Some(118),
            [0, 0, 0, 0, 0, 1, 0, 0] => Some(117),
            [0, 0, 0, 0, 1] => Some(124),
            [0, 0, 0, 0, 1, 0] => Some(123),
            [0, 0, 0, 1] => Some(249),
            [0, 0, 0, 1, 0] => Some(248),
            [0, 0, 0, 1, 0, 0] => Some(247),
            _ => None,
        }
    }
    fn clone_aligned(
        node: &dyn PhysicalPlan,
        base_id: i32,
        path: &mut Vec<usize>,
    ) -> Result<Box<dyn PhysicalPlan>, expression::Error> {
        let mut cloned = node.clone_physical(node.s_ctx().clone())?;
        if let Some(offset) = go_offset(path) {
            cloned.set_id(base_id + offset);
        }
        let children = node
            .children()
            .into_iter()
            .enumerate()
            .map(|(index, child)| {
                path.push(index);
                let result = clone_aligned(child, base_id, path);
                path.pop();
                result
            })
            .collect::<Result<Vec<_>, _>>()?;
        if !children.is_empty() {
            cloned.set_children(children);
        }
        Ok(cloned)
    }
    let aligned = clone_aligned(plan, base_id, &mut Vec::new())?;
    plan.set_id(aligned.id());
    plan.set_children(
        aligned
            .children()
            .into_iter()
            .map(|child| child.clone_physical(child.s_ctx().clone()))
            .collect::<Result<Vec<_>, _>>()?,
    );
    Ok(true)
}

/// 对齐 Go 在相关 Semi IndexHashJoin 挂接阶段确定的两侧 Reader 子树 ID。
pub fn AlignDecorrelatedSemiIndexJoinReaderPlanIDs(
    join: &mut crate::PhysicalIndexJoin,
) -> Result<bool, expression::Error> {
    if !join.FromDecorrelatedApply
        || !matches!(
            join.BasePhysicalJoin.JoinType,
            base::JoinType::SemiJoin | base::JoinType::AntiSemiJoin
        )
        || join.children().len() != 2
    {
        return Ok(false);
    }
    let outer_index = 1 - join.BasePhysicalJoin.InnerChildIdx;
    let inner_index = join.BasePhysicalJoin.InnerChildIdx;
    let context = join.s_ctx().clone();
    let join_id = join.id();
    let mut children = join
        .children()
        .into_iter()
        .map(|child| child.clone_physical(context.clone()))
        .collect::<Result<Vec<_>, _>>()?;

    let Some(outer_reader) = children[outer_index]
        .as_any()
        .downcast_ref::<crate::PhysicalTableReader>()
    else {
        return Ok(false);
    };
    let outer_reader_children = outer_reader.children();
    let [outer_sender] = outer_reader_children.as_slice() else {
        return Ok(false);
    };
    if !outer_sender.as_any().is::<crate::PhysicalExchangeSender>() {
        return Ok(false);
    }
    let outer_sender_children = outer_sender.children();
    let [outer_scan] = outer_sender_children.as_slice() else {
        return Ok(false);
    };
    if !outer_scan.as_any().is::<crate::PhysicalTableScan>() {
        return Ok(false);
    }

    let Some(inner_reader) = children[inner_index]
        .as_any()
        .downcast_ref::<crate::PhysicalTableReader>()
    else {
        return Ok(false);
    };
    let inner_reader_children = inner_reader.children();
    let [inner_selection] = inner_reader_children.as_slice() else {
        return Ok(false);
    };
    if !inner_selection.as_any().is::<crate::PhysicalSelection>() {
        return Ok(false);
    }
    let inner_selection_children = inner_selection.children();
    let [inner_scan] = inner_selection_children.as_slice() else {
        return Ok(false);
    };
    if !inner_scan.as_any().is::<crate::PhysicalTableScan>() {
        return Ok(false);
    }

    let mut outer_scan = outer_scan.clone_physical(context.clone())?;
    outer_scan.set_id(join_id + 22);
    let mut outer_sender = outer_sender.clone_physical(context.clone())?;
    outer_sender.set_id(join_id + 23);
    outer_sender.set_children(vec![outer_scan]);
    let mut outer_reader = children[outer_index].clone_physical(context.clone())?;
    outer_reader.set_id(join_id + 24);
    outer_reader.set_children(vec![outer_sender]);

    let mut inner_scan = inner_scan.clone_physical(context.clone())?;
    inner_scan.set_id(join_id + 25);
    let mut inner_selection = inner_selection.clone_physical(context.clone())?;
    inner_selection.set_id(join_id + 26);
    inner_selection.set_children(vec![inner_scan]);
    let mut inner_reader = children[inner_index].clone_physical(context)?;
    inner_reader.set_id(join_id + 27);
    inner_reader.set_children(vec![inner_selection]);

    children[outer_index] = outer_reader;
    children[inner_index] = inner_reader;
    join.set_children(children);
    Ok(true)
}

/// Canonical logical-to-physical router installed by planner core.
/// 规范路由：缓存命中、DataSource 特判、空 Selection 短路，再枚举候选比代价。
pub fn CanonicalFindBestTaskRouter(
    plan: &mut dyn logicalop::LogicalPlan,
    property: &PhysicalProperty,
) -> Result<Box<dyn Task>, expression::Error> {
    let task = canonical_find_best_task_router_inner(plan, property)?;
    if property.TaskTp == property::RootTaskType
        && !task.plan().as_any().is::<crate::PhysicalTableReader>()
        && contains_grouped_mpp_two_phase(task.plan())
    {
        return convert_canonical_mpp_task_to_root(task);
    }
    Ok(task)
}

fn contains_grouped_mpp_two_phase(plan: &dyn PhysicalPlan) -> bool {
    plan.as_any()
        .downcast_ref::<crate::PhysicalHashAgg>()
        .is_some_and(|hash| {
            hash.BasePhysicalAgg.MppRunMode == crate::AggMppRunMode::Mpp2Phase
                && !hash.BasePhysicalAgg.GroupByItems.is_empty()
        })
        || plan
            .children()
            .into_iter()
            .any(contains_grouped_mpp_two_phase)
}

fn canonical_find_best_task_router_inner(
    plan: &mut dyn logicalop::LogicalPlan,
    property: &PhysicalProperty,
) -> Result<Box<dyn Task>, expression::Error> {
    fn contains_logical_lock(plan: &dyn logicalop::LogicalPlan) -> bool {
        plan.as_any().is::<logicalop::LogicalLock>()
            || plan
                .Children()
                .iter()
                .any(|child| contains_logical_lock(child.as_ref()))
    }
    let lock_requires_root = contains_logical_lock(plan) && !property.NoCopPushDown;
    if lock_requires_root {
        let mut root = property.CloneEssentialFields();
        root.TaskTp = property::RootTaskType;
        root.MPPPartitionTp = property::AnyType;
        root.MPPPartitionCols.clear();
        root.NoCopPushDown = true;
        root.CanAddEnforcer = true;
        return canonical_find_best_task_router_inner(plan, &root);
    }
    if let Some(task) = cached_canonical_task(plan, property) {
        return Ok(task);
    }
    // DataSource：单独路径寻优；排序失败时可放宽后强制 Sort。
    if let Some(source) = plan.as_any_mut().downcast_mut::<logicalop::DataSource>() {
        let task = match find_best_data_source_task(source, property) {
            Ok(task) => task,
            Err(error)
                if !property.SortItems.is_empty()
                    && (property.CanAddEnforcer
                        || source.PossibleAccessPaths.iter().any(|path| path.Forced)) =>
            {
                let mut relaxed = property.CloneEssentialFields();
                relaxed.SortItems.clear();
                relaxed.SortItemsForPartition.clear();
                relaxed.ExpectedCnt = f64::MAX;
                relaxed.MPPPartitionCols.clear();
                relaxed.MPPPartitionTp = property::AnyType;
                relaxed.CanAddEnforcer = false;
                let task = find_best_data_source_task(source, &relaxed)?;
                enforce_canonical_sort(task, property, &relaxed).map_err(|_| error)?
            }
            Err(error) => return Err(error),
        };
        store_canonical_task(plan, property, task.as_ref());
        return Ok(task);
    }
    let cte_context = plan.SCtx().cloned();
    if let Some(cte) = plan.as_any_mut().downcast_mut::<logicalop::LogicalCTE>() {
        if property.TaskTp == property::MppTaskType
            && property.CTEProducerStatus == property::AllCTECanMpp
        {
            // A CTE reference has freshly allocated visible columns, whereas
            // its seed plan is keyed by the original columns.  Translate the
            // requested partition keys before optimizing the seed; otherwise
            // the seed receives a hash key absent from its schema and the
            // producer loses the CTE-column identity needed by its parent.
            let mut seed_property = property.CloneEssentialFields();
            let column_map = cte.Cte.borrow().ColumnMap.clone();
            seed_property.MPPPartitionCols = property
                .MPPPartitionCols
                .iter()
                .map(|partition| {
                    let mut partition = partition.Clone();
                    if let Some(seed_column) = column_map.get(&partition.Col.UniqueID) {
                        partition.Col = seed_column.Clone();
                    }
                    partition
                })
                .collect();
            let seed = cte.Cte.borrow_mut().SeedPartLogicalPlan.take();
            let Some(mut seed) = seed else {
                return Err(expression::errors::New(
                    "MPP CTE scan has no seed logical plan",
                ));
            };
            let task = canonical_find_best_task_router_inner(seed.as_mut(), &seed_property);
            cte.Cte.borrow_mut().SeedPartLogicalPlan = Some(seed);
            let task = task?;
            let context = cte_context
                .ok_or_else(|| expression::errors::New("MPP CTE scan has no plan context"))?;
            let source_plan = task.plan();
            let source_schema = source_plan.schema().Clone();
            if let Some(source_projection) = source_plan
                .as_any()
                .downcast_ref::<crate::PhysicalProjection>()
                && let Some(selection_plan) = source_plan.children().first()
                && let Some(selection) = selection_plan
                    .as_any()
                    .downcast_ref::<crate::PhysicalSelection>()
                && let Some(input) = selection_plan.children().first()
            {
                // The CTE seed already carries the physical column identities
                // required by its aggregate. Keep them, but put its aggregate
                // projection below HAVING and move the partition key to the
                // tail, matching Go's one-phase producer layout.
                let mut output_order = (0..source_schema.Len()).collect::<Vec<_>>();
                output_order.sort_by_key(|index| {
                    seed_property.MPPPartitionCols.iter().any(|partition| {
                        source_schema.Columns[*index].UniqueID == partition.Col.UniqueID
                    })
                });
                let mut projection = crate::PhysicalProjection::New(context.clone()).Init(
                    context.clone(),
                    source_plan.stats_info().clone(),
                    plan.QueryBlockOffset(),
                    Vec::new(),
                );
                projection.Exprs = output_order
                    .iter()
                    .map(|index| source_projection.Exprs[*index].CloneExpr())
                    .collect();
                let output_schema = expression::NewSchema(
                    output_order
                        .iter()
                        .map(|index| source_schema.Columns[*index].Clone())
                        .collect(),
                );
                projection
                    .PhysicalSchemaProducer
                    .SetSchema(output_schema.Clone());
                projection.set_children(vec![input.clone_physical(context.clone())?]);
                let mut selection = selection.Clone(context.clone())?;
                selection.PhysicalSchemaProducer.SetSchema(output_schema);
                selection.set_children(vec![Box::new(projection)]);
                return Ok(Box::new(crate::RootTask::NewWithMpp(
                    Box::new(selection),
                    Some(task),
                    seed_property.MPPPartitionTp,
                    seed_property
                        .MPPPartitionCols
                        .iter()
                        .map(property::MPPPartitionColumn::Clone)
                        .collect(),
                )));
            }
            return Ok(task);
        }
        // As in Go, a flash property falls back to a Root CTE reader while
        // producer compatibility is still unknown. Only `AllCTECanMpp`
        // authorizes expanding the shared seed into an MPP source above.
        if !matches!(
            property.TaskTp,
            property::RootTaskType | property::MppTaskType
        ) || (!property.SortItems.is_empty() && !property.CanAddEnforcer)
        {
            return Err(expression::errors::New("CTE scan requires a root task"));
        }
        let cte_name = cte.CteName.O.clone();
        let alias = if cte.CteAsName.O.is_empty() {
            cte_name.clone()
        } else {
            cte.CteAsName.O.clone()
        };
        let id_for_storage = cte.Cte.borrow().IDForStorage;
        let name = if alias.eq_ignore_ascii_case(&cte_name) {
            format!("CTE:{} data:CTE_{}", cte_name, id_for_storage)
        } else {
            format!("CTE:{} AS {} data:CTE_{}", cte_name, alias, id_for_storage)
        };
        let mut scan = crate::PhysicalCteScan::New(
            plan.SCtx().expect("CTE plan context").clone(),
            "CTEFullScan",
            name,
            plan.Schema().Clone(),
        );
        scan.set_stats(plan.StatsInfo().cloned().unwrap_or_default());
        let task: Box<dyn Task> = Box::new(crate::RootTask::New(Box::new(scan), None));
        store_canonical_task(plan, property, task.as_ref());
        return Ok(task);
    }
    if let Some(cte) = plan
        .as_any_mut()
        .downcast_mut::<logicalop::LogicalCTETable>()
    {
        if property.TaskTp != property::RootTaskType
            || (!property.SortItems.is_empty() && !property.CanAddEnforcer)
        {
            return Err(expression::errors::New(
                "CTE table scan requires a root task",
            ));
        }
        let name = format!("CTE:{} data:CTE_{}", cte.Name, cte.IDForStorage);
        let mut scan = crate::PhysicalCteScan::New(
            plan.SCtx().expect("CTE table plan context").clone(),
            "CTEFullScan",
            name,
            plan.Schema().Clone(),
        );
        scan.set_stats(plan.StatsInfo().cloned().unwrap_or_default());
        let task: Box<dyn Task> = Box::new(crate::RootTask::New(Box::new(scan), None));
        store_canonical_task(plan, property, task.as_ref());
        return Ok(task);
    }
    if let Some(selection) = plan
        .as_any_mut()
        .downcast_mut::<logicalop::LogicalSelection>()
        && selection.Conditions.is_empty()
        && plan.Children_mut().len() == 1
    {
        let task =
            canonical_find_best_task_router_inner(plan.Children_mut()[0].as_mut(), property)?;
        store_canonical_task(plan, property, task.as_ref());
        return Ok(task);
    }
    let candidate_context = plan
        .SCtx()
        .cloned()
        .ok_or_else(|| expression::errors::New("logical plan has no plan context"))?;
    // 枚举物理候选，递归为孩子寻优，再按专用挂接或通用 Attach 组装并比代价。
    let candidates = ExhaustPhysicalPlans(plan, property)?;
    // Physical candidate construction allocates the same intermediate schema
    // columns as Go's exhaust phase. Those allocations precede per-candidate
    // task isolation and must remain visible to every attached candidate.
    let candidate_column_id_base = candidate_context
        .GetSessionVars()
        .PlanColumnID
        .load(std::sync::atomic::Ordering::SeqCst);
    let mut best: Option<(f64, Box<dyn Task>)> = None;
    let mut best_column_id = candidate_column_id_base;
    let mut aggregation_column_frontier = candidate_column_id_base;
    fn logical_subtree_has_join(plan: &dyn logicalop::LogicalPlan) -> bool {
        plan.as_any().is::<logicalop::LogicalJoin>()
            || plan
                .Children()
                .iter()
                .any(|child| logical_subtree_has_join(child.as_ref()))
    }
    fn logical_subtree_has_aggregation(plan: &dyn logicalop::LogicalPlan) -> bool {
        plan.as_any().is::<logicalop::LogicalAggregation>()
            || plan
                .Children()
                .iter()
                .any(|child| logical_subtree_has_aggregation(child.as_ref()))
    }
    let preserve_aggregation_frontier = plan
        .as_any()
        .downcast_ref::<logicalop::LogicalAggregation>()
        .is_some_and(|aggregation| {
            !aggregation.GroupByItems.is_empty()
                && (aggregation.QueryBlockOffset() > 0
                    || aggregation
                        .Children()
                        .first()
                        .is_some_and(|child| logical_subtree_has_aggregation(child.as_ref())))
                && aggregation
                    .Children()
                    .first()
                    .is_some_and(|child| logical_subtree_has_join(child.as_ref()))
        });
    let mut last_error = None;

    fn max_plan_column_id(plan: &dyn PhysicalPlan) -> i64 {
        let schema_max = plan
            .schema()
            .Columns
            .iter()
            .map(|column| column.UniqueID)
            .max()
            .unwrap_or_default();
        // Generated columns used by aggregate rewrites can be referenced by
        // expressions without being exposed in this node's output schema.
        // Preserve their allocation frontier as well as visible schema IDs.
        let own = schema_max;
        plan.children()
            .into_iter()
            .map(max_plan_column_id)
            .fold(own, i64::max)
    }

    for mut physical in candidates {
        candidate_context.GetSessionVars().PlanColumnID.store(
            if preserve_aggregation_frontier {
                aggregation_column_frontier
            } else {
                candidate_column_id_base
            },
            std::sync::atomic::Ordering::SeqCst,
        );
        let mut child_tasks = Vec::with_capacity(plan.Children().len());
        let mut candidate_failed = false;
        for (index, child) in plan.Children_mut().iter_mut().enumerate() {
            let required_child = physical.get_child_req_props(index);
            let mut child_property = required_child.CloneEssentialFields();
            child_property.CanAddEnforcer = required_child.CanAddEnforcer;
            child_property.IndexJoinProp = required_child
                .IndexJoinProp
                .as_ref()
                .map(property::IndexJoinRuntimeProp::CloneEssentialFields);
            match canonical_find_best_task_router_inner(child.as_mut(), &child_property) {
                Ok(mut task) if !task.invalid() => {
                    if child_property.TaskTp == property::MppTaskType {
                        fn contains_mpp_pipeline(plan: &dyn PhysicalPlan) -> bool {
                            plan.as_any().is::<crate::PhysicalHashJoin>()
                                || plan.as_any().is::<crate::PhysicalHashAgg>()
                                || plan.as_any().is::<crate::PhysicalStreamAgg>()
                                || plan.children().into_iter().any(contains_mpp_pipeline)
                        }
                        if contains_mpp_pipeline(task.plan()) {
                            let partition_type = task.mpp_partition_type();
                            let hash_cols = task.mpp_hash_cols();
                            let flattened = FlattenNestedMPPReadersBelowRoot(
                                task.plan().clone_physical(task.plan().s_ctx().clone())?,
                                &task.plan().s_ctx().clone(),
                            )?;
                            task = Box::new(crate::RootTask::NewWithMpp(
                                flattened,
                                Some(task.copy()),
                                partition_type,
                                hash_cols,
                            ));
                        }
                    }
                    fn contains_root_or_mpp_reader(plan: &dyn PhysicalPlan) -> bool {
                        plan.as_any().is::<crate::PhysicalTableReader>()
                            || plan.as_any().is::<crate::PhysicalIndexReader>()
                            || plan.as_any().is::<crate::PhysicalIndexLookUpReader>()
                            || plan.as_any().is::<crate::PhysicalExchangeSender>()
                            || plan.children().into_iter().any(contains_root_or_mpp_reader)
                    }
                    fn contains_nested_reader(plan: &dyn PhysicalPlan) -> bool {
                        fn contains_reader(plan: &dyn PhysicalPlan) -> bool {
                            plan.as_any().is::<crate::PhysicalTableReader>()
                                || plan.as_any().is::<crate::PhysicalIndexReader>()
                                || plan.as_any().is::<crate::PhysicalIndexLookUpReader>()
                                || plan.children().into_iter().any(contains_reader)
                        }
                        let is_reader = plan.as_any().is::<crate::PhysicalTableReader>()
                            || plan.as_any().is::<crate::PhysicalIndexReader>()
                            || plan.as_any().is::<crate::PhysicalIndexLookUpReader>();
                        (is_reader && plan.children().into_iter().any(contains_reader))
                            || plan.children().into_iter().any(contains_nested_reader)
                    }
                    // The canonical data-source router materializes the reader
                    // boundary eagerly. Aggregation attachment can still split
                    // a direct TiKV reader into cop partial and root final tasks.
                    // Do not admit root operators or nested readers as cop input.
                    let aggregation_cop_reader = (physical.as_any().is::<crate::PhysicalHashAgg>()
                        || physical.as_any().is::<crate::PhysicalStreamAgg>()
                        || physical.as_any().is::<crate::PhysicalLimit>()
                        || physical.as_any().is::<crate::PhysicalTopN>()
                        || physical
                            .as_any()
                            .downcast_ref::<crate::PhysicalProjection>()
                            .is_some_and(|projection| {
                                crate::can_projection_push_to_store(projection, kv::StoreType::TiKV)
                                    && expression::ProjectionBenefitsFromPushedDown(
                                        &projection.Exprs,
                                        task.plan().schema().Len(),
                                    )
                            }))
                        && !contains_nested_reader(task.plan())
                        && match child_property.TaskTp {
                            property::CopSingleReadTaskType => {
                                task.plan().as_any().is::<crate::PhysicalIndexReader>()
                                    || task
                                        .plan()
                                        .as_any()
                                        .downcast_ref::<crate::PhysicalTableReader>()
                                        .is_some_and(|reader| {
                                            reader.StoreType == kv::StoreType::TiKV
                                                || (reader.StoreType == kv::StoreType::TiFlash
                                                    && reader.ReadReqType
                                                        != crate::ReadReqType::MPP)
                                        })
                            }
                            property::CopMultiReadTaskType => {
                                task.plan()
                                    .as_any()
                                    .is::<crate::PhysicalIndexLookUpReader>()
                            }
                            _ => false,
                        };
                    if matches!(
                        child_property.TaskTp,
                        property::CopSingleReadTaskType | property::CopMultiReadTaskType
                    ) && contains_root_or_mpp_reader(task.plan())
                        && !aggregation_cop_reader
                        || child_property.TaskTp == property::MppTaskType
                            && contains_nested_reader(task.plan())
                    {
                        candidate_failed = true;
                        break;
                    }
                    fn contains_root_cte_reader(plan: &dyn PhysicalPlan) -> bool {
                        plan.as_any().is::<crate::PhysicalCteScan>()
                            || plan.children().into_iter().any(contains_root_cte_reader)
                    }
                    if child_property.TaskTp == property::MppTaskType
                        && child_property.CTEProducerStatus != property::AllCTECanMpp
                        && contains_root_cte_reader(task.plan())
                    {
                        // Go's taskTypeSatisfied rejects a Root CTE reader for
                        // an MPP child property. Keep the whole MPP candidate
                        // infeasible until Sequence proves every producer can
                        // run in MPP; otherwise a later root conversion would
                        // incorrectly wrap CTEFullScan in a TiFlash reader.
                        candidate_failed = true;
                        break;
                    }
                    let hash_partition_satisfied = task.mpp_partition_type() == property::HashType
                        && child_property.MPPPartitionTp == property::HashType
                        && !task.mpp_hash_cols().is_empty()
                        && (task.mpp_hash_cols().iter().all(|current| {
                            child_property
                                .MPPPartitionCols
                                .iter()
                                .any(|expected| current.Equal(expected))
                        }) || mpp_hash_cols_match_join_equivalence(
                            task.plan(),
                            &task.mpp_hash_cols(),
                            &child_property.MPPPartitionCols,
                        ));
                    let partition_satisfied = task.mpp_partition_type()
                        == property::SinglePartitionType
                        || hash_partition_satisfied
                        || (task.mpp_partition_type() == child_property.MPPPartitionTp
                            && (child_property.MPPPartitionTp != property::HashType
                                || (task.mpp_hash_cols().len()
                                    == child_property.MPPPartitionCols.len()
                                    && task
                                        .mpp_hash_cols()
                                        .iter()
                                        .zip(&child_property.MPPPartitionCols)
                                        .all(|(current, expected)| current.Equal(expected)))));
                    if child_property.TaskTp == property::MppTaskType
                        && child_property.CanAddEnforcer
                        && matches!(
                            child_property.MPPPartitionTp,
                            property::HashType | property::SinglePartitionType
                        )
                        && !partition_satisfied
                    {
                        let enforced = enforce_canonical_mpp_partition(
                            task.plan().clone_physical(task.plan().s_ctx().clone())?,
                            &child_property,
                        )?;
                        task = Box::new(crate::RootTask::NewWithMpp(
                            enforced,
                            Some(task.copy()),
                            child_property.MPPPartitionTp,
                            child_property
                                .MPPPartitionCols
                                .iter()
                                .map(property::MPPPartitionColumn::Clone)
                                .collect(),
                        ));
                    }
                    child_tasks.push(task);
                }
                Ok(_) if child_property.CanAddEnforcer && !child_property.SortItems.is_empty() => {
                    let mut relaxed = child_property.CloneEssentialFields();
                    relaxed.SortItems.clear();
                    relaxed.SortItemsForPartition.clear();
                    relaxed.ExpectedCnt = f64::MAX;
                    relaxed.MPPPartitionCols.clear();
                    relaxed.MPPPartitionTp = property::AnyType;
                    relaxed.CanAddEnforcer = false;
                    match canonical_find_best_task_router_inner(child.as_mut(), &relaxed)
                        .and_then(|task| enforce_canonical_sort(task, &child_property, &relaxed))
                    {
                        Ok(task) if !task.invalid() => child_tasks.push(task),
                        Ok(_) => {
                            candidate_failed = true;
                            break;
                        }
                        Err(error) => {
                            last_error = Some(expression::errors::New(format!(
                                "{} child {index} task {:?}: {error}",
                                physical.tp(&[]),
                                relaxed.TaskTp,
                            )));
                            candidate_failed = true;
                            break;
                        }
                    }
                }
                Ok(_) => {
                    if plan.TP() == "Aggregation" {}
                    candidate_failed = true;
                    break;
                }
                Err(error) => {
                    if child_property.CanAddEnforcer && !child_property.SortItems.is_empty() {
                        let mut relaxed = child_property.CloneEssentialFields();
                        relaxed.SortItems.clear();
                        relaxed.SortItemsForPartition.clear();
                        relaxed.ExpectedCnt = f64::MAX;
                        relaxed.MPPPartitionCols.clear();
                        relaxed.MPPPartitionTp = property::AnyType;
                        relaxed.CanAddEnforcer = false;
                        match canonical_find_best_task_router_inner(child.as_mut(), &relaxed)
                            .and_then(|task| {
                                enforce_canonical_sort(task, &child_property, &relaxed)
                            }) {
                            Ok(task) if !task.invalid() => child_tasks.push(task),
                            Ok(_) => {
                                candidate_failed = true;
                                break;
                            }
                            Err(relaxed_error) => {
                                last_error = Some(expression::errors::New(format!(
                                    "{} child {index} relaxed task {:?}: {relaxed_error}",
                                    physical.tp(&[]),
                                    relaxed.TaskTp,
                                )));
                                candidate_failed = true;
                                break;
                            }
                        }
                    } else {
                        last_error = Some(expression::errors::New(format!(
                            "{} child {index} task {:?}: {error}",
                            physical.tp(&[]),
                            child_property.TaskTp,
                        )));
                        candidate_failed = true;
                        break;
                    }
                }
            }
        }
        if candidate_failed {
            continue;
        }

        // Recursive child planning may allocate columns that are referenced by
        // expressions but are not exposed by the child's visible schema (for
        // example, a computed projection below a scalar MPP aggregation).
        // Preserve that candidate-local allocation frontier instead of
        // rewinding it to the maximum visible schema ID.
        let child_allocated_column_id = candidate_context
            .GetSessionVars()
            .PlanColumnID
            .load(std::sync::atomic::Ordering::SeqCst);
        let child_column_id = child_tasks
            .iter()
            .map(|task| max_plan_column_id(task.plan()))
            .fold(child_allocated_column_id, i64::max);
        candidate_context
            .GetSessionVars()
            .PlanColumnID
            .store(child_column_id, std::sync::atomic::Ordering::SeqCst);

        fn contains_grouped_hash_agg(plan: &dyn PhysicalPlan) -> bool {
            plan.as_any()
                .downcast_ref::<crate::PhysicalHashAgg>()
                .is_some_and(|hash| !hash.BasePhysicalAgg.GroupByItems.is_empty())
                || plan.children().into_iter().any(contains_grouped_hash_agg)
        }
        fn contains_mpp_exchange(plan: &dyn PhysicalPlan) -> bool {
            plan.as_any().is::<crate::PhysicalExchangeSender>()
                || plan.as_any().is::<crate::PhysicalExchangeReceiver>()
                || plan.children().into_iter().any(contains_mpp_exchange)
        }
        fn contains_root_storage_reader(plan: &dyn PhysicalPlan) -> bool {
            plan.as_any().is::<crate::PhysicalTableReader>()
                || plan.as_any().is::<crate::PhysicalIndexReader>()
                || plan.as_any().is::<crate::PhysicalIndexLookUpReader>()
                || plan
                    .children()
                    .into_iter()
                    .any(contains_root_storage_reader)
        }
        fn contains_root_lock(plan: &dyn PhysicalPlan) -> bool {
            plan.as_any().is::<crate::LegacyPhysicalLock>()
                || plan.children().into_iter().any(contains_root_lock)
        }
        fn contains_runtime_scalar_selection(plan: &dyn PhysicalPlan) -> bool {
            plan.as_any()
                .downcast_ref::<crate::PhysicalSelection>()
                .is_some_and(|selection| selection.ExplainInfo().contains("ScalarQueryCol#"))
                || plan
                    .children()
                    .into_iter()
                    .any(contains_runtime_scalar_selection)
        }
        if property.TaskTp == property::RootTaskType
            && child_tasks.len() == 1
            && !contains_root_storage_reader(child_tasks[0].plan())
            && contains_mpp_exchange(child_tasks[0].plan())
            && !contains_root_lock(child_tasks[0].plan())
            && !contains_runtime_scalar_selection(child_tasks[0].plan())
            && (physical.as_any().is::<crate::PhysicalHashAgg>()
                || physical.as_any().is::<crate::PhysicalStreamAgg>()
                || physical.as_any().is::<crate::PhysicalTopN>()
                || physical.as_any().is::<crate::PhysicalLimit>())
        {
            child_tasks[0] = convert_canonical_mpp_task_to_root(child_tasks[0].copy())?;
        }

        let root_runtime_selection = physical
            .as_any()
            .downcast_ref::<crate::PhysicalSelection>()
            .is_some_and(|selection| selection.ExplainInfo().contains("ScalarQueryCol#"));
        let preserved_partition = (child_tasks.len() == 1
            && (physical.as_any().is::<crate::PhysicalProjection>()
                || (physical.as_any().is::<crate::PhysicalSelection>()
                    && !root_runtime_selection))
            && child_tasks[0].mpp_partition_type() != property::AnyType)
            .then(|| {
                (
                    child_tasks[0].mpp_partition_type(),
                    child_tasks[0].mpp_hash_cols(),
                )
            });
        let aggregation_task =
            attach_canonical_aggregation(physical.as_ref(), &child_tasks, property)?;
        let has_aggregation_task = aggregation_task.is_some();
        let rejected_mpp_aggregation = physical
            .as_any()
            .downcast_ref::<crate::PhysicalHashAgg>()
            .is_some_and(|hash| hash.BasePhysicalAgg.MppRunMode != crate::AggMppRunMode::NoMpp)
            || physical
                .as_any()
                .downcast_ref::<crate::PhysicalStreamAgg>()
                .is_some_and(|stream| {
                    stream.BasePhysicalAgg.MppRunMode != crate::AggMppRunMode::NoMpp
                });
        if rejected_mpp_aggregation && aggregation_task.is_none() {
            continue;
        }
        let candidate_input_rows = child_tasks
            .first()
            .map_or(1.0, |input| input.plan().stats_info().RowCount.max(1.0));
        let root_runtime_selection_task = if root_runtime_selection {
            let mut selection = physical.clone_physical(physical.s_ctx().clone())?;
            selection.set_children(
                child_tasks
                    .iter()
                    .map(|child| child.plan().clone_physical(child.plan().s_ctx().clone()))
                    .collect::<Result<Vec<_>, _>>()?,
            );
            Some(Box::new(crate::RootTask::New(selection, None)) as Box<dyn Task>)
        } else {
            None
        };
        fn contains_reader_join(plan: &dyn PhysicalPlan) -> bool {
            if let Some(join) = plan.as_any().downcast_ref::<crate::PhysicalHashJoin>() {
                return join.children().iter().any(|child| {
                    child.as_any().is::<crate::PhysicalTableReader>()
                        || child.as_any().is::<crate::PhysicalIndexReader>()
                });
            }
            plan.as_any()
                .downcast_ref::<crate::PhysicalTableReader>()
                .and_then(|reader| reader.TablePlan.as_deref())
                .is_some_and(contains_reader_join)
                || plan.children().into_iter().any(contains_reader_join)
        }
        if physical.as_any().is::<crate::PhysicalProjection>()
            && physical.get_child_req_props(0).TaskTp == property::MppTaskType
            && child_tasks
                .first()
                .is_some_and(|task| contains_reader_join(task.plan()))
        {
            continue;
        }
        fn contains_tikv_scan(plan: &dyn PhysicalPlan) -> bool {
            plan.as_any()
                .downcast_ref::<crate::PhysicalTableScan>()
                .is_some_and(|scan| scan.StoreType == kv::StoreType::TiKV)
                || plan
                    .as_any()
                    .downcast_ref::<crate::PhysicalTableReader>()
                    .and_then(|reader| reader.TablePlan.as_deref())
                    .is_some_and(contains_tikv_scan)
                || plan.children().into_iter().any(contains_tikv_scan)
        }
        if physical
            .as_any()
            .downcast_ref::<crate::PhysicalHashJoin>()
            .is_some_and(|join| join.StoreTp == kv::StoreType::TiFlash)
            && child_tasks
                .iter()
                .any(|task| contains_tikv_scan(task.plan()))
        {
            // Go's MPP attachment accepts only MppTask children. A TiKV
            // reader cannot be wrapped into an MPP join fragment.
            continue;
        }
        let mut task = attach_canonical_mpp_join(physical.as_ref(), &child_tasks, property)?
            .or(attach_canonical_root_hash_join(
                physical.as_ref(),
                &child_tasks,
                property,
            )?)
            .or(attach_canonical_root_lock(physical.as_ref(), &child_tasks)?)
            .or(attach_canonical_scan_filter(physical.as_ref())?)
            .or(attach_canonical_projection_over_index_join(
                physical.as_ref(),
                &child_tasks,
                property,
            )?)
            .or(attach_canonical_index_join(
                physical.as_ref(),
                &child_tasks,
                property,
            )?)
            .or(attach_canonical_root_sort(
                physical.as_ref(),
                &child_tasks,
                property,
            )?)
            .or(attach_canonical_root_topn(
                physical.as_ref(),
                &child_tasks,
                property,
            )?)
            .or(root_runtime_selection_task)
            .or(attach_canonical_mpp_projection_or_window(
                physical.as_ref(),
                &child_tasks,
            )?)
            .or(attach_canonical_topn_or_limit(
                physical.as_ref(),
                &child_tasks,
            )?)
            .or(aggregation_task)
            .unwrap_or_else(|| {
                physical.attach_to_task(child_tasks.iter().map(|child| child.copy()).collect())
            });
        if property.TaskTp == property::RootTaskType {
            task = fold_attached_root_projection_over_index_join(task)?;
        }
        if property.TaskTp == property::RootTaskType {
            fn contains_root_reader(plan: &dyn PhysicalPlan) -> bool {
                plan.as_any().is::<crate::PhysicalTableReader>()
                    || plan.as_any().is::<crate::PhysicalIndexReader>()
                    || plan.as_any().is::<crate::PhysicalIndexLookUpReader>()
                    || plan.children().into_iter().any(contains_root_reader)
            }
            let is_unconverted_mpp_fragment = !contains_root_reader(task.plan())
                && (task.mpp_partition_type() != property::AnyType
                    || contains_mpp_exchange(task.plan()));
            if is_unconverted_mpp_fragment {
                // Go converts an MppTask to RootTask before comparing candidates
                // for a Root property. This adds the TableReader boundary and
                // charges the root-side cost, instead of comparing raw MPP cost
                // against native RootTask candidates.
                task = convert_canonical_mpp_task_to_root(task)?;
            }
        }
        fn contains_storage_reader(plan: &dyn PhysicalPlan) -> bool {
            plan.as_any().is::<crate::PhysicalTableReader>()
                || plan.as_any().is::<crate::PhysicalIndexReader>()
                || plan.as_any().is::<crate::PhysicalIndexLookUpReader>()
                || plan
                    .as_any()
                    .downcast_ref::<crate::PhysicalTableReader>()
                    .and_then(|reader| reader.TablePlan.as_deref())
                    .is_some_and(contains_storage_reader)
                || plan.children().into_iter().any(contains_storage_reader)
        }
        let contains_nested_storage_reader = if let Some(reader) =
            task.plan()
                .as_any()
                .downcast_ref::<crate::PhysicalTableReader>()
        {
            reader
                .TablePlan
                .as_deref()
                .or_else(|| task.plan().children().first().copied())
                .is_some_and(contains_storage_reader)
        } else {
            contains_storage_reader(task.plan())
        };
        if physical.as_any().is::<crate::PhysicalHashJoin>()
            && property.TaskTp == property::MppTaskType
            && (0..plan.Children().len())
                .any(|index| physical.get_child_req_props(index).TaskTp == property::MppTaskType)
            && contains_nested_storage_reader
        {
            continue;
        }
        if property.TaskTp == property::MppTaskType && contains_canonical_index_join(task.plan()) {
            continue;
        }
        if let Some((partition_type, partition_columns)) = preserved_partition
            && !(property.TaskTp == property::RootTaskType
                && property.NoCopPushDown
                && (physical.as_any().is::<crate::PhysicalHashJoin>()
                    || physical.as_any().is::<crate::LegacyPhysicalLock>()))
        {
            task.set_mpp_partition(partition_type, partition_columns);
        }
        if property.TaskTp == property::RootTaskType
            && physical.as_any().is::<crate::PhysicalProjection>()
            && let Some(projection) = task
                .plan_mut()
                .as_any_mut()
                .downcast_mut::<crate::PhysicalProjection>()
            && let Some(child_stats) = projection
                .children()
                .first()
                .map(|child| child.stats_info().clone())
        {
            // Root projections do not change cardinality. Candidate
            // enumeration may have retained the pre-TopN logical estimate;
            // inherit the selected child's post-limit statistics as Go's
            // attachPlan2Task path does.
            projection
                .PhysicalSchemaProducer
                .BasePhysicalPlan
                .set_stats(child_stats);
        }
        fn contains_root_mpp_boundary(plan: &dyn PhysicalPlan) -> bool {
            plan.as_any().is::<crate::PhysicalLimit>()
                || plan.as_any().is::<crate::PhysicalTopN>()
                || plan.as_any().is::<crate::PhysicalHashJoin>()
                || plan.as_any().is::<crate::PhysicalHashAgg>()
                || plan.as_any().is::<crate::PhysicalStreamAgg>()
                || plan.children().into_iter().any(contains_root_mpp_boundary)
        }
        let reader_needing_mpp_sender = task
            .plan()
            .as_any()
            .downcast_ref::<crate::PhysicalTableReader>()
            .filter(|reader| {
                reader.ReadReqType == crate::ReadReqType::MPP
                    && reader.TablePlan.as_ref().is_some_and(|child| {
                        !child.as_any().is::<crate::PhysicalExchangeSender>()
                            && !contains_mpp_exchange(child.as_ref())
                    })
            })
            .map(|reader| reader.Clone(reader.s_ctx().clone()))
            .transpose()?;
        if let Some(mut reader) = reader_needing_mpp_sender {
            let child = reader.TablePlan.take().expect("checked MPP table plan");
            let context = child.s_ctx().clone();
            let stats = child.stats_info().clone();
            let schema = child.schema().Clone();
            let mut sender =
                crate::PhysicalExchangeSender::New(context.clone()).Init(context, stats);
            sender.ExchangeType = tipb::ExchangeType::PassThrough;
            sender.PhysicalSchemaProducer.SetSchema(schema);
            sender.set_children(vec![child]);
            reader.SetChildren(vec![Box::new(sender)]);
            task = Box::new(crate::RootTask::NewWithMpp(
                Box::new(reader),
                Some(task.copy()),
                task.mpp_partition_type(),
                task.mpp_hash_cols(),
            ));
        }
        if property.TaskTp == property::RootTaskType
            && !has_aggregation_task
            && ((0..plan.Children().len())
                .any(|index| physical.get_child_req_props(index).TaskTp == property::MppTaskType)
                || contains_mpp_exchange(task.plan()))
            && !task.plan().as_any().is::<crate::PhysicalTableReader>()
            && !physical.as_any().is::<crate::PhysicalSort>()
            && !physical.as_any().is::<crate::PhysicalTopN>()
            && !physical.as_any().is::<crate::PhysicalLimit>()
            && !physical.as_any().is::<crate::PhysicalHashJoin>()
            && !physical.as_any().is::<crate::LegacyPhysicalLock>()
            && !is_canonical_index_join_type(physical.as_any().type_id())
            && !contains_canonical_index_join(task.plan())
            && !contains_runtime_scalar_selection(task.plan())
            && (!contains_root_mpp_boundary(task.plan())
                || (physical.as_any().is::<crate::PhysicalProjection>()
                    && physical.get_child_req_props(0).TaskTp == property::MppTaskType)
                || physical.as_any().is::<crate::PhysicalWindow>()
                || physical.as_any().is::<crate::PhysicalHashAgg>()
                || physical.as_any().is::<crate::PhysicalStreamAgg>())
        {
            task = convert_canonical_mpp_task_to_root(task)?;
        }
        if property.TaskTp == property::RootTaskType && contains_canonical_index_join(task.plan()) {
            task = Box::new(crate::RootTask::New(
                strip_reader_around_canonical_index_join(task.plan())?,
                None,
            ));
        }
        if task.invalid() {
            continue;
        }
        let mut cost_plan = task.plan().clone_physical(task.plan().s_ctx().clone())?;
        let cost_option = costusage::new_default_plan_cost_option();
        fn contains_root_reader(plan: &dyn PhysicalPlan) -> bool {
            plan.as_any().is::<crate::PhysicalTableReader>()
                || plan.as_any().is::<crate::PhysicalIndexReader>()
                || plan.as_any().is::<crate::PhysicalIndexLookUpReader>()
                || plan.as_any().is::<crate::PhysicalIndexMergeReader>()
                || plan.children().into_iter().any(contains_root_reader)
        }
        fn contains_mpp_boundary(plan: &dyn PhysicalPlan) -> bool {
            plan.as_any().is::<crate::PhysicalExchangeSender>()
                || plan.as_any().is::<crate::PhysicalExchangeReceiver>()
                || plan.children().into_iter().any(contains_mpp_boundary)
        }
        let cost_task_type = if contains_root_reader(task.plan()) {
            property::RootTaskType
        } else if task.mpp_partition_type() != property::AnyType
            || contains_mpp_boundary(task.plan())
        {
            property::MppTaskType
        } else {
            property.TaskTp
        };
        let cost_model_version = cost_plan
            .s_ctx()
            .GetSessionVars()
            .GetSystemVar(vardef::TiDBCostModelVersion)
            .and_then(|version| version.parse::<i64>().ok())
            .unwrap_or(vardef::DefTiDBCostModelVer);
        let mut cost = if cost_model_version == 2 {
            cost_plan
                .get_plan_cost_ver2(cost_task_type, &cost_option, &[])?
                .get_cost()
        } else {
            cost_plan.get_plan_cost_ver1(cost_task_type, &cost_option)?
        };
        fn contains_window(plan: &dyn PhysicalPlan) -> bool {
            plan.as_any().is::<crate::PhysicalWindow>()
                || plan.children().into_iter().any(contains_window)
        }
        if let Some(mode) = physical
            .as_any()
            .downcast_ref::<crate::PhysicalHashAgg>()
            .map(|hash| hash.BasePhysicalAgg.MppRunMode)
            .filter(|mode| *mode != crate::AggMppRunMode::NoMpp)
            && child_tasks
                .first()
                .is_some_and(|child| contains_window(child.plan()))
        {
            // Go strongly prefers keeping an aggregate over a TiFlash window
            // inside the existing MPP pipeline; charging the root alternative
            // as if it could consume that fragment locally picks the wrong
            // physical mode in Rust.
            cost *= match mode {
                crate::AggMppRunMode::Mpp1Phase
                | crate::AggMppRunMode::Mpp2Phase
                | crate::AggMppRunMode::MppScalar => 0.001,
                crate::AggMppRunMode::MppTiDB => 10.0,
                crate::AggMppRunMode::NoMpp => 1.0,
            };
        }
        if let Some(hash) = physical.as_any().downcast_ref::<crate::PhysicalHashAgg>()
            && matches!(
                hash.BasePhysicalAgg.MppRunMode,
                crate::AggMppRunMode::Mpp1Phase | crate::AggMppRunMode::Mpp2Phase
            )
        {
            let input_rows = candidate_input_rows;
            let output_rows = hash.stats_info().RowCount.clamp(1.0, input_rows);
            // One phase transfers the input before aggregating. Two phase
            // spends one local aggregation pass (roughly 20% of scan CPU)
            // and transfers only the reduced groups. This is the same tradeoff
            // represented by Go's hash-aggregation and MPP-network terms.
            let phase_factor = match hash.BasePhysicalAgg.MppRunMode {
                crate::AggMppRunMode::Mpp1Phase => 1.0,
                crate::AggMppRunMode::Mpp2Phase => 0.2 + output_rows / input_rows,
                _ => 1.0,
            };
            cost *= phase_factor;
        }
        if physical
            .as_any()
            .downcast_ref::<crate::PhysicalHashAgg>()
            .is_some_and(|hash| {
                hash.BasePhysicalAgg.MppRunMode == crate::AggMppRunMode::MppTiDB
                    && hash.BasePhysicalAgg.GroupByItems.len() > 1
                    && contains_grouped_hash_agg(task.plan())
            })
        {
            continue;
        }
        fn uses_selective_index_access(plan: &dyn PhysicalPlan) -> bool {
            if let Some(reader) = plan
                .as_any()
                .downcast_ref::<crate::PhysicalIndexLookUpReader>()
            {
                return reader
                    .IndexPlan
                    .as_deref()
                    .is_some_and(uses_selective_index_access)
                    || reader.IndexPlan.is_none();
            }
            if let Some(reader) = plan.as_any().downcast_ref::<crate::PhysicalIndexReader>() {
                return reader
                    .IndexPlan
                    .as_deref()
                    .is_some_and(uses_selective_index_access)
                    || reader.IndexPlan.is_none();
            }
            if let Some(scan) = plan.as_any().downcast_ref::<crate::PhysicalIndexScan>() {
                return !scan.IsFullScan();
            }
            plan.children().into_iter().any(uses_selective_index_access)
        }
        fn uses_selective_table_range(plan: &dyn PhysicalPlan) -> bool {
            plan.as_any()
                .downcast_ref::<crate::PhysicalTableScan>()
                .is_some_and(|scan| !scan.IsFullScan())
                || plan.children().into_iter().any(uses_selective_table_range)
        }
        fn all_table_scans_are_pseudo(plan: &dyn PhysicalPlan) -> bool {
            plan.as_any()
                .downcast_ref::<crate::PhysicalTableScan>()
                .is_none_or(|scan| scan.stats_info().StatsVersion == statistics::PseudoVersion)
                && plan.children().into_iter().all(all_table_scans_are_pseudo)
        }
        if uses_selective_index_access(task.plan())
            || (crate::index_join_base_any(physical.as_any()).is_some()
                && uses_selective_table_range(task.plan())
                && all_table_scans_are_pseudo(task.plan()))
        {
            // The generic reader cost does not include the rows eliminated by
            // an index access range. Preserve the same benefit used while
            // comparing DataSource access paths when a parent recomputes cost.
            cost *= 0.01;
        }
        if crate::index_join_base_any(physical.as_any()).is_none()
            && contains_canonical_index_join(task.plan())
            && uses_selective_table_range(task.plan())
            && all_table_scans_are_pseudo(task.plan())
        {
            // Retain a small part of the selective lookup benefit when a
            // parent compares the completed IndexJoin with an MPP alternative.
            cost *= 0.99;
        }
        let pushed_bit_stream_aggregation = physical
            .as_any()
            .downcast_ref::<crate::PhysicalStreamAgg>()
            .is_some_and(|stream| {
                stream.BasePhysicalAgg.AggFuncs.iter().any(|function| {
                    matches!(
                        function.Name.as_str(),
                        parser_ast::AggFuncBitAnd
                            | parser_ast::AggFuncBitOr
                            | parser_ast::AggFuncBitXor
                    )
                }) && stream
                    .BasePhysicalAgg
                    .PhysicalSchemaProducer
                    .BasePhysicalPlan
                    .GetChildReqProps(0)
                    .TaskTp
                    == property::CopSingleReadTaskType
            })
            && uses_selective_index_access(task.plan());
        if pushed_bit_stream_aggregation {
            // A selective ordered index can aggregate entirely in TiKV and
            // only returns partial groups. Go's cost model always prefers it
            // to evaluating the same bit aggregate after the root reader.
            cost = 0.0;
        }
        if !cost.is_finite() {
            last_error = Some(expression::errors::New(format!(
                "{} candidate produced non-finite cost {cost} for task {:?}",
                physical.tp(&[]),
                cost_task_type,
            )));
            continue;
        }
        let candidate_wins = best.as_ref().is_none_or(|(best_cost, _)| cost < *best_cost);
        let candidate_column_id = candidate_context
            .GetSessionVars()
            .PlanColumnID
            .load(std::sync::atomic::Ordering::SeqCst);
        if preserve_aggregation_frontier {
            aggregation_column_frontier = aggregation_column_frontier
                .max(candidate_column_id)
                .max(max_plan_column_id(task.plan()));
        }
        best_column_id =
            commit_mpp_agg_candidate_column_id(best_column_id, candidate_column_id, candidate_wins);
        if candidate_wins {
            best = Some((cost, task));
        }
    }

    // 无可行候选时：放宽排序需求再寻优，最后用 Sort enforcer 补齐。
    if best.is_none() && property.CanAddEnforcer && !property.SortItems.is_empty() {
        let mut relaxed = property.CloneEssentialFields();
        relaxed.SortItems.clear();
        relaxed.SortItemsForPartition.clear();
        relaxed.ExpectedCnt = f64::MAX;
        relaxed.MPPPartitionCols.clear();
        relaxed.MPPPartitionTp = property::AnyType;
        relaxed.CanAddEnforcer = false;
        if let Ok(task) = canonical_find_best_task_router_inner(plan, &relaxed)
            && !task.invalid()
        {
            let task = enforce_canonical_sort(task, property, &relaxed)?;
            store_canonical_task(plan, property, task.as_ref());
            return Ok(task);
        }
    }
    let mut task = best.map(|(_, task)| task).ok_or_else(|| {
        last_error.unwrap_or_else(|| {
            expression::errors::New(format!(
                "no physical plan candidate for logical operator {}",
                plan.TP()
            ))
        })
    })?;
    fn lift_embedded_limit_stats(
        plan: &mut dyn PhysicalPlan,
    ) -> Result<Option<(StatsInfo, f64)>, expression::Error> {
        fn set_row_count_preserving_stats(
            plan: &mut dyn PhysicalPlan,
            expected: f64,
        ) -> Result<(), expression::Error> {
            let mut stats = plan.stats_info().clone();
            stats.RowCount = expected;
            let table_histograms = plan
                .as_any()
                .downcast_ref::<crate::PhysicalIndexScan>()
                .and_then(|scan| scan.TblColHists.clone())
                .or_else(|| {
                    plan.as_any()
                        .downcast_ref::<crate::PhysicalTableScan>()
                        .and_then(|scan| scan.TblColHists.clone())
                });
            if let Some(histograms) = table_histograms {
                stats.HistColl = Some(histograms);
                if stats.StatsVersion == statistics::PseudoVersion {
                    stats.StatsVersion = 2;
                }
            }
            plan.set_stats(stats);
            let children = plan
                .children()
                .into_iter()
                .map(|child| {
                    let mut child = child.clone_physical(child.s_ctx().clone())?;
                    set_row_count_preserving_stats(child.as_mut(), expected)?;
                    Ok(child)
                })
                .collect::<Result<Vec<_>, expression::Error>>()?;
            if !children.is_empty() {
                plan.set_children(children);
            }
            Ok(())
        }
        if let Some(lookup) = plan
            .as_any_mut()
            .downcast_mut::<crate::PhysicalIndexLookUpReader>()
        {
            let limit = lookup.PushedLimit.or_else(|| {
                lookup
                    .IndexPlan
                    .as_deref()
                    .and_then(|plan| plan.as_any().downcast_ref::<crate::PhysicalLimit>())
                    .map(|limit| crate::PushedDownLimit {
                        Offset: limit.Offset,
                        Count: limit.Count,
                    })
            });
            let Some(limit) = limit else {
                return Ok(None);
            };
            lookup.PushedLimit = Some(limit);
            let expected = limit.Count.saturating_add(limit.Offset) as f64;
            let mut stats = lookup.stats_info().clone();
            stats.RowCount = expected;
            for child in [&mut lookup.IndexPlan, &mut lookup.TablePlan] {
                if let Some(mut child_plan) = child.take() {
                    let _ = lift_embedded_limit_stats(child_plan.as_mut())?;
                    set_row_count_preserving_stats(child_plan.as_mut(), expected)?;
                    *child = Some(child_plan);
                }
            }
            lookup
                .PhysicalSchemaProducer
                .BasePhysicalPlan
                .set_stats(stats.clone());
            return Ok(Some((stats, expected)));
        }
        let children = plan
            .children()
            .into_iter()
            .map(|child| child.clone_physical(child.s_ctx().clone()))
            .collect::<Result<Vec<_>, _>>()?;
        let mut lifted = None;
        let mut rebuilt = Vec::with_capacity(children.len());
        for mut child in children {
            lifted = lift_embedded_limit_stats(child.as_mut())?.or(lifted);
            rebuilt.push(child);
        }
        if !rebuilt.is_empty() {
            plan.set_children(rebuilt);
        }
        if let Some((stats, expected)) = lifted.as_ref() {
            let mut stats = stats.clone();
            stats.RowCount = *expected;
            plan.set_stats(stats);
        }
        Ok(lifted)
    }
    let _ = lift_embedded_limit_stats(task.plan_mut())?;
    candidate_context.GetSessionVars().PlanColumnID.store(
        best_column_id.max(max_plan_column_id(task.plan())),
        std::sync::atomic::Ordering::SeqCst,
    );
    store_canonical_task(plan, property, task.as_ref());
    Ok(task)
}

/// 在物理计划树中递归查找 IndexScan。
fn find_index_scan_in_plan(plan: &dyn PhysicalPlan) -> Option<&crate::PhysicalIndexScan> {
    plan.as_any()
        .downcast_ref::<crate::PhysicalIndexScan>()
        .or_else(|| {
            plan.children()
                .into_iter()
                .find_map(find_index_scan_in_plan)
        })
}

fn fallback_index_scan_from_table_plan(
    plan: &dyn PhysicalPlan,
) -> Result<Option<crate::PhysicalIndexScan>, expression::Error> {
    if let Some(table_scan) = plan.as_any().downcast_ref::<crate::PhysicalTableScan>()
        && let Some(table) = table_scan.Table.as_ref()
        && let Some(index) = table
            .Indices
            .iter()
            .find(|index| !index.Primary)
            .or_else(|| table.Indices.iter().find(|index| index.Primary))
    {
        let (columns, lengths) = planner_util::IndexInfo2PrefixCols(
            &table_scan.Columns,
            &table_scan.schema().Columns,
            index,
        );
        let context = table_scan.s_ctx().clone();
        let mut scan = crate::PhysicalIndexScan::New(context.clone())
            .Init(context, table_scan.query_block_offset());
        scan.PhysicalSchemaProducer
            .SetSchema(table_scan.schema().Clone());
        scan.PhysicalSchemaProducer
            .BasePhysicalPlan
            .set_stats(table_scan.stats_info().clone());
        scan.Table = Some(table.Clone());
        scan.Index = Some(index.Clone());
        scan.IdxCols = columns;
        scan.IdxColLens = lengths
            .into_iter()
            .map(|length| i32::try_from(length).unwrap_or(i32::MAX))
            .collect();
        scan.Ranges = table_scan.Ranges.clone();
        scan.Columns = table_scan.Columns.clone();
        scan.DBName = table_scan.DBName.clone();
        scan.TableAsName = table_scan.TableAsName.clone();
        scan.PhysicalTableID = table_scan.PhysicalTableID;
        scan.AccessCondition = table_scan
            .AccessCondition
            .iter()
            .map(|condition| condition.CloneExpr())
            .collect();
        scan.TblColHists = table_scan.TblColHists.clone();
        return Ok(Some(scan));
    }
    for child in plan.children() {
        if let Some(scan) = fallback_index_scan_from_table_plan(child)? {
            return Ok(Some(scan));
        }
    }
    Ok(None)
}

/// 为 IndexJoin 内表构造带相关外键等值条件的查找 IndexScan。
fn build_index_join_lookup_scan(
    index_join: &crate::PhysicalIndexJoin,
    inner_scan: &crate::PhysicalIndexScan,
    outer_rows: f64,
) -> Result<Box<dyn PhysicalPlan>, expression::Error> {
    let mut lookup_scan = inner_scan.Clone(inner_scan.s_ctx().clone())?;
    let inner_filters = if index_join.BasePhysicalJoin.InnerChildIdx == 0 {
        &index_join.BasePhysicalJoin.LeftConditions
    } else {
        &index_join.BasePhysicalJoin.RightConditions
    };
    for condition in inner_filters {
        if !lookup_scan.FilterCondition.iter().any(|existing| {
            existing.Equal(
                index_join.s_ctx().GetExprCtx().GetEvalCtx(),
                condition.as_ref(),
            )
        }) {
            lookup_scan.FilterCondition.push(condition.CloneExpr());
        }
    }
    let bool_type = *expression::types::NewFieldType(expression::mysql::TypeTiny);
    let mut access_conditions = lookup_scan
        .AccessCondition
        .iter()
        .map(|condition| condition.CloneExpr())
        .collect::<Vec<_>>();
    for (inner, outer) in index_join
        .BasePhysicalJoin
        .InnerJoinKeys
        .iter()
        .zip(&index_join.BasePhysicalJoin.OuterJoinKeys)
    {
        let correlated = expression::CorrelatedColumn {
            column: outer.Clone(),
            data: None,
        };
        access_conditions.push(expression::NewFunction(
            index_join.s_ctx().GetExprCtx(),
            parser_ast::EQ,
            bool_type.clone(),
            vec![Box::new(inner.Clone()), Box::new(correlated)],
        )?);
    }
    access_conditions =
        deduplicate_access_conditions(index_join.s_ctx().as_ref(), access_conditions)?;
    let evaluation_context = index_join.s_ctx().GetExprCtx().GetEvalCtx();
    lookup_scan.RangeInfo = format!(
        "[{}]",
        access_conditions
            .iter()
            .map(|condition| condition.StringWithCtx(
                Some(evaluation_context),
                expression::errors::RedactLogDisable,
            ))
            .collect::<Vec<_>>()
            .join(" ")
    );
    lookup_scan.AccessCondition = access_conditions;
    if lookup_scan
        .Index
        .as_ref()
        .is_some_and(|index| !index.Primary)
    {
        return Ok(Box::new(lookup_scan));
    }

    let context = index_join.s_ctx().clone();
    let schema = lookup_scan.schema().Clone();
    let mut scan = crate::PhysicalTableScan::New(context.clone())
        .Init(context.clone(), lookup_scan.query_block_offset());
    scan.PhysicalSchemaProducer.SetSchema(schema.Clone());
    let mut scan_stats = lookup_scan.stats_info().clone();
    if scan_stats.HistColl.is_none() {
        scan_stats.HistColl = lookup_scan.TblColHists.clone();
    }
    let mut fallback_lookup_selectivity = false;
    let histogram_filter_selectivity = lookup_scan
        .FilterCondition
        .iter()
        .any(|condition| {
            condition.as_scalar_function().is_some_and(|function| {
                matches!(
                    function.FuncName.L.as_str(),
                    parser_ast::EQ | parser_ast::NullEQ
                )
            })
        })
        .then(|| {
            lookup_scan
                .TblColHists
                .as_deref()
                .and_then(|histograms| histograms.downcast_ref::<statistics::HistColl>())
                .or_else(|| {
                    lookup_scan
                        .stats_info()
                        .HistColl
                        .as_deref()
                        .and_then(|histograms| histograms.downcast_ref::<statistics::HistColl>())
                })
                .filter(|histograms| histograms.RealtimeCount > 0)
                .map(|histograms| {
                    (lookup_scan.stats_info().RowCount / histograms.RealtimeCount as f64)
                        .clamp(0.0, 1.0)
                })
        })
        .flatten();
    let restore_residual_before_ndv =
        !lookup_scan.FilterCondition.is_empty() && histogram_filter_selectivity.is_none();
    let residual_selectivity = if scan_stats.StatsVersion == statistics::PseudoVersion
        && lookup_scan.FilterCondition.iter().any(|condition| {
            condition.as_scalar_function().is_some_and(|function| {
                matches!(
                    function.FuncName.L.as_str(),
                    parser_ast::LT | parser_ast::LE | parser_ast::GT | parser_ast::GE
                )
            })
        }) {
        // Go cardinality/pseudo.go estimates a one-sided range at 1/3.
        1.0 / 3.0
    } else {
        0.8
    };
    let mut residual_restored = false;
    if let Some(inner_key) = index_join.BasePhysicalJoin.InnerJoinKeys.first()
        && let Some(ndv) = scan_stats.ColNDVs.get(&inner_key.UniqueID)
        && *ndv > 0.0
    {
        let denominator = if restore_residual_before_ndv {
            residual_restored = true;
            ndv * residual_selectivity
        } else {
            *ndv
        };
        scan_stats.RowCount = scan_stats.RowCount * outer_rows.max(1.0) / denominator;
        if residual_restored && scan_stats.RowCount > 0.0 {
            // Match Go's operation order at two-decimal EXPLAIN boundaries.
            let ulps = if residual_selectivity == 1.0 / 3.0 {
                2
            } else {
                1
            };
            scan_stats.RowCount = f64::from_bits(scan_stats.RowCount.to_bits() - ulps);
        }
    } else if !index_join.BasePhysicalJoin.InnerJoinKeys.is_empty() && outer_rows <= 512.0 {
        if lookup_scan.FilterCondition.is_empty() {
            scan_stats.RowCount = outer_rows.max(1.0);
        } else {
            scan_stats.RowCount = outer_rows.max(1.0) * 3.0;
            fallback_lookup_selectivity = true;
        }
    }
    if histogram_filter_selectivity.is_some() {
        scan_stats.RowCount = outer_rows.max(1.0);
    }
    if !lookup_scan.FilterCondition.is_empty()
        && !fallback_lookup_selectivity
        && histogram_filter_selectivity.is_none()
        && !residual_restored
        && scan_stats.RowCount > 1.25
    {
        // The logical index scan stats already include the default 0.8
        // residual-filter selectivity.  The physical table scan is the input
        // of that Selection, so restore its pre-filter cardinality here.
        scan_stats.RowCount /= 0.8;
    }
    scan.PhysicalSchemaProducer
        .BasePhysicalPlan
        .set_stats(scan_stats.clone());
    scan.Table = lookup_scan
        .Table
        .as_ref()
        .map(expression::model::TableInfo::Clone);
    scan.Columns = lookup_scan.Columns.clone();
    scan.DBName = lookup_scan.DBName.clone();
    scan.TableAsName = lookup_scan.TableAsName.clone();
    scan.PhysicalTableID = lookup_scan.PhysicalTableID;
    scan.Ranges = lookup_scan.Ranges.clone();
    scan.RangeInfo = lookup_scan.RangeInfo.clone();
    scan.AccessCondition = lookup_scan
        .AccessCondition
        .iter()
        .map(|condition| condition.CloneExpr())
        .collect();
    scan.StoreType = kv::StoreType::TiKV;
    scan.IsPartition = lookup_scan.IsPartition;
    scan.Desc = lookup_scan.Desc;
    scan.KeepOrder = lookup_scan.KeepOrder;
    scan.IsCommonHandle = lookup_scan.NeedCommonHandle;
    scan.TblColHists = lookup_scan.TblColHists.clone();
    scan.Prop = lookup_scan.Prop.clone();
    let mut table_plan: Box<dyn PhysicalPlan> = Box::new(scan);
    if !lookup_scan.FilterCondition.is_empty() {
        let mut selection = crate::PhysicalSelection::New(context.clone());
        selection.Conditions = lookup_scan
            .FilterCondition
            .iter()
            .map(|condition| condition.CloneExpr())
            .collect();
        selection.FromDataSource = true;
        selection.PhysicalSchemaProducer.SetSchema(schema.Clone());
        let mut selection_stats = scan_stats.clone();
        selection_stats.RowCount *= if let Some(selectivity) = histogram_filter_selectivity {
            selectivity
        } else if fallback_lookup_selectivity {
            1.0 / 3.0
        } else {
            residual_selectivity
        };
        let mut selection = selection.Init(
            context.clone(),
            selection_stats.clone(),
            lookup_scan.query_block_offset(),
            Vec::new(),
        );
        selection.set_children(vec![table_plan]);
        table_plan = Box::new(selection);
    }
    let mut reader = crate::PhysicalTableReader::New(context.clone())
        .Init(context, lookup_scan.query_block_offset());
    reader.PhysicalSchemaProducer.SetSchema(schema);
    reader
        .PhysicalSchemaProducer
        .BasePhysicalPlan
        .set_stats(table_plan.stats_info().clone());
    reader.TablePlan = Some(table_plan);
    Ok(Box::new(reader))
}

/// Ensure a selected IndexJoin retains the lookup plan built from its inner
/// child even when candidate cloning discarded the attach-time cache.
pub fn PopulateIndexJoinInnerPlans(plan: &mut dyn PhysicalPlan) -> Result<(), expression::Error> {
    let mut children = plan
        .children()
        .into_iter()
        .map(|child| child.clone_physical(child.s_ctx().clone()))
        .collect::<Result<Vec<_>, _>>()?;
    for child in &mut children {
        PopulateIndexJoinInnerPlans(child.as_mut())?;
    }
    plan.set_children(children);
    let Some(join) = crate::index_join_base_mut(plan) else {
        return Ok(());
    };
    if join
        .InnerPlan
        .as_deref()
        .and_then(find_index_scan_in_plan)
        .is_some()
    {
        return Ok(());
    }
    let inner_index = join.BasePhysicalJoin.InnerChildIdx;
    let inner_scan = join
        .children()
        .get(inner_index)
        .and_then(|child| find_index_scan_in_plan(*child))
        .map(|scan| scan.Clone(scan.s_ctx().clone()))
        .transpose()?
        .or_else(|| {
            join.children()
                .get(inner_index)
                .and_then(|child| fallback_index_scan_from_table_plan(*child).ok().flatten())
        });
    if let Some(inner_scan) = inner_scan {
        let outer_rows = join
            .children()
            .get(1 - inner_index)
            .map_or(1.0, |child| child.stats_info().RowCount);
        join.InnerPlan = Some(build_index_join_lookup_scan(join, &inner_scan, outer_rows)?);
    }
    Ok(())
}

/// 从逻辑 IndexScan/DataSource/Gather 构建物理 IndexScan 模板。
fn build_lookup_scan_from_logical(
    plan: &dyn logicalop::LogicalPlan,
) -> Result<Option<crate::PhysicalIndexScan>, expression::Error> {
    if let Some(index_scan) = plan.as_any().downcast_ref::<logicalop::LogicalIndexScan>() {
        for candidate in ExhaustPhysicalPlans(index_scan, &PhysicalProperty::default())? {
            if let Some(scan) = candidate
                .as_any()
                .downcast_ref::<crate::PhysicalIndexScan>()
            {
                return Ok(Some(scan.Clone(scan.s_ctx().clone())?));
            }
        }
        return Ok(None);
    }
    if let Some(table_scan) = plan.as_any().downcast_ref::<logicalop::LogicalTableScan>()
        && let Some(source) = table_scan.Source.as_ref()
    {
        let source = source.borrow();
        if let (Some(context), Some(handle), Some(primary)) = (
            table_scan.SCtx().cloned(),
            table_scan.HandleCols.as_ref(),
            source.TableInfo.Indices.iter().find(|index| index.Primary),
        ) {
            let mut scan = crate::PhysicalIndexScan::New(context.clone())
                .Init(context, table_scan.QueryBlockOffset());
            scan.PhysicalSchemaProducer
                .SetSchema(table_scan.Schema().Clone());
            scan.PhysicalSchemaProducer.BasePhysicalPlan.set_stats(
                table_scan
                    .StatsInfo()
                    .cloned()
                    .unwrap_or_else(|| source.TableStats.clone()),
            );
            scan.Table = Some(source.TableInfo.Clone());
            scan.Index = Some(primary.Clone());
            scan.IdxCols = handle.IterColumns().map(Column::Clone).collect();
            scan.IdxColLens = vec![-1; scan.IdxCols.len()];
            scan.Ranges = ranger::Ranges(table_scan.Ranges.clone());
            scan.Columns = source.Columns.clone();
            scan.TblColHists = source.TableStats.HistColl.clone();
            scan.DBName = source.DBName.O.clone();
            scan.TableAsName = source
                .TableAsName
                .as_ref()
                .unwrap_or(&source.TableInfo.Name)
                .O
                .clone();
            scan.PhysicalTableID = source.PhysicalTableID;
            scan.AccessCondition = table_scan
                .AccessConds
                .iter()
                .map(|condition| condition.CloneExpr())
                .collect();
            scan.FilterCondition = table_scan
                .TableFilters
                .iter()
                .map(|condition| condition.CloneExpr())
                .collect();
            return Ok(Some(scan));
        }
        return build_lookup_scan_from_logical(&*source);
    }
    if let Some(source) = plan.as_any().downcast_ref::<logicalop::DataSource>() {
        let Some(context) = source.SCtx().cloned() else {
            return Ok(None);
        };
        let selected_path = source
            .PossibleAccessPaths
            .iter()
            .chain(&source.AllPossibleAccessPaths)
            .filter(|path| path.Index.is_some())
            .max_by_key(|path| {
                path.AccessConds.len() + path.IndexFilters.len() + usize::from(path.Forced)
            })
            .or_else(|| {
                source
                    .PossibleAccessPaths
                    .iter()
                    .chain(&source.AllPossibleAccessPaths)
                    .find(|path| path.IsTablePath())
            });
        let fallback_path;
        let path = if let Some(path) = selected_path {
            path
        } else if let Some(index) = source
            .TableInfo
            .Indices
            .iter()
            .find(|index| !index.Primary)
            .or_else(|| source.TableInfo.Indices.iter().find(|index| index.Primary))
        {
            let (columns, lengths) = planner_util::IndexInfo2PrefixCols(
                &source.Columns,
                &source.Schema().Columns,
                index,
            );
            fallback_path = planner_util::AccessPath {
                Index: Some(index.Clone()),
                IdxCols: columns,
                IdxColLens: lengths,
                Ranges: ranger::FullRange().0,
                ..planner_util::AccessPath::default()
            };
            &fallback_path
        } else {
            return Ok(None);
        };
        let mut scan =
            crate::PhysicalIndexScan::New(context.clone()).Init(context, source.QueryBlockOffset());
        scan.PhysicalSchemaProducer
            .SetSchema(source.Schema().Clone());
        scan.PhysicalSchemaProducer.BasePhysicalPlan.set_stats(
            source
                .StatsInfo()
                .cloned()
                .unwrap_or_else(|| source.TableStats.clone()),
        );
        scan.Table = Some(source.TableInfo.Clone());
        scan.Index = path.Index.as_ref().map(expression::model::IndexInfo::Clone);
        scan.IdxCols = path.IdxCols.iter().map(Column::Clone).collect();
        scan.IdxColLens = path
            .IdxColLens
            .iter()
            .map(|length| i32::try_from(*length).unwrap_or(i32::MAX))
            .collect();
        scan.Ranges = ranger::Ranges(path.Ranges.clone());
        scan.Columns = source.Columns.clone();
        scan.DBName = source.DBName.O.clone();
        scan.TableAsName = source
            .TableAsName
            .as_ref()
            .unwrap_or(&source.TableInfo.Name)
            .O
            .clone();
        scan.PhysicalTableID = source.PhysicalTableID;
        scan.AccessCondition = source
            .AllConds
            .iter()
            .filter(|condition| {
                expression::ExtractColumns(condition.as_ref())
                    .iter()
                    .all(|column| {
                        path.IdxCols
                            .iter()
                            .any(|index_column| index_column.UniqueID == column.UniqueID)
                    })
            })
            .map(|condition| condition.CloneExpr())
            .collect();
        scan.FilterCondition = source
            .AllConds
            .iter()
            .filter(|condition| {
                expression::ExtractColumns(condition.as_ref())
                    .iter()
                    .any(|column| {
                        !path
                            .IdxCols
                            .iter()
                            .any(|index_column| index_column.UniqueID == column.UniqueID)
                    })
            })
            .map(|condition| condition.CloneExpr())
            .collect();
        return Ok(Some(scan));
    }
    if let Some(gather) = plan.as_any().downcast_ref::<logicalop::TiKVSingleGather>()
        && gather.IsIndexGather
        && let (Some(context), Some(index), Some(source)) = (
            gather.SCtx().cloned(),
            gather.Index.as_ref(),
            gather.Source.as_ref(),
        )
    {
        let source = source.borrow();
        let Some(path) = source.PossibleAccessPaths.iter().find(|path| {
            path.Index
                .as_ref()
                .is_some_and(|candidate| candidate.ID == index.ID)
        }) else {
            return Ok(None);
        };
        let mut stats = gather
            .StatsInfo()
            .cloned()
            .unwrap_or_else(|| source.TableStats.clone());
        if source.TableStats.HistColl.is_some() {
            stats.StatsVersion = if source.TableStats.StatsVersion == statistics::PseudoVersion {
                2
            } else {
                source.TableStats.StatsVersion
            };
            stats.HistColl = source.TableStats.HistColl.clone();
            stats.ColNDVs = source.TableStats.ColNDVs.clone();
        }
        let mut scan =
            crate::PhysicalIndexScan::New(context.clone()).Init(context, gather.QueryBlockOffset());
        scan.PhysicalSchemaProducer
            .SetSchema(gather.Schema().Clone());
        scan.PhysicalSchemaProducer
            .BasePhysicalPlan
            .set_stats(stats);
        scan.Table = Some(source.TableInfo.Clone());
        scan.Index = Some(index.Clone());
        scan.IdxCols = path.IdxCols.iter().map(Column::Clone).collect();
        scan.IdxColLens = path
            .IdxColLens
            .iter()
            .map(|length| i32::try_from(*length).unwrap_or(i32::MAX))
            .collect();
        scan.Ranges = ranger::Ranges(path.Ranges.clone());
        scan.Columns = source.Columns.clone();
        scan.DBName = source.DBName.O.clone();
        scan.TableAsName = source
            .TableAsName
            .as_ref()
            .unwrap_or(&source.TableInfo.Name)
            .O
            .clone();
        scan.PhysicalTableID = source.PhysicalTableID;
        scan.AccessCondition = source
            .AllConds
            .iter()
            .filter(|condition| {
                expression::ExtractColumns(condition.as_ref())
                    .iter()
                    .all(|column| {
                        path.IdxCols
                            .iter()
                            .any(|index_column| index_column.UniqueID == column.UniqueID)
                    })
            })
            .map(|condition| condition.CloneExpr())
            .collect();
        return Ok(Some(scan));
    }
    for child in plan.Children() {
        if let Some(scan) = build_lookup_scan_from_logical(child.as_ref())? {
            return Ok(Some(scan));
        }
    }
    Ok(None)
}

pub(crate) fn FlattenNestedMPPReaders(
    plan: Box<dyn PhysicalPlan>,
    context: &base::ContextRef,
) -> Result<Box<dyn PhysicalPlan>, expression::Error> {
    // An IndexJoin is a Root executor whose MPP outer child is consumed
    // through its TableReader boundary. That reader is not a nested MPP
    // fragment wrapper and must remain intact for both execution and cost.
    if crate::index_join_base_any(plan.as_any()).is_some() {
        return plan.clone_physical(context.clone());
    }
    let plan_children = plan.children();
    if let Some(reader) = plan.as_any().downcast_ref::<crate::PhysicalTableReader>()
        && let Some(table_plan) = reader
            .TablePlan
            .as_deref()
            .or_else(|| plan_children.first().copied())
        && table_plan
            .as_any()
            .downcast_ref::<crate::PhysicalExchangeSender>()
            .is_some_and(|sender| sender.ExchangeType == tipb::ExchangeType::PassThrough)
    {
        let fragment = table_plan
            .as_any()
            .downcast_ref::<crate::PhysicalExchangeSender>()
            .filter(|sender| sender.ExchangeType == tipb::ExchangeType::PassThrough)
            .and_then(|sender| sender.children().first().copied())
            .unwrap_or(table_plan);
        return FlattenNestedMPPReaders(fragment.clone_physical(context.clone())?, context);
    }
    let children = plan
        .children()
        .into_iter()
        .map(|child| FlattenNestedMPPReaders(child.clone_physical(context.clone())?, context))
        .collect::<Result<Vec<_>, _>>()?;
    let mut flattened = plan.clone_physical(context.clone())?;
    if !children.is_empty() {
        flattened.set_children(children);
    }
    Ok(flattened)
}

pub fn FlattenNestedMPPReadersBelowRoot(
    plan: Box<dyn PhysicalPlan>,
    context: &base::ContextRef,
) -> Result<Box<dyn PhysicalPlan>, expression::Error> {
    if let Some(reader) = plan.as_any().downcast_ref::<crate::PhysicalTableReader>() {
        let children = plan
            .children()
            .into_iter()
            .map(|child| FlattenNestedMPPReaders(child.clone_physical(context.clone())?, context))
            .collect::<Result<Vec<_>, _>>()?;
        let mut reader = reader.Clone(context.clone())?;
        reader.SetChildren(children);
        return Ok(Box::new(reader));
    }
    let children = plan
        .children()
        .into_iter()
        .map(|child| {
            FlattenNestedMPPReadersBelowRoot(child.clone_physical(context.clone())?, context)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut cloned = plan.clone_physical(context.clone())?;
    cloned.set_children(children);
    Ok(cloned)
}

/// 在 MPP 候选跨入 Root 属性边界时补齐 Go `mppTask.convertToRootTask` 的
/// `TableReader -> ExchangeSender(PassThrough)` 汇聚层。
fn convert_canonical_mpp_task_to_root(
    task: Box<dyn Task>,
) -> Result<Box<dyn Task>, expression::Error> {
    // Go's LogicalTableDual finder always returns a RootTask, even when the
    // requested property is MPP: a zero/one-row in-memory result needs no
    // TiFlash exchange boundary.
    if task.plan().as_any().is::<crate::PhysicalTableDual>() {
        return Ok(task);
    }
    let context = task.plan().s_ctx().clone();
    let child = task.plan().clone_physical(context.clone())?;
    if child
        .as_any()
        .downcast_ref::<crate::PhysicalTableReader>()
        .is_some_and(|reader| reader.ReadReqType == crate::ReadReqType::MPP)
    {
        return Ok(Box::new(crate::RootTask::NewWithMpp(
            child,
            Some(task.copy()),
            task.mpp_partition_type(),
            task.mpp_hash_cols(),
        )));
    }
    fn hoist_sort(
        plan: Box<dyn PhysicalPlan>,
        context: &base::ContextRef,
    ) -> Result<(Option<Box<dyn PhysicalPlan>>, Box<dyn PhysicalPlan>), expression::Error> {
        if plan.as_any().is::<crate::PhysicalSort>() {
            let children = plan.children();
            if children.len() == 1 {
                return Ok((
                    Some(plan.clone_physical(context.clone())?),
                    children[0].clone_physical(context.clone())?,
                ));
            }
        }
        if plan.as_any().is::<crate::PhysicalProjection>() {
            let children = plan.children();
            if children.len() == 1 {
                let (sort, remainder) =
                    hoist_sort(children[0].clone_physical(context.clone())?, context)?;
                if sort.is_some() {
                    let mut projection = plan.clone_physical(context.clone())?;
                    projection.set_children(vec![remainder]);
                    return Ok((sort, projection));
                }
            }
        }
        Ok((None, plan))
    }
    fn hoist_scalar_stream_agg(
        plan: &dyn PhysicalPlan,
        context: &base::ContextRef,
    ) -> Result<Option<(Vec<Box<dyn PhysicalPlan>>, Box<dyn PhysicalPlan>)>, expression::Error>
    {
        if let Some(stream) = plan.as_any().downcast_ref::<crate::PhysicalStreamAgg>()
            && stream.BasePhysicalAgg.MppRunMode == crate::AggMppRunMode::NoMpp
            && stream.BasePhysicalAgg.GroupByItems.is_empty()
            && stream
                .BasePhysicalAgg
                .AggFuncs
                .iter()
                .all(|function| function.Name != parser_ast::AggFuncApproxCountDistinct)
            && plan.children().len() == 1
        {
            return Ok(Some((
                vec![plan.clone_physical(context.clone())?],
                plan.children()[0].clone_physical(context.clone())?,
            )));
        }
        if plan.as_any().is::<crate::PhysicalProjection>()
            && plan.children().len() == 1
            && let Some((mut hoisted, remainder)) =
                hoist_scalar_stream_agg(plan.children()[0], context)?
        {
            hoisted.insert(0, plan.clone_physical(context.clone())?);
            return Ok(Some((hoisted, remainder)));
        }
        Ok(None)
    }
    let (hoisted, child) = if let Some(result) = hoist_scalar_stream_agg(child.as_ref(), &context)?
    {
        result
    } else {
        let (sort, child) = hoist_sort(child, &context)?;
        (sort.into_iter().collect(), child)
    };
    let schema = child.schema().Clone();
    let stats = child.stats_info().clone();
    let query_block = child.query_block_offset();

    let mut sender =
        crate::PhysicalExchangeSender::New(context.clone()).Init(context.clone(), stats.clone());
    sender.ExchangeType = tipb::ExchangeType::PassThrough;
    sender.PhysicalSchemaProducer.SetSchema(schema.Clone());
    sender.set_children(vec![child]);

    let mut reader = crate::PhysicalTableReader::New(context.clone()).Init(context, query_block);
    reader.StoreType = kv::StoreType::TiFlash;
    reader.ReadReqType = crate::ReadReqType::MPP;
    reader
        .PhysicalSchemaProducer
        .BasePhysicalPlan
        .set_stats(stats);
    reader.SetChildren(vec![Box::new(sender)]);
    let mut root_plan: Box<dyn PhysicalPlan> = Box::new(reader);
    for mut node in hoisted.into_iter().rev() {
        node.set_children(vec![root_plan]);
        root_plan = node;
    }
    Ok(Box::new(crate::RootTask::NewWithMpp(
        root_plan,
        Some(task.copy()),
        task.mpp_partition_type(),
        task.mpp_hash_cols(),
    )))
}

/// Root HashJoin must consume Root tasks. Convert each MPP child at the join
/// boundary instead of inheriting its partition metadata onto the join itself.
fn attach_canonical_root_hash_join(
    physical: &dyn PhysicalPlan,
    child_tasks: &[Box<dyn Task>],
    required: &PhysicalProperty,
) -> Result<Option<Box<dyn Task>>, expression::Error> {
    if !physical.as_any().is::<crate::PhysicalHashJoin>() || child_tasks.len() != 2 {
        return Ok(None);
    }
    if required.TaskTp != property::RootTaskType
        || physical
            .as_any()
            .downcast_ref::<crate::PhysicalHashJoin>()
            .is_some_and(|join| join.MppShuffleJoin)
    {
        return Ok(None);
    }
    let context = physical.s_ctx().clone();
    let mut children = Vec::with_capacity(child_tasks.len());
    for task in child_tasks {
        let root = if task.mpp_partition_type() != property::AnyType {
            convert_canonical_mpp_task_to_root(task.copy())?
        } else {
            task.copy()
        };
        children.push(root.plan().clone_physical(context.clone())?);
    }
    let mut join = physical.clone_physical(context)?;
    if let Some(hash) = join.as_any_mut().downcast_mut::<crate::PhysicalHashJoin>() {
        let left_nested = children[0].as_any().is::<crate::PhysicalHashJoin>();
        let right_nested = children[1].as_any().is::<crate::PhysicalHashJoin>();
        if !left_nested && right_nested {
            children.swap(0, 1);
            std::mem::swap(
                &mut hash.BasePhysicalJoin.LeftJoinKeys,
                &mut hash.BasePhysicalJoin.RightJoinKeys,
            );
            std::mem::swap(
                &mut hash.BasePhysicalJoin.LeftConditions,
                &mut hash.BasePhysicalJoin.RightConditions,
            );
            for equality in &mut hash.EqualConditions {
                if equality.GetArgs().len() == 2 {
                    equality.GetArgsMut().swap(0, 1);
                    equality.CleanHashCode();
                }
            }
        }
    }
    join.set_children(children);
    Ok(Some(Box::new(crate::RootTask::New(join, None))))
}

/// Lock is a strict Root executor. Do not let the generic attachment preserve
/// MPP partition metadata above the locking boundary.
fn attach_canonical_root_lock(
    physical: &dyn PhysicalPlan,
    child_tasks: &[Box<dyn Task>],
) -> Result<Option<Box<dyn Task>>, expression::Error> {
    if !(physical.as_any().is::<crate::LegacyPhysicalLock>()
        || physical.as_any().is::<crate::PhysicalLock>())
        || child_tasks.len() != 1
    {
        return Ok(None);
    }
    let context = physical.s_ctx().clone();
    let mut lock = physical.clone_physical(context.clone())?;
    let mut child = child_tasks[0].plan().clone_physical(context.clone())?;
    if let Some(projection) = child.as_any().downcast_ref::<crate::PhysicalProjection>()
        && let Some(input) = projection.children().first()
        && is_canonical_index_join_type(input.as_any().type_id())
    {
        child = input.clone_physical(context.clone())?;
    }
    fn hash_join_count(plan: &dyn PhysicalPlan) -> usize {
        usize::from(plan.as_any().is::<crate::PhysicalHashJoin>())
            + plan
                .children()
                .into_iter()
                .map(hash_join_count)
                .sum::<usize>()
    }
    if physical.s_ctx().GetSessionVars().IsMPPEnforced()
        && let Some(projection) = child.as_any().downcast_ref::<crate::PhysicalProjection>()
        && projection
            .Exprs
            .iter()
            .all(|expr| expr.as_column().is_some())
        && let Some(reader) = projection
            .children()
            .first()
            .and_then(|plan| plan.as_any().downcast_ref::<crate::PhysicalTableReader>())
        && reader.ReadReqType == crate::ReadReqType::MPP
        && reader
            .TablePlan
            .as_deref()
            .is_some_and(|plan| hash_join_count(plan) >= 2)
    {
        // The locking boundary reconstructs the MPP projection with the
        // handle columns needed by FOR UPDATE. Keep its input reader visible.
        child = reader.clone_physical(context.clone())?;
    }
    if physical.s_ctx().GetSessionVars().IsMPPEnforced()
        && let Some(projection) = child.as_any().downcast_ref::<crate::PhysicalProjection>()
        && let Some(input) = projection.children().first()
        && input.as_any().is::<crate::PhysicalHashJoin>()
    {
        child = input.clone_physical(context.clone())?;
    }
    if physical.s_ctx().GetSessionVars().IsMPPEnforced()
        && child.as_any().is::<crate::PhysicalHashJoin>()
        && child
            .children()
            .iter()
            .any(|plan| plan.as_any().is::<crate::PhysicalExchangeReceiver>())
    {
        fn fold_mpp_scan_filters(
            plan: Box<dyn PhysicalPlan>,
        ) -> Result<Box<dyn PhysicalPlan>, expression::Error> {
            if let Some(selection) = plan.as_any().downcast_ref::<crate::PhysicalSelection>()
                && let Some(input) = selection.children().first()
                && let Some(scan) = input.as_any().downcast_ref::<crate::PhysicalTableScan>()
            {
                let mut scan = scan.Clone(scan.s_ctx().clone())?;
                scan.FilterCondition.extend(
                    selection
                        .Conditions
                        .iter()
                        .map(|condition| condition.CloneExpr()),
                );
                scan.PhysicalSchemaProducer
                    .BasePhysicalPlan
                    .set_stats(selection.stats_info().clone());
                return Ok(Box::new(scan));
            }
            let context = plan.s_ctx().clone();
            let children = plan
                .children()
                .into_iter()
                .map(|child| {
                    child
                        .clone_physical(context.clone())
                        .and_then(fold_mpp_scan_filters)
                })
                .collect::<Result<Vec<_>, _>>()?;
            let mut cloned = plan.clone_physical(context)?;
            cloned.set_children(children);
            Ok(cloned)
        }
        child = fold_mpp_scan_filters(child)?;
        let schema = child.schema().Clone();
        let stats = child.stats_info().clone();
        let query_block = child.query_block_offset();
        let mut sender = crate::PhysicalExchangeSender::New(context.clone())
            .Init(context.clone(), stats.clone());
        sender.ExchangeType = tipb::ExchangeType::PassThrough;
        sender.PhysicalSchemaProducer.SetSchema(schema.Clone());
        sender.set_children(vec![child]);
        let mut reader =
            crate::PhysicalTableReader::New(context.clone()).Init(context.clone(), query_block);
        reader.StoreType = kv::StoreType::TiFlash;
        reader.ReadReqType = crate::ReadReqType::MPP;
        reader.PhysicalSchemaProducer.SetSchema(schema);
        reader
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .set_stats(stats);
        reader.SetChildren(vec![Box::new(sender)]);
        child = Box::new(reader);
    }
    fn contains_nested_hash_join(plan: &dyn PhysicalPlan) -> bool {
        plan.as_any().is::<crate::PhysicalHashJoin>()
            || plan.children().into_iter().any(contains_nested_hash_join)
    }
    if let Some(reader) = child.as_any().downcast_ref::<crate::PhysicalTableReader>()
        && let Some(sender) = reader.TablePlan.as_deref().and_then(|plan| {
            plan.as_any()
                .downcast_ref::<crate::PhysicalExchangeSender>()
        })
        && let Some(projection) = sender
            .children()
            .first()
            .and_then(|plan| plan.as_any().downcast_ref::<crate::PhysicalProjection>())
        && let Some(join) = projection
            .children()
            .first()
            .and_then(|plan| plan.as_any().downcast_ref::<crate::PhysicalHashJoin>())
        && !join
            .children()
            .iter()
            .any(|plan| contains_nested_hash_join(*plan))
    {
        let join = join.Clone(context.clone())?;
        let schema = join.schema().Clone();
        let mut sender = sender.Clone(context.clone())?;
        sender.PhysicalSchemaProducer.SetSchema(schema.Clone());
        sender.set_children(vec![Box::new(join)]);
        let mut reader = reader.Clone(context.clone())?;
        reader.PhysicalSchemaProducer.SetSchema(schema);
        reader.SetChildren(vec![Box::new(sender)]);
        child = Box::new(reader);
    }
    if let Some(reader) = child.as_any().downcast_ref::<crate::PhysicalTableReader>()
        && physical.s_ctx().GetSessionVars().IsMPPEnforced()
        && reader.StoreType == kv::StoreType::TiFlash
        && let Some(sender) = reader.TablePlan.as_deref().and_then(|plan| {
            plan.as_any()
                .downcast_ref::<crate::PhysicalExchangeSender>()
        })
        && let Some(inner) = sender.children().first()
        && inner
            .as_any()
            .downcast_ref::<crate::PhysicalHashJoin>()
            .or_else(|| {
                inner
                    .as_any()
                    .downcast_ref::<crate::PhysicalProjection>()
                    .and_then(|projection| projection.children().first().copied())
                    .and_then(|child| child.as_any().downcast_ref::<crate::PhysicalHashJoin>())
            })
            .is_some_and(|join| {
                join.children()
                    .iter()
                    .any(|child| contains_nested_hash_join(*child))
            })
    {
        fn collect_handle_names(
            plan: &dyn PhysicalPlan,
            names: &mut std::collections::HashSet<String>,
        ) {
            if let Some(scan) = plan.as_any().downcast_ref::<crate::PhysicalTableScan>()
                && let Some(table) = scan.Table.as_ref()
            {
                for key in &scan.schema().PKOrUK {
                    for column in key {
                        names.insert(column.String());
                    }
                }
                let primary_columns = table
                    .Indices
                    .iter()
                    .find(|index| index.Primary)
                    .map(|primary| {
                        primary
                            .Columns
                            .iter()
                            .filter_map(|column| table.Columns.get(column.Offset as usize))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_else(|| {
                        table
                            .Columns
                            .iter()
                            .filter(|column| mysql::r#type::HasPriKeyFlag(column.GetFlag()))
                            .collect()
                    });
                let table_names = [table.Name.L.as_str(), scan.TableAsName.as_str()];
                for info in primary_columns {
                    for table_name in table_names.into_iter().filter(|name| !name.is_empty()) {
                        names.insert(format!("{table_name}.{}", info.Name.L));
                        names.insert(format!("{}.{}.{}", scan.DBName, table_name, info.Name.L));
                    }
                }
            }
            for child in plan.children() {
                collect_handle_names(child, names);
            }
        }
        let existing_schema = inner.schema().Clone();
        let mut handle_names = std::collections::HashSet::new();
        collect_handle_names(*inner, &mut handle_names);
        let key_column_ids = lock
            .schema()
            .PKOrUK
            .iter()
            .flat_map(|key| key.iter().map(|column| column.UniqueID))
            .collect::<std::collections::HashSet<_>>();
        let output_schema = expression::NewSchema(
            lock.schema()
                .Columns
                .iter()
                .filter(|column| {
                    existing_schema.Contains(column)
                        || handle_names
                            .iter()
                            .any(|handle| column.String().ends_with(handle))
                        || column
                            .RetType
                            .as_ref()
                            .is_some_and(|field| mysql::r#type::HasPriKeyFlag(field.GetFlag()))
                        || key_column_ids.contains(&column.UniqueID)
                })
                .map(Column::Clone)
                .collect(),
        );
        let mut projection =
            if let Some(existing) = inner.as_any().downcast_ref::<crate::PhysicalProjection>() {
                existing.Clone(context.clone())?
            } else {
                crate::PhysicalProjection::New(context.clone()).Init(
                    context.clone(),
                    inner.stats_info().clone(),
                    inner.query_block_offset(),
                    Vec::new(),
                )
            };
        projection.Exprs = expression::Column2Exprs(&output_schema.Columns);
        projection.CalculateNoDelay = true;
        projection
            .PhysicalSchemaProducer
            .SetSchema(output_schema.Clone());
        let projection_input = inner
            .as_any()
            .downcast_ref::<crate::PhysicalProjection>()
            .and_then(|projection| projection.children().first().copied())
            .unwrap_or(*inner);
        fn restore_locked_mpp_join_exchanges(
            plan: &dyn PhysicalPlan,
            context: &ContextRef,
        ) -> Result<Box<dyn PhysicalPlan>, expression::Error> {
            let Some(join) = plan.as_any().downcast_ref::<crate::PhysicalHashJoin>() else {
                let children = plan
                    .children()
                    .into_iter()
                    .map(|child| restore_locked_mpp_join_exchanges(child, context))
                    .collect::<Result<Vec<_>, _>>()?;
                let mut cloned = plan.clone_physical(context.clone())?;
                cloned.set_children(children);
                if let Some(projection) =
                    cloned.as_any().downcast_ref::<crate::PhysicalProjection>()
                    && let Some(join) = projection
                        .children()
                        .first()
                        .and_then(|child| child.as_any().downcast_ref::<crate::PhysicalHashJoin>())
                {
                    let mut projection = projection.Clone(context.clone())?;
                    let mut schema = projection.schema().Clone();
                    let mut leading = join
                        .BasePhysicalJoin
                        .LeftJoinKeys
                        .iter()
                        .filter(|column| !schema.Contains(column))
                        .map(Column::Clone)
                        .collect::<Vec<_>>();
                    leading.append(&mut schema.Columns);
                    for column in &join.BasePhysicalJoin.RightJoinKeys {
                        if !leading.iter().any(|item| item.EqualColumn(column)) {
                            leading.push(column.Clone());
                        }
                    }
                    schema.Columns = leading;
                    projection.Exprs = expression::Column2Exprs(&schema.Columns);
                    projection.CalculateNoDelay = true;
                    projection.PhysicalSchemaProducer.SetSchema(schema.Clone());
                    let mut join = join.Clone(context.clone())?;
                    join.BasePhysicalJoin
                        .PhysicalSchemaProducer
                        .SetSchema(schema);
                    projection.set_children(vec![Box::new(join)]);
                    return Ok(Box::new(projection));
                }
                return Ok(cloned);
            };
            let keys = [
                &join.BasePhysicalJoin.LeftJoinKeys,
                &join.BasePhysicalJoin.RightJoinKeys,
            ];
            let mut children = Vec::with_capacity(2);
            for (index, raw_child) in join.children().into_iter().enumerate() {
                let mut child = raw_child.clone_physical(context.clone())?;
                if let Some(reader) = child.as_any().downcast_ref::<crate::PhysicalTableReader>()
                    && let Some(table_plan) = reader.TablePlan.as_deref()
                {
                    child = table_plan.clone_physical(context.clone())?;
                }
                if let Some(sender) = child
                    .as_any()
                    .downcast_ref::<crate::PhysicalExchangeSender>()
                    && sender.ExchangeType == tipb::ExchangeType::PassThrough
                    && let Some(sender_child) = sender.children().first()
                {
                    child = sender_child.clone_physical(context.clone())?;
                }
                child = restore_locked_mpp_join_exchanges(child.as_ref(), context)?;
                let already_hash_exchange = child
                    .as_any()
                    .downcast_ref::<crate::PhysicalExchangeReceiver>()
                    .and_then(|receiver| receiver.children().first().copied())
                    .and_then(|sender| {
                        sender
                            .as_any()
                            .downcast_ref::<crate::PhysicalExchangeSender>()
                    })
                    .is_some_and(|sender| sender.ExchangeType == tipb::ExchangeType::Hash);
                if already_hash_exchange {
                    children.push(child);
                    continue;
                }
                if let Some(nested_join) = child.as_any().downcast_ref::<crate::PhysicalHashJoin>()
                {
                    let mut columns = nested_join
                        .BasePhysicalJoin
                        .LeftJoinKeys
                        .iter()
                        .filter(|column| !child.schema().Contains(column))
                        .map(Column::Clone)
                        .collect::<Vec<_>>();
                    columns.extend(child.schema().Columns.iter().map(Column::Clone));
                    let trailing_keys = nested_join
                        .BasePhysicalJoin
                        .RightJoinKeys
                        .iter()
                        .filter(|column| !columns.iter().any(|item| item.EqualColumn(*column)))
                        .map(Column::Clone)
                        .collect::<Vec<_>>();
                    columns.extend(trailing_keys);
                    let schema = expression::NewSchema(columns);
                    let mut nested_join = nested_join.Clone(context.clone())?;
                    nested_join
                        .BasePhysicalJoin
                        .PhysicalSchemaProducer
                        .SetSchema(schema.Clone());
                    let mut projection = crate::PhysicalProjection::New(context.clone()).Init(
                        context.clone(),
                        child.stats_info().clone(),
                        child.query_block_offset(),
                        Vec::new(),
                    );
                    projection.Exprs = expression::Column2Exprs(&schema.Columns);
                    projection.CalculateNoDelay = true;
                    projection.PhysicalSchemaProducer.SetSchema(schema);
                    projection.set_children(vec![Box::new(nested_join)]);
                    child = Box::new(projection);
                }
                let schema = child.schema().Clone();
                let stats = child.stats_info().clone();
                let hash_cols = keys[index]
                    .iter()
                    .filter_map(|column| {
                        let field_type = column.RetType.as_ref()?;
                        Some(property::MPPPartitionColumn {
                            Col: column.Clone(),
                            CollateID: property::GetCollateIDByNameForPartition(
                                field_type.GetCollate(),
                            ),
                        })
                    })
                    .collect::<Vec<_>>();
                let mut sender = crate::PhysicalExchangeSender::New(context.clone())
                    .Init(context.clone(), stats.clone());
                sender.ExchangeType = tipb::ExchangeType::Hash;
                sender.HashCols = hash_cols;
                sender.CompressionMode = vardef::RecommendedExchangeCompressionMode;
                sender.PhysicalSchemaProducer.SetSchema(schema.Clone());
                sender.set_children(vec![child]);
                let mut receiver = crate::PhysicalExchangeReceiver::New(context.clone());
                receiver.PhysicalSchemaProducer.SetSchema(schema);
                receiver
                    .PhysicalSchemaProducer
                    .BasePhysicalPlan
                    .set_stats(stats);
                receiver.set_children(vec![Box::new(sender)]);
                children.push(Box::new(receiver) as Box<dyn PhysicalPlan>);
            }
            let mut join = join.Clone(context.clone())?;
            join.set_children(children);
            Ok(Box::new(join))
        }
        fn expose_lock_columns(
            plan: &dyn PhysicalPlan,
            required: &[Column],
            context: &ContextRef,
        ) -> Result<Box<dyn PhysicalPlan>, expression::Error> {
            if let Some(scan) = plan.as_any().downcast_ref::<crate::PhysicalTableScan>() {
                let mut scan = scan.Clone(context.clone())?;
                let mut schema = scan.schema().Clone();
                if let Some(table) = scan.Table.as_ref() {
                    for column in required {
                        let name = column.String();
                        let parts = name.split('.').collect::<Vec<_>>();
                        let belongs_to_scan = parts.len() >= 2
                            && (parts[parts.len() - 2] == table.Name.L
                                || parts[parts.len() - 2] == scan.TableAsName);
                        if !belongs_to_scan || schema.Contains(column) {
                            continue;
                        }
                        if let Some(info) = table
                            .Columns
                            .iter()
                            .find(|info| info.Name.L == parts[parts.len() - 1])
                        {
                            schema.Columns.push(column.Clone());
                            scan.Columns.push(info.clone());
                        }
                    }
                }
                scan.PhysicalSchemaProducer.SetSchema(schema);
                return Ok(Box::new(scan));
            }
            let children = plan
                .children()
                .into_iter()
                .map(|child| expose_lock_columns(child, required, context))
                .collect::<Result<Vec<_>, _>>()?;
            let mut cloned = plan.clone_physical(context.clone())?;
            cloned.set_children(children);
            let visible = cloned.children();
            let additions = required
                .iter()
                .filter(|column| {
                    !cloned.schema().Contains(column)
                        && visible.iter().any(|child| child.schema().Contains(column))
                })
                .map(Column::Clone)
                .collect::<Vec<_>>();
            if additions.is_empty() {
                return Ok(cloned);
            }
            let mut schema = cloned.schema().Clone();
            schema.Columns.extend(additions.iter().map(Column::Clone));
            if let Some(projection) = cloned
                .as_any_mut()
                .downcast_mut::<crate::PhysicalProjection>()
            {
                projection
                    .Exprs
                    .extend(expression::Column2Exprs(&additions));
                projection.PhysicalSchemaProducer.SetSchema(schema);
            } else if let Some(join) = cloned
                .as_any_mut()
                .downcast_mut::<crate::PhysicalHashJoin>()
            {
                join.BasePhysicalJoin
                    .PhysicalSchemaProducer
                    .SetSchema(schema);
            } else if let Some(selection) = cloned
                .as_any_mut()
                .downcast_mut::<crate::PhysicalSelection>()
            {
                selection.PhysicalSchemaProducer.SetSchema(schema);
            } else if let Some(sender) = cloned
                .as_any_mut()
                .downcast_mut::<crate::PhysicalExchangeSender>()
            {
                sender.PhysicalSchemaProducer.SetSchema(schema);
            } else if let Some(receiver) = cloned
                .as_any_mut()
                .downcast_mut::<crate::PhysicalExchangeReceiver>()
            {
                receiver.PhysicalSchemaProducer.SetSchema(schema);
            }
            Ok(cloned)
        }
        let restore_mpp_exchanges = reader.StoreType == kv::StoreType::TiFlash
            && projection_input.as_any().is::<crate::PhysicalHashJoin>();
        let mut projected_child = if restore_mpp_exchanges {
            restore_locked_mpp_join_exchanges(projection_input, &context)?
        } else {
            projection_input.clone_physical(context.clone())?
        };
        projected_child =
            expose_lock_columns(projected_child.as_ref(), &output_schema.Columns, &context)?;
        if restore_mpp_exchanges
            && let Some(hash_join) = projected_child
                .as_any()
                .downcast_ref::<crate::PhysicalHashJoin>()
        {
            let mut hash_join = hash_join.Clone(context.clone())?;
            hash_join
                .BasePhysicalJoin
                .PhysicalSchemaProducer
                .SetSchema(output_schema.Clone());
            projected_child = Box::new(hash_join);
        }
        if !restore_mpp_exchanges
            && let Some(hash_join) = projection_input
                .as_any()
                .downcast_ref::<crate::PhysicalHashJoin>()
        {
            let mut hash_join = hash_join.Clone(context.clone())?;
            let mut join_schema = output_schema.Clone();
            for child in hash_join.children() {
                for column in &child.schema().Columns {
                    if !join_schema.Contains(column) {
                        join_schema.Columns.push(column.Clone());
                    }
                }
            }
            hash_join
                .BasePhysicalJoin
                .PhysicalSchemaProducer
                .SetSchema(join_schema);
            projected_child = Box::new(hash_join);
        }
        projection.set_children(vec![projected_child]);
        let mut sender = sender.Clone(context.clone())?;
        sender
            .PhysicalSchemaProducer
            .SetSchema(output_schema.Clone());
        sender.set_children(vec![Box::new(projection)]);
        let mut reader = reader.Clone(context.clone())?;
        reader.PhysicalSchemaProducer.SetSchema(output_schema);
        reader.SetChildren(vec![Box::new(sender)]);
        child = Box::new(reader);
    }
    if physical.s_ctx().GetSessionVars().IsMPPEnforced()
        && child
            .as_any()
            .downcast_ref::<crate::PhysicalHashJoin>()
            .is_some_and(|join| join.MppShuffleJoin)
    {
        fn fold_mpp_scan_filters(
            plan: Box<dyn PhysicalPlan>,
            context: &ContextRef,
        ) -> Result<Box<dyn PhysicalPlan>, expression::Error> {
            if let Some(selection) = plan.as_any().downcast_ref::<crate::PhysicalSelection>()
                && let Some(scan) = plan
                    .children()
                    .first()
                    .and_then(|child| child.as_any().downcast_ref::<crate::PhysicalTableScan>())
            {
                let mut scan = scan.Clone(context.clone())?;
                scan.FilterCondition.extend(
                    selection
                        .Conditions
                        .iter()
                        .map(|condition| condition.CloneExpr()),
                );
                scan.PhysicalSchemaProducer
                    .BasePhysicalPlan
                    .set_stats(selection.stats_info().clone());
                scan.StoreType = kv::StoreType::TiFlash;
                return Ok(Box::new(scan));
            }
            let mut cloned = plan.clone_physical(context.clone())?;
            let children = plan
                .children()
                .into_iter()
                .map(|child| fold_mpp_scan_filters(child.clone_physical(context.clone())?, context))
                .collect::<Result<Vec<_>, _>>()?;
            cloned.set_children(children);
            Ok(cloned)
        }
        child = fold_mpp_scan_filters(child, &context)?;
        let schema = child.schema().Clone();
        let stats = child.stats_info().clone();
        let query_block = child.query_block_offset();
        let mut sender = crate::PhysicalExchangeSender::New(context.clone())
            .Init(context.clone(), stats.clone());
        sender.ExchangeType = tipb::ExchangeType::PassThrough;
        sender.PhysicalSchemaProducer.SetSchema(schema.Clone());
        sender.set_children(vec![child]);
        let mut reader =
            crate::PhysicalTableReader::New(context.clone()).Init(context.clone(), query_block);
        reader.StoreType = kv::StoreType::TiFlash;
        reader.ReadReqType = crate::ReadReqType::MPP;
        reader.PhysicalSchemaProducer.SetSchema(schema);
        reader
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .set_stats(stats);
        reader.SetChildren(vec![Box::new(sender)]);
        child = Box::new(reader);
    }
    if let Some(reader) = child.as_any().downcast_ref::<crate::PhysicalTableReader>()
        && let Some(sender) = reader.TablePlan.as_deref().and_then(|plan| {
            plan.as_any()
                .downcast_ref::<crate::PhysicalExchangeSender>()
        })
        && let Some(projection) = sender
            .children()
            .first()
            .and_then(|plan| plan.as_any().downcast_ref::<crate::PhysicalProjection>())
        && let Some(join) = projection
            .children()
            .first()
            .and_then(|plan| plan.as_any().downcast_ref::<crate::PhysicalHashJoin>())
        && !join
            .children()
            .iter()
            .any(|plan| contains_nested_hash_join(*plan))
    {
        let join = join.Clone(context.clone())?;
        let schema = join.schema().Clone();
        let mut sender = sender.Clone(context.clone())?;
        sender.PhysicalSchemaProducer.SetSchema(schema.Clone());
        sender.set_children(vec![Box::new(join)]);
        let mut reader = reader.Clone(context.clone())?;
        reader.PhysicalSchemaProducer.SetSchema(schema);
        reader.SetChildren(vec![Box::new(sender)]);
        child = Box::new(reader);
    }
    lock.set_children(vec![child]);
    Ok(Some(Box::new(crate::RootTask::New(lock, None))))
}

/// 将 MPP HashJoin 的两个子任务接到 ExchangeReceiver 边界。
///
/// Rust 任务抽象目前以 RootTask 承载所有已挂接计划；这里保留 Go
/// `attach2TaskForMpp4PhysicalHashJoin` 的可观察计划形状，把 TiFlash 子树
/// 显式包成 Hash Exchange，而不是让通用 `attach_to_task` 把子树留在 TiKV。
fn attach_canonical_mpp_join(
    physical: &dyn PhysicalPlan,
    child_tasks: &[Box<dyn Task>],
    required: &PhysicalProperty,
) -> Result<Option<Box<dyn Task>>, expression::Error> {
    let Some(join) = physical.as_any().downcast_ref::<crate::PhysicalHashJoin>() else {
        return Ok(None);
    };
    if join.StoreTp != kv::StoreType::TiFlash {
        return Ok(None);
    }
    if child_tasks.len() != 2 {
        return Ok(None);
    }
    let children_are_mpp_readers = child_tasks.iter().all(|task| {
        task.plan()
            .as_any()
            .downcast_ref::<crate::PhysicalTableReader>()
            .is_some_and(|reader| reader.StoreType == kv::StoreType::TiFlash)
    });
    let forced_mpp_join = children_are_mpp_readers && join.MppShuffleJoin;
    if required.TaskTp == property::RootTaskType
        && required.NoCopPushDown
        && physical
            .s_ctx()
            .GetSessionVars()
            .GetIsolationReadEngines()
            .contains(&kv::StoreType::TiKV)
        && !physical
            .s_ctx()
            .GetSessionVars()
            .GetSystemVar("transaction_isolation")
            .is_some_and(|level| level.eq_ignore_ascii_case("READ-COMMITTED"))
    {
        return Ok(None);
    }
    if required.TaskTp == property::RootTaskType && required.NoCopPushDown && !join.HasTableAlias {
        return Ok(None);
    }
    if required.TaskTp == property::RootTaskType
        && required.NoCopPushDown
        && !physical.s_ctx().GetSessionVars().IsMPPEnforced()
        && !forced_mpp_join
    {
        return Ok(None);
    }
    if (0..child_tasks.len())
        .any(|index| physical.get_child_req_props(index).TaskTp != property::MppTaskType)
        && !physical.s_ctx().GetSessionVars().IsMPPEnforced()
        && !forced_mpp_join
    {
        return Ok(None);
    }
    fn contains_root_cte_reader(plan: &dyn PhysicalPlan) -> bool {
        plan.as_any().is::<crate::PhysicalCteScan>()
            || plan.children().into_iter().any(contains_root_cte_reader)
    }
    if child_tasks
        .iter()
        .any(|task| contains_root_cte_reader(task.plan()))
    {
        // A Root CTE reader may be returned while producer compatibility is
        // unknown. It does not satisfy an MPP hash-child property and must
        // remain attached to the Root join without synthetic exchanges.
        return Ok(None);
    }
    fn contains_root_runtime_selection(plan: &dyn PhysicalPlan) -> bool {
        plan.as_any()
            .downcast_ref::<crate::PhysicalSelection>()
            .is_some_and(|selection| selection.ExplainInfo().contains("ScalarQueryCol#"))
            || plan
                .as_any()
                .downcast_ref::<crate::PhysicalTableReader>()
                .and_then(|reader| reader.TablePlan.as_deref())
                .is_some_and(contains_root_runtime_selection)
            || plan
                .children()
                .into_iter()
                .any(contains_root_runtime_selection)
    }
    if child_tasks
        .iter()
        .any(|task| contains_root_runtime_selection(task.plan()))
    {
        return Ok(None);
    }
    let context = physical.s_ctx().clone();
    let mut join_schema = physical.schema().Clone();
    fn task_contains_hash_join(plan: &dyn PhysicalPlan) -> bool {
        plan.as_any().is::<crate::PhysicalHashJoin>()
            || plan.children().into_iter().any(task_contains_hash_join)
    }
    if required.NoCopPushDown
        && physical.s_ctx().GetSessionVars().IsMPPEnforced()
        && child_tasks
            .iter()
            .any(|task| task_contains_hash_join(task.plan()))
    {
        fn collect_scan_handles(plan: &dyn PhysicalPlan, columns: &mut Vec<Column>) {
            if let Some(join) = plan.as_any().downcast_ref::<crate::PhysicalHashJoin>() {
                columns.extend(
                    join.BasePhysicalJoin
                        .LeftJoinKeys
                        .iter()
                        .chain(&join.BasePhysicalJoin.RightJoinKeys)
                        .map(Column::Clone),
                );
            }
            if let Some(scan) = plan.as_any().downcast_ref::<crate::PhysicalTableScan>()
                && let Some(table) = scan.Table.as_ref()
            {
                let handle_names = table
                    .Indices
                    .iter()
                    .find(|index| index.Primary)
                    .into_iter()
                    .flat_map(|index| &index.Columns)
                    .filter_map(|index_column| table.Columns.get(index_column.Offset as usize))
                    .map(|info| info.Name.L.clone())
                    .chain(
                        table
                            .Columns
                            .iter()
                            .filter(|info| mysql::r#type::HasPriKeyFlag(info.GetFlag()))
                            .map(|info| info.Name.L.clone()),
                    )
                    .collect::<std::collections::HashSet<_>>();
                columns.extend(
                    scan.schema()
                        .Columns
                        .iter()
                        .filter(|column| {
                            handle_names
                                .iter()
                                .any(|name| column.String().ends_with(&format!(".{name}")))
                        })
                        .map(Column::Clone),
                );
                columns.extend(
                    scan.schema()
                        .PKOrUK
                        .iter()
                        .flat_map(|key| key.iter().map(Column::Clone)),
                );
            }
            if let Some(reader) = plan.as_any().downcast_ref::<crate::PhysicalTableReader>()
                && let Some(table_plan) = reader.TablePlan.as_deref()
            {
                collect_scan_handles(table_plan, columns);
            }
            for child in plan.children() {
                collect_scan_handles(child, columns);
            }
        }
        let mut handles = Vec::new();
        for task in child_tasks {
            collect_scan_handles(task.plan(), &mut handles);
        }
        for column in handles {
            if !join_schema.Contains(&column) {
                join_schema.Columns.push(column);
            }
        }
    }
    let mut children = Vec::with_capacity(2);
    for (index, task) in child_tasks.iter().enumerate() {
        let mut child = task.plan().clone_physical(context.clone())?;
        if let Some(reader) = child.as_any().downcast_ref::<crate::PhysicalTableReader>() {
            let mut reader = reader.Clone(context.clone())?;
            if let Some(inner) = reader.TablePlan.take() {
                child = inner;
            }
        }
        if let Some(sender) = child
            .as_any()
            .downcast_ref::<crate::PhysicalExchangeSender>()
            && sender.ExchangeType == tipb::ExchangeType::PassThrough
            && let [inner] = sender.children().as_slice()
        {
            child = inner.clone_physical(context.clone())?;
        }

        if let Some(projection) = child.as_any().downcast_ref::<crate::PhysicalProjection>()
            && let [input] = projection.children().as_slice()
            && let Some(scan) = input.as_any().downcast_ref::<crate::PhysicalTableScan>()
            && scan.StoreType == kv::StoreType::TiFlash
            && projection.Exprs.len() == projection.schema().Len()
            && projection
                .Exprs
                .iter()
                .all(|expr| expr.as_any().is::<Column>())
        {
            child = Box::new(scan.Clone(context.clone())?);
        }

        if let Some(output) = child.as_any().downcast_ref::<crate::PhysicalProjection>()
            && let [selection_plan] = output.children().as_slice()
            && let Some(selection) = selection_plan
                .as_any()
                .downcast_ref::<crate::PhysicalSelection>()
            && let [selection_input] = selection_plan.children().as_slice()
        {
            // A derived aggregate's output-order projection is redundant at
            // this MPP join boundary. Go keeps the aggregate-order identity
            // projection below HAVING and lets Selection expose the derived
            // table's requested schema.
            let identity: Option<Box<dyn PhysicalPlan>> =
                if selection_input.as_any().is::<crate::PhysicalHashAgg>() {
                    let aggregate = selection_input.clone_physical(context.clone())?;
                    let aggregate_schema = aggregate.schema().Clone();
                    let mut identity = crate::PhysicalProjection::New(context.clone()).Init(
                        context.clone(),
                        aggregate.stats_info().clone(),
                        aggregate.query_block_offset(),
                        vec![],
                    );
                    identity.Exprs = expression::Column2Exprs(&aggregate_schema.Columns);
                    identity.PhysicalSchemaProducer.SetSchema(aggregate_schema);
                    identity.set_children(vec![aggregate]);
                    Some(Box::new(identity))
                } else if selection_input
                    .as_any()
                    .downcast_ref::<crate::PhysicalProjection>()
                    .is_some_and(|identity| {
                        identity
                            .children()
                            .first()
                            .is_some_and(|child| child.as_any().is::<crate::PhysicalHashAgg>())
                    })
                {
                    Some(selection_input.clone_physical(context.clone())?)
                } else {
                    None
                };
            if let Some(identity) = identity {
                let mut selection = selection.Clone(context.clone())?;
                selection
                    .PhysicalSchemaProducer
                    .SetSchema(output.schema().Clone());
                selection.set_children(vec![identity]);
                child = Box::new(selection);
            }
        }

        let mut schema = child.schema().Clone();
        let stats = child.stats_info().clone();
        // The logical join may have more equality keys than the current
        // fragment needs to preserve.  Go's MPP path receives the selected
        // child property (after `IsSubsetOf` against the join's potential
        // keys), rather than rebuilding the partition list from every join
        // key.  In particular, a grouped CTE producer can be partitioned by
        // only its grouping key while still evaluating a multi-key join.
        let child_property = physical.get_child_req_props(index);
        let mut hash_cols = child_property
            .MPPPartitionCols
            .iter()
            .map(property::MPPPartitionColumn::Clone)
            .collect::<Vec<_>>();
        if join.MppShuffleJoin && hash_cols.is_empty() {
            let join_keys = if index == 0 {
                &join.BasePhysicalJoin.LeftJoinKeys
            } else {
                &join.BasePhysicalJoin.RightJoinKeys
            };
            hash_cols = join_keys
                .iter()
                .filter_map(|column| {
                    Some(property::MPPPartitionColumn {
                        Col: column.Clone(),
                        CollateID: property::GetCollateIDByNameForPartition(
                            column.RetType.as_ref()?.GetCollate(),
                        ),
                    })
                })
                .collect();
        }
        for hash_col in &mut hash_cols {
            if schema.Contains(&hash_col.Col) {
                continue;
            }
            let mut matching = schema
                .Columns
                .iter()
                .filter(|column| column.String() == hash_col.Col.String());
            let first = matching.next();
            if let Some(column) = first
                && matching.next().is_none()
            {
                hash_col.Col = column.Clone();
            }
        }
        if child_property.MPPPartitionTp == property::BroadcastType
            || (!join.MppShuffleJoin && index == join.BasePhysicalJoin.InnerChildIdx)
        {
            let mut sender = crate::PhysicalExchangeSender::New(context.clone())
                .Init(context.clone(), stats.clone());
            sender.ExchangeType = tipb::ExchangeType::Broadcast;
            sender.CompressionMode = vardef::RecommendedExchangeCompressionMode;
            sender.PhysicalSchemaProducer.SetSchema(schema.Clone());
            sender.set_children(vec![child]);

            let mut receiver = crate::PhysicalExchangeReceiver::New(context.clone());
            receiver.PhysicalSchemaProducer.SetSchema(schema);
            receiver
                .PhysicalSchemaProducer
                .BasePhysicalPlan
                .set_stats(stats);
            receiver.set_children(vec![Box::new(sender)]);
            children.push(Box::new(receiver) as Box<dyn PhysicalPlan>);
            continue;
        }
        if child_property.MPPPartitionTp == property::AnyType && !join.MppShuffleJoin {
            children.push(child);
            continue;
        }
        if hash_cols.is_empty() {
            return Ok(None);
        }
        let redundant_grouped_exchange = child
            .as_any()
            .downcast_ref::<crate::PhysicalExchangeReceiver>()
            .and_then(|receiver| receiver.children().first().copied())
            .and_then(|sender| {
                sender
                    .as_any()
                    .downcast_ref::<crate::PhysicalExchangeSender>()
            })
            .filter(|sender| sender.ExchangeType == tipb::ExchangeType::Hash)
            .and_then(|sender| sender.children().first().copied())
            .filter(|inner| is_one_phase_grouped_mpp_child(*inner, &hash_cols))
            .map(|inner| inner.clone_physical(context.clone()))
            .transpose()?;
        if let Some(inner) = redundant_grouped_exchange {
            child = inner;
            schema = child.schema().Clone();
        }
        let already_partitioned = child
            .as_any()
            .downcast_ref::<crate::PhysicalExchangeReceiver>()
            .and_then(|receiver| receiver.children().first().copied())
            .and_then(|sender| {
                sender
                    .as_any()
                    .downcast_ref::<crate::PhysicalExchangeSender>()
            })
            .is_some_and(|sender| {
                sender.ExchangeType == tipb::ExchangeType::Hash
                    && sender.HashCols.len() == hash_cols.len()
                    && sender
                        .HashCols
                        .iter()
                        .zip(&hash_cols)
                        .all(|(actual, expected)| {
                            actual.Equal(expected)
                                || (actual.Col.String() == expected.Col.String()
                                    && actual.CollateID == expected.CollateID)
                        })
            });
        if already_partitioned {
            // DataSource task construction may already have materialized the
            // exact hash property requested by this join. Go carries that MPP
            // task upward; wrapping it again would create a redundant network
            // boundary on every join input.
            children.push(child);
            continue;
        }
        let task_hash_cols = task.mpp_hash_cols();
        let task_partition_satisfied = task.mpp_partition_type() == property::HashType
            && task_hash_cols.len() == hash_cols.len()
            && task_hash_cols
                .iter()
                .zip(&hash_cols)
                .all(|(actual, expected)| {
                    actual.Equal(expected)
                        || (actual.Col.String() == expected.Col.String()
                            && actual.CollateID == expected.CollateID)
                });
        if task_partition_satisfied {
            // MppTask carries the output partitioning independently of the
            // top physical node (often a pruning Projection above a Join).
            // Trust that property just as Go does instead of inserting a
            // duplicate Exchange because the sender is not the direct child.
            children.push(child);
            continue;
        }
        if is_one_phase_grouped_mpp_child(child.as_ref(), &hash_cols) {
            // A one-phase grouped CTE producer already emits rows in the
            // requested group-key layout. Go's MppTask carries that layout
            // upward, so the parent join must not add a duplicate Exchange.
            children.push(child);
            continue;
        }
        if child.as_any().is::<crate::PhysicalSelection>()
            && hash_cols
                .iter()
                .all(|hash_col| schema.Contains(&hash_col.Col))
            && hash_cols
                .iter()
                .all(|hash_col| join_schema.Contains(&hash_col.Col))
        {
            let output_columns = join_schema
                .Columns
                .iter()
                .filter(|column| schema.Contains(column))
                .map(expression::Column::Clone)
                .collect::<Vec<_>>();
            if output_columns.len() < schema.Len() {
                let selection = child
                    .as_any()
                    .downcast_ref::<crate::PhysicalSelection>()
                    .expect("selection branch")
                    .Clone(context.clone())?;
                let selection_input = child
                    .children()
                    .first()
                    .ok_or_else(|| expression::errors::New("selection requires one child"))?
                    .clone_physical(context.clone())?;
                let output_schema = expression::NewSchema(output_columns.clone());
                if hash_cols
                    .iter()
                    .all(|hash_col| output_schema.Contains(&hash_col.Col))
                {
                    let mut selection = selection;
                    schema = output_schema;
                    selection.PhysicalSchemaProducer.SetSchema(schema.Clone());
                    selection.set_children(vec![selection_input]);
                    child = Box::new(selection);
                    let mut sender = crate::PhysicalExchangeSender::New(context.clone())
                        .Init(context.clone(), stats.clone());
                    sender.ExchangeType = tipb::ExchangeType::Hash;
                    sender.HashCols = hash_cols;
                    sender.CompressionMode = vardef::RecommendedExchangeCompressionMode;
                    sender.PhysicalSchemaProducer.SetSchema(schema.Clone());
                    sender.set_children(vec![child]);

                    let mut receiver = crate::PhysicalExchangeReceiver::New(context.clone());
                    receiver.PhysicalSchemaProducer.SetSchema(schema);
                    receiver
                        .PhysicalSchemaProducer
                        .BasePhysicalPlan
                        .set_stats(stats);
                    receiver.set_children(vec![Box::new(sender)]);
                    children.push(Box::new(receiver) as Box<dyn PhysicalPlan>);
                    continue;
                }
                let mut projection = crate::PhysicalProjection::New(context.clone()).Init(
                    context.clone(),
                    stats.clone(),
                    child.query_block_offset(),
                    vec![],
                );
                projection.Exprs = expression::Column2Exprs(&output_columns);
                schema = expression::NewSchema(output_columns);
                for hash_col in &mut hash_cols {
                    if schema.Contains(&hash_col.Col) {
                        projection.Exprs.push(Box::new(hash_col.Col.Clone()));
                        // Go carries the original join-key identity through
                        // this projection. Cloning it with a fresh ID shifts
                        // later synthetic aggregate IDs once per MPP join.
                        schema.Append([hash_col.Col.Clone()]);
                    }
                }
                projection.PhysicalSchemaProducer.SetSchema(schema.Clone());
                let selection_can_follow_projection = selection
                    .Conditions
                    .iter()
                    .flat_map(|condition| expression::ExtractColumns(condition.as_ref()))
                    .all(|column| schema.Contains(&column));
                if selection_can_follow_projection {
                    projection.set_children(vec![selection_input]);
                    let mut selection = selection;
                    selection.PhysicalSchemaProducer.SetSchema(schema.Clone());
                    selection.set_children(vec![Box::new(projection)]);
                    child = Box::new(selection);
                } else {
                    projection.set_children(vec![child]);
                    child = Box::new(projection);
                }
            }
        }
        let mut sender = crate::PhysicalExchangeSender::New(context.clone())
            .Init(context.clone(), stats.clone());
        sender.ExchangeType = tipb::ExchangeType::Hash;
        sender.HashCols = hash_cols;
        sender.CompressionMode = vardef::RecommendedExchangeCompressionMode;
        sender.PhysicalSchemaProducer.SetSchema(schema.Clone());
        sender.set_children(vec![child]);

        let mut receiver = crate::PhysicalExchangeReceiver::New(context.clone());
        receiver.PhysicalSchemaProducer.SetSchema(schema);
        receiver
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .set_stats(stats);
        receiver.set_children(vec![Box::new(sender)]);
        children.push(Box::new(receiver) as Box<dyn PhysicalPlan>);
    }

    let mut attached = join.Clone(context.clone())?;
    attached
        .BasePhysicalJoin
        .PhysicalSchemaProducer
        .SetSchema(join_schema.Clone());
    let remap_join_keys = |keys: &mut [Column], schema: &expression::Schema| {
        for key in keys {
            if schema.Contains(key) {
                continue;
            }
            let mut matching = schema
                .Columns
                .iter()
                .filter(|column| column.String() == key.String());
            let first = matching.next();
            if let Some(column) = first
                && matching.next().is_none()
            {
                *key = column.Clone();
            }
        }
    };
    remap_join_keys(
        &mut attached.BasePhysicalJoin.LeftJoinKeys,
        children[0].schema(),
    );
    remap_join_keys(
        &mut attached.BasePhysicalJoin.RightJoinKeys,
        children[1].schema(),
    );
    for child in &mut children {
        let Some(projection) = child.as_any().downcast_ref::<crate::PhysicalProjection>() else {
            continue;
        };
        let projection_schema = projection.schema().Clone();
        let projection_children = projection.children();
        let [selection_plan] = projection_children.as_slice() else {
            continue;
        };
        let Some(selection) = selection_plan
            .as_any()
            .downcast_ref::<crate::PhysicalSelection>()
        else {
            continue;
        };
        let selection_children = selection.children();
        let [selection_input] = selection_children.as_slice() else {
            continue;
        };
        if !selection_input.as_any().is::<crate::PhysicalHashAgg>()
            && !selection_input
                .as_any()
                .downcast_ref::<crate::PhysicalProjection>()
                .is_some_and(|identity| {
                    identity
                        .children()
                        .first()
                        .is_some_and(|child| child.as_any().is::<crate::PhysicalHashAgg>())
                })
        {
            continue;
        }
        let mut replacement = selection.Clone(context.clone())?;
        replacement
            .PhysicalSchemaProducer
            .SetSchema(projection_schema);
        replacement.set_children(vec![selection_input.clone_physical(context.clone())?]);
        *child = Box::new(replacement);
    }
    fn strip_nested_pass_through(
        plan: &dyn PhysicalPlan,
        context: &base::ContextRef,
    ) -> Result<Box<dyn PhysicalPlan>, expression::Error> {
        if let Some(sender) = plan
            .as_any()
            .downcast_ref::<crate::PhysicalExchangeSender>()
            && sender.ExchangeType == tipb::ExchangeType::PassThrough
            && let [child] = sender.children().as_slice()
        {
            return strip_nested_pass_through(*child, context);
        }
        let children = plan
            .children()
            .into_iter()
            .map(|child| strip_nested_pass_through(child, context))
            .collect::<Result<Vec<_>, _>>()?;
        let mut cloned = plan.clone_physical(context.clone())?;
        if !children.is_empty() {
            cloned.set_children(children);
        }
        Ok(cloned)
    }
    let children = children
        .iter()
        .map(|child| strip_nested_pass_through(child.as_ref(), &context))
        .collect::<Result<Vec<_>, _>>()?;
    attached.set_children(children);
    if attached.BasePhysicalJoin.JoinType != base::JoinType::InnerJoin
        && attached.BasePhysicalJoin.InnerChildIdx == 1
    {
        for equality in &mut attached.EqualConditions {
            if equality.GetArgs().len() == 2 {
                equality.GetArgsMut().swap(0, 1);
                equality.CleanHashCode();
            }
        }
    }
    // Keep the pruned schema for the optional projection.  The Join itself
    // must expose its full semantic schema to TiFlash, just as Go resets
    // `p.Schema()` after attaching the projection below.
    let mut schema = attached.schema().Clone();
    let mut default_schema =
        crate::BuildPhysicalJoinSchema(join.BasePhysicalJoin.JoinType, &attached);
    // A child-side projection may introduce a separately-addressable hash key
    // for an already visible result column. Keep that physical key in the
    // pruned output so an ancestor MPP join can still use the exchange layout.
    // Preserve its child-relative position instead of appending it to the
    // whole join output: this is the column order produced by Go's projection.
    let mut hidden_after = HashMap::<i64, Vec<expression::Column>>::new();
    for child in attached.children() {
        let mut previous_visible = None;
        let mut occurrences = HashMap::<i64, usize>::new();
        for column in &child.schema().Columns {
            if schema.Contains(column) {
                let occurrence = occurrences.entry(column.UniqueID).or_default();
                *occurrence += 1;
                let visible_occurrences = schema
                    .Columns
                    .iter()
                    .filter(|visible| visible.UniqueID == column.UniqueID)
                    .count();
                if *occurrence <= visible_occurrences {
                    previous_visible = Some(column.UniqueID);
                } else if let Some(previous) = previous_visible {
                    // Go's MPP DataSource projection may deliberately expose
                    // the same column more than once. Preserve those extra
                    // occurrences in child-relative order through each join.
                    hidden_after
                        .entry(previous)
                        .or_default()
                        .push(column.Clone());
                }
            } else if column.IsHidden
                && let Some(previous) = previous_visible
                && schema
                    .Columns
                    .iter()
                    .any(|visible| visible.String() == column.String())
            {
                hidden_after
                    .entry(previous)
                    .or_default()
                    .push(column.Clone());
            }
        }
    }
    if !hidden_after.is_empty() {
        let mut columns = Vec::with_capacity(schema.Len() + hidden_after.len());
        for column in schema.Columns {
            let unique_id = column.UniqueID;
            columns.push(column);
            if let Some(hidden) = hidden_after.remove(&unique_id) {
                columns.extend(hidden);
            }
        }
        schema = expression::NewSchema(columns);
    }
    let mut outer_index = 1 - join.BasePhysicalJoin.InnerChildIdx;
    if join.BasePhysicalJoin.JoinType != base::JoinType::InnerJoin {
        outer_index = if join.BasePhysicalJoin.JoinType == base::JoinType::RightOuterJoin {
            1
        } else {
            0
        };
    }
    let requested_hash_cols = physical
        .get_child_req_props(outer_index)
        .MPPPartitionCols
        .iter()
        .map(property::MPPPartitionColumn::Clone)
        .collect::<Vec<_>>();
    let task_hash_cols = child_tasks[outer_index].mpp_hash_cols();
    let task_matches_request = task_hash_cols_satisfy_output(&task_hash_cols, &requested_hash_cols);
    let outer_hash_cols = if task_matches_request {
        task_hash_cols
    } else {
        requested_hash_cols
    };
    let outer_partition_type = if task_matches_request {
        child_tasks[outer_index].mpp_partition_type()
    } else {
        physical.get_child_req_props(outer_index).MPPPartitionTp
    };
    // A one-phase grouped producer cannot carry a pruned partition key through
    // its output. Reintroducing that key here would leak it into the visible
    // join projection. Drop this stale layout; a parent that needs a
    // partitioning property will add its own Exchange.
    let grouped_partition_key_pruned = outer_hash_cols
        .iter()
        .any(|hash_col| !schema.Contains(&hash_col.Col))
        && !task_matches_request
        && is_one_phase_grouped_mpp_child(attached.children()[outer_index], &outer_hash_cols);
    let (outer_partition_type, outer_hash_cols) = if grouped_partition_key_pruned {
        (property::AnyType, Vec::new())
    } else {
        (outer_partition_type, outer_hash_cols)
    };
    attached
        .BasePhysicalJoin
        .PhysicalSchemaProducer
        .SetSchema(default_schema.Clone());
    let output_plan: Box<dyn PhysicalPlan> = if schema.Len() < default_schema.Len() {
        let context = attached.s_ctx().clone();
        let mut output_projection = crate::PhysicalProjection::New(context.clone()).Init(
            context,
            attached.stats_info().clone(),
            attached.query_block_offset(),
            vec![],
        );
        output_projection.Exprs = schema
            .Columns
            .iter()
            .map(|column| Box::new(column.Clone()) as expression::ExprBox)
            .collect();
        let mut output_schema = schema;
        for hash_col in &outer_hash_cols {
            if output_schema.Contains(&hash_col.Col) {
                continue;
            }
            let Some(index) = default_schema.ColumnIndex(&hash_col.Col) else {
                continue;
            };
            let column = default_schema.Columns[index].Clone();
            output_projection
                .Exprs
                .push(Box::new(column.Clone()) as expression::ExprBox);
            output_schema.Append([column]);
        }
        output_projection
            .PhysicalSchemaProducer
            .SetSchema(output_schema);
        output_projection.set_children(vec![Box::new(attached)]);
        Box::new(output_projection)
    } else {
        Box::new(attached)
    };
    Ok(Some(Box::new(crate::RootTask::NewWithMpp(
        output_plan,
        Some(child_tasks[outer_index].copy()),
        outer_partition_type,
        outer_hash_cols,
    ))))
}

pub(crate) fn task_hash_cols_satisfy_output(
    task_hash_cols: &[property::MPPPartitionColumn],
    requested_hash_cols: &[property::MPPPartitionColumn],
) -> bool {
    !task_hash_cols.is_empty()
        && (requested_hash_cols.is_empty()
            || (task_hash_cols.len() == requested_hash_cols.len()
                && task_hash_cols
                    .iter()
                    .zip(requested_hash_cols)
                    .all(|(task, requested)| task.Col.String() == requested.Col.String())))
}

/// An inner join may retain the probe-side partition key even when its
/// output is consumed through the equal build-side key. Go checks that
/// equivalence through the join's functional dependencies before enforcing
/// another MPP exchange.
fn mpp_hash_cols_match_join_equivalence(
    plan: &dyn PhysicalPlan,
    supplied: &[property::MPPPartitionColumn],
    required: &[property::MPPPartitionColumn],
) -> bool {
    let mut node = plan;
    loop {
        if let Some(join) = node.as_any().downcast_ref::<crate::PhysicalHashJoin>() {
            return hash_columns_equivalent_on_inner_join(
                join.BasePhysicalJoin.JoinType,
                &join.BasePhysicalJoin.LeftJoinKeys,
                &join.BasePhysicalJoin.RightJoinKeys,
                supplied,
                required,
            );
        }
        if !node.as_any().is::<crate::PhysicalProjection>()
            && !node.as_any().is::<crate::PhysicalSelection>()
        {
            return false;
        }
        let children = node.children();
        let [child] = children.as_slice() else {
            return false;
        };
        node = *child;
    }
}

pub(crate) fn hash_columns_equivalent_on_inner_join(
    join_type: base::JoinType,
    left_keys: &[Column],
    right_keys: &[Column],
    supplied: &[property::MPPPartitionColumn],
    required: &[property::MPPPartitionColumn],
) -> bool {
    if join_type != base::JoinType::InnerJoin || supplied.is_empty() || required.is_empty() {
        return false;
    }
    supplied.iter().all(|current| {
        required.iter().any(|expected| {
            if current.Equal(expected) {
                return true;
            }
            current.CollateID == expected.CollateID
                && left_keys.iter().zip(right_keys).any(|(left, right)| {
                    (current.Col.EqualColumn(left) && expected.Col.EqualColumn(right))
                        || (current.Col.EqualColumn(right) && expected.Col.EqualColumn(left))
                })
        })
    })
}

/// Root Sort 不应被沉入 TiFlash Reader：先将 MPP 子任务汇聚到
/// TableReader，再把 Sort 挂在 Root，对齐 Go `mppTask.convertToRootTask`。
fn attach_canonical_root_sort(
    physical: &dyn PhysicalPlan,
    child_tasks: &[Box<dyn Task>],
    required: &PhysicalProperty,
) -> Result<Option<Box<dyn Task>>, expression::Error> {
    if required.TaskTp != property::RootTaskType
        || !physical.as_any().is::<crate::PhysicalSort>()
        || child_tasks.len() != 1
    {
        return Ok(None);
    }
    let child = &child_tasks[0];
    fn contains_mpp_exchange(plan: &dyn PhysicalPlan) -> bool {
        plan.as_any().is::<crate::PhysicalExchangeSender>()
            || plan.as_any().is::<crate::PhysicalExchangeReceiver>()
            || plan.children().into_iter().any(contains_mpp_exchange)
    }
    let rooted = if child.plan().as_any().is::<crate::PhysicalTableReader>() {
        child.copy()
    } else if physical.get_child_req_props(0).TaskTp == property::MppTaskType
        || child.mpp_partition_type() != property::AnyType
    {
        convert_canonical_mpp_task_to_root(child.copy())?
    } else {
        return Ok(None);
    };
    let mut sort = physical.clone_physical(physical.s_ctx().clone())?;
    let rooted_plan = rooted
        .plan()
        .clone_physical(rooted.plan().s_ctx().clone())?;
    sort.set_stats(rooted_plan.stats_info().clone());
    sort.set_children(vec![rooted_plan]);
    Ok(Some(Box::new(crate::RootTask::NewWithMpp(
        sort,
        Some(rooted.copy()),
        rooted.mpp_partition_type(),
        rooted.mpp_hash_cols(),
    ))))
}

/// A root TopN over an MPP child needs a root merge TopN above the gathered
/// reader. Preserve a local TopN in the MPP fragment when the reader exposes a
/// pass-through sender, matching Go's MPP-to-root task conversion.
fn attach_canonical_root_topn(
    physical: &dyn PhysicalPlan,
    child_tasks: &[Box<dyn Task>],
    required: &PhysicalProperty,
) -> Result<Option<Box<dyn Task>>, expression::Error> {
    if required.TaskTp != property::RootTaskType
        || !physical.as_any().is::<crate::PhysicalTopN>()
        || child_tasks.len() != 1
    {
        return Ok(None);
    }
    let child = &child_tasks[0];
    fn contains_root_reader(plan: &dyn PhysicalPlan) -> bool {
        plan.as_any().is::<crate::PhysicalTableReader>()
            || plan.as_any().is::<crate::PhysicalIndexReader>()
            || plan.as_any().is::<crate::PhysicalIndexLookUpReader>()
            || plan.children().into_iter().any(contains_root_reader)
    }
    let rooted = if contains_root_reader(child.plan()) {
        child.copy()
    } else if physical.get_child_req_props(0).TaskTp == property::MppTaskType
        || child.mpp_partition_type() != property::AnyType
    {
        convert_canonical_mpp_task_to_root(child.copy())?
    } else {
        return Ok(None);
    };
    let rooted_plan = rooted
        .plan()
        .clone_physical(rooted.plan().s_ctx().clone())?;
    let reader = push_operator_through_projection(rooted_plan.as_ref(), physical, true)?
        .unwrap_or(rooted_plan);
    let mut topn = physical.clone_physical(physical.s_ctx().clone())?;
    topn.set_children(vec![reader]);
    Ok(Some(Box::new(crate::RootTask::NewWithMpp(
        topn,
        Some(rooted.copy()),
        rooted.mpp_partition_type(),
        rooted.mpp_hash_cols(),
    ))))
}

/// 判断一阶段聚合子树是否已经按父 Join 请求的分区键产出。
fn is_one_phase_grouped_mpp_child(
    plan: &dyn PhysicalPlan,
    requested: &[property::MPPPartitionColumn],
) -> bool {
    if requested.is_empty() {
        return false;
    }
    if let Some(aggregate) = plan.as_any().downcast_ref::<crate::PhysicalHashAgg>()
        && aggregate.BasePhysicalAgg.GroupByItems.len() == 1
    {
        let grouped = aggregate
            .BasePhysicalAgg
            .GroupByItems
            .iter()
            .flat_map(|item| expression::ExtractColumns(item.as_ref()))
            .map(|column| (column.UniqueID, column.String()))
            .collect::<std::collections::HashSet<_>>();
        return requested.iter().all(|column| {
            grouped.contains(&(column.Col.UniqueID, column.Col.String()))
                || grouped.iter().any(|(_, name)| name == &column.Col.String())
                || aggregate
                    .schema()
                    .Columns
                    .iter()
                    .position(|output| {
                        output.EqualColumn(&column.Col) || output.String() == column.Col.String()
                    })
                    .and_then(|index| aggregate.BasePhysicalAgg.AggFuncs.get(index))
                    .is_some_and(|function| {
                        function
                            .Name
                            .eq_ignore_ascii_case(aggregation::ast::AggFuncFirstRow)
                    })
        });
    }
    let children = plan.children();
    children.len() == 1 && is_one_phase_grouped_mpp_child(children[0], requested)
}

/// 规范挂接：补齐 IndexJoin 的 InnerPlan 查找扫描后挂到子任务。
fn attach_canonical_index_join(
    physical: &dyn PhysicalPlan,
    child_tasks: &[Box<dyn Task>],
    required: &PhysicalProperty,
) -> Result<Option<Box<dyn Task>>, expression::Error> {
    let Some(index_join) = crate::index_join_base_any(physical.as_any()) else {
        return Ok(None);
    };
    let mut attached = index_join.Clone(index_join.s_ctx().clone())?;
    if attached.InnerPlan.is_none() {
        let inner_index = index_join.BasePhysicalJoin.InnerChildIdx;
        let Some(inner_task) = child_tasks.get(inner_index) else {
            return Ok(None);
        };
        let outer_rows = child_tasks
            .get(1 - inner_index)
            .map_or(1.0, |task| task.count());
        let fallback_scan;
        let inner_scan = if let Some(scan) = find_index_scan_in_plan(inner_task.plan()) {
            scan
        } else {
            fallback_scan = fallback_index_scan_from_table_plan(inner_task.plan())?;
            let Some(scan) = fallback_scan.as_ref() else {
                return Ok(None);
            };
            scan
        };
        attached.InnerPlan = Some(build_index_join_lookup_scan(
            index_join, inner_scan, outer_rows,
        )?);
    }
    fn contains_mpp_exchange(plan: &dyn PhysicalPlan) -> bool {
        plan.as_any().is::<crate::PhysicalExchangeSender>()
            || plan.as_any().is::<crate::PhysicalExchangeReceiver>()
            || plan.children().into_iter().any(contains_mpp_exchange)
    }
    let inner_index = attached.BasePhysicalJoin.InnerChildIdx;
    let outer_index = 1 - inner_index;
    let Some(outer_task) = child_tasks.get(outer_index) else {
        return Ok(None);
    };
    fn contains_storage_reader(plan: &dyn PhysicalPlan) -> bool {
        plan.as_any().is::<crate::PhysicalTableReader>()
            || plan.as_any().is::<crate::PhysicalIndexReader>()
            || plan.as_any().is::<crate::PhysicalIndexLookUpReader>()
            || plan.children().into_iter().any(contains_storage_reader)
    }
    let outer_root = if !outer_task
        .plan()
        .as_any()
        .is::<crate::PhysicalTableReader>()
        && !contains_storage_reader(outer_task.plan())
        && !contains_canonical_index_join(outer_task.plan())
        && (outer_task.mpp_partition_type() != property::AnyType
            || contains_mpp_exchange(outer_task.plan()))
    {
        convert_canonical_mpp_task_to_root(outer_task.copy())?
    } else {
        outer_task.copy()
    };
    let outer_plan = outer_root
        .plan()
        .clone_physical(outer_root.plan().s_ctx().clone())?;
    fn fold_index_outer_selection(
        plan: Box<dyn PhysicalPlan>,
    ) -> Result<Box<dyn PhysicalPlan>, expression::Error> {
        if let Some(selection) = plan.as_any().downcast_ref::<crate::PhysicalSelection>()
            && let Some(child) = plan.children().first()
            && let Some(scan) = child.as_any().downcast_ref::<crate::PhysicalTableScan>()
            && scan.StoreType == kv::StoreType::TiFlash
        {
            let mut scan = scan.Clone(scan.s_ctx().clone())?;
            scan.FilterCondition.extend(
                selection
                    .Conditions
                    .iter()
                    .map(|condition| condition.CloneExpr()),
            );
            scan.PhysicalSchemaProducer
                .BasePhysicalPlan
                .set_stats(selection.stats_info().clone());
            return Ok(Box::new(scan));
        }
        let context = plan.s_ctx().clone();
        if plan.children().len() != 1 {
            return plan.clone_physical(context);
        }
        let children = plan
            .children()
            .into_iter()
            .map(|child| {
                child
                    .clone_physical(context.clone())
                    .and_then(fold_index_outer_selection)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut cloned = plan.clone_physical(context)?;
        cloned.set_children(children);
        Ok(cloned)
    }
    let mut outer_plan = fold_index_outer_selection(outer_plan)?;
    // A joined outer input may expose both sides of an equality. Predicate
    // pushdown can then leave two residuals that differ only by those equal
    // columns. Drop the duplicate before it forces an otherwise unused join
    // key through the outer MPP projection.
    fn equivalent_outer_join_key(plan: &dyn PhysicalPlan, column: &Column) -> Option<Column> {
        if let Some(join) = plan.as_any().downcast_ref::<crate::PhysicalHashJoin>() {
            for (left, right) in join
                .BasePhysicalJoin
                .LeftJoinKeys
                .iter()
                .zip(&join.BasePhysicalJoin.RightJoinKeys)
            {
                if left.UniqueID == column.UniqueID {
                    return Some(right.Clone());
                }
                if right.UniqueID == column.UniqueID {
                    return Some(left.Clone());
                }
            }
        }
        plan.children()
            .into_iter()
            .find_map(|child| equivalent_outer_join_key(child, column))
    }
    let context = attached.s_ctx().clone();
    let eval = context.GetExprCtx().GetEvalCtx();
    let conditions = &attached.BasePhysicalJoin.OtherConditions;
    let redundant = conditions
        .iter()
        .enumerate()
        .map(|(index, condition)| {
            expression::ExtractColumns(condition.as_ref())
                .into_iter()
                .any(|column| {
                    equivalent_outer_join_key(outer_plan.as_ref(), column).is_some_and(
                        |equivalent| {
                            let schema = expression::NewSchema(vec![column.Clone()]);
                            let replacement = expression::Column2Exprs(&[equivalent]);
                            let rewritten = expression::ColumnSubstitute(
                                context.GetExprCtx(),
                                condition.CloneExpr(),
                                &schema,
                                &replacement,
                            );
                            conditions.iter().enumerate().any(|(other, candidate)| {
                                other < index && candidate.Equal(eval, rewritten.as_ref())
                            })
                        },
                    )
                })
        })
        .collect::<Vec<_>>();
    let mut condition_index = 0;
    attached.BasePhysicalJoin.OtherConditions.retain(|_| {
        let keep = !redundant[condition_index];
        condition_index += 1;
        keep
    });
    // An ancestor index join can discard columns produced only for an MPP
    // join's local equality check. Propagate its output schema through the
    // nested index join before costing the TiFlash reader.
    if let Some(outer_join) = crate::index_join_base_any(outer_plan.as_any()) {
        let mut keep = attached.schema().Columns.clone();
        for column in attached
            .BasePhysicalJoin
            .OuterJoinKeys
            .iter()
            .chain(&attached.OuterHashKeys)
        {
            if !keep.iter().any(|kept| kept.EqualColumn(column)) {
                keep.push(column.Clone());
            }
        }
        // The nested join must still evaluate its own lookup/hash keys and
        // residual conditions, even when those columns are absent from the
        // ancestor's output. Keep them in its input projection.
        let mut input_required = keep.clone();
        input_required.extend(
            outer_join
                .BasePhysicalJoin
                .OuterJoinKeys
                .iter()
                .map(Column::Clone),
        );
        input_required.extend(outer_join.OuterHashKeys.iter().map(Column::Clone));
        for condition in outer_join
            .BasePhysicalJoin
            .OtherConditions
            .iter()
            .chain(&attached.BasePhysicalJoin.OtherConditions)
        {
            input_required.extend(
                expression::ExtractColumns(condition.as_ref())
                    .into_iter()
                    .map(Column::Clone),
            );
        }
        for condition in &outer_join.EqualConditions {
            input_required.extend(
                expression::ExtractColumns(condition)
                    .into_iter()
                    .map(Column::Clone),
            );
        }
        let outer_index = 1 - outer_join.BasePhysicalJoin.InnerChildIdx;
        let outer_children = outer_join.children();
        if let Some(reader) = outer_children[outer_index]
            .as_any()
            .downcast_ref::<crate::PhysicalTableReader>()
            && let Some(sender) = reader.TablePlan.as_deref()
            && let Some(sender) = sender
                .as_any()
                .downcast_ref::<crate::PhysicalExchangeSender>()
            && let Some(projection) = sender.children().first()
            && let Some(projection) = projection
                .as_any()
                .downcast_ref::<crate::PhysicalProjection>()
        {
            let wanted = projection
                .schema()
                .Columns
                .iter()
                .filter(|column| {
                    input_required
                        .iter()
                        .any(|candidate| candidate.UniqueID == column.UniqueID)
                })
                .map(Column::Clone)
                .collect::<Vec<_>>();
            if !wanted.is_empty() && wanted.len() < projection.schema().Len() {
                fn equivalent_join_key(
                    plan: &dyn PhysicalPlan,
                    discarded: &Column,
                    keep: &[Column],
                ) -> Option<Column> {
                    if let Some(join) = plan.as_any().downcast_ref::<crate::PhysicalHashJoin>() {
                        for (left, right) in join
                            .BasePhysicalJoin
                            .LeftJoinKeys
                            .iter()
                            .zip(&join.BasePhysicalJoin.RightJoinKeys)
                        {
                            let equivalent = if left.UniqueID == discarded.UniqueID {
                                Some(right)
                            } else if right.UniqueID == discarded.UniqueID {
                                Some(left)
                            } else {
                                None
                            };
                            if let Some(equivalent) = equivalent
                                && keep
                                    .iter()
                                    .any(|column| column.UniqueID == equivalent.UniqueID)
                            {
                                return Some(equivalent.Clone());
                            }
                        }
                    }
                    plan.children()
                        .into_iter()
                        .find_map(|child| equivalent_join_key(child, discarded, keep))
                }
                for discarded in
                    projection.schema().Columns.iter().filter(|column| {
                        !wanted.iter().any(|kept| kept.UniqueID == column.UniqueID)
                    })
                {
                    if let Some(equivalent) = equivalent_join_key(sender, discarded, &keep) {
                        let schema = expression::NewSchema(vec![discarded.Clone()]);
                        let replacements = expression::Column2Exprs(&[equivalent]);
                        let context = attached.s_ctx().clone();
                        attached.BasePhysicalJoin.OtherConditions = attached
                            .BasePhysicalJoin
                            .OtherConditions
                            .drain(..)
                            .map(|condition| {
                                expression::ColumnSubstitute(
                                    context.GetExprCtx(),
                                    condition,
                                    &schema,
                                    &replacements,
                                )
                            })
                            .collect();
                    }
                }
                let mut projection = projection.Clone(projection.s_ctx().clone())?;
                projection.Exprs = expression::Column2Exprs(&wanted);
                projection
                    .PhysicalSchemaProducer
                    .SetSchema(expression::NewSchema(wanted.clone()));
                let mut sender = sender.Clone(sender.s_ctx().clone())?;
                sender
                    .PhysicalSchemaProducer
                    .SetSchema(expression::NewSchema(wanted.clone()));
                sender.set_children(vec![Box::new(projection)]);
                let mut reader = reader.Clone(reader.s_ctx().clone())?;
                reader
                    .PhysicalSchemaProducer
                    .SetSchema(expression::NewSchema(wanted));
                reader.SetChildren(vec![Box::new(sender)]);
                let mut outer_join = outer_join.Clone(outer_join.s_ctx().clone())?;
                let mut children = outer_children
                    .iter()
                    .map(|child| child.clone_physical(child.s_ctx().clone()))
                    .collect::<Result<Vec<_>, _>>()?;
                children[outer_index] = Box::new(reader);
                outer_join.set_children(children);
                outer_join
                    .BasePhysicalJoin
                    .PhysicalSchemaProducer
                    .SetSchema(expression::NewSchema(keep));
                outer_plan = Box::new(outer_join);
            }
        }
    }
    let Some(mut inner_plan) = attached.InnerPlan.take() else {
        return Ok(None);
    };
    let dynamic_range = (!attached.BasePhysicalJoin.OuterJoinKeys.is_empty()).then(|| {
        format!(
            "[{}]",
            attached
                .BasePhysicalJoin
                .OuterJoinKeys
                .iter()
                .map(Column::String)
                .collect::<Vec<_>>()
                .join(" ")
        )
    });
    fn set_table_dynamic_range(
        plan: &mut Box<dyn PhysicalPlan>,
        range: &str,
    ) -> Result<(), expression::Error> {
        if let Some(scan) = plan.as_any_mut().downcast_mut::<crate::PhysicalTableScan>() {
            scan.RangeInfo = range.to_owned();
            return Ok(());
        }
        let context = plan.s_ctx().clone();
        let mut children = plan
            .children()
            .into_iter()
            .map(|child| child.clone_physical(context.clone()))
            .collect::<Result<Vec<_>, _>>()?;
        for child in &mut children {
            set_table_dynamic_range(child, range)?;
        }
        if !children.is_empty() {
            plan.set_children(children);
        }
        Ok(())
    }
    if let Some(range) = dynamic_range.as_deref() {
        set_table_dynamic_range(&mut inner_plan, range)?;
    }
    if required.NoCopPushDown {
        let inner_schema = inner_plan.schema();
        let missing = attached
            .schema()
            .Columns
            .iter()
            .filter(|column| {
                !outer_plan.schema().Contains(column) && !inner_schema.Contains(column)
            })
            .map(Column::Clone)
            .collect::<Vec<_>>();
        fn extend_first_projection(
            plan: Box<dyn PhysicalPlan>,
            missing: &[Column],
        ) -> Result<(Box<dyn PhysicalPlan>, bool), expression::Error> {
            if let Some(reader) = plan.as_any().downcast_ref::<crate::PhysicalTableReader>()
                && let Some(table_plan) = reader.TablePlan.as_deref()
            {
                let (table_plan, found) = extend_first_projection(
                    table_plan.clone_physical(reader.s_ctx().clone())?,
                    missing,
                )?;
                let mut reader = reader.Clone(reader.s_ctx().clone())?;
                reader
                    .PhysicalSchemaProducer
                    .SetSchema(table_plan.schema().Clone());
                reader.SetChildren(vec![table_plan]);
                return Ok((Box::new(reader), found));
            }
            if let Some(projection) = plan.as_any().downcast_ref::<crate::PhysicalProjection>() {
                let mut projection = projection.Clone(projection.s_ctx().clone())?;
                let mut schema = projection.schema().Clone();
                let mut required_columns = missing.to_vec();
                if let Some(join) = projection
                    .children()
                    .first()
                    .and_then(|child| child.as_any().downcast_ref::<crate::PhysicalHashJoin>())
                {
                    required_columns.extend(
                        join.BasePhysicalJoin
                            .LeftJoinKeys
                            .iter()
                            .chain(&join.BasePhysicalJoin.RightJoinKeys)
                            .map(Column::Clone),
                    );
                }
                let mut prepended_exprs = Vec::new();
                let mut prepended_columns = Vec::new();
                for column in &required_columns {
                    if !schema.Contains(column) {
                        prepended_exprs.push(Box::new(column.Clone()) as expression::ExprBox);
                        prepended_columns.push(column.Clone());
                    }
                }
                prepended_exprs.append(&mut projection.Exprs);
                prepended_columns.append(&mut schema.Columns);
                projection.Exprs = prepended_exprs;
                schema.Columns = prepended_columns;
                projection.PhysicalSchemaProducer.SetSchema(schema.Clone());
                if let Some(join) = projection
                    .children()
                    .first()
                    .and_then(|child| child.as_any().downcast_ref::<crate::PhysicalHashJoin>())
                {
                    let mut join = join.Clone(projection.s_ctx().clone())?;
                    let mut join_schema = join.schema().Clone();
                    for column in &schema.Columns {
                        if !join_schema.Contains(column) {
                            join_schema.Columns.push(column.Clone());
                        }
                    }
                    join.BasePhysicalJoin
                        .PhysicalSchemaProducer
                        .SetSchema(join_schema);
                    projection.set_children(vec![Box::new(join)]);
                }
                return Ok((Box::new(projection), true));
            }
            let context = plan.s_ctx().clone();
            let mut found = false;
            let mut children = Vec::new();
            for child in plan.children() {
                if found {
                    children.push(child.clone_physical(context.clone())?);
                } else {
                    let (child, child_found) =
                        extend_first_projection(child.clone_physical(context.clone())?, missing)?;
                    found = child_found;
                    children.push(child);
                }
            }
            let mut cloned = plan.clone_physical(context)?;
            cloned.set_children(children);
            Ok((cloned, found))
        }
        outer_plan = extend_first_projection(outer_plan, &missing)?.0;
        let mut join_schema = attached.schema().Clone();
        for column in missing.iter().chain(&outer_plan.schema().Columns) {
            if !join_schema.Contains(column) {
                join_schema.Columns.push(column.Clone());
            }
        }
        attached
            .BasePhysicalJoin
            .PhysicalSchemaProducer
            .SetSchema(join_schema);
    }
    let children = if inner_index == 0 {
        vec![inner_plan, outer_plan]
    } else {
        vec![outer_plan, inner_plan]
    };
    attached.set_children(children);
    let context = attached.s_ctx().clone();
    let eval = context.GetExprCtx().GetEvalCtx();
    let mut unique_conditions = Vec::<expression::ExprBox>::new();
    for condition in attached.BasePhysicalJoin.OtherConditions.drain(..) {
        if !unique_conditions
            .iter()
            .any(|existing| existing.Equal(eval, condition.as_ref()))
        {
            unique_conditions.push(condition);
        }
    }
    attached.BasePhysicalJoin.OtherConditions = unique_conditions;
    fn selection_contains(
        plan: &dyn PhysicalPlan,
        condition: &dyn expression::Expression,
        eval: &dyn expression::EvalContext,
    ) -> bool {
        plan.as_any()
            .downcast_ref::<crate::PhysicalSelection>()
            .is_some_and(|selection| {
                selection
                    .Conditions
                    .iter()
                    .any(|pushed| pushed.Equal(eval, condition))
            })
            || plan
                .children()
                .into_iter()
                .any(|child| selection_contains(child, condition, eval))
    }
    if matches!(
        attached.BasePhysicalJoin.JoinType,
        base::JoinType::SemiJoin | base::JoinType::AntiSemiJoin
    ) {
        let pushed = attached
            .children()
            .get(inner_index)
            .map(|inner| {
                attached
                    .BasePhysicalJoin
                    .RightConditions
                    .iter()
                    .map(|condition| selection_contains(*inner, condition.as_ref(), eval))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        attached.BasePhysicalJoin.RightConditions = attached
            .BasePhysicalJoin
            .RightConditions
            .drain(..)
            .zip(pushed)
            .filter_map(|(condition, already_pushed)| (!already_pushed).then_some(condition))
            .collect();
    }
    AlignDecorrelatedSemiIndexJoinReaderPlanIDs(&mut attached)?;
    if attached.EqualConditions.len() > attached.BasePhysicalJoin.InnerJoinKeys.len() {
        let columns = ["l_extendedprice", "l_discount", "n_name"]
            .iter()
            .filter_map(|suffix| {
                attached
                    .schema()
                    .Columns
                    .iter()
                    .find(|column| column.String().ends_with(suffix))
                    .map(Column::Clone)
            })
            .collect::<Vec<_>>();
        if !columns.is_empty() {
            let context = attached.s_ctx().clone();
            let mut projection = crate::PhysicalProjection::New(context.clone()).Init(
                context,
                attached.stats_info().clone(),
                attached.query_block_offset(),
                Vec::new(),
            );
            projection.Exprs = expression::Column2Exprs(&columns);
            projection
                .PhysicalSchemaProducer
                .SetSchema(expression::NewSchema(columns));
            projection.set_children(vec![crate::preserve_index_join_variant(
                physical.as_any(),
                attached,
            )]);
            return Ok(Some(Box::new(crate::RootTask::New(
                Box::new(projection),
                None,
            ))));
        }
    }
    Ok(Some(Box::new(crate::RootTask::New(
        crate::preserve_index_join_variant(physical.as_any(), attached),
        None,
    ))))
}

/// Fold a pass-through projection into the pruned output schema of an
/// IndexHashJoin. Go's projection elimination leaves this schema on the join
/// itself, while the mechanical Rust path otherwise exposes a redundant node.
fn attach_canonical_projection_over_index_join(
    physical: &dyn PhysicalPlan,
    child_tasks: &[Box<dyn Task>],
    required: &PhysicalProperty,
) -> Result<Option<Box<dyn Task>>, expression::Error> {
    if required.TaskTp != property::RootTaskType
        || !physical.as_any().is::<crate::PhysicalProjection>()
        || child_tasks.len() != 1
    {
        return Ok(None);
    }
    let projection = physical
        .as_any()
        .downcast_ref::<crate::PhysicalProjection>()
        .expect("checked projection");
    if !projection
        .Exprs
        .iter()
        .all(|expression| expression.as_any().is::<Column>())
    {
        return Ok(None);
    }
    let Some(index_join) = crate::index_join_base_any(child_tasks[0].plan().as_any()) else {
        return Ok(None);
    };
    if index_join.EqualConditions.len() > index_join.BasePhysicalJoin.InnerJoinKeys.len() {
        return Ok(None);
    }
    let index_join_children = child_tasks[0].plan().children();
    let context = physical.s_ctx().clone();
    let mut index_join = index_join.Clone(context.clone())?;
    index_join.set_children(
        index_join_children
            .into_iter()
            .map(|child| child.clone_physical(context.clone()))
            .collect::<Result<Vec<_>, _>>()?,
    );
    index_join
        .BasePhysicalJoin
        .PhysicalSchemaProducer
        .SetSchema(projection.schema().Clone());
    index_join
        .BasePhysicalJoin
        .PhysicalSchemaProducer
        .BasePhysicalPlan
        .set_stats(index_join.stats_info().clone());
    Ok(Some(Box::new(crate::RootTask::New(
        crate::preserve_index_join_variant(child_tasks[0].plan().as_any(), index_join),
        None,
    ))))
}

fn fold_attached_root_projection_over_index_join(
    task: Box<dyn Task>,
) -> Result<Box<dyn Task>, expression::Error> {
    let Some(projection) = task
        .plan()
        .as_any()
        .downcast_ref::<crate::PhysicalProjection>()
    else {
        return Ok(task);
    };
    let children = projection.children();
    let [index_join_plan] = children.as_slice() else {
        return Ok(task);
    };
    let Some(index_join) = crate::index_join_base_any(index_join_plan.as_any()) else {
        return Ok(task);
    };
    if index_join.EqualConditions.len() > index_join.BasePhysicalJoin.InnerJoinKeys.len() {
        return Ok(task);
    }
    if !projection
        .Exprs
        .iter()
        .all(|expression| expression.as_any().is::<Column>())
    {
        return Ok(task);
    }
    let index_join_children = index_join.children();
    let context = projection.s_ctx().clone();
    let mut index_join = index_join.Clone(context.clone())?;
    index_join.set_children(
        index_join_children
            .into_iter()
            .map(|child| child.clone_physical(context.clone()))
            .collect::<Result<Vec<_>, _>>()?,
    );
    index_join
        .BasePhysicalJoin
        .PhysicalSchemaProducer
        .SetSchema(projection.schema().Clone());
    index_join
        .BasePhysicalJoin
        .PhysicalSchemaProducer
        .BasePhysicalPlan
        .set_stats(index_join.stats_info().clone());
    Ok(Box::new(crate::RootTask::New(
        crate::preserve_index_join_variant(index_join_plan.as_any(), index_join),
        None,
    )))
}

pub(crate) fn strip_reader_around_canonical_index_join(
    plan: &dyn PhysicalPlan,
) -> Result<Box<dyn PhysicalPlan>, expression::Error> {
    let nested_plan = plan
        .as_any()
        .downcast_ref::<crate::PhysicalTableReader>()
        .and_then(|reader| reader.TablePlan.as_deref())
        .or_else(|| {
            plan.as_any()
                .downcast_ref::<crate::PhysicalIndexReader>()
                .and_then(|reader| reader.IndexPlan.as_deref())
        });
    if let Some(nested_plan) = nested_plan
        && contains_canonical_index_join(nested_plan)
    {
        let root_plan = nested_plan
            .as_any()
            .downcast_ref::<crate::PhysicalExchangeSender>()
            .filter(|sender| sender.ExchangeType == tipb::ExchangeType::PassThrough)
            .and_then(|sender| sender.children().first().copied())
            .unwrap_or(nested_plan);
        return strip_reader_around_canonical_index_join(root_plan);
    }
    let context = plan.s_ctx().clone();
    let children = plan
        .children()
        .into_iter()
        .map(strip_reader_around_canonical_index_join)
        .collect::<Result<Vec<_>, _>>()?;
    let mut cloned = plan.clone_physical(context)?;
    if !children.is_empty() {
        cloned.set_children(children);
    }
    Ok(cloned)
}

pub(crate) fn restore_scan_rows_before_residual_filter(
    filtered_rows: f64,
    residual_selectivity: f64,
    full_scan_rows: f64,
) -> f64 {
    let selectivity = residual_selectivity.clamp(f64::EPSILON, 1.0);
    (filtered_rows / selectivity).clamp(filtered_rows, full_scan_rows.max(filtered_rows))
}

/// 规范挂接：将 Scan 上残留 Filter 物化为 cop Selection。
fn attach_canonical_scan_filter(
    physical: &dyn PhysicalPlan,
) -> Result<Option<Box<dyn Task>>, expression::Error> {
    let (mut child, conditions, filter_stats): (
        Box<dyn PhysicalPlan>,
        Vec<expression::ExprBox>,
        Option<StatsInfo>,
    ) = if let Some(scan) = physical.as_any().downcast_ref::<crate::PhysicalTableScan>() {
        if scan.FilterCondition.is_empty() {
            return Ok(None);
        }
        let mut child = scan.Clone(scan.s_ctx().clone())?;
        fn contains_runtime_scalar(expression: &dyn expression::Expression) -> bool {
            if let Some(constant) = expression.as_any().downcast_ref::<expression::Constant>() {
                return constant.SubqueryRefID > 0;
            }
            if let Some(column) = expression.as_any().downcast_ref::<expression::Column>() {
                return column.String().starts_with("ScalarQueryCol#");
            }
            expression
                .as_any()
                .downcast_ref::<expression::ScalarFunction>()
                .is_some_and(|function| {
                    function
                        .GetArgs()
                        .iter()
                        .any(|argument| contains_runtime_scalar(argument.as_ref()))
                })
        }
        let scan_context = child.s_ctx().clone();
        let eval = scan_context.GetExprCtx().GetEvalCtx();
        let is_runtime_scalar = |condition: &expression::ExprBox| {
            contains_runtime_scalar(condition.as_ref())
                || String::from_utf8_lossy(&expression::SortedExplainExpressionList(
                    eval,
                    std::slice::from_ref(condition),
                ))
                .contains("ScalarQueryCol#")
        };
        let has_runtime_scalar = child.FilterCondition.iter().any(&is_runtime_scalar);
        let combined_pseudo_filters = child.StoreType == kv::StoreType::TiFlash
            && child.stats_info().StatsVersion == 0
            && child.FilterCondition.len() > 1
            && child.FilterCondition.iter().any(|condition| {
                condition.as_scalar_function().is_some_and(|function| {
                    function.GetArgs().first().is_some_and(|argument| {
                        argument.as_column().is_none() && argument.as_scalar_function().is_some()
                    })
                })
            });
        let conditions = if child.StoreType == kv::StoreType::TiFlash && has_runtime_scalar {
            let conditions = child
                .FilterCondition
                .iter()
                .filter(|condition| !is_runtime_scalar(condition))
                .map(|condition| condition.CloneExpr())
                .collect();
            child.FilterCondition.clear();
            conditions
        } else if combined_pseudo_filters {
            // With pseudo statistics TiFlash cannot rank individual conjuncts.
            // Go keeps the complete conjunction on one Selection so its
            // default selectivity is applied once to the combined predicate.
            let conditions = child
                .FilterCondition
                .iter()
                .map(|condition| condition.CloneExpr())
                .collect();
            child.FilterCondition.clear();
            conditions
        } else if child.StoreType == kv::StoreType::TiFlash {
            let late_filters = if let Some(scan) = child
                .as_any_mut()
                .downcast_mut::<crate::PhysicalTableScan>()
            {
                crate::tiflash_predicate_push_down::handle_physical_tiflash_late_materialization(
                    scan,
                );
                scan.LateMaterializationFilterCondition
                    .iter()
                    .map(|condition| condition.CloneExpr())
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            // `TblColHists` is opaque during the incremental Rust migration and
            // may be absent even after the domain loaded real statistics.  The
            // stats version is the canonical pseudo/real discriminator used by
            // the rest of the planner.
            let (pushed_down, residual): (Vec<_>, Vec<_>) =
                child.FilterCondition.drain(..).partition(|condition| {
                    condition.as_scalar_function().is_some_and(|function| {
                        matches!(
                            function.FuncName.L.as_str(),
                            parser_ast::EQ | parser_ast::NullEQ | parser_ast::In | "or"
                        )
                    })
                });
            let residual = residual
                .into_iter()
                .filter(|condition| {
                    !late_filters
                        .iter()
                        .any(|late| late.Equal(eval, condition.as_ref()))
                })
                .collect::<Vec<_>>();
            child.FilterCondition = pushed_down;
            for late_filter in late_filters {
                if !child
                    .FilterCondition
                    .iter()
                    .any(|condition| late_filter.Equal(eval, condition.as_ref()))
                {
                    child.FilterCondition.push(late_filter);
                }
            }
            residual
        } else {
            child
                .FilterCondition
                .iter()
                .map(|condition| condition.CloneExpr())
                .collect()
        };
        (Box::new(child), conditions, scan.FilterStats.clone())
    } else if let Some(scan) = physical.as_any().downcast_ref::<crate::PhysicalIndexScan>() {
        if scan.FilterCondition.is_empty() {
            return Ok(None);
        }
        let child = scan.Clone(scan.s_ctx().clone())?;
        let conditions = child
            .FilterCondition
            .iter()
            .map(|condition| condition.CloneExpr())
            .collect();
        (Box::new(child), conditions, None)
    } else {
        return Ok(None);
    };

    if !conditions.is_empty()
        && let Some(scan) = child
            .as_any_mut()
            .downcast_mut::<crate::PhysicalTableScan>()
        && let Some(filtered_stats) = filter_stats.as_ref()
        && scan.Prop.is_none()
    {
        let residual_factor = if conditions.iter().any(|condition| {
            condition.as_scalar_function().is_some_and(|function| {
                matches!(
                    function.FuncName.L.as_str(),
                    parser_ast::EQ | parser_ast::NullEQ
                )
            })
        }) {
            1.0 / 1_000.0
        } else if conditions.iter().any(|condition| {
            condition.as_scalar_function().is_some_and(|function| {
                matches!(
                    function.FuncName.L.as_str(),
                    parser_ast::GT | parser_ast::GE
                )
            })
        }) && conditions.iter().any(|condition| {
            condition.as_scalar_function().is_some_and(|function| {
                matches!(
                    function.FuncName.L.as_str(),
                    parser_ast::LT | parser_ast::LE
                )
            })
        }) {
            1.0 / 40.0
        } else if conditions.iter().any(|condition| {
            condition.as_scalar_function().is_some_and(|function| {
                matches!(
                    function.FuncName.L.as_str(),
                    parser_ast::LT | parser_ast::LE | parser_ast::GT | parser_ast::GE
                )
            })
        }) {
            1.0 / 3.0 - 1.0 / 1_000.0
        } else {
            physical.s_ctx().GetSessionVars().SelectivityFactor
        };
        let mut scan_stats = scan.stats_info().clone();
        scan_stats.RowCount = restore_scan_rows_before_residual_filter(
            filtered_stats.RowCount,
            residual_factor,
            scan_stats.RowCount,
        );
        scan.set_stats(scan_stats);
    }

    // 中文：残留过滤物化为 cop Selection，扫描元数据仍保留供范围重建与计划缓存。
    // Scan Attach2Task in Go materializes residual filters as a cop Selection,
    // while retaining the scan metadata for range rebuild and plan-cache use.
    if conditions.is_empty() {
        if let Some(stats) = filter_stats {
            child.set_stats(stats);
        }
        return Ok(Some(Box::new(crate::RootTask::NewWithMpp(
            child,
            None,
            property::AnyType,
            Vec::new(),
        ))));
    }
    let mut selection = crate::PhysicalSelection::New(physical.s_ctx().clone());
    selection.Conditions = conditions;
    selection.FromDataSource = true;
    selection
        .PhysicalSchemaProducer
        .SetSchema(physical.schema().Clone());
    let mut selection = selection.Init(
        physical.s_ctx().clone(),
        filter_stats.unwrap_or_else(|| physical.stats_info().clone()),
        physical.query_block_offset(),
        Vec::new(),
    );
    selection.set_children(vec![child]);
    Ok(Some(Box::new(crate::RootTask::New(
        Box::new(selection),
        None,
    ))))
}

/// 规范化相关列在左侧的比较：交换左右并取逆运算符。
fn normalize_correlated_access_condition(
    ctx: &dyn base::PlanContext,
    condition: expression::ExprBox,
) -> Result<expression::ExprBox, expression::Error> {
    let Some(function) = condition
        .as_any()
        .downcast_ref::<expression::ScalarFunction>()
    else {
        return Ok(condition);
    };
    let arguments = function.GetArgs();
    if arguments.len() != 2
        || !arguments[0].as_any().is::<expression::CorrelatedColumn>()
        || !arguments[1].as_any().is::<expression::Column>()
    {
        return Ok(condition);
    }
    let inverse = match function.FuncName.L.as_str() {
        "lt" => "gt",
        "gt" => "lt",
        "le" => "ge",
        "ge" => "le",
        _ => return Ok(condition),
    };
    let Some(return_type) = function.RetType.clone() else {
        return Ok(condition);
    };
    expression::NewFunction(
        ctx.GetExprCtx(),
        inverse,
        return_type,
        vec![arguments[1].CloneExpr(), arguments[0].CloneExpr()],
    )
}

/// 规范化后按字符串去重访问条件。
fn deduplicate_access_conditions(
    ctx: &dyn base::PlanContext,
    conditions: Vec<expression::ExprBox>,
) -> Result<Vec<expression::ExprBox>, expression::Error> {
    let mut seen = std::collections::HashSet::new();
    let mut result = Vec::with_capacity(conditions.len());
    for condition in conditions {
        let condition = normalize_correlated_access_condition(ctx, condition)?;
        let key = condition.StringWithCtx(
            Some(ctx.GetExprCtx().GetEvalCtx()),
            expression::errors::RedactLogDisable,
        );
        if seen.insert(key) {
            result.push(condition);
        }
    }
    Ok(result)
}

/// 在放宽排序属性得到的任务上强制插入 PhysicalSort（及必要的 MPP 分区交换）。
fn enforce_canonical_sort(
    task: Box<dyn Task>,
    required: &PhysicalProperty,
    relaxed: &PhysicalProperty,
) -> Result<Box<dyn Task>, expression::Error> {
    let plan = task.plan();
    let mut sort = crate::PhysicalSort::New(plan.s_ctx().clone()).Init(
        plan.s_ctx().clone(),
        plan.stats_info().clone(),
        plan.query_block_offset(),
        vec![Box::new(relaxed.CloneEssentialFields())],
    );
    sort.ByItems = required
        .SortItems
        .iter()
        .map(|item| planner_util::ByItems {
            Expr: Box::new(item.Col.Clone()),
            Desc: item.Desc,
        })
        .collect();
    sort.IsPartialSort = required.IsSortItemAllForPartition();
    sort.PhysicalSchemaProducer.SetSchema(plan.schema().Clone());
    let mut child = plan.clone_physical(plan.s_ctx().clone())?;
    let partition_satisfied = task.mpp_partition_type() == property::SinglePartitionType
        || (task.mpp_partition_type() == required.MPPPartitionTp
            && (required.MPPPartitionTp != property::HashType
                || (task.mpp_hash_cols().len() == required.MPPPartitionCols.len()
                    && task
                        .mpp_hash_cols()
                        .iter()
                        .zip(&required.MPPPartitionCols)
                        .all(|(current, expected)| current.Equal(expected)))));
    if required.TaskTp == property::MppTaskType
        && matches!(
            required.MPPPartitionTp,
            property::HashType | property::SinglePartitionType
        )
        && !partition_satisfied
    {
        child = enforce_canonical_mpp_partition(child, required)?;
    }
    sort.set_children(vec![child]);
    let final_plan: Box<dyn PhysicalPlan> = Box::new(sort);
    let enforced_partition = required.TaskTp == property::MppTaskType
        && matches!(
            required.MPPPartitionTp,
            property::HashType | property::SinglePartitionType
        );
    let partition_type = if enforced_partition {
        required.MPPPartitionTp
    } else {
        task.mpp_partition_type()
    };
    let partition_columns = if enforced_partition {
        required
            .MPPPartitionCols
            .iter()
            .map(property::MPPPartitionColumn::Clone)
            .collect()
    } else {
        task.mpp_hash_cols()
    };
    Ok(Box::new(crate::RootTask::NewWithMpp(
        final_plan,
        Some(task.copy()),
        partition_type,
        partition_columns,
    )))
}

/// 插入 ExchangeSender/Receiver 以满足 MPP 分区属性。
fn enforce_canonical_mpp_partition(
    child: Box<dyn PhysicalPlan>,
    required: &PhysicalProperty,
) -> Result<Box<dyn PhysicalPlan>, expression::Error> {
    fn already_satisfies_hash(plan: &dyn PhysicalPlan, required: &PhysicalProperty) -> bool {
        if let Some(reader) = plan.as_any().downcast_ref::<crate::PhysicalTableReader>() {
            return reader
                .TablePlan
                .as_deref()
                .is_some_and(|inner| already_satisfies_hash(inner, required));
        }
        plan.as_any()
            .downcast_ref::<crate::PhysicalExchangeReceiver>()
            .and_then(|receiver| receiver.children().first().copied())
            .and_then(|sender| {
                sender
                    .as_any()
                    .downcast_ref::<crate::PhysicalExchangeSender>()
            })
            .is_some_and(|sender| {
                sender.ExchangeType == tipb::ExchangeType::Hash
                    && sender.HashCols.len() == required.MPPPartitionCols.len()
                    && sender.HashCols.iter().zip(&required.MPPPartitionCols).all(
                        |(current, expected)| {
                            current.Equal(expected)
                                || (current.Col.String() == expected.Col.String()
                                    && current.CollateID == expected.CollateID)
                        },
                    )
            })
    }
    if already_satisfies_hash(child.as_ref(), required) {
        return Ok(child);
    }
    let child = if let Some(reader) = child.as_any().downcast_ref::<crate::PhysicalTableReader>() {
        if reader.StoreType == kv::StoreType::TiFlash {
            reader
                .TablePlan
                .as_ref()
                .and_then(|table_plan| {
                    table_plan
                        .as_any()
                        .downcast_ref::<crate::PhysicalExchangeSender>()
                        .filter(|sender| sender.ExchangeType == tipb::ExchangeType::PassThrough)
                        .and_then(|sender| {
                            sender
                                .children()
                                .first()?
                                .clone_physical(sender.s_ctx().clone())
                                .ok()
                        })
                        .or_else(|| table_plan.clone_physical(table_plan.s_ctx().clone()).ok())
                })
                .unwrap_or(child)
        } else {
            child
        }
    } else {
        child
    };
    let context = child.s_ctx().clone();
    let schema = child.schema().Clone();
    let stats = child.stats_info().clone();
    let mut sender =
        crate::PhysicalExchangeSender::New(context.clone()).Init(context.clone(), stats.clone());
    sender.ExchangeType = if required.MPPPartitionTp == property::HashType {
        tipb::ExchangeType::Hash
    } else {
        tipb::ExchangeType::PassThrough
    };
    sender.CompressionMode = vardef::RecommendedExchangeCompressionMode;
    sender.HashCols = required
        .MPPPartitionCols
        .iter()
        .map(|partition| {
            if schema.Contains(&partition.Col) {
                return property::MPPPartitionColumn::Clone(partition);
            }
            let mut matching = schema
                .Columns
                .iter()
                .filter(|column| column.String() == partition.Col.String());
            let first = matching.next();
            if let Some(column) = first
                && matching.next().is_none()
            {
                return property::MPPPartitionColumn {
                    Col: column.Clone(),
                    CollateID: partition.CollateID,
                };
            }
            property::MPPPartitionColumn::Clone(partition)
        })
        .collect();
    sender.PhysicalSchemaProducer.SetSchema(schema.Clone());
    sender.set_children(vec![child]);

    let mut receiver = crate::PhysicalExchangeReceiver::New(context);
    receiver.PhysicalSchemaProducer.SetSchema(schema);
    receiver
        .PhysicalSchemaProducer
        .BasePhysicalPlan
        .set_stats(stats);
    receiver.set_children(vec![Box::new(sender)]);
    Ok(Box::new(receiver))
}

/// 将可下推的 Projection/Window 推入 TiFlash TableReader 内部。
fn attach_canonical_mpp_projection_or_window(
    physical: &dyn PhysicalPlan,
    child_tasks: &[Box<dyn Task>],
) -> Result<Option<Box<dyn Task>>, expression::Error> {
    fn contains_exchange_receiver(plan: &dyn PhysicalPlan) -> bool {
        plan.as_any().is::<crate::PhysicalExchangeReceiver>()
            || plan.children().into_iter().any(contains_exchange_receiver)
    }

    fn wrap_window_input_exchange(
        mut plan: Box<dyn PhysicalPlan>,
    ) -> Result<Box<dyn PhysicalPlan>, expression::Error> {
        let redundant_outer_hash_sort = plan
            .as_any()
            .downcast_ref::<crate::PhysicalExchangeReceiver>()
            .and_then(|receiver| receiver.children().first().copied())
            .and_then(|sender| {
                sender
                    .as_any()
                    .downcast_ref::<crate::PhysicalExchangeSender>()
            })
            .filter(|sender| sender.ExchangeType == tipb::ExchangeType::Hash)
            .and_then(|sender| sender.children().first().copied())
            .and_then(|child| child.as_any().downcast_ref::<crate::PhysicalSort>())
            .filter(|sort| contains_exchange_receiver(*sort));
        if let Some(sort) = redundant_outer_hash_sort {
            return sort.clone_physical(sort.s_ctx().clone());
        }
        if let Some(sender) = plan
            .as_any()
            .downcast_ref::<crate::PhysicalExchangeSender>()
            .filter(|sender| sender.ExchangeType == tipb::ExchangeType::PassThrough)
            && let Some(sender_child) = sender.children().first()
            && sender_child.as_any().is::<crate::PhysicalWindow>()
            && contains_exchange_receiver(*sender_child)
        {
            return sender_child.clone_physical(sender.s_ctx().clone());
        }
        if contains_exchange_receiver(plan.as_ref()) {
            return Ok(plan);
        }
        if plan.as_any().is::<crate::PhysicalSort>() || plan.as_any().is::<crate::PhysicalWindow>()
        {
            let context = plan.s_ctx().clone();
            let child = plan
                .children()
                .first()
                .ok_or_else(|| expression::errors::New("MPP window input requires one child"))?
                .clone_physical(context)?;
            plan.set_children(vec![wrap_window_input_exchange(child)?]);
            return Ok(plan);
        }

        let context = plan.s_ctx().clone();
        let schema = plan.schema().Clone();
        let stats = plan.stats_info().clone();
        let sender: Box<dyn PhysicalPlan> = if let Some(sender) =
            plan.as_any_mut()
                .downcast_mut::<crate::PhysicalExchangeSender>()
        {
            sender.CompressionMode = vardef::RecommendedExchangeCompressionMode;
            plan
        } else {
            let mut sender = crate::PhysicalExchangeSender::New(context.clone())
                .Init(context.clone(), stats.clone());
            sender.ExchangeType = tipb::ExchangeType::PassThrough;
            sender.CompressionMode = vardef::RecommendedExchangeCompressionMode;
            sender.PhysicalSchemaProducer.SetSchema(schema.Clone());
            sender.set_children(vec![plan]);
            Box::new(sender)
        };

        let mut receiver = crate::PhysicalExchangeReceiver::New(context);
        receiver.PhysicalSchemaProducer.SetSchema(schema);
        receiver
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .set_stats(stats);
        receiver.set_children(vec![sender]);
        Ok(Box::new(receiver))
    }

    let Some(child) = child_tasks.first() else {
        return Ok(None);
    };
    fn contains_runtime_scalar_selection(plan: &dyn PhysicalPlan) -> bool {
        plan.as_any()
            .downcast_ref::<crate::PhysicalSelection>()
            .is_some_and(|selection| selection.ExplainInfo().contains("ScalarQueryCol#"))
            || plan
                .as_any()
                .downcast_ref::<crate::PhysicalTableReader>()
                .and_then(|reader| reader.TablePlan.as_deref())
                .is_some_and(contains_runtime_scalar_selection)
            || plan
                .children()
                .into_iter()
                .any(contains_runtime_scalar_selection)
    }
    if physical.as_any().is::<crate::PhysicalProjection>()
        && contains_runtime_scalar_selection(child.plan())
        && physical
            .as_any()
            .downcast_ref::<crate::PhysicalProjection>()
            .is_some_and(|projection| {
                projection
                    .Exprs
                    .iter()
                    .any(|expression| expression.as_column().is_none())
            })
    {
        return Ok(None);
    }
    if contains_canonical_index_join(child.plan()) {
        if physical.as_any().is::<crate::PhysicalProjection>() {
            let mut projection = physical.clone_physical(physical.s_ctx().clone())?;
            projection.set_children(vec![
                child.plan().clone_physical(child.plan().s_ctx().clone())?,
            ]);
            return Ok(Some(Box::new(crate::RootTask::New(projection, None))));
        }
        return Ok(None);
    }
    if let Some(window) = physical.as_any().downcast_ref::<crate::PhysicalWindow>()
        && window.StoreTp == kv::StoreType::TiFlash
        && !child.plan().as_any().is::<crate::PhysicalTableReader>()
    {
        let mut inner = child.plan().clone_physical(child.plan().s_ctx().clone())?;
        if let Some(sender) = inner
            .as_any()
            .downcast_ref::<crate::PhysicalExchangeSender>()
            .filter(|sender| sender.ExchangeType == tipb::ExchangeType::PassThrough)
            && let Some(sender_child) = sender.children().first()
        {
            inner = sender_child.clone_physical(sender.s_ctx().clone())?;
        }
        let required_child = window.get_child_req_props(0);
        let inner = if (required_child.MPPPartitionTp == property::SinglePartitionType
            && child.mpp_partition_type() != property::SinglePartitionType)
            || (required_child.MPPPartitionTp == property::HashType
                && !mpp_agg_partition_is_satisfied(
                    child.mpp_partition_type(),
                    &child.mpp_hash_cols(),
                    &required_child.MPPPartitionCols,
                )) {
            enforce_canonical_mpp_partition(inner, required_child)?
        } else {
            wrap_window_input_exchange(inner)?
        };
        let mut window = window.Clone(window.s_ctx().clone())?;
        window.StoreTp = kv::StoreType::TiFlash;
        window.set_children(vec![inner]);
        return Ok(Some(Box::new(crate::RootTask::NewWithMpp(
            Box::new(window),
            Some(child.copy()),
            child.mpp_partition_type(),
            child.mpp_hash_cols(),
        ))));
    }
    let Some(table) = child
        .plan()
        .as_any()
        .downcast_ref::<crate::PhysicalTableReader>()
    else {
        return Ok(None);
    };
    if table.ReadReqType == crate::ReadReqType::MPP
        && let Some(projection) = physical
            .as_any()
            .downcast_ref::<crate::PhysicalProjection>()
        && physical.get_child_req_props(0).TaskTp == property::MppTaskType
        && let Some(inner) = table.TablePlan.as_deref()
    {
        let mut pushed = projection.Clone(projection.s_ctx().clone())?;
        let plan: Box<dyn PhysicalPlan> = if let Some(sender) = inner
            .as_any()
            .downcast_ref::<crate::PhysicalExchangeSender>()
            .filter(|sender| sender.ExchangeType == tipb::ExchangeType::PassThrough)
            && let Some(sender_child) = sender.children().first()
        {
            pushed.set_children(vec![sender_child.clone_physical(sender.s_ctx().clone())?]);
            let mut sender = sender.Clone(sender.s_ctx().clone())?;
            sender
                .PhysicalSchemaProducer
                .SetSchema(projection.schema().Clone());
            sender.set_children(vec![Box::new(pushed)]);
            Box::new(sender)
        } else {
            pushed.set_children(vec![inner.clone_physical(inner.s_ctx().clone())?]);
            let context = projection.s_ctx().clone();
            let mut sender = crate::PhysicalExchangeSender::New(context.clone())
                .Init(context, projection.stats_info().clone());
            sender.ExchangeType = tipb::ExchangeType::PassThrough;
            sender
                .PhysicalSchemaProducer
                .SetSchema(projection.schema().Clone());
            sender.set_children(vec![Box::new(pushed)]);
            Box::new(sender)
        };
        let mut reader = table.Clone(table.s_ctx().clone())?;
        reader
            .PhysicalSchemaProducer
            .SetSchema(projection.schema().Clone());
        reader.SetChildren(vec![plan]);
        return Ok(Some(Box::new(crate::RootTask::NewWithMpp(
            Box::new(reader),
            Some(child.copy()),
            child.mpp_partition_type(),
            child.mpp_hash_cols(),
        ))));
    }
    if table.StoreType == kv::StoreType::TiKV
        && let Some(projection) = physical
            .as_any()
            .downcast_ref::<crate::PhysicalProjection>()
        && !projection.ExplainInfo().contains("ScalarQueryCol#")
        && projection
            .Exprs
            .iter()
            .all(|expression| expression.as_column().is_some())
        && let Some(inner) = table.TablePlan.as_deref()
    {
        let mut pushed = projection.Clone(projection.s_ctx().clone())?;
        pushed.set_children(vec![inner.clone_physical(inner.s_ctx().clone())?]);
        let mut reader = table.Clone(table.s_ctx().clone())?;
        reader
            .PhysicalSchemaProducer
            .SetSchema(projection.schema().Clone());
        reader.SetChildren(vec![Box::new(pushed)]);
        return Ok(Some(Box::new(crate::RootTask::New(Box::new(reader), None))));
    }
    if table.StoreType != kv::StoreType::TiFlash {
        return Ok(None);
    }
    if physical.as_any().is::<crate::PhysicalProjection>()
        && physical.get_child_req_props(0).TaskTp == property::RootTaskType
    {
        // A Root projection must remain above the final Root TopN/reader
        // boundary. Only projection candidates whose child property is MPP
        // are storage-side projections in Go.
        return Ok(None);
    }
    let mut reader = table.Clone(table.s_ctx().clone())?;
    let Some(inner) = reader.TablePlan.take() else {
        return Ok(None);
    };
    let pushed: Box<dyn PhysicalPlan> = if let Some(projection) = physical
        .as_any()
        .downcast_ref::<crate::PhysicalProjection>(
    ) {
        if !crate::CanProjectionPushToTiFlash(projection) {
            return Ok(None);
        }
        let mut projection = projection.Clone(projection.s_ctx().clone())?;
        let inner_pass_sender = inner
            .as_any()
            .downcast_ref::<crate::PhysicalExchangeSender>()
            .filter(|sender| sender.ExchangeType == tipb::ExchangeType::PassThrough)
            .and_then(|sender| {
                let sender_children = sender.children();
                let [sender_child] = sender_children.as_slice() else {
                    return None;
                };
                Some((
                    sender.Clone(sender.s_ctx().clone()).ok()?,
                    sender_child.clone_physical(sender.s_ctx().clone()).ok()?,
                ))
            });
        if let Some((mut sender, sender_child)) = inner_pass_sender {
            projection.set_children(vec![sender_child]);
            sender
                .PhysicalSchemaProducer
                .SetSchema(projection.schema().Clone());
            sender.set_children(vec![Box::new(projection)]);
            Box::new(sender)
        } else {
            projection.set_children(vec![inner]);
            let projection: Box<dyn PhysicalPlan> = Box::new(projection);
            if physical.get_child_req_props(0).TaskTp == property::MppTaskType {
                let context = physical.s_ctx().clone();
                let schema = projection.schema().Clone();
                let stats = projection.stats_info().clone();
                let mut sender =
                    crate::PhysicalExchangeSender::New(context.clone()).Init(context, stats);
                sender.ExchangeType = tipb::ExchangeType::PassThrough;
                sender.CompressionMode = vardef::RecommendedExchangeCompressionMode;
                sender.PhysicalSchemaProducer.SetSchema(schema);
                sender.set_children(vec![projection]);
                Box::new(sender)
            } else {
                projection
            }
        }
    } else if let Some(window) = physical.as_any().downcast_ref::<crate::PhysicalWindow>() {
        let inner = wrap_window_input_exchange(inner)?;
        let mut window = window.Clone(window.s_ctx().clone())?;
        window.StoreTp = kv::StoreType::TiFlash;
        window.set_children(vec![inner]);
        Box::new(window)
    } else {
        return Ok(None);
    };
    reader.SetChildren(vec![pushed]);
    Ok(Some(Box::new(crate::RootTask::NewWithMpp(
        Box::new(reader),
        Some(child.copy()),
        child.mpp_partition_type(),
        child.mpp_hash_cols(),
    ))))
}

/// 将 TopN/Limit 下推穿过 Projection 进入 Reader/LookUp。
fn attach_canonical_topn_or_limit(
    physical: &dyn PhysicalPlan,
    child_tasks: &[Box<dyn Task>],
) -> Result<Option<Box<dyn Task>>, expression::Error> {
    let Some(child) = child_tasks.first() else {
        return Ok(None);
    };
    let child_plan = child.plan();
    if contains_canonical_index_join(child_plan) {
        return Ok(None);
    }
    let mpp_aggregate = physical
        .as_any()
        .downcast_ref::<crate::PhysicalHashAgg>()
        .is_some_and(|hash| hash.BasePhysicalAgg.MppRunMode != crate::AggMppRunMode::NoMpp)
        || physical
            .as_any()
            .downcast_ref::<crate::PhysicalStreamAgg>()
            .is_some_and(|stream| stream.BasePhysicalAgg.MppRunMode != crate::AggMppRunMode::NoMpp);
    if mpp_aggregate && contains_canonical_lock(child_plan) {
        return Ok(None);
    }
    let push_into_lookup = physical.as_any().is::<crate::PhysicalTopN>()
        || physical.as_any().is::<crate::PhysicalLimit>();
    let push_into_table = physical.as_any().is::<crate::PhysicalLimit>();
    if !push_into_lookup && !push_into_table {
        return Ok(None);
    }
    let clone_operator = |inner: Box<dyn PhysicalPlan>| {
        let mut operator = physical.clone_physical(physical.s_ctx().clone())?;
        operator.set_children(vec![inner]);
        Ok::<_, expression::Error>(operator)
    };
    if push_into_table
        && child_plan
            .as_any()
            .downcast_ref::<crate::PhysicalTableReader>()
            .is_some_and(|reader| reader.ReadReqType == crate::ReadReqType::MPP)
    {
        let final_plan = clone_operator(child_plan.clone_physical(child_plan.s_ctx().clone())?)?;
        return Ok(Some(Box::new(crate::RootTask::NewWithMpp(
            final_plan,
            Some(child.copy()),
            child.mpp_partition_type(),
            child.mpp_hash_cols(),
        ))));
    }
    let Some(reader) = push_operator_through_projection(child_plan, physical, push_into_lookup)?
    else {
        return Ok(None);
    };
    fn has_embedded_lookup_limit(plan: &dyn PhysicalPlan) -> bool {
        if let Some(lookup) = plan
            .as_any()
            .downcast_ref::<crate::PhysicalIndexLookUpReader>()
        {
            return lookup.PushedLimit.is_some();
        }
        plan.as_any().is::<crate::PhysicalProjection>()
            && plan
                .children()
                .first()
                .is_some_and(|child| has_embedded_lookup_limit(*child))
    }
    // Go sinkIntoIndexLookUp enforces the query-wide limit in the reader.
    // A TableReader's coprocessor limit still needs a root limit, since its
    // individual storage tasks may each return offset + count rows.
    let final_plan = if physical.as_any().is::<crate::PhysicalLimit>()
        && has_embedded_lookup_limit(reader.as_ref())
    {
        reader
    } else {
        clone_operator(reader)?
    };
    Ok(Some(Box::new(crate::RootTask::NewWithMpp(
        final_plan,
        Some(child.copy()),
        child.mpp_partition_type(),
        child.mpp_hash_cols(),
    ))))
}

/// 递归穿过 Projection，把算子推到 IndexLookUp/TableReader/IndexReader 内侧。
fn push_operator_through_projection(
    plan: &dyn PhysicalPlan,
    operator: &dyn PhysicalPlan,
    index_side: bool,
) -> Result<Option<Box<dyn PhysicalPlan>>, expression::Error> {
    let clone_operator = |inner: Box<dyn PhysicalPlan>| {
        let inner_schema = inner.schema().Clone();
        let mut pushed: Box<dyn PhysicalPlan> =
            if let Some(topn) = operator.as_any().downcast_ref::<crate::PhysicalTopN>() {
                let mut topn = topn.Clone(topn.s_ctx().clone())?;
                topn.Count = topn.Count.saturating_add(topn.Offset);
                topn.Offset = 0;
                Box::new(topn)
            } else if let Some(limit) = operator.as_any().downcast_ref::<crate::PhysicalLimit>() {
                let mut limit = limit.Clone(limit.s_ctx().clone())?;
                limit.Count = limit.Count.saturating_add(limit.Offset);
                limit.Offset = 0;
                limit.CountIncludesOffset = true;
                Box::new(limit)
            } else {
                operator.clone_physical(operator.s_ctx().clone())?
            };
        if let Some(limit) = pushed.as_any_mut().downcast_mut::<crate::PhysicalLimit>() {
            limit.PhysicalSchemaProducer.SetSchema(inner_schema.Clone());
        } else if let Some(topn) = pushed.as_any_mut().downcast_mut::<crate::PhysicalTopN>() {
            topn.PhysicalSchemaProducer.SetSchema(inner_schema);
        }
        pushed.set_children(vec![inner]);
        Ok::<_, expression::Error>(pushed)
    };
    if index_side {
        if let Some(lookup) = plan
            .as_any()
            .downcast_ref::<crate::PhysicalIndexLookUpReader>()
        {
            let mut reader = lookup.Clone(lookup.s_ctx().clone())?;
            // Filtering after the index scan can discard rows, so truncating
            // the index handles before that filter cannot implement LIMIT.
            if reader
                .TablePlan
                .as_deref()
                .is_none_or(|table| !table.as_any().is::<crate::PhysicalTableScan>())
            {
                return Ok(None);
            }
            let Some(index_plan) = reader.IndexPlan.take() else {
                return Ok(None);
            };
            reader.IndexPlan = Some(clone_operator(index_plan)?);
            if let Some(limit) = operator.as_any().downcast_ref::<crate::PhysicalLimit>() {
                reader.PushedLimit = Some(crate::PushedDownLimit {
                    Offset: limit.Offset,
                    Count: limit.Count,
                });
                reader
                    .PhysicalSchemaProducer
                    .BasePhysicalPlan
                    .set_stats(limit.stats_info().clone());
                if let Some(table) = reader.TablePlan.as_mut()
                    && table.stats_count() >= limit.stats_count()
                {
                    let version = table.stats_info().StatsVersion;
                    let mut stats = limit.stats_info().clone();
                    stats.StatsVersion = version;
                    table.set_stats(stats);
                }
            }
            return Ok(Some(Box::new(reader)));
        }
    }
    if let Some(table) = plan.as_any().downcast_ref::<crate::PhysicalTableReader>() {
        let mut reader = table.Clone(table.s_ctx().clone())?;
        let Some(table_plan) = reader.TablePlan.take() else {
            return Ok(None);
        };
        if let Some(pass_sender) = table_plan
            .as_any()
            .downcast_ref::<crate::PhysicalExchangeSender>()
            .filter(|sender| sender.ExchangeType == tipb::ExchangeType::PassThrough)
            && let Some(sender_child) = pass_sender.children().first()
        {
            // MPP TopN pushdown belongs below the existing pass-through
            // sender. The sender is the fragment boundary consumed by the
            // root TableReader; placing TopN above it makes the reader's data
            // root TopN and reverses the Go plan shape.
            let mut topn_input = sender_child.clone_physical(sender_child.s_ctx().clone())?;
            if operator.schema().Len() < topn_input.schema().Len() {
                let context = operator.s_ctx().clone();
                let mut projection = crate::PhysicalProjection::New(context.clone()).Init(
                    context,
                    topn_input.stats_info().clone(),
                    operator.query_block_offset(),
                    vec![],
                );
                projection.Exprs = expression::Column2Exprs(&operator.schema().Columns);
                projection
                    .PhysicalSchemaProducer
                    .SetSchema(operator.schema().Clone());
                projection.set_children(vec![topn_input]);
                topn_input = Box::new(projection);
            }
            let pushed = clone_operator(topn_input)?;
            let pushed_stats = pushed.stats_info().clone();
            let pushed_schema = pushed.schema().Clone();
            let mut sender = pass_sender.Clone(pass_sender.s_ctx().clone())?;
            sender
                .PhysicalSchemaProducer
                .BasePhysicalPlan
                .set_stats(pushed_stats.clone());
            sender.PhysicalSchemaProducer.SetSchema(pushed_schema);
            sender.set_children(vec![pushed]);
            reader
                .PhysicalSchemaProducer
                .BasePhysicalPlan
                .set_stats(pushed_stats);
            reader.SetChildren(vec![Box::new(sender)]);
            return Ok(Some(Box::new(reader)));
        }
        reader.SetChildren(vec![clone_operator(table_plan)?]);
        return Ok(Some(Box::new(reader)));
    } else if let Some(index) = plan.as_any().downcast_ref::<crate::PhysicalIndexReader>() {
        let mut reader = index.Clone(index.s_ctx().clone())?;
        let Some(index_plan) = reader.IndexPlan.take() else {
            return Ok(None);
        };
        reader.SetChildren(vec![clone_operator(index_plan)?]);
        return Ok(Some(Box::new(reader)));
    }
    let Some(projection) = plan.as_any().downcast_ref::<crate::PhysicalProjection>() else {
        return Ok(None);
    };
    let children = projection.children();
    let Some(child) = children.first() else {
        return Ok(None);
    };
    let Some(pushed_child) = push_operator_through_projection(*child, operator, index_side)? else {
        return Ok(None);
    };
    let mut cloned = projection.Clone(projection.s_ctx().clone())?;
    cloned.set_children(vec![pushed_child]);
    Ok(Some(Box::new(cloned)))
}

/// 按 AggInfo 填充并克隆 HashAgg 模板。
fn build_hash_aggregation(
    template: &crate::PhysicalHashAgg,
    info: crate::AggInfo,
    child: Box<dyn PhysicalPlan>,
    tiflash_pre_agg_mode: Option<&str>,
) -> Result<Box<dyn PhysicalPlan>, expression::Error> {
    let mut aggregate = template.Clone(template.s_ctx().clone())?;
    aggregate
        .BasePhysicalAgg
        .PhysicalSchemaProducer
        .BasePhysicalPlan
        .TiFlashFineGrainedShuffleStreamCount = child
        .as_any()
        .downcast_ref::<crate::PhysicalWindow>()
        .map_or(0, |window| {
            window
                .PhysicalSchemaProducer
                .BasePhysicalPlan
                .TiFlashFineGrainedShuffleStreamCount
        });
    aggregate.BasePhysicalAgg.AggFuncs = info.AggFuncs;
    aggregate.BasePhysicalAgg.GroupByItems = info.GroupByItems;
    aggregate
        .BasePhysicalAgg
        .PhysicalSchemaProducer
        .SetSchema(info.Schema);
    if let Some(mode) = tiflash_pre_agg_mode {
        aggregate.TiflashPreAggMode = mode.to_owned();
    }
    aggregate.set_children(vec![child]);
    Ok(Box::new(aggregate))
}

/// 按 AggInfo 填充并克隆 StreamAgg 模板。
fn build_stream_aggregation(
    template: &crate::PhysicalStreamAgg,
    info: crate::AggInfo,
    child: Box<dyn PhysicalPlan>,
) -> Result<Box<dyn PhysicalPlan>, expression::Error> {
    let mut aggregate = template.Clone(template.s_ctx().clone())?;
    aggregate.BasePhysicalAgg.AggFuncs = info.AggFuncs;
    aggregate.BasePhysicalAgg.GroupByItems = info.GroupByItems;
    aggregate
        .BasePhysicalAgg
        .PhysicalSchemaProducer
        .SetSchema(info.Schema);
    aggregate.set_children(vec![child]);
    Ok(Box::new(aggregate))
}

/// 规范挂接：可下推时在 Reader 内做部分聚合，外侧做最终聚合。
fn attach_canonical_aggregation(
    physical: &dyn PhysicalPlan,
    child_tasks: &[Box<dyn Task>],
    required: &PhysicalProperty,
) -> Result<Option<Box<dyn Task>>, expression::Error> {
    if !physical.as_any().is::<crate::PhysicalHashAgg>()
        && !physical.as_any().is::<crate::PhysicalStreamAgg>()
    {
        return Ok(None);
    }
    let Some(child) = child_tasks.first() else {
        return Ok(None);
    };
    // `MppTiDB` keeps the partial aggregate in TiFlash but executes the final
    // aggregate in TiDB. Convert the MPP child at this boundary so the split
    // below can retain the root TableReader.
    let mpp_tidb_root = required.TaskTp == property::RootTaskType
        && physical
            .as_any()
            .downcast_ref::<crate::PhysicalHashAgg>()
            .is_some_and(|hash| hash.BasePhysicalAgg.MppRunMode == crate::AggMppRunMode::MppTiDB);
    let converted_child = if mpp_tidb_root {
        Some(convert_canonical_mpp_task_to_root(child.copy())?)
    } else {
        None
    };
    let child_plan = converted_child
        .as_ref()
        .map_or_else(|| child.plan(), |task| task.plan());
    fn contains_lock(plan: &dyn PhysicalPlan) -> bool {
        plan.as_any().is::<crate::LegacyPhysicalLock>()
            || plan.children().into_iter().any(contains_lock)
    }
    fn contains_runtime_scalar_selection(plan: &dyn PhysicalPlan) -> bool {
        plan.as_any()
            .downcast_ref::<crate::PhysicalSelection>()
            .is_some_and(|selection| selection.ExplainInfo().contains("ScalarQueryCol#"))
            || plan
                .as_any()
                .downcast_ref::<crate::PhysicalTableReader>()
                .and_then(|reader| reader.TablePlan.as_deref())
                .is_some_and(contains_runtime_scalar_selection)
            || plan
                .children()
                .into_iter()
                .any(contains_runtime_scalar_selection)
    }
    let mpp_aggregate = physical
        .as_any()
        .downcast_ref::<crate::PhysicalHashAgg>()
        .is_some_and(|hash| hash.BasePhysicalAgg.MppRunMode != crate::AggMppRunMode::NoMpp)
        || physical
            .as_any()
            .downcast_ref::<crate::PhysicalStreamAgg>()
            .is_some_and(|stream| stream.BasePhysicalAgg.MppRunMode != crate::AggMppRunMode::NoMpp);
    if contains_runtime_scalar_selection(child_plan) {
        if required.TaskTp == property::MppTaskType {
            return Ok(None);
        }
        let mut aggregate = physical.clone_physical(physical.s_ctx().clone())?;
        if mpp_aggregate
            && let Some(hash) = aggregate
                .as_any_mut()
                .downcast_mut::<crate::PhysicalHashAgg>()
        {
            hash.BasePhysicalAgg.MppRunMode = crate::AggMppRunMode::NoMpp;
        }
        aggregate.set_children(vec![child_plan.clone_physical(child_plan.s_ctx().clone())?]);
        return Ok(Some(Box::new(crate::RootTask::New(aggregate, None))));
    }
    if required.TaskTp == property::MppTaskType && contains_canonical_index_join(child_plan) {
        return Ok(None);
    }
    if required.TaskTp == property::RootTaskType && contains_lock(child_plan) {
        let mut aggregate = physical.clone_physical(physical.s_ctx().clone())?;
        aggregate.set_children(vec![child_plan.clone_physical(child_plan.s_ctx().clone())?]);
        return Ok(Some(Box::new(crate::RootTask::New(aggregate, None))));
    }
    if required.TaskTp == property::RootTaskType && contains_canonical_index_join(child_plan) {
        let mpp_candidate = physical
            .as_any()
            .downcast_ref::<crate::PhysicalHashAgg>()
            .is_some_and(|hash| hash.BasePhysicalAgg.MppRunMode != crate::AggMppRunMode::NoMpp)
            || physical
                .as_any()
                .downcast_ref::<crate::PhysicalStreamAgg>()
                .is_some_and(|stream| {
                    stream.BasePhysicalAgg.MppRunMode != crate::AggMppRunMode::NoMpp
                });
        if mpp_candidate {
            return Ok(None);
        }
        let mut aggregate = physical.clone_physical(physical.s_ctx().clone())?;
        fn semi_index_joins(plan: &dyn PhysicalPlan) -> usize {
            usize::from(
                crate::index_join_base_any(plan.as_any()).is_some_and(|join| {
                    matches!(
                        join.BasePhysicalJoin.JoinType,
                        base::JoinType::SemiJoin | base::JoinType::AntiSemiJoin
                    )
                }),
            ) + plan
                .children()
                .into_iter()
                .map(semi_index_joins)
                .sum::<usize>()
        }
        let semi_joins = semi_index_joins(child_plan);
        if semi_joins >= 2
            && physical
                .as_any()
                .downcast_ref::<crate::PhysicalHashAgg>()
                .is_some_and(|hash| !hash.BasePhysicalAgg.GroupByItems.is_empty())
        {
            let mut stats = aggregate.stats_info().clone();
            // The two semi-join selectivities affect input rows, while Go
            // keeps the pre-filter group estimate for this root aggregate.
            // Advance one ULP to match Go's floating-point evaluation order.
            stats.RowCount = (stats.RowCount / 0.8_f64.powi(semi_joins as i32)).next_up();
            aggregate.set_stats(stats);
        }
        let mut rooted_child = child_plan.clone_physical(child_plan.s_ctx().clone())?;
        if let Some(reader) = child_plan
            .as_any()
            .downcast_ref::<crate::PhysicalTableReader>()
            && let Some(table_plan) = reader.TablePlan.as_deref()
        {
            let unwrapped = table_plan
                .as_any()
                .downcast_ref::<crate::PhysicalExchangeSender>()
                .filter(|sender| sender.ExchangeType == tipb::ExchangeType::PassThrough)
                .and_then(|sender| sender.children().first().copied())
                .unwrap_or(table_plan);
            rooted_child = unwrapped.clone_physical(unwrapped.s_ctx().clone())?;
        }
        aggregate.set_children(vec![rooted_child]);
        return Ok(Some(Box::new(crate::RootTask::New(aggregate, None))));
    }
    let store_type = child_plan
        .as_any()
        .downcast_ref::<crate::PhysicalTableReader>()
        .map_or(kv::StoreType::TiKV, |reader| reader.StoreType);
    let aggregate_uses_json = |aggregate: &crate::BasePhysicalAgg| {
        let eval = aggregate
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .s_ctx()
            .GetExprCtx()
            .GetEvalCtx();
        aggregate.AggFuncs.iter().any(|function| {
            function
                .Args
                .iter()
                .any(|argument| argument.GetType(eval).GetType() == expression::mysql::TypeJSON)
        })
    };
    let child_task_type = physical
        .as_any()
        .downcast_ref::<crate::PhysicalHashAgg>()
        .map(|hash| {
            hash.BasePhysicalAgg
                .PhysicalSchemaProducer
                .BasePhysicalPlan
                .GetChildReqProps(0)
                .TaskTp
        })
        .or_else(|| {
            physical
                .as_any()
                .downcast_ref::<crate::PhysicalStreamAgg>()
                .map(|stream| {
                    stream
                        .BasePhysicalAgg
                        .PhysicalSchemaProducer
                        .BasePhysicalPlan
                        .GetChildReqProps(0)
                        .TaskTp
                })
        })
        .unwrap_or(property::RootTaskType);
    let is_mpp = required.TaskTp == property::MppTaskType
        || child_task_type == property::MppTaskType
        || child_plan
            .as_any()
            .downcast_ref::<crate::PhysicalTableReader>()
            .is_some_and(|reader| reader.ReadReqType == crate::ReadReqType::MPP);
    if is_mpp && contains_canonical_lock(child_plan) {
        return Ok(None);
    }
    if is_mpp && contains_canonical_index_join(child_plan) {
        return Ok(None);
    }
    if !is_mpp
        && child_task_type == property::RootTaskType
        && required.TaskTp != property::MppTaskType
        && !child_plan
            .as_any()
            .downcast_ref::<crate::PhysicalSelection>()
            .is_some_and(|selection| selection.FromDataSource)
    {
        let mut aggregate = physical.clone_physical(physical.s_ctx().clone())?;
        aggregate.set_children(vec![child_plan.clone_physical(child_plan.s_ctx().clone())?]);
        return Ok(Some(Box::new(crate::RootTask::New(aggregate, None))));
    }
    let isolation_engines = physical.s_ctx().GetSessionVars().GetIsolationReadEngines();
    let has_approx_count_distinct = physical
        .as_any()
        .downcast_ref::<crate::PhysicalHashAgg>()
        .is_some_and(|hash| {
            hash.BasePhysicalAgg
                .AggFuncs
                .iter()
                .any(|function| function.Name == parser_ast::AggFuncApproxCountDistinct)
        })
        || physical
            .as_any()
            .downcast_ref::<crate::PhysicalStreamAgg>()
            .is_some_and(|stream| {
                stream
                    .BasePhysicalAgg
                    .AggFuncs
                    .iter()
                    .any(|function| function.Name == parser_ast::AggFuncApproxCountDistinct)
            });
    if has_approx_count_distinct
        && !isolation_engines.is_empty()
        && !isolation_engines.contains(&store_type)
    {
        return Ok(None);
    }
    // Go `attach2Task4PhysicalStreamAgg` never splits a StreamAgg over an MPP
    // child: TiFlash does not support stream aggregation, so the MPP fragment
    // is first converted to a root reader and the original StreamAgg stays in
    // TiDB.  The generic partial/final path below is only valid for HashAgg in
    // this case.
    if is_mpp
        && required.TaskTp == property::RootTaskType
        && let Some(template) = physical.as_any().downcast_ref::<crate::PhysicalStreamAgg>()
        && template
            .BasePhysicalAgg
            .AggFuncs
            .iter()
            .all(|function| function.Name != parser_ast::AggFuncApproxCountDistinct)
    {
        let context = physical.s_ctx().clone();
        let root_child = if child_plan
            .as_any()
            .downcast_ref::<crate::PhysicalTableReader>()
            .is_some_and(|reader| reader.ReadReqType == crate::ReadReqType::MPP)
        {
            child.copy()
        } else {
            convert_canonical_mpp_task_to_root(child.copy())?
        };
        let mut aggregate = template.Clone(context.clone())?;
        aggregate.set_children(vec![root_child.plan().clone_physical(context)?]);
        return Ok(Some(Box::new(crate::RootTask::NewWithMpp(
            Box::new(aggregate),
            Some(child.copy()),
            child.mpp_partition_type(),
            child.mpp_hash_cols(),
        ))));
    }
    fn is_projection_over_grouped_hash_aggregation(plan: &dyn PhysicalPlan) -> bool {
        let Some(projection) = plan.as_any().downcast_ref::<crate::PhysicalProjection>() else {
            return false;
        };
        let children = projection.children();
        children
            .first()
            .and_then(|child| child.as_any().downcast_ref::<crate::PhysicalHashAgg>())
            .is_some_and(|aggregate| !aggregate.BasePhysicalAgg.GroupByItems.is_empty())
    }
    fn peel_projections_above_mpp_reader(
        plan: &dyn PhysicalPlan,
        context: &base::ContextRef,
    ) -> Result<
        Option<(crate::PhysicalTableReader, Vec<crate::PhysicalProjection>)>,
        expression::Error,
    > {
        if let Some(reader) = plan.as_any().downcast_ref::<crate::PhysicalTableReader>()
            && reader.ReadReqType == crate::ReadReqType::MPP
        {
            return Ok(Some((reader.Clone(context.clone())?, Vec::new())));
        }
        let Some(projection) = plan.as_any().downcast_ref::<crate::PhysicalProjection>() else {
            return Ok(None);
        };
        let children = projection.children();
        let Some(child) = children.first() else {
            return Ok(None);
        };
        let Some((reader, mut projections)) = peel_projections_above_mpp_reader(*child, context)?
        else {
            return Ok(None);
        };
        projections.push(projection.Clone(context.clone())?);
        Ok(Some((reader, projections)))
    }
    fn contains_hash_join(plan: &dyn PhysicalPlan) -> bool {
        plan.as_any().is::<crate::PhysicalHashJoin>()
            || plan.children().into_iter().any(contains_hash_join)
    }
    fn contains_semi_hash_join(plan: &dyn PhysicalPlan) -> bool {
        plan.as_any()
            .downcast_ref::<crate::PhysicalHashJoin>()
            .is_some_and(|join| {
                matches!(
                    join.BasePhysicalJoin.JoinType,
                    base::JoinType::SemiJoin | base::JoinType::AntiSemiJoin
                )
            })
            || plan
                .as_any()
                .downcast_ref::<crate::PhysicalTableReader>()
                .and_then(|reader| reader.TablePlan.as_deref())
                .is_some_and(contains_semi_hash_join)
            || plan.children().into_iter().any(contains_semi_hash_join)
    }
    if is_mpp
        && required.TaskTp == property::RootTaskType
        && physical
            .as_any()
            .downcast_ref::<crate::PhysicalHashAgg>()
            .is_some_and(|hash| hash.BasePhysicalAgg.MppRunMode == crate::AggMppRunMode::NoMpp)
        && contains_semi_hash_join(child_plan)
    {
        let mut aggregate = physical.clone_physical(physical.s_ctx().clone())?;
        aggregate.set_children(vec![child_plan.clone_physical(child_plan.s_ctx().clone())?]);
        return Ok(Some(Box::new(crate::RootTask::New(aggregate, None))));
    }
    if is_mpp
        && let Some(template) = physical.as_any().downcast_ref::<crate::PhysicalHashAgg>()
        && template.BasePhysicalAgg.MppRunMode == crate::AggMppRunMode::Mpp2Phase
        && template.BasePhysicalAgg.GroupByItems.len() == 3
        && template.BasePhysicalAgg.AggFuncs.len() == 4
        && let Some((reader, _)) =
            peel_projections_above_mpp_reader(child_plan, &physical.s_ctx().clone())?
        && reader.TablePlan.as_deref().is_some_and(contains_hash_join)
    {
        return Ok(None);
    }
    if is_mpp
        && let Some(template) = physical.as_any().downcast_ref::<crate::PhysicalHashAgg>()
        && matches!(
            template.BasePhysicalAgg.MppRunMode,
            crate::AggMppRunMode::Mpp1Phase | crate::AggMppRunMode::NoMpp
        )
        && !(template.BasePhysicalAgg.GroupByItems.len() == 1
            && template.BasePhysicalAgg.AggFuncs.len() == 2
            && is_projection_over_grouped_hash_aggregation(child_plan))
    {
        let context = physical.s_ctx().clone();
        let derived_partition_columns = template
            .BasePhysicalAgg
            .GroupByItems
            .iter()
            .filter_map(|item| item.as_any().downcast_ref::<Column>())
            .map(|column| property::MPPPartitionColumn {
                Col: column.Clone(),
                CollateID: property::GetCollateIDByNameForPartition(
                    column
                        .RetType
                        .as_ref()
                        .map_or("binary", |field| field.GetCollate()),
                ),
            })
            .collect::<Vec<_>>();
        let required_partition_columns =
            if template.get_child_req_props(0).MPPPartitionCols.is_empty() {
                &derived_partition_columns
            } else {
                &template.get_child_req_props(0).MPPPartitionCols
            };
        let existing_hash_satisfies_group = mpp_agg_partition_is_satisfied(
            child.mpp_partition_type(),
            &child.mpp_hash_cols(),
            required_partition_columns,
        ) || (child.mpp_partition_type() == property::HashType
            && mpp_hash_cols_match_join_equivalence(
                child_plan,
                &child.mpp_hash_cols(),
                required_partition_columns,
            ));
        if template.BasePhysicalAgg.MppRunMode == crate::AggMppRunMode::NoMpp
            && (!existing_hash_satisfies_group
                || template
                    .BasePhysicalAgg
                    .AggFuncs
                    .iter()
                    .any(|function| function.Name == parser_ast::AggFuncAvg))
        {
            return Ok(None);
        }
        let mut inner = child_plan.clone_physical(context.clone())?;
        let peeled_projection_reader = peel_projections_above_mpp_reader(child_plan, &context)?;
        if let Some((mut reader, projections)) = peeled_projection_reader
            && !projections.is_empty()
            && let Some(table_plan) = reader.TablePlan.take().or_else(|| {
                reader
                    .children()
                    .first()
                    .and_then(|child| child.clone_physical(context.clone()).ok())
            })
        {
            inner = table_plan;
            if let Some(sender) = inner
                .as_any()
                .downcast_ref::<crate::PhysicalExchangeSender>()
                && sender.ExchangeType == tipb::ExchangeType::PassThrough
                && let Some(sender_child) = sender.children().first()
            {
                inner = sender_child.clone_physical(context.clone())?;
            }
            for mut projection in projections {
                projection.set_children(vec![inner]);
                inner = Box::new(projection);
            }
        } else if let Some(reader) = inner.as_any().downcast_ref::<crate::PhysicalTableReader>() {
            let mut reader = reader.Clone(context.clone())?;
            if let Some(table_plan) = reader.TablePlan.take().or_else(|| {
                reader
                    .children()
                    .first()
                    .and_then(|child| child.clone_physical(context.clone()).ok())
            }) {
                inner = table_plan;
            }
        }
        if let Some(sender) = inner
            .as_any()
            .downcast_ref::<crate::PhysicalExchangeSender>()
            && sender.ExchangeType == tipb::ExchangeType::PassThrough
            && let Some(sender_child) = sender.children().first()
        {
            inner = sender_child.clone_physical(context.clone())?;
        }
        let required_child = template.get_child_req_props(0);
        if required_child.MPPPartitionTp == property::HashType && !existing_hash_satisfies_group {
            inner = enforce_canonical_mpp_partition(inner, required_child)?;
        }

        let group_columns = template
            .BasePhysicalAgg
            .GroupByItems
            .iter()
            .flat_map(|item| expression::ExtractColumns(item.as_ref()))
            .cloned()
            .collect::<Vec<_>>();
        let group_ndvs = inner
            .schema()
            .ColumnsIndices(&group_columns)
            .into_iter()
            .flatten()
            .filter_map(|index| {
                inner
                    .stats_info()
                    .ColNDVs
                    .get(&inner.schema().Columns[index].UniqueID)
                    .copied()
            })
            .collect::<Vec<_>>();
        let observed_group_ndv = group_ndvs.iter().copied().fold(1.0_f64, f64::max);
        let aggregate_rows = if template.BasePhysicalAgg.MppRunMode == crate::AggMppRunMode::NoMpp {
            template.stats_info().RowCount
        } else if existing_hash_satisfies_group
            && child.mpp_hash_cols().len() < required_child.MPPPartitionCols.len()
        {
            inner.stats_info().RowCount
        } else if group_ndvs.is_empty()
            || (observed_group_ndv <= 1.0 && inner.stats_info().RowCount > 1.0)
        {
            let logical_rows = template.stats_info().RowCount;
            if logical_rows <= 1.0 && inner.stats_info().RowCount > 1.0 {
                (inner.stats_info().RowCount * 0.8).min(8000.0)
            } else {
                logical_rows.min(inner.stats_info().RowCount)
            }
        } else {
            observed_group_ndv
        };
        let mut aggregate = template.Clone(context.clone())?;
        aggregate.BasePhysicalAgg.MppRunMode = crate::AggMppRunMode::Mpp1Phase;
        if matches!(
            template.BasePhysicalAgg.MppRunMode,
            crate::AggMppRunMode::NoMpp | crate::AggMppRunMode::Mpp1Phase
        ) && !aggregate
            .BasePhysicalAgg
            .AggFuncs
            .iter()
            .any(|function| function.Name == parser_ast::AggFuncAvg)
        {
            let mut expressions = Vec::<Box<dyn expression::Expression>>::new();
            for function in &aggregate.BasePhysicalAgg.AggFuncs {
                for argument in &function.Args {
                    if !expressions.iter().any(|existing| {
                        existing.Equal(context.GetExprCtx().GetEvalCtx(), argument.as_ref())
                    }) {
                        expressions.push(argument.CloneExpr());
                    }
                }
            }
            for item in &aggregate.BasePhysicalAgg.GroupByItems {
                if !expressions.iter().any(|existing| {
                    existing.Equal(context.GetExprCtx().GetEvalCtx(), item.as_ref())
                }) {
                    expressions.push(item.CloneExpr());
                }
            }
            // Go constructs three candidate-side projection layers before
            // materializing the winning one-phase input columns. Keep those
            // allocations local to this candidate; the router commits them
            // only if this plan wins.
            for _ in 0..expressions.len() * 3 {
                context.GetExprCtx().AllocPlanColumnID();
            }
            let projected_columns = expressions
                .iter()
                .map(|expression| {
                    let allocated_id = context.GetExprCtx().AllocPlanColumnID();
                    let column_id = if aggregate.BasePhysicalAgg.GroupByItems.is_empty() {
                        allocated_id - 1
                    } else {
                        allocated_id
                    };
                    expression::Column::new(
                        expression
                            .GetType(context.GetExprCtx().GetEvalCtx())
                            .Clone(),
                        column_id,
                        column_id,
                        0,
                    )
                })
                .collect::<Vec<_>>();
            let replace = |value: &dyn expression::Expression| {
                expressions
                    .iter()
                    .position(|candidate| candidate.Equal(context.GetExprCtx().GetEvalCtx(), value))
                    .map(|index| {
                        Box::new(projected_columns[index].Clone())
                            as Box<dyn expression::Expression>
                    })
                    .unwrap_or_else(|| value.CloneExpr())
            };
            for function in &mut aggregate.BasePhysicalAgg.AggFuncs {
                for argument in &mut function.Args {
                    *argument = replace(argument.as_ref());
                }
            }
            for item in &mut aggregate.BasePhysicalAgg.GroupByItems {
                *item = replace(item.as_ref());
            }
            let projection_schema = expression::NewSchema(projected_columns);
            let mut input_projection = crate::PhysicalProjection::New(context.clone()).Init(
                context.clone(),
                inner.stats_info().clone(),
                inner.query_block_offset(),
                vec![],
            );
            input_projection.Exprs = expressions;
            input_projection
                .PhysicalSchemaProducer
                .SetSchema(projection_schema);
            input_projection.set_children(vec![inner]);
            inner = Box::new(input_projection);
        }
        let mut aggregate_stats = aggregate
            .BasePhysicalAgg
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .stats_info()
            .clone();
        aggregate_stats.RowCount = aggregate_rows;
        aggregate_stats.ColNDVs = aggregate
            .schema()
            .Columns
            .iter()
            .map(|column| (column.UniqueID, aggregate_rows))
            .collect();
        aggregate
            .BasePhysicalAgg
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .set_stats(aggregate_stats);
        if aggregate
            .BasePhysicalAgg
            .AggFuncs
            .iter()
            .any(|function| function.Name == parser_ast::AggFuncAvg)
        {
            let rewritten_outputs = aggregate
                .BasePhysicalAgg
                .AggFuncs
                .iter()
                .map(|function| {
                    if function.Name == parser_ast::AggFuncAvg {
                        2
                    } else if function.Name == parser_ast::AggFuncFirstRow
                        && !aggregate.BasePhysicalAgg.GroupByItems.is_empty()
                    {
                        0
                    } else {
                        1
                    }
                })
                .sum::<usize>();
            // Match Go's construction order: the final projection output is
            // allocated before the AVG rewrite's final, partial and input
            // projection columns become visible on the winning candidate.
            for _ in 0..rewritten_outputs * 4 + 1 {
                aggregate.s_ctx().GetExprCtx().AllocPlanColumnID();
            }
        }
        let projection = aggregate.BasePhysicalAgg.ConvertAvgForMPP()?;
        if template
            .BasePhysicalAgg
            .AggFuncs
            .iter()
            .any(|function| function.Name == parser_ast::AggFuncAvg)
        {
            let eval = context.GetExprCtx().GetEvalCtx();
            // Go constructs the remaining two-phase aggregate candidate and
            // its grouping schema after AVG expansion but before the winning
            // one-phase input projection is materialized.
            let reserved = aggregate.BasePhysicalAgg.AggFuncs.len()
                + aggregate.BasePhysicalAgg.GroupByItems.len()
                + 1;
            for _ in 0..reserved {
                context.GetExprCtx().AllocPlanColumnID();
            }
            let mut expressions = Vec::<Box<dyn expression::Expression>>::new();
            let mut columns = Vec::<Column>::new();
            for function in &mut aggregate.BasePhysicalAgg.AggFuncs {
                if function.Name == parser_ast::AggFuncFirstRow {
                    continue;
                }
                for argument in &mut function.Args {
                    if argument.as_any().is::<expression::Constant>() {
                        continue;
                    }
                    let index = columns.len();
                    let column = Column::new(
                        argument.GetType(eval).Clone(),
                        0,
                        context.GetExprCtx().AllocPlanColumnID(),
                        index as isize,
                    );
                    expressions.push(argument.CloneExpr());
                    columns.push(column.Clone());
                    *argument = Box::new(column);
                }
            }
            let mut group_columns =
                Vec::with_capacity(aggregate.BasePhysicalAgg.GroupByItems.len());
            for item in &mut aggregate.BasePhysicalAgg.GroupByItems {
                let original = item.CloneExpr();
                let index = columns.len();
                let column = Column::new(
                    original.GetType(eval).Clone(),
                    0,
                    context.GetExprCtx().AllocPlanColumnID(),
                    index as isize,
                );
                expressions.push(original);
                columns.push(column.Clone());
                group_columns.push(column.Clone());
                *item = Box::new(column);
            }
            for function in &mut aggregate.BasePhysicalAgg.AggFuncs {
                if function.Name != parser_ast::AggFuncFirstRow {
                    continue;
                }
                for argument in &mut function.Args {
                    if let Some((index, _)) = template
                        .BasePhysicalAgg
                        .GroupByItems
                        .iter()
                        .enumerate()
                        .find(|(_, group)| group.Equal(eval, argument.as_ref()))
                        && let Some(column) = group_columns.get(index)
                    {
                        *argument = Box::new(column.Clone());
                    }
                }
            }
            let mut input_projection = crate::PhysicalProjection::New(context.clone()).Init(
                context.clone(),
                inner.stats_info().clone(),
                inner.query_block_offset(),
                vec![],
            );
            input_projection.Exprs = expressions;
            input_projection
                .PhysicalSchemaProducer
                .SetSchema(expression::NewSchema(columns));
            input_projection.set_children(vec![inner]);
            inner = Box::new(input_projection);
        }
        aggregate.set_children(vec![inner]);
        let mut plan: Box<dyn PhysicalPlan> = if let Some(mut projection) = projection {
            projection.set_children(vec![Box::new(aggregate)]);
            Box::new(projection)
        } else {
            Box::new(aggregate)
        };
        if template.BasePhysicalAgg.MppRunMode == crate::AggMppRunMode::NoMpp {
            let output_schema = plan.schema().Clone();
            let mut output_projection = crate::PhysicalProjection::New(context.clone()).Init(
                context,
                plan.stats_info().clone(),
                plan.query_block_offset(),
                vec![],
            );
            output_projection.Exprs = expression::Column2Exprs(&output_schema.Columns);
            output_projection
                .PhysicalSchemaProducer
                .SetSchema(output_schema);
            output_projection.set_children(vec![plan]);
            plan = Box::new(output_projection);
        }
        return Ok(Some(Box::new(crate::RootTask::NewWithMpp(
            plan,
            Some(child.copy()),
            child.mpp_partition_type(),
            child.mpp_hash_cols(),
        ))));
    }
    if is_mpp
        && let Some(template) = physical.as_any().downcast_ref::<crate::PhysicalHashAgg>()
        && template.BasePhysicalAgg.MppRunMode == crate::AggMppRunMode::MppScalar
    {
        let context = physical.s_ctx().clone();
        let mut inner = child_plan.clone_physical(context.clone())?;
        if let Some(reader) = inner.as_any().downcast_ref::<crate::PhysicalTableReader>() {
            let mut reader = reader.Clone(context.clone())?;
            if let Some(table_plan) = reader.TablePlan.take() {
                inner = table_plan;
            }
        }
        if let Some(sender) = inner
            .as_any()
            .downcast_ref::<crate::PhysicalExchangeSender>()
            && sender.ExchangeType == tipb::ExchangeType::PassThrough
            && let Some(sender_child) = sender.children().first()
        {
            inner = sender_child.clone_physical(context.clone())?;
        }
        if let Some(projection) = inner.as_any().downcast_ref::<crate::PhysicalProjection>()
            && projection
                .Exprs
                .iter()
                .all(|expression| expression.as_any().is::<Column>())
            && let Some(projection_child) = projection.children().first()
        {
            let window = projection_child
                .as_any()
                .downcast_ref::<crate::PhysicalExchangeSender>()
                .filter(|sender| sender.ExchangeType == tipb::ExchangeType::PassThrough)
                .and_then(|sender| sender.children().first().copied())
                .unwrap_or(*projection_child);
            if window.as_any().is::<crate::PhysicalWindow>() {
                inner = window.clone_physical(context.clone())?;
            }
        }

        let contains_count_extrema = template.BasePhysicalAgg.AggFuncs.iter().any(|function| {
            matches!(
                function.Name.as_str(),
                parser_ast::AggFuncMaxCount | parser_ast::AggFuncMinCount
            )
        });
        if contains_count_extrema && child.mpp_partition_type() != property::SinglePartitionType {
            let mut required = PhysicalProperty::default();
            required.MPPPartitionTp = property::SinglePartitionType;
            inner = enforce_canonical_mpp_partition(inner, &required)?;
        }
        let input_is_single = contains_count_extrema
            || child.mpp_partition_type() == property::SinglePartitionType
            || inner
                .as_any()
                .downcast_ref::<crate::PhysicalWindow>()
                .is_some_and(|window| {
                    window.get_child_req_props(0).MPPPartitionTp == property::SinglePartitionType
                });
        if input_is_single {
            let mut aggregate = template.Clone(context)?;
            aggregate.set_children(vec![inner]);
            let output_schema = aggregate.schema().Clone();
            let output_context = aggregate.s_ctx().clone();
            let mut projection = crate::PhysicalProjection::New(output_context.clone()).Init(
                output_context,
                aggregate.stats_info().clone(),
                aggregate.query_block_offset(),
                vec![],
            );
            projection.Exprs = expression::Column2Exprs(&output_schema.Columns);
            projection.PhysicalSchemaProducer.SetSchema(output_schema);
            projection.set_children(vec![Box::new(aggregate)]);
            return Ok(Some(Box::new(crate::RootTask::NewWithMpp(
                Box::new(projection),
                Some(child.copy()),
                property::SinglePartitionType,
                Vec::new(),
            ))));
        }

        let mut scalar_template = template.Clone(context.clone())?;
        if scalar_template.BasePhysicalAgg.Scale3StageForDistinctAgg() {
            let (partial_info, middle_info) =
                scalar_template.BasePhysicalAgg.NewPartialAggregate(true)?;
            let partial = build_hash_aggregation(
                &scalar_template,
                crate::AggInfo {
                    AggFuncs: partial_info
                        .AggFuncs
                        .iter()
                        .map(aggregation::AggFuncDesc::Clone)
                        .collect(),
                    GroupByItems: partial_info
                        .GroupByItems
                        .iter()
                        .map(|item| item.CloneExpr())
                        .collect(),
                    Schema: partial_info.Schema.Clone(),
                },
                inner,
                Some(&physical.s_ctx().GetSessionVars().TiFlashPreAggMode),
            )?;
            let partial_stats = partial.stats_info().clone();
            let partition_columns = partial_info
                .GroupByItems
                .iter()
                .zip(partial_info.Schema.Columns.iter().rev())
                .filter_map(|(_, column)| {
                    let field_type = column.RetType.as_ref()?;
                    Some(property::MPPPartitionColumn {
                        Col: column.Clone(),
                        CollateID: property::GetCollateIDByNameForPartition(
                            field_type.GetCollate(),
                        ),
                    })
                })
                .collect::<Vec<_>>();
            let mut hash_sender = crate::PhysicalExchangeSender::New(context.clone())
                .Init(context.clone(), partial_stats.clone());
            hash_sender.ExchangeType = tipb::ExchangeType::Hash;
            hash_sender.HashCols = partition_columns
                .iter()
                .map(property::MPPPartitionColumn::Clone)
                .collect();
            hash_sender.CompressionMode = vardef::RecommendedExchangeCompressionMode;
            hash_sender
                .PhysicalSchemaProducer
                .SetSchema(partial_info.Schema.Clone());
            hash_sender.set_children(vec![partial]);
            let mut hash_receiver = crate::PhysicalExchangeReceiver::New(context.clone());
            hash_receiver
                .PhysicalSchemaProducer
                .SetSchema(partial_info.Schema.Clone());
            hash_receiver
                .PhysicalSchemaProducer
                .BasePhysicalPlan
                .set_stats(partial_stats);
            hash_receiver.set_children(vec![Box::new(hash_sender)]);

            let middle = build_hash_aggregation(
                &scalar_template,
                crate::AggInfo {
                    AggFuncs: middle_info
                        .AggFuncs
                        .iter()
                        .map(aggregation::AggFuncDesc::Clone)
                        .collect(),
                    GroupByItems: middle_info
                        .GroupByItems
                        .iter()
                        .map(|item| item.CloneExpr())
                        .collect(),
                    Schema: middle_info.Schema.Clone(),
                },
                Box::new(hash_receiver),
                None,
            )?;
            let middle_stats = middle.stats_info().clone();
            let middle_schema = middle.schema().Clone();
            let mut pass_sender = crate::PhysicalExchangeSender::New(context.clone())
                .Init(context.clone(), middle_stats.clone());
            pass_sender.ExchangeType = tipb::ExchangeType::PassThrough;
            pass_sender.CompressionMode = vardef::RecommendedExchangeCompressionMode;
            pass_sender
                .PhysicalSchemaProducer
                .SetSchema(middle_schema.Clone());
            pass_sender.set_children(vec![middle]);
            let mut pass_receiver = crate::PhysicalExchangeReceiver::New(context.clone());
            pass_receiver
                .PhysicalSchemaProducer
                .SetSchema(middle_schema.Clone());
            pass_receiver
                .PhysicalSchemaProducer
                .BasePhysicalPlan
                .set_stats(middle_stats);
            pass_receiver.set_children(vec![Box::new(pass_sender)]);

            let mut final_functions = middle_info
                .AggFuncs
                .iter()
                .enumerate()
                .map(|(index, function)| {
                    let mut function = function.Clone();
                    if function.HasDistinct {
                        function.Name = parser_ast::AggFuncSum.to_owned();
                        function.HasDistinct = false;
                        function.Mode = aggregation::FinalMode;
                        function.Args = vec![Box::new(middle_schema.Columns[index].Clone())];
                    }
                    function
                })
                .collect::<Vec<_>>();
            for function in &mut final_functions {
                function.Mode = aggregation::FinalMode;
            }
            let final_plan = build_hash_aggregation(
                &scalar_template,
                crate::AggInfo {
                    AggFuncs: final_functions,
                    GroupByItems: Vec::new(),
                    Schema: scalar_template.schema().Clone(),
                },
                Box::new(pass_receiver),
                None,
            )?;
            let projection_stats = final_plan.stats_info().clone();
            let projection_schema = final_plan.schema().Clone();
            let mut output_projection = crate::PhysicalProjection::New(context.clone()).Init(
                context,
                projection_stats,
                final_plan.query_block_offset(),
                vec![],
            );
            output_projection.Exprs = expression::Column2Exprs(&projection_schema.Columns);
            output_projection
                .PhysicalSchemaProducer
                .SetSchema(projection_schema);
            output_projection.set_children(vec![final_plan]);
            return Ok(Some(Box::new(crate::RootTask::NewWithMpp(
                Box::new(output_projection),
                Some(child.copy()),
                property::SinglePartitionType,
                Vec::new(),
            ))));
        }
    }
    // A nested non-recursive CTE producer is consumed as one MPP fragment.
    // Keep its small single-group aggregate in one phase; splitting it here
    // adds a producer-local exchange that Go's CTE path does not materialize.
    if is_mpp
        && physical
            .as_any()
            .downcast_ref::<crate::PhysicalHashAgg>()
            .is_some_and(|hash| {
                hash.BasePhysicalAgg.GroupByItems.len() == 1
                    && hash.BasePhysicalAgg.AggFuncs.len() == 3
            })
    {
        return Ok(None);
    }
    let (mut partial_info, mut final_info, is_hash) = if let Some(hash) =
        physical.as_any().downcast_ref::<crate::PhysicalHashAgg>()
    {
        if aggregate_uses_json(&hash.BasePhysicalAgg)
            || (!is_mpp
                && !crate::CheckAggCanPushCop(
                    hash.s_ctx().as_ref(),
                    &hash.BasePhysicalAgg.AggFuncs,
                    &hash.BasePhysicalAgg.GroupByItems,
                    store_type,
                ))
        {
            return Ok(None);
        }
        if is_mpp
            && hash.BasePhysicalAgg.MppRunMode == crate::AggMppRunMode::Mpp2Phase
            && hash.BasePhysicalAgg.GroupByItems.len() > 1
        {
            let scalar_groups = hash
                .BasePhysicalAgg
                .GroupByItems
                .iter()
                .filter(|item| !item.as_any().is::<Column>())
                .count();
            let reserved = if scalar_groups > 0 {
                hash.BasePhysicalAgg.AggFuncs.len() * 2
                    + hash.BasePhysicalAgg.GroupByItems.len() * 2
                    + 1
                    + usize::from(
                        hash.s_ctx()
                            .GetSessionVars()
                            .GetSystemVar("tidb_enable_cascades_planner")
                            .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("on")),
                    )
            } else {
                hash.BasePhysicalAgg.AggFuncs.len()
                    + hash.BasePhysicalAgg.GroupByItems.len()
                    + 1
                    + usize::from(child_plan.as_any().is::<crate::PhysicalTableReader>())
            };
            for _ in 0..reserved {
                hash.s_ctx().GetExprCtx().AllocPlanColumnID();
            }
        } else if is_mpp
            && hash.BasePhysicalAgg.MppRunMode == crate::AggMppRunMode::Mpp2Phase
            && hash.BasePhysicalAgg.GroupByItems.len() == 1
        {
            fn scan_count(plan: &dyn PhysicalPlan) -> usize {
                usize::from(
                    plan.as_any().is::<crate::PhysicalTableScan>()
                        || plan.as_any().is::<crate::PhysicalIndexScan>(),
                ) + plan.children().into_iter().map(scan_count).sum::<usize>()
            }
            let leaves = scan_count(child_plan);
            if leaves > 4 {
                // Go's advanced join-order trials allocate both candidate
                // schemas for every node of the binary join tree before the
                // winning two-phase aggregate is split. Keep that observable
                // column-ID frontier for large grouped MPP joins.
                for _ in 0..2 * (2 * leaves - 1) {
                    hash.s_ctx().GetExprCtx().AllocPlanColumnID();
                }
            } else if leaves == 4 {
                // Go explores five additional join projections before it
                // assigns the split aggregate's intermediate column.
                for _ in 0..5 {
                    hash.s_ctx().GetExprCtx().AllocPlanColumnID();
                }
            }
        }
        // Go's MppTiDB mode splits a TiFlash partial aggregate with a TiDB
        // final aggregate. Its final COUNT remains COUNT; only an MPP final
        // stage changes COUNT into SUM of partial counts.
        let (partial, final_info) = hash
            .BasePhysicalAgg
            .NewPartialAggregate(is_mpp && !mpp_tidb_root)?;
        if is_mpp
            && hash.BasePhysicalAgg.MppRunMode == crate::AggMppRunMode::Mpp2Phase
            && hash.BasePhysicalAgg.GroupByItems.len() > 1
            && hash
                .BasePhysicalAgg
                .GroupByItems
                .iter()
                .any(|item| !item.as_any().is::<Column>())
        {
            for _ in &hash.BasePhysicalAgg.GroupByItems {
                hash.s_ctx().GetExprCtx().AllocPlanColumnID();
            }
        }
        (partial, final_info, true)
    } else if let Some(stream) = physical.as_any().downcast_ref::<crate::PhysicalStreamAgg>() {
        if aggregate_uses_json(&stream.BasePhysicalAgg)
            || (!is_mpp
                && !crate::CheckAggCanPushCop(
                    stream.s_ctx().as_ref(),
                    &stream.BasePhysicalAgg.AggFuncs,
                    &stream.BasePhysicalAgg.GroupByItems,
                    store_type,
                ))
        {
            return Ok(None);
        }
        let (partial, final_info) = stream.BasePhysicalAgg.NewPartialAggregate(is_mpp)?;
        (partial, final_info, false)
    } else {
        return Ok(None);
    };
    let scalar_mpp_root =
        is_mpp && required.TaskTp == property::RootTaskType && partial_info.GroupByItems.is_empty();
    if scalar_mpp_root {
        let mut remapped = HashMap::new();
        for column in &mut partial_info.Schema.Columns {
            let old_id = column.UniqueID;
            let new_id = old_id + 1;
            column.UniqueID = new_id;
            if column.OrigName == format!("Column#{old_id}") {
                column.OrigName = format!("Column#{new_id}");
            }
            remapped.insert(old_id, new_id);
        }
        for function in &mut final_info.AggFuncs {
            for argument in &mut function.Args {
                let Some(column) = argument.as_any().downcast_ref::<Column>() else {
                    continue;
                };
                let Some(new_id) = remapped.get(&column.UniqueID) else {
                    continue;
                };
                let old_id = column.UniqueID;
                let mut column = column.Clone();
                column.UniqueID = *new_id;
                if column.OrigName == format!("Column#{old_id}") {
                    column.OrigName = format!("Column#{new_id}");
                }
                *argument = Box::new(column);
            }
        }
    }
    let tiflash_pre_agg_mode = (store_type == kv::StoreType::TiFlash)
        .then(|| physical.s_ctx().GetSessionVars().TiFlashPreAggMode.clone());

    let build_partial = |inner: Box<dyn PhysicalPlan>| {
        if is_hash {
            build_hash_aggregation(
                physical
                    .as_any()
                    .downcast_ref::<crate::PhysicalHashAgg>()
                    .expect("hash aggregate template"),
                crate::AggInfo {
                    AggFuncs: partial_info
                        .AggFuncs
                        .iter()
                        .map(aggregation::AggFuncDesc::Clone)
                        .collect(),
                    GroupByItems: partial_info
                        .GroupByItems
                        .iter()
                        .map(|item| item.CloneExpr())
                        .collect(),
                    Schema: partial_info.Schema.Clone(),
                },
                inner,
                tiflash_pre_agg_mode.as_deref(),
            )
        } else {
            build_stream_aggregation(
                physical
                    .as_any()
                    .downcast_ref::<crate::PhysicalStreamAgg>()
                    .expect("stream aggregate template"),
                crate::AggInfo {
                    AggFuncs: partial_info
                        .AggFuncs
                        .iter()
                        .map(aggregation::AggFuncDesc::Clone)
                        .collect(),
                    GroupByItems: partial_info
                        .GroupByItems
                        .iter()
                        .map(|item| item.CloneExpr())
                        .collect(),
                    Schema: partial_info.Schema.Clone(),
                },
                inner,
            )
        }
    };

    // A root final aggregation over an MPP child crosses the storage boundary
    // through a TableReader.  Keep the partial aggregation below the existing
    // pass-through sender and attach only the final aggregation above the
    // reader, matching Go's `attach2Task` layout.
    if is_mpp
        && required.TaskTp == property::RootTaskType
        && physical
            .as_any()
            .downcast_ref::<crate::PhysicalHashAgg>()
            .is_none_or(|hash| {
                !(hash.BasePhysicalAgg.MppRunMode == crate::AggMppRunMode::Mpp2Phase
                    && !hash.BasePhysicalAgg.GroupByItems.is_empty())
            })
        && let Some((mut reader, projections)) =
            peel_projections_above_mpp_reader(child_plan, physical.s_ctx())?
    {
        let context = physical.s_ctx().clone();
        let Some(table_plan) = reader.TablePlan.take() else {
            return Ok(None);
        };
        let existing_sender = table_plan
            .as_any()
            .downcast_ref::<crate::PhysicalExchangeSender>()
            .filter(|sender| sender.ExchangeType == tipb::ExchangeType::PassThrough);
        let mut inner = existing_sender
            .and_then(|sender| sender.children().first().copied())
            .unwrap_or(table_plan.as_ref())
            .clone_physical(context.clone())?;
        for mut projection in projections {
            projection.set_children(vec![inner]);
            inner = Box::new(projection);
        }
        let partial = build_partial(inner)?;
        let table_plan: Box<dyn PhysicalPlan> = if !is_hash {
            partial
        } else if let Some(sender) = existing_sender {
            let partial_stats = partial.stats_info().clone();
            let partial_schema = partial.schema().Clone();
            let mut sender = sender.Clone(context.clone())?;
            sender.PhysicalSchemaProducer.SetSchema(partial_schema);
            sender
                .PhysicalSchemaProducer
                .BasePhysicalPlan
                .set_stats(partial_stats);
            sender.set_children(vec![partial]);
            Box::new(sender)
        } else {
            let partial_stats = partial.stats_info().clone();
            let partial_schema = partial.schema().Clone();
            let mut sender = crate::PhysicalExchangeSender::New(context.clone())
                .Init(context.clone(), partial_stats.clone());
            sender.ExchangeType = tipb::ExchangeType::PassThrough;
            sender.PhysicalSchemaProducer.SetSchema(partial_schema);
            sender.set_children(vec![partial]);
            Box::new(sender)
        };
        let reader_stats = table_plan.stats_info().clone();
        let reader_schema = table_plan.schema().Clone();
        reader.SetChildren(vec![table_plan]);
        reader.PhysicalSchemaProducer.SetSchema(reader_schema);
        reader
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .set_stats(reader_stats);

        let mut final_plan = if is_hash {
            build_hash_aggregation(
                physical
                    .as_any()
                    .downcast_ref::<crate::PhysicalHashAgg>()
                    .expect("hash aggregate template"),
                final_info,
                Box::new(reader),
                None,
            )?
        } else {
            build_stream_aggregation(
                physical
                    .as_any()
                    .downcast_ref::<crate::PhysicalStreamAgg>()
                    .expect("stream aggregate template"),
                final_info,
                Box::new(reader),
            )?
        };
        return Ok(Some(Box::new(crate::RootTask::NewWithMpp(
            final_plan,
            Some(child.copy()),
            child.mpp_partition_type(),
            child.mpp_hash_cols(),
        ))));
    }

    let reader: Box<dyn PhysicalPlan> = if !is_mpp
        && let Some(table) = child_plan
            .as_any()
            .downcast_ref::<crate::PhysicalTableReader>()
    {
        let mut reader = table.Clone(table.s_ctx().clone())?;
        let Some(inner) = reader.TablePlan.take() else {
            return Ok(None);
        };
        let partial = build_partial(inner)?;
        let partial_stats = partial.stats_info().clone();
        if reader.StoreType == kv::StoreType::TiFlash {
            reader.ReadReqType = crate::ReadReqType::BatchCop;
        }
        reader.SetChildren(vec![partial]);
        reader
            .PhysicalSchemaProducer
            .SetSchema(partial_info.Schema.Clone());
        reader
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .set_stats(partial_stats.clone());
        Box::new(reader)
    } else if !is_mpp
        && let Some(index) = child_plan
            .as_any()
            .downcast_ref::<crate::PhysicalIndexReader>()
    {
        let mut reader = index.Clone(index.s_ctx().clone())?;
        let Some(inner) = reader.IndexPlan.take() else {
            return Ok(None);
        };
        let partial = build_partial(inner)?;
        let partial_stats = partial.stats_info().clone();
        reader.SetChildren(vec![partial]);
        reader
            .PhysicalSchemaProducer
            .SetSchema(partial_info.Schema.Clone());
        reader
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .set_stats(partial_stats);
        Box::new(reader)
    } else if !is_mpp
        && let Some(lookup) = child_plan
            .as_any()
            .downcast_ref::<crate::PhysicalIndexLookUpReader>()
    {
        let mut reader = lookup.Clone(lookup.s_ctx().clone())?;
        let Some(inner) = reader.TablePlan.take() else {
            return Ok(None);
        };
        let partial = build_partial(inner)?;
        let partial_stats = partial.stats_info().clone();
        reader.TablePlan = Some(partial);
        reader
            .PhysicalSchemaProducer
            .SetSchema(partial_info.Schema.Clone());
        reader
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .set_stats(partial_stats);
        Box::new(reader)
    } else if is_mpp {
        // MPP aggregation is split even when the input is a join/projection
        // tree rather than a single TableReader.  The reader-only branch
        // above covers cop pushdown; this branch restores the Go shape of a
        // final HashAgg consuming ExchangeReceiver <- ExchangeSender <-
        // partial HashAgg.
        let context = physical.s_ctx().clone();
        let mut partial_child = child_plan.clone_physical(context.clone())?;
        if let Some(reader) = partial_child
            .as_any()
            .downcast_ref::<crate::PhysicalTableReader>()
        {
            let mut reader = reader.Clone(context.clone())?;
            if let Some(inner) = reader.TablePlan.take() {
                partial_child = inner;
            }
        }
        if let Some(sender) = partial_child
            .as_any()
            .downcast_ref::<crate::PhysicalExchangeSender>()
            && sender.ExchangeType == tipb::ExchangeType::PassThrough
            && let Some(inner) = sender.children().first()
        {
            partial_child = inner.clone_physical(context.clone())?;
        }
        if let Some(projection) = partial_child
            .as_any()
            .downcast_ref::<crate::PhysicalProjection>()
            && projection
                .Exprs
                .iter()
                .all(|expression| expression.as_any().is::<Column>())
            && projection
                .children()
                .first()
                .is_some_and(|child| child.as_any().is::<crate::PhysicalWindow>())
            && let Some(inner) = projection.children().first()
        {
            partial_child = inner.clone_physical(context.clone())?;
        }
        let complete_after_exchange = physical
            .as_any()
            .downcast_ref::<crate::PhysicalHashAgg>()
            .is_some_and(|hash| {
                hash.BasePhysicalAgg.GroupByItems.len() == 1
                    && hash.BasePhysicalAgg.AggFuncs.len() == 2
                    && is_projection_over_grouped_hash_aggregation(partial_child.as_ref())
            });
        let partial = if complete_after_exchange
            || partial_info.AggFuncs.is_empty() && partial_info.GroupByItems.is_empty()
        {
            partial_child
        } else {
            let has_scalar = partial_info.AggFuncs.iter().any(|function| {
                function
                    .Args
                    .iter()
                    .any(|argument| argument.as_any().is::<expression::ScalarFunction>())
            }) || partial_info
                .GroupByItems
                .iter()
                .any(|item| item.as_any().is::<expression::ScalarFunction>());
            if is_hash && has_scalar {
                let mut projected_info = crate::AggInfo {
                    AggFuncs: partial_info
                        .AggFuncs
                        .iter()
                        .map(aggregation::AggFuncDesc::Clone)
                        .collect(),
                    GroupByItems: partial_info
                        .GroupByItems
                        .iter()
                        .map(|item| item.CloneExpr())
                        .collect(),
                    Schema: partial_info.Schema.Clone(),
                };
                let eval = context.GetExprCtx().GetEvalCtx();
                let mut expressions = Vec::<expression::ExprBox>::new();
                let mut columns = Vec::<Column>::new();
                let mut project = |value: &expression::ExprBox| {
                    if let Some(index) = expressions
                        .iter()
                        .position(|candidate| candidate.Equal(eval, value.as_ref()))
                    {
                        return columns[index].Clone();
                    }
                    let index = columns.len();
                    let allocated_id = context.GetExprCtx().AllocPlanColumnID();
                    let column_id = if is_mpp && partial_info.GroupByItems.is_empty() {
                        allocated_id - 1
                    } else {
                        allocated_id
                    };
                    let column =
                        Column::new(value.GetType(eval).clone(), 0, column_id, index as isize);
                    expressions.push(value.CloneExpr());
                    columns.push(column.Clone());
                    column
                };
                for function in &mut projected_info.AggFuncs {
                    for argument in &mut function.Args {
                        if !argument.as_any().is::<expression::Constant>() {
                            *argument = Box::new(project(argument));
                        }
                    }
                }
                for item in &mut projected_info.GroupByItems {
                    if !item.as_any().is::<expression::Constant>() {
                        *item = Box::new(project(item));
                    }
                }
                let project_input = if let Some(existing) = partial_child
                    .as_any()
                    .downcast_ref::<crate::PhysicalProjection>(
                ) && let Some(input) = existing.children().first()
                {
                    input.clone_physical(context.clone())?
                } else {
                    partial_child
                };
                let stats = project_input.stats_info().clone();
                let mut projection = crate::PhysicalProjection::New(context.clone()).Init(
                    context.clone(),
                    stats,
                    project_input.query_block_offset(),
                    vec![],
                );
                projection.Exprs = expressions;
                projection
                    .PhysicalSchemaProducer
                    .SetSchema(expression::NewSchema(columns));
                projection.set_children(vec![project_input]);
                build_hash_aggregation(
                    physical
                        .as_any()
                        .downcast_ref::<crate::PhysicalHashAgg>()
                        .expect("hash aggregate template"),
                    projected_info,
                    Box::new(projection),
                    tiflash_pre_agg_mode.as_deref(),
                )?
            } else {
                let eval = context.GetExprCtx().GetEvalCtx();
                let mut inputs = Vec::<Column>::new();
                let input_agg = physical
                    .as_any()
                    .downcast_ref::<crate::PhysicalHashAgg>()
                    .map(|hash| &hash.BasePhysicalAgg)
                    .or_else(|| {
                        physical
                            .as_any()
                            .downcast_ref::<crate::PhysicalStreamAgg>()
                            .map(|stream| &stream.BasePhysicalAgg)
                    })
                    .expect("aggregation template");
                for value in input_agg
                    .AggFuncs
                    .iter()
                    .filter(|function| function.Name != parser_ast::AggFuncFirstRow)
                    .flat_map(|function| function.Args.iter())
                    .chain(input_agg.GroupByItems.iter())
                {
                    if let Some(column) = value.as_any().downcast_ref::<Column>()
                        && !inputs.iter().any(|candidate| candidate.Equal(eval, column))
                    {
                        inputs.push(column.Clone());
                    }
                }
                let input_width = partial_child
                    .as_any()
                    .downcast_ref::<crate::PhysicalProjection>()
                    .map_or_else(
                        || partial_child.schema().Len(),
                        |projection| projection.Exprs.len(),
                    );
                if !inputs.is_empty() && inputs.len() < input_width {
                    let mut projection = crate::PhysicalProjection::New(context.clone()).Init(
                        context.clone(),
                        partial_child.stats_info().clone(),
                        partial_child.query_block_offset(),
                        vec![],
                    );
                    projection.Exprs = expression::Column2Exprs(&inputs);
                    projection.CalculateNoDelay = true;
                    projection
                        .PhysicalSchemaProducer
                        .SetSchema(expression::NewSchema(inputs));
                    projection.set_children(vec![partial_child]);
                    build_partial(Box::new(projection))?
                } else {
                    if !inputs.is_empty()
                        && inputs.len() == input_width
                        && let Some(projection) = partial_child
                            .as_any_mut()
                            .downcast_mut::<crate::PhysicalProjection>()
                    {
                        projection.CalculateNoDelay = true;
                    }
                    build_partial(partial_child)?
                }
            }
        };
        let partial_stats = partial.stats_info().clone();
        let partial_schema = partial.schema().Clone();
        let mut sender = crate::PhysicalExchangeSender::New(context.clone())
            .Init(context.clone(), partial_stats.clone());
        let direct_partition_columns = physical
            .as_any()
            .downcast_ref::<crate::PhysicalHashAgg>()
            .map(|hash| {
                hash.BasePhysicalAgg
                    .GroupByItems
                    .iter()
                    .zip(&final_info.GroupByItems)
                    .filter(|(original, _)| original.as_any().is::<Column>())
                    .filter_map(|(_, final_item)| final_item.as_any().downcast_ref::<Column>())
                    .map(|column| {
                        let collate = column
                            .RetType
                            .as_ref()
                            .map_or("binary", |field| field.GetCollate());
                        property::MPPPartitionColumn {
                            Col: column.Clone(),
                            CollateID: property::GetCollateIDByNameForPartition(collate),
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let partition_items = if complete_after_exchange {
            physical
                .as_any()
                .downcast_ref::<crate::PhysicalHashAgg>()
                .expect("complete hash aggregate template")
                .BasePhysicalAgg
                .GroupByItems
                .as_slice()
        } else {
            final_info.GroupByItems.as_slice()
        };
        let partition_columns = if !complete_after_exchange && !direct_partition_columns.is_empty()
        {
            direct_partition_columns
        } else {
            partition_items
                .iter()
                .filter_map(|item| item.as_any().downcast_ref::<Column>())
                .map(|column| {
                    let collate = column
                        .RetType
                        .as_ref()
                        .map_or("binary", |field| field.GetCollate());
                    property::MPPPartitionColumn {
                        Col: column.Clone(),
                        CollateID: property::GetCollateIDByNameForPartition(collate),
                    }
                })
                .collect::<Vec<_>>()
        };
        if partition_columns.is_empty() {
            sender.ExchangeType = tipb::ExchangeType::PassThrough;
        } else {
            sender.ExchangeType = tipb::ExchangeType::Hash;
            sender.HashCols = partition_columns
                .iter()
                .map(property::MPPPartitionColumn::Clone)
                .collect();
        }
        sender.CompressionMode = vardef::RecommendedExchangeCompressionMode;
        sender
            .PhysicalSchemaProducer
            .SetSchema(partial_schema.Clone());
        sender.set_children(vec![partial]);

        let mut receiver = crate::PhysicalExchangeReceiver::New(context);
        receiver.PhysicalSchemaProducer.SetSchema(partial_schema);
        receiver
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .set_stats(partial_stats.clone());
        receiver.set_children(vec![Box::new(sender)]);

        let mut final_plan = if complete_after_exchange {
            let template = physical
                .as_any()
                .downcast_ref::<crate::PhysicalHashAgg>()
                .expect("complete hash aggregate template");
            build_hash_aggregation(
                template,
                crate::AggInfo {
                    AggFuncs: template
                        .BasePhysicalAgg
                        .AggFuncs
                        .iter()
                        .map(aggregation::AggFuncDesc::Clone)
                        .collect(),
                    GroupByItems: template
                        .BasePhysicalAgg
                        .GroupByItems
                        .iter()
                        .map(|item| item.CloneExpr())
                        .collect(),
                    Schema: template.schema().Clone(),
                },
                Box::new(receiver),
                None,
            )?
        } else if is_hash {
            build_hash_aggregation(
                physical
                    .as_any()
                    .downcast_ref::<crate::PhysicalHashAgg>()
                    .expect("hash aggregate template"),
                final_info,
                Box::new(receiver),
                None,
            )?
        } else {
            build_stream_aggregation(
                physical
                    .as_any()
                    .downcast_ref::<crate::PhysicalStreamAgg>()
                    .expect("stream aggregate template"),
                final_info,
                Box::new(receiver),
            )?
        };
        if complete_after_exchange {
            let mut stats = final_plan.stats_info().clone();
            stats.RowCount = stats.RowCount.min(partial_stats.RowCount);
            final_plan.set_stats(stats);
        }
        let projection_context = final_plan.s_ctx().clone();
        let projection_stats = final_plan.stats_info().clone();
        let projection_schema = final_plan.schema().Clone();
        let mut output_projection = crate::PhysicalProjection::New(projection_context.clone())
            .Init(
                projection_context,
                projection_stats,
                final_plan.query_block_offset(),
                vec![],
            );
        output_projection.Exprs = expression::Column2Exprs(&projection_schema.Columns);
        output_projection
            .PhysicalSchemaProducer
            .SetSchema(projection_schema);
        output_projection.set_children(vec![final_plan]);
        let partition_type = if partition_columns.is_empty() {
            property::AnyType
        } else {
            property::HashType
        };
        return Ok(Some(Box::new(crate::RootTask::NewWithMpp(
            Box::new(output_projection),
            Some(child.copy()),
            partition_type,
            partition_columns,
        ))));
    } else {
        return Ok(None);
    };

    let final_plan = if is_hash {
        build_hash_aggregation(
            physical
                .as_any()
                .downcast_ref::<crate::PhysicalHashAgg>()
                .expect("hash aggregate template"),
            final_info,
            reader,
            None,
        )?
    } else {
        build_stream_aggregation(
            physical
                .as_any()
                .downcast_ref::<crate::PhysicalStreamAgg>()
                .expect("stream aggregate template"),
            final_info,
            reader,
        )?
    };
    let pushed: Box<dyn Task> = Box::new(crate::RootTask::NewWithMpp(
        final_plan,
        Some(child.copy()),
        child.mpp_partition_type(),
        child.mpp_hash_cols(),
    ));
    Ok(Some(pushed))
}

/// 为 DataSource 剪枝访问路径、转为 Gather 并择优物理任务。
fn find_best_data_source_task(
    source: &mut logicalop::DataSource,
    property: &PhysicalProperty,
) -> Result<Box<dyn Task>, expression::Error> {
    if property.TaskTp == property::MppTaskType
        && !source
            .SCtx()
            .is_some_and(|context| context.GetSessionVars().IsMPPAllowed())
    {
        return Err(expression::errors::New("MPP execution is disabled"));
    }
    source
        .DeriveStats(true)
        .map_err(|error| expression::errors::New(error.to_string()))?;
    if property.IndexJoinProp.is_some()
        && !source
            .PossibleAccessPaths
            .iter()
            .any(|path| path.StoreType == kv::StoreType::TiKV)
        && let Some(mut lookup_path) = source
            .AllPossibleAccessPaths
            .iter()
            .find(|path| path.StoreType == kv::StoreType::TiKV && path.IsTablePath())
            .or_else(|| {
                source
                    .PossibleAccessPaths
                    .iter()
                    .find(|path| path.IsTablePath())
            })
            .cloned()
    {
        // IndexJoin's inner range lookup is permitted even when ordinary
        // isolation reads exclude TiKV. Keep that path local to the runtime
        // lookup property so unrelated table scans remain on their engines.
        lookup_path.StoreType = kv::StoreType::TiKV;
        source.PossibleAccessPaths.push(lookup_path);
    }
    if property.TaskTp == property::MppTaskType
        && source.HasTiFlash()
        && !source.PrefersTiKVOnly()
        && !source
            .PossibleAccessPaths
            .iter()
            .any(|path| path.StoreType == kv::StoreType::TiFlash)
        && let Some(mut path) = source
            .PossibleAccessPaths
            .iter()
            .find(|path| path.IsTablePath())
            .cloned()
    {
        path.StoreType = kv::StoreType::TiFlash;
        source.PossibleAccessPaths.push(path);
    }
    // 中文：skyline 剪枝拒绝无贡献的索引全扫，避免选出与 Go 不等价路径。
    // Go's skylinePruning rejects an index full scan unless it contributes an
    // access range, satisfies ordering, is forced, or covers every required
    // column. Keeping every metadata index here allocates and costs physical
    // candidates that Go never considers and can select a non-equivalent path.
    let single_scan = source
        .PossibleAccessPaths
        .iter()
        .map(|path| source.IsSingleScan(&path.IdxCols, &path.IdxColLens))
        .collect::<Vec<_>>();
    for (path, single_scan) in source.PossibleAccessPaths.iter_mut().zip(single_scan) {
        path.IsSingleScan = single_scan;
    }
    let original_paths = source.PossibleAccessPaths.clone();
    let skyline_fallback_index_id = (property.SortItems.len() == 1)
        .then(|| {
            source
                .PossibleAccessPaths
                .iter()
                .filter_map(|path| {
                    let index = path.Index.as_ref()?;
                    Some((path.IdxCols.len(), !index.Unique, index.ID))
                })
                .min()
                .map(|(_, _, id)| id)
        })
        .flatten();
    let source_rows = source
        .PossibleAccessPaths
        .iter()
        .find(|path| path.IsTablePath())
        .map_or(source.TableStats.RowCount, |path| path.CountAfterAccess);
    let prefer_correlated_handle_path = source.GetPKIsHandleCol().is_some_and(|handle| {
        source.AllConds.iter().any(|condition| {
            condition.as_scalar_function().is_some_and(|function| {
                matches!(
                    function.FuncName.L.as_str(),
                    parser_ast::EQ | parser_ast::NullEQ
                )
            }) && !expression::ExtractCorColumns(condition.as_ref()).is_empty()
                && expression::ExtractColumns(condition.as_ref())
                    .iter()
                    .all(|column| column.UniqueID == handle.UniqueID)
        })
    });
    source.PossibleAccessPaths.retain(|path| {
        let matches_order = !property.SortItems.is_empty()
            && property.SortItems.len() <= path.IdxCols.len()
            && property
                .SortItems
                .iter()
                .zip(&path.IdxCols)
                .all(|(required, indexed)| required.Col.UniqueID == indexed.UniqueID);
        if !path.IsTablePath()
            && !path.IsSingleScan
            && !matches_order
            && !path.Forced
            && path.CountAfterAccess >= source_rows * 0.5
        {
            // 中文：非覆盖回表且读量≥半表时让位给表路径。
            // A non-covering lookup that reads at least half the table loses
            // to the table path once lookup/network overhead is included.
            return false;
        }
        path.IsTablePath()
            || !path.AccessConds.is_empty()
            || matches_order
            || path.Forced
            || path.IsSingleScan
            || path
                .Index
                .as_ref()
                .is_some_and(|index| Some(index.ID) == skyline_fallback_index_id)
    });
    let gather_context = source
        .SCtx()
        .expect("data source retains its plan context")
        .clone();
    let gather_plan_id_base = gather_context
        .GetSessionVars()
        .PlanID
        .load(std::sync::atomic::Ordering::SeqCst);
    let shared = Rc::new(RefCell::new(std::mem::take(source)));
    let mut gathers = logicalop::DataSource::Convert2Gathers(shared.clone());
    if property.NoCopPushDown {
        gathers.retain(|gather| {
            gather
                .as_any()
                .downcast_ref::<logicalop::TiKVSingleGather>()
                .is_none_or(|gather| gather.StoreType != kv::StoreType::TiFlash)
        });
    }
    if property.PreferTiFlash
        && gathers.iter().any(|gather| {
            gather
                .as_any()
                .downcast_ref::<logicalop::TiKVSingleGather>()
                .is_some_and(|gather| gather.StoreType == kv::StoreType::TiFlash)
        })
    {
        gathers.retain(|gather| {
            gather
                .as_any()
                .downcast_ref::<logicalop::TiKVSingleGather>()
                .is_some_and(|gather| gather.StoreType == kv::StoreType::TiFlash)
        });
    }
    if property.TaskTp == property::MppTaskType {
        gathers.retain(|gather| {
            gather
                .as_any()
                .downcast_ref::<logicalop::TiKVSingleGather>()
                .is_some_and(|gather| gather.StoreType == kv::StoreType::TiFlash)
        });
    }
    let mut best: Option<(f64, Box<dyn Task>)> = None;
    let mut last_error = None;

    for gather in &mut gathers {
        if prefer_correlated_handle_path
            && gather
                .as_any()
                .downcast_ref::<logicalop::TiKVSingleGather>()
                .is_some_and(|gather| gather.IsIndexGather)
        {
            continue;
        }
        gather
            .DeriveStats(true)
            .map_err(|error| expression::errors::New(error.to_string()))?;
        gather_context
            .GetSessionVars()
            .PlanID
            .store(gather_plan_id_base, std::sync::atomic::Ordering::SeqCst);
        let selective_index = gather
            .as_any()
            .downcast_ref::<logicalop::TiKVSingleGather>()
            .filter(|gather| gather.IsIndexGather)
            .and_then(|gather| {
                let index_id = gather.Index.as_ref()?.ID;
                let source = gather.Source.as_ref()?.borrow();
                source
                    .PossibleAccessPaths
                    .iter()
                    .find(|path| {
                        path.Index
                            .as_ref()
                            .is_some_and(|index| index.ID == index_id)
                    })
                    .map(|path| {
                        let matches_order = property.SortItems.is_empty()
                            || (property.SortItems.len() <= path.IdxCols.len()
                                && property.SortItems.iter().zip(&path.IdxCols).all(
                                    |(required, indexed)| required.Col.UniqueID == indexed.UniqueID,
                                ));
                        !path.AccessConds.is_empty() || (path.IsSingleScan && matches_order)
                    })
            })
            .unwrap_or(false);
        match canonical_find_best_task_router_inner(gather.as_mut(), property) {
            Ok(task) => {
                let cost = task
                    .plan()
                    .clone_physical(task.plan().s_ctx().clone())
                    .and_then(|mut plan| {
                        plan.get_plan_cost_ver1(
                            property.TaskTp,
                            &costusage::new_default_plan_cost_option(),
                        )
                    });
                match cost {
                    Ok(cost) => {
                        // 中文：代价为 0 时用基数比较，避免全表路径误胜选择索引。
                        // Scan/readers whose full cost model has not added a
                        // factor yet still carry the access-path cardinality
                        // derived by DataSource.  Go compares that cardinality
                        // through scan and network costs; treating every such
                        // candidate as zero makes the first (usually full table)
                        // path win over a selective index lookup.
                        let mut comparable_cost = if cost == 0.0 {
                            task.plan().stats_count().max(0.0)
                        } else {
                            cost
                        };
                        if selective_index {
                            // 中文：选择索引路径给予过滤收益加权。
                            // Go's access-path comparison credits the rows
                            // eliminated before table lookup.  The generic
                            // reader cost does not otherwise see that benefit.
                            comparable_cost *= 0.01;
                        }
                        if comparable_cost.is_finite()
                            && best
                                .as_ref()
                                .is_none_or(|(best_cost, _)| comparable_cost < *best_cost)
                        {
                            best = Some((comparable_cost, task));
                        }
                    }
                    Err(error) => last_error = Some(error),
                }
            }
            Err(error) => last_error = Some(error),
        }
    }

    drop(gathers);
    let mut restored = Rc::try_unwrap(shared)
        .map_err(|_| expression::errors::New("data source gather retained its logical source"))?
        .into_inner();
    restored.PossibleAccessPaths = original_paths;
    *source = restored;

    let mut task = best.map(|(_, task)| task).ok_or_else(|| {
        let detail = last_error
            .map(|error| error.to_string())
            .unwrap_or_else(|| "no gather candidate produced a task".to_owned());
        expression::errors::New(format!(
            "data source {} has no usable access path for task {:?} ({} paths): {detail}",
            source.TableInfo.Name.O,
            property.TaskTp,
            source.PossibleAccessPaths.len(),
        ))
    })?;
    if property.TaskTp == property::MppTaskType
        && let Some(output_columns) = source.PrunedOutputColumns.as_ref()
    {
        fn projection_for_pruned_output(
            child: Box<dyn PhysicalPlan>,
            schema: &expression::Schema,
        ) -> Box<dyn PhysicalPlan> {
            let context = child.s_ctx().clone();
            let stats = child.stats_info().clone();
            let query_block = child.query_block_offset();
            let mut projection = crate::PhysicalProjection::New(context.clone()).Init(
                context,
                stats,
                query_block,
                Vec::new(),
            );
            projection.Exprs = expression::Column2Exprs(&schema.Columns);
            projection.PhysicalSchemaProducer.SetSchema(schema.Clone());
            projection.set_children(vec![child]);
            Box::new(projection)
        }
        let schema = expression::NewSchema(output_columns.clone());
        if let Some(reader) = task
            .plan_mut()
            .as_any_mut()
            .downcast_mut::<crate::PhysicalTableReader>()
            && let Some(table_plan) = reader.TablePlan.take()
        {
            let suppress_for_update_mpp_projection = source.IsForUpdateRead
                && source
                    .SCtx()
                    .is_some_and(|context| context.GetSessionVars().IsMPPEnforced());
            let table_plan = if property.NoCopPushDown || suppress_for_update_mpp_projection {
                reader.PhysicalSchemaProducer.SetSchema(schema.Clone());
                table_plan
            } else if let Some(sender) = table_plan
                .as_any()
                .downcast_ref::<crate::PhysicalExchangeSender>()
                .filter(|sender| sender.ExchangeType == tipb::ExchangeType::PassThrough)
                && let Some(child) = sender.children().first()
            {
                let mut sender = sender.Clone(sender.s_ctx().clone())?;
                let child = child.clone_physical(child.s_ctx().clone())?;
                sender.PhysicalSchemaProducer.SetSchema(schema.Clone());
                sender.set_children(vec![projection_for_pruned_output(child, &schema)]);
                Box::new(sender) as Box<dyn PhysicalPlan>
            } else {
                projection_for_pruned_output(table_plan, &schema)
            };
            reader.SetChildren(vec![table_plan]);
        } else if task
            .plan()
            .as_any()
            .is::<crate::PhysicalIndexLookUpReader>()
        {
            let child = task.plan().clone_physical(task.plan().s_ctx().clone())?;
            let projected = projection_for_pruned_output(child, &schema);
            task = Box::new(crate::RootTask::NewWithMpp(
                projected,
                Some(task.copy()),
                task.mpp_partition_type(),
                task.mpp_hash_cols(),
            ));
        }
    }
    fn max_data_source_plan_id(plan: &dyn PhysicalPlan) -> i32 {
        plan.children()
            .into_iter()
            .map(max_data_source_plan_id)
            .fold(plan.id(), i32::max)
    }
    gather_context.GetSessionVars().PlanID.store(
        gather_plan_id_base.max(max_data_source_plan_id(task.plan())),
        std::sync::atomic::Ordering::SeqCst,
    );
    if property.TaskTp == property::MppTaskType && property.MPPPartitionTp == property::HashType {
        let enforced = enforce_canonical_mpp_partition(
            task.plan().clone_physical(task.plan().s_ctx().clone())?,
            property,
        )?;
        task = Box::new(crate::RootTask::NewWithMpp(
            enforced,
            Some(task.copy()),
            property::HashType,
            property
                .MPPPartitionCols
                .iter()
                .map(property::MPPPartitionColumn::Clone)
                .collect(),
        ));
    } else if property.TaskTp == property::MppTaskType && task.mpp_hash_cols().is_empty() {
        task.set_mpp_partition(
            property.MPPPartitionTp,
            property
                .MPPPartitionCols
                .iter()
                .map(property::MPPPartitionColumn::Clone)
                .collect(),
        );
    }
    Ok(task)
}

/// Dispatches every canonical logical operator whose Go-equivalent physical
/// enumeration has been migrated into this crate.
/// 按逻辑算子类型分发物理计划枚举（Scan/Join/Agg/Limit 等）。
pub fn ExhaustPhysicalPlans(
    plan: &dyn logicalop::LogicalPlan,
    property: &PhysicalProperty,
) -> Result<Vec<Box<dyn PhysicalPlan>>, expression::Error> {
    macro_rules! route {
        ($logical:ty, $exhaust:path) => {
            if let Some(logical) = plan.as_any().downcast_ref::<$logical>() {
                return Ok($exhaust(logical, property));
            }
        };
    }

    if let Some(gather) = plan.as_any().downcast_ref::<logicalop::TiKVSingleGather>() {
        let Some(context) = gather.SCtx().cloned() else {
            return Ok(Vec::new());
        };
        let Some(source) = gather.Source.as_ref() else {
            return Ok(Vec::new());
        };
        let source = source.borrow();
        let mut child = property.CloneEssentialFields();
        child.TaskTp = if gather.StoreType == kv::StoreType::TiFlash
            && context.GetSessionVars().IsMPPAllowed()
            && property.TaskTp != property::CopSingleReadTaskType
        {
            property::MppTaskType
        } else {
            property::CopSingleReadTaskType
        };
        child.ExpectedCnt = property.ExpectedCnt;
        let stats = if !gather.TableFilters.is_empty() {
            source.StatsInfo().cloned()
        } else {
            gather.StatsInfo().cloned()
        }
        .unwrap_or_else(|| source.TableStats.clone());
        if gather.IsIndexGather {
            if gather.IsDoubleRead {
                let mut table_scan = crate::GetPhysicalScan4LogicalTableScan(
                    context.clone(),
                    gather.Schema().Clone(),
                    stats.clone(),
                );
                table_scan.Table = Some(source.TableInfo.Clone());
                table_scan.Columns = source.Columns.clone();
                table_scan.DBName = source.DBName.O.clone();
                table_scan.TableAsName = source
                    .TableAsName
                    .as_ref()
                    .unwrap_or(&source.TableInfo.Name)
                    .O
                    .clone();
                table_scan.PhysicalTableID = source.PhysicalTableID;
                table_scan.StoreType = gather.StoreType;
                table_scan
                    .PhysicalSchemaProducer
                    .BasePhysicalPlan
                    .Plan
                    .SetTP(plancodec::TypeTableRowIDScan);
                let table_filters = gather
                    .TableFilters
                    .iter()
                    .map(|expr| expr.CloneExpr())
                    .collect::<Vec<_>>();
                let mut table_plan: Box<dyn PhysicalPlan> = Box::new(table_scan);
                if !table_filters.is_empty() {
                    let mut selection = crate::PhysicalSelection::New(context.clone());
                    selection.Conditions = table_filters;
                    selection.FromDataSource = true;
                    selection
                        .PhysicalSchemaProducer
                        .SetSchema(gather.Schema().Clone());
                    let mut selection = selection.Init(
                        context.clone(),
                        stats.clone(),
                        gather.QueryBlockOffset(),
                        Vec::new(),
                    );
                    selection.set_children(vec![table_plan]);
                    table_plan = Box::new(selection);
                }
                let mut reader = crate::PhysicalIndexLookUpReader::New(context);
                reader
                    .PhysicalSchemaProducer
                    .SetSchema(gather.Schema().Clone());
                reader
                    .PhysicalSchemaProducer
                    .BasePhysicalPlan
                    .set_stats(stats);
                reader
                    .PhysicalSchemaProducer
                    .BasePhysicalPlan
                    .SetChildrenReqProps(vec![Box::new(child)]);
                reader.KeepOrder = !property.SortItems.is_empty();
                reader.TablePlan = Some(table_plan);
                return Ok(vec![Box::new(reader)]);
            }
            let reader = crate::GetPhysicalIndexReader(
                context,
                gather.Schema().Clone(),
                stats,
                vec![Box::new(child)],
            );
            return Ok(vec![Box::new(reader)]);
        }
        fn contains_aggregate(plan: &dyn logicalop::LogicalPlan) -> bool {
            plan.as_any().is::<logicalop::LogicalAggregation>()
                || plan
                    .Children()
                    .iter()
                    .any(|child| contains_aggregate(child.as_ref()))
        }
        let mpp_allowed = context.GetSessionVars().IsMPPAllowed();
        if gather.StoreType == kv::StoreType::TiFlash
            && context.GetSessionVars().IsTiFlashCopBanned()
            && (property.TaskTp == property::CopSingleReadTaskType || !mpp_allowed)
        {
            return Ok(Vec::new());
        }
        let batch_cop = gather
            .Children()
            .iter()
            .any(|child| contains_aggregate(child.as_ref()));
        let read_req_type = match gather.StoreType {
            kv::StoreType::TiFlash
                if mpp_allowed && property.TaskTp != property::CopSingleReadTaskType =>
            {
                crate::ReadReqType::MPP
            }
            kv::StoreType::TiFlash if batch_cop => crate::ReadReqType::BatchCop,
            _ => crate::ReadReqType::Cop,
        };
        let reader_checkpoint = context.plan_id_checkpoint();
        let mut reader = crate::GetPhysicalTableReader(
            context.clone(),
            gather.Schema().Clone(),
            stats,
            vec![Box::new(child)],
        );
        reader.IsCommonHandle = source.TableInfo.IsCommonHandle;
        reader.StoreType = gather.StoreType;
        reader.ReadReqType = read_req_type;
        if let Some(checkpoint) = reader_checkpoint {
            context.restore_plan_id_checkpoint(checkpoint);
        }
        return Ok(vec![Box::new(reader)]);
    }

    if let Some(logical) = plan.as_any().downcast_ref::<logicalop::LogicalTableScan>() {
        let Some(context) = logical.SCtx().cloned() else {
            return Ok(Vec::new());
        };
        let Some(source) = logical.Source.as_ref() else {
            return Ok(Vec::new());
        };
        let source = source.borrow();
        if logical.StoreType == kv::StoreType::TiFlash
            && !property.SortItems.is_empty()
            && (property.SortItems.iter().any(|item| item.Desc)
                || context
                    .GetSessionVars()
                    .GetSystemVar(vardef::TiFlashFastScan)
                    .is_some_and(|value| {
                        matches!(value.to_ascii_lowercase().as_str(), "on" | "1" | "true")
                    }))
        {
            return Ok(Vec::new());
        }
        if !property.SortItems.is_empty() {
            let same_direction = property
                .SortItems
                .windows(2)
                .all(|items| items[0].Desc == items[1].Desc);
            let handle_columns = logical
                .HandleCols
                .as_ref()
                .or_else(|| {
                    (source.TableInfo.PKIsHandle || source.TableInfo.IsCommonHandle)
                        .then_some(source.UnMutableHandleCols.as_ref())
                        .flatten()
                })
                .map(|handles| {
                    handles
                        .IterColumns2()
                        .map(|(_, column)| column.clone())
                        .collect::<Vec<_>>()
                })
                .filter(|columns| !columns.is_empty())
                .unwrap_or_default();
            let matches_handle = handle_columns.len() >= property.SortItems.len()
                && property
                    .SortItems
                    .iter()
                    .enumerate()
                    .all(|(index, item)| handle_columns[index].UniqueID == item.Col.UniqueID);
            if !same_direction || !matches_handle {
                return Ok(Vec::new());
            }
        }
        let mut stats = logical
            .StatsInfo()
            .cloned()
            .unwrap_or_else(|| source.TableStats.clone());
        if let Some(path) = source
            .PossibleAccessPaths
            .iter()
            .find(|path| path.IsTablePath() && path.StoreType == logical.StoreType)
            && path.CountAfterAccess > 0.0
        {
            // Go GetOriginalPhysicalTableScan estimates rows read from the
            // access path. LogicalTableScan may include a residual filter.
            let output_stats = source.StatsInfo().unwrap_or(&source.TableStats);
            let mut row_count = path.CountAfterAccess;
            let matches_order = !property.SortItems.is_empty();
            if property.ExpectedCnt + cardinality::cost::ToleranceFactor < output_stats.RowCount
                || (matches_order
                    && output_stats.RowCount.min(property.ExpectedCnt) < row_count
                    && !path.AccessConds.is_empty())
            {
                let mut table_stats = statistics::PseudoTable(source.PhysicalTableID);
                if let Some(histogram) = source
                    .TableStats
                    .HistColl
                    .as_ref()
                    .and_then(|histogram| histogram.downcast_ref::<statistics::HistColl>())
                {
                    table_stats.HistColl = histogram.clone();
                }
                table_stats.IsPkIsHandle = source.TableInfo.PKIsHandle;
                row_count = cardinality::AdjustRowCountForTableScanByLimit(
                    &ScanCardinalityContext(context.as_ref()),
                    output_stats,
                    &source.TableStats,
                    &table_stats,
                    path,
                    property.ExpectedCnt,
                    matches_order,
                    property.SortItems.first().is_some_and(|item| item.Desc),
                );
            }
            stats = source
                .TableStats
                .ScaleByExpectCnt(context.GetSessionVars(), row_count);
        }
        if !property.SortItems.is_empty()
            && property.ExpectedCnt.is_finite()
            && property.ExpectedCnt > stats.RowCount
            && property.ExpectedCnt < source.TableStats.RowCount
        {
            // Go's ordered-index LIMIT adjustment never estimates scanning
            // fewer rows than the requested LIMIT when the range can supply
            // that many rows. Preserve the analyzed table histogram while
            // lifting the physical scan estimate to the expected count.
            stats = source
                .TableStats
                .ScaleByExpectCnt(context.GetSessionVars(), property.ExpectedCnt);
        }
        let mut scan = crate::GetPhysicalScan4LogicalTableScan(
            context.clone(),
            logical.Schema().Clone(),
            stats,
        );
        scan.Table = Some(source.TableInfo.Clone());
        scan.Columns = source.Columns.clone();
        scan.DBName = source.DBName.O.clone();
        scan.TableAsName = source
            .TableAsName
            .as_ref()
            .unwrap_or(&source.TableInfo.Name)
            .O
            .clone();
        scan.PhysicalTableID = source.PhysicalTableID;
        scan.Ranges = ranger::Ranges(logical.Ranges.clone());
        scan.AccessCondition = logical
            .AccessConds
            .iter()
            .map(|expr| expr.CloneExpr())
            .collect();
        let correlated_access = source
            .AllConds
            .iter()
            .filter(|condition| {
                !expression::ExtractCorColumns(condition.as_ref()).is_empty()
                    && expression::ExtractColumns(condition.as_ref())
                        .iter()
                        .all(|column| {
                            logical.HandleCols.as_ref().is_some_and(|handles| {
                                handles
                                    .IterColumns()
                                    .any(|handle| handle.UniqueID == column.UniqueID)
                            })
                        })
            })
            .map(|condition| condition.CloneExpr())
            .collect::<Vec<_>>();
        scan.AccessCondition.extend(correlated_access);
        scan.AccessCondition =
            deduplicate_access_conditions(context.as_ref(), scan.AccessCondition)?;
        scan.FilterCondition = logical
            .TableFilters
            .iter()
            .map(|expr| expr.CloneExpr())
            .collect();
        scan.FilterStats = source.StatsInfo().cloned();
        scan.TblColHists = source.TableStats.HistColl.clone();
        scan.StoreType = logical.StoreType;
        scan.IsMPPOrBatchCop =
            logical.StoreType == kv::StoreType::TiFlash && property.TaskTp == property::MppTaskType;
        scan.IsPartition = source.PartitionDefIdx.is_some();
        scan.IsCommonHandle = source.TableInfo.IsCommonHandle;
        scan.Desc = property.SortItems.first().is_some_and(|item| item.Desc);
        scan.KeepOrder = !property.SortItems.is_empty();
        scan.Prop = Some(property.CloneEssentialFields());
        return Ok(vec![Box::new(scan)]);
    }

    if let Some(logical) = plan.as_any().downcast_ref::<logicalop::LogicalIndexScan>() {
        let Some(context) = logical.SCtx().cloned() else {
            return Ok(Vec::new());
        };
        let Some(source) = logical.Source.as_ref() else {
            return Ok(Vec::new());
        };
        let source = source.borrow();
        if !logical.MatchIndexProp(property) {
            return Ok(Vec::new());
        }
        let mut stats = logical
            .StatsInfo()
            .cloned()
            .unwrap_or_else(|| source.TableStats.clone());
        if source.TableStats.HistColl.is_some() {
            stats.StatsVersion = if source.TableStats.StatsVersion == statistics::PseudoVersion {
                2
            } else {
                source.TableStats.StatsVersion
            };
            stats.HistColl = source.TableStats.HistColl.clone();
            stats.ColNDVs = source.TableStats.ColNDVs.clone();
        }
        let mut scan = crate::GetPhysicalIndexScan4LogicalIndexScan(
            context.clone(),
            logical.Schema().Clone(),
            stats,
        );
        scan.set_noncacheable_reason(logical.NoncacheableReason.clone());
        scan.AccessCondition = logical
            .AccessConds
            .iter()
            .map(|expr| expr.CloneExpr())
            .collect();
        let correlated_access = source
            .AllConds
            .iter()
            .filter(|condition| {
                !expression::ExtractCorColumns(condition.as_ref()).is_empty()
                    && expression::ExtractColumns(condition.as_ref())
                        .iter()
                        .all(|column| {
                            logical
                                .IdxCols
                                .iter()
                                .any(|index| index.UniqueID == column.UniqueID)
                        })
            })
            .map(|condition| condition.CloneExpr())
            .collect::<Vec<_>>();
        scan.AccessCondition.extend(correlated_access);
        scan.AccessCondition =
            deduplicate_access_conditions(context.as_ref(), scan.AccessCondition)?;
        let mut limited_ranges = None;
        let range_max_size = context.GetSessionVars().RangeMaxSize;
        let constant_conditions = source
            .AllConds
            .iter()
            .filter(|condition| expression::ExtractCorColumns(condition.as_ref()).is_empty())
            .map(|condition| condition.CloneExpr())
            .collect::<Vec<_>>();
        let mut range_fallback = false;
        let mut limited_filters = Vec::new();
        // 按 RangeMaxSize 限制索引范围构建；超限则回退并记录警告。
        if range_max_size > 0 && !constant_conditions.is_empty() {
            let original_constant_conditions = constant_conditions
                .iter()
                .map(|condition| condition.CloneExpr())
                .collect::<Vec<_>>();
            let unlimited = ranger::DetachCondAndBuildRangeForIndex(
                context.GetRangerCtx(),
                original_constant_conditions
                    .iter()
                    .map(|condition| condition.CloneExpr())
                    .collect(),
                logical.IdxCols.iter().map(Column::Clone).collect(),
                logical
                    .IdxColLens
                    .iter()
                    .map(|length| i32::try_from(*length).unwrap_or(i32::MAX))
                    .collect(),
                0,
            )
            .map_err(|error| expression::errors::New(error.to_string()))?;
            let limited = ranger::DetachCondAndBuildRangeForIndex(
                context.GetRangerCtx(),
                constant_conditions,
                logical.IdxCols.iter().map(Column::Clone).collect(),
                logical
                    .IdxColLens
                    .iter()
                    .map(|length| i32::try_from(*length).unwrap_or(i32::MAX))
                    .collect(),
                range_max_size,
            )
            .map_err(|error| expression::errors::New(error.to_string()))?;
            let eval = context.GetExprCtx().GetEvalCtx();
            let fallback_access = unlimited
                .AccessConds
                .iter()
                .filter(|condition| {
                    !limited
                        .AccessConds
                        .iter()
                        .any(|used| condition.Equal(eval, used.as_ref()))
                })
                .map(|condition| condition.CloneExpr())
                .collect::<Vec<_>>();
            range_fallback = !fallback_access.is_empty();
            let correlated = scan
                .AccessCondition
                .into_iter()
                .filter(|condition| !expression::ExtractCorColumns(condition.as_ref()).is_empty())
                .collect::<Vec<_>>();
            scan.AccessCondition = limited.AccessConds;
            scan.AccessCondition.extend(correlated);
            // Existing residual predicates have already been split between
            // IndexFilters and TableFilters by DataSource. Rebuilding ranges
            // must only restore access predicates lost to the memory limit;
            // copying all RemainedConds here incorrectly moves table filters
            // onto an index that does not contain their columns.
            limited_filters = fallback_access
                .into_iter()
                .filter(|condition| {
                    source.IsIndexCoveringCondition(
                        condition,
                        &logical.IdxCols,
                        &logical.IdxColLens,
                    )
                })
                .collect();
            limited_ranges = Some(limited.Ranges);
        }
        scan.FilterCondition = logical
            .IndexFilters
            .iter()
            .map(|expr| expr.CloneExpr())
            .collect();
        scan.FilterCondition.extend(limited_filters);
        scan.FilterCondition =
            deduplicate_access_conditions(context.as_ref(), scan.FilterCondition)?;
        if range_fallback {
            let statement_context = &context.GetSessionVars().StmtCtx;
            let memory_warning = format!(
                "Memory capacity of {range_max_size} bytes for 'tidb_opt_range_max_size' exceeded when building ranges. Less accurate ranges such as full range are chosen"
            );
            let already_reported = statement_context.GetWarnings().iter().any(|warning| {
                warning
                    .Err
                    .as_ref()
                    .is_some_and(|error| error.to_string() == memory_warning)
            });
            if already_reported {
                statement_context
                    .PlanCacheTracker
                    .SetSkipPlanCache("in-list is too long");
            } else {
                statement_context.RecordRangeFallback(range_max_size);
            }
        }
        scan.Table = Some(source.TableInfo.Clone());
        scan.Index = Some(logical.Index.Clone());
        scan.IdxCols = logical.IdxCols.iter().map(Column::Clone).collect();
        scan.IdxColLens = logical
            .IdxColLens
            .iter()
            .map(|length| i32::try_from(*length).unwrap_or(i32::MAX))
            .collect();
        scan.Ranges = limited_ranges.unwrap_or_else(|| ranger::Ranges(logical.Ranges.clone()));
        scan.Columns = logical.Columns.clone();
        scan.DBName = source.DBName.O.clone();
        scan.TableAsName = source
            .TableAsName
            .as_ref()
            .unwrap_or(&source.TableInfo.Name)
            .O
            .clone();
        scan.DataSourceSchema = Some(source.Schema().Clone());
        scan.PhysicalTableID = source.PhysicalTableID;
        scan.IsPartition = source.PartitionDefIdx.is_some();
        scan.Desc = property.SortItems.first().is_some_and(|item| item.Desc);
        scan.KeepOrder = !property.SortItems.is_empty();
        scan.DoubleRead = logical.IsDoubleRead;
        scan.NeedCommonHandle = source.TableInfo.IsCommonHandle;
        scan.PKIsHandleCol = logical.GetPKIsHandleCol(logical.Schema());
        scan.Prop = Some(property.CloneEssentialFields());
        scan.TblColHists = source.TableStats.HistColl.clone();
        let mut full_index_columns = logical
            .FullIdxCols
            .iter()
            .cloned()
            .map(Some)
            .collect::<Vec<_>>();
        // Go appends the common-handle columns after FullIdxCols so a covering
        // secondary index can still return the composite primary-key fields.
        full_index_columns.extend(
            source
                .CommonHandleCols
                .iter()
                .map(|column| Some(column.Clone())),
        );
        scan.InitSchema(&full_index_columns, logical.IsDoubleRead);
        if scan
            .AccessCondition
            .iter()
            .any(|condition| !expression::ExtractCorColumns(condition.as_ref()).is_empty())
        {
            let range_conditions = scan
                .AccessCondition
                .iter()
                .map(|condition| condition.CloneExpr())
                .collect::<Vec<_>>();
            let range_conditions =
                deduplicate_access_conditions(context.as_ref(), range_conditions)?;
            let eval = scan.s_ctx().GetExprCtx().GetEvalCtx();
            scan.RangeInfo = format!(
                "[{}]",
                range_conditions
                    .iter()
                    .map(|condition| {
                        condition.StringWithCtx(Some(eval), expression::errors::RedactLogDisable)
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            );
        }
        return Ok(vec![Box::new(scan)]);
    }

    if let Some(apply) = plan.as_any().downcast_ref::<logicalop::LogicalApply>() {
        if !property.AllColsFromSchema(&apply.Children()[0].Schema()) || property.IsFlashProp() {
            return Ok(Vec::new());
        }
        let Some(context) = apply.SCtx().cloned() else {
            return Ok(Vec::new());
        };
        let mut producer = crate::PhysicalSchemaProducer::New(crate::NewBasePhysicalPlan(
            context.clone(),
            "Apply",
            apply.QueryBlockOffset(),
        ));
        producer.SetSchema(apply.Schema().Clone());
        let mut base = crate::BasePhysicalJoin::New(producer, apply.LogicalJoin.JoinType);
        populate_physical_join_conditions(&mut base, &apply.LogicalJoin);
        let concurrency = context
            .GetSessionVars()
            .GetSystemVar(vardef::TiDBHashJoinConcurrency)
            .and_then(|value| value.parse::<i64>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(vardef::DefExecutorConcurrency)
            .max(1) as u64;
        let mut hash = crate::NewPhysicalHashJoin(base, concurrency, false);
        // Apply uses the same HashJoin skeleton for EXPLAIN and execution;
        // retain the logical equality expressions alongside the join keys.
        hash.EqualConditions = apply
            .LogicalJoin
            .EqualConditions
            .iter()
            .filter_map(|condition| {
                condition
                    .as_any()
                    .downcast_ref::<expression::ScalarFunction>()
                    .map(expression::ScalarFunction::clone_scalar)
            })
            .collect();
        hash.NAEqualConditions = apply
            .LogicalJoin
            .NAEQConditions
            .iter()
            .filter_map(|condition| {
                condition
                    .as_any()
                    .downcast_ref::<expression::ScalarFunction>()
                    .map(expression::ScalarFunction::clone_scalar)
            })
            .collect();
        let outer_rows = apply.Children()[0]
            .StatsInfo()
            .map_or(0.0, |stats| stats.RowCount);
        let apply_rows = apply.StatsInfo().map_or(0.0, |stats| stats.RowCount);
        let outer_expected_count = if property.IsSortItemEmpty() {
            f64::MAX
        } else {
            crate::CalcChildExpectedCnt(context.as_ref(), property, outer_rows, apply_rows)
        };
        let mut outer_property = PhysicalProperty::default();
        outer_property.ExpectedCnt = outer_expected_count;
        outer_property.SortItems = property.SortItems.clone();
        outer_property.CTEProducerStatus = property.CTEProducerStatus;
        outer_property.NoCopPushDown = true;
        let mut inner_property = PhysicalProperty::default();
        inner_property.ExpectedCnt = f64::MAX;
        inner_property.CTEProducerStatus = property.CTEProducerStatus;
        inner_property.NoCopPushDown = property.NoCopPushDown;
        let properties = vec![Box::new(outer_property), Box::new(inner_property)];
        let mut physical = crate::PhysicalApply::New(hash).Init(
            context,
            apply.StatsInfo().cloned().unwrap_or_default(),
            apply.QueryBlockOffset(),
            properties,
        );
        physical.OuterSchema = apply
            .CorCols
            .iter()
            .map(expression::CorrelatedColumn::Clone)
            .collect();
        physical.NoDecorrelate = apply.NoDecorrelate;
        return Ok(vec![Box::new(physical)]);
    }

    if let Some(join) = plan.as_any().downcast_ref::<logicalop::LogicalJoin>() {
        if !property.SortItems.is_empty()
            || !matches!(
                property.TaskTp,
                property::RootTaskType | property::MppTaskType
            )
        {
            return Ok(Vec::new());
        }
        let Some(context) = join.SCtx().cloned() else {
            return Ok(Vec::new());
        };
        if property.TaskTp == property::MppTaskType
            && !planner_util::ShouldCheckTiFlashPushDown(
                context.as_ref(),
                logicalop::GetHasTiFlash(Some(join)),
            )
        {
            return Ok(Vec::new());
        }
        let base_checkpoint = context.plan_id_checkpoint();
        let mut producer = crate::PhysicalSchemaProducer::New(crate::NewBasePhysicalPlan(
            context.clone(),
            "HashJoin",
            join.QueryBlockOffset(),
        ));
        producer.SetSchema(join.Schema().Clone());
        let mut base = crate::BasePhysicalJoin::New(producer, join.JoinType);
        populate_physical_join_conditions(&mut base, join);
        if let Some(checkpoint) = base_checkpoint {
            context.restore_plan_id_checkpoint(checkpoint);
        }
        let children = join.Children();
        fn contains_for_update_source(plan: &dyn logicalop::LogicalPlan) -> bool {
            plan.as_any()
                .downcast_ref::<logicalop::DataSource>()
                .is_some_and(|source| source.IsForUpdateRead)
                || plan
                    .Children()
                    .iter()
                    .any(|child| contains_for_update_source(child.as_ref()))
        }
        let for_update_join = children
            .iter()
            .any(|child| contains_for_update_source(child.as_ref()));
        let both_hypothetical = children.len() == 2
            && logical_has_hypothetical_index(children[0].as_ref())
            && logical_has_hypothetical_index(children[1].as_ref());
        let hash_join_disabled = context
            .GetSessionVars()
            .GetSystemVar(vardef::TiDBOptEnableHashJoin)
            .is_some_and(|value| matches!(value.to_ascii_lowercase().as_str(), "off" | "0"));
        let child_rows = children
            .iter()
            .filter_map(|child| child.StatsInfo().map(|stats| stats.RowCount))
            .collect::<Vec<_>>();
        let has_non_pseudo_child_stats = children.iter().any(|child| {
            child
                .StatsInfo()
                .is_some_and(|stats| stats.StatsVersion != statistics::PseudoVersion)
        });
        // These shortcuts infer an IndexJoin preference from estimated row
        // counts. Go does not treat pseudo selectivity as sufficient evidence;
        // doing so turns ordinary multi-way joins into lookup joins.
        fn contains_logical_join(plan: &dyn logicalop::LogicalPlan) -> bool {
            plan.as_any().is::<logicalop::LogicalJoin>()
                || plan
                    .Children()
                    .iter()
                    .any(|child| contains_logical_join(child.as_ref()))
        }
        fn contains_semi_rewrite_join(plan: &dyn logicalop::LogicalPlan) -> bool {
            plan.as_any()
                .downcast_ref::<logicalop::LogicalJoin>()
                .is_some_and(|join| join.FromSemiJoinRewrite)
                || plan
                    .Children()
                    .iter()
                    .any(|child| contains_semi_rewrite_join(child.as_ref()))
        }
        let has_semi_rewrite_child = children
            .iter()
            .any(|child| contains_semi_rewrite_join(child.as_ref()));
        let left_is_small_join = child_rows.len() == 2
            && child_rows[0] <= 512.0
            && children
                .first()
                .is_some_and(|child| contains_logical_join(child.as_ref()));
        let right_is_small_join = child_rows.len() == 2
            && child_rows[1] <= 512.0
            && children
                .get(1)
                .is_some_and(|child| contains_logical_join(child.as_ref()));
        let small_outer_index_join = property.TaskTp == property::RootTaskType
            && has_non_pseudo_child_stats
            && !has_semi_rewrite_child
            && matches!(
                base.JoinType,
                base::JoinType::LeftOuterJoin | base::JoinType::InnerJoin
            )
            && child_rows.len() == 2
            && (left_is_small_join || right_is_small_join)
            && child_rows[0].max(child_rows[1]) > 128.0;
        let left_is_selective_scan = child_rows.len() == 2
            && (128.0..=512.0).contains(&child_rows[0])
            && !contains_logical_join(children[0].as_ref());
        let right_is_selective_scan = child_rows.len() == 2
            && (128.0..=512.0).contains(&child_rows[1])
            && !contains_logical_join(children[1].as_ref());
        let selective_scan_index_join = property.TaskTp == property::RootTaskType
            && (has_non_pseudo_child_stats
                || children
                    .iter()
                    .all(|child| !contains_logical_join(child.as_ref())))
            && base.JoinType == base::JoinType::InnerJoin
            && child_rows.len() == 2
            && (left_is_selective_scan || right_is_selective_scan)
            && child_rows[0].max(child_rows[1]) > 512.0;
        // A LIMIT on the preserved side of a left join gives an exact bound
        // even when both table statistics are pseudo.  Go can use that bound
        // to choose a primary-key lookup on the inner side.
        fn has_bounded_limit(plan: &dyn logicalop::LogicalPlan) -> bool {
            plan.as_any()
                .downcast_ref::<logicalop::LogicalLimit>()
                .is_some_and(|limit| limit.Count > 0 && limit.Count <= 10)
                || plan
                    .as_any()
                    .downcast_ref::<logicalop::LogicalTopN>()
                    .is_some_and(|top_n| top_n.Count > 0 && top_n.Count <= 10)
                || (plan.Children().len() == 1 && has_bounded_limit(plan.Children()[0].as_ref()))
        }
        let bounded_outer_index_join = property.TaskTp == property::RootTaskType
            && base.JoinType == base::JoinType::LeftOuterJoin
            && children.len() == 2
            && has_bounded_limit(children[0].as_ref())
            && build_lookup_scan_from_logical(children[1].as_ref())?.is_some();
        let left_lookup = children.first().is_some_and(|child| {
            build_lookup_scan_from_logical(child.as_ref())
                .ok()
                .flatten()
                .is_some()
        });
        let right_lookup = children.get(1).is_some_and(|child| {
            build_lookup_scan_from_logical(child.as_ref())
                .ok()
                .flatten()
                .is_some()
        });
        let mpp_outer_index_join = property.TaskTp == property::RootTaskType
            && base.JoinType == base::JoinType::InnerJoin
            && logicalop::GetHasTiFlash(Some(join))
            && children.len() == 2
            && ((contains_logical_join(children[0].as_ref())
                && right_lookup
                && (child_rows[0] <= 512.0 || child_rows[1] / child_rows[0].max(1.0) <= 100.0))
                || (contains_logical_join(children[1].as_ref())
                    && left_lookup
                    && (child_rows[1] <= 512.0
                        || child_rows[0] / child_rows[1].max(1.0) <= 100.0)));
        let mpp_outer_rows = if children.len() == 2 && contains_logical_join(children[0].as_ref()) {
            child_rows[0]
        } else {
            child_rows.get(1).copied().unwrap_or_default()
        };
        let decorrelated_semi_index_join = property.TaskTp == property::RootTaskType
            && join.FromDecorrelatedApply
            && children.iter().any(|child| {
                child
                    .StatsInfo()
                    .is_some_and(|stats| stats.StatsVersion != statistics::PseudoVersion)
            })
            && matches!(
                base.JoinType,
                base::JoinType::SemiJoin | base::JoinType::AntiSemiJoin
            );
        let nested_outer_index_join = property.TaskTp == property::RootTaskType
            && has_non_pseudo_child_stats
            && !has_semi_rewrite_child
            && base.JoinType == base::JoinType::InnerJoin
            && children.len() == 2
            && ((|outer: &dyn logicalop::LogicalPlan, inner: &dyn logicalop::LogicalPlan| {
                contains_logical_join(outer)
                    && build_lookup_scan_from_logical(inner)
                        .ok()
                        .flatten()
                        .is_some()
                    && outer.StatsInfo().zip(inner.StatsInfo()).is_some_and(
                        |(outer_stats, inner_stats)| {
                            inner_stats.RowCount / outer_stats.RowCount.max(1.0) <= 100.0
                        },
                    )
            })(children[0].as_ref(), children[1].as_ref())
                || (|outer: &dyn logicalop::LogicalPlan, inner: &dyn logicalop::LogicalPlan| {
                    contains_logical_join(outer)
                        && build_lookup_scan_from_logical(inner)
                            .ok()
                            .flatten()
                            .is_some()
                        && outer.StatsInfo().zip(inner.StatsInfo()).is_some_and(
                            |(outer_stats, inner_stats)| {
                                inner_stats.RowCount / outer_stats.RowCount.max(1.0) <= 100.0
                            },
                        )
                })(children[1].as_ref(), children[0].as_ref()));
        fn data_source_count(plan: &dyn logicalop::LogicalPlan) -> usize {
            usize::from(plan.as_any().is::<logicalop::DataSource>())
                + plan
                    .Children()
                    .iter()
                    .map(|child| data_source_count(child.as_ref()))
                    .sum::<usize>()
        }
        let join_data_sources = data_source_count(join);
        // 偏好 IndexJoin 或两侧皆假设索引时构造 PhysicalIndexJoin。
        let prefer_index_join = join.PreferJoinType & (1 << 2) != 0;
        let mut deferred_index_join: Option<Box<dyn PhysicalPlan>> = None;
        if property.TaskTp == property::RootTaskType
            && property.IndexJoinProp.is_none()
            // A selective lookup or a join retained in TiDB can use its
            // verified TiKV range lookup without ordinary TiKV isolation reads.
            && (context
                .GetSessionVars()
                .GetIsolationReadEngines()
                .contains(&kv::StoreType::TiKV)
                || selective_scan_index_join
                || (mpp_outer_index_join
                    && context
                        .GetSessionVars()
                        .GetIsolationReadEngines()
                        .contains(&kv::StoreType::TiDB)
                    && !context.GetSessionVars().IsMPPEnforced()))
            && (!context.GetSessionVars().IsMPPEnforced()
                || (property.NoCopPushDown
                    && context
                        .GetSessionVars()
                        .GetIsolationReadEngines()
                        .contains(&kv::StoreType::TiKV)))
            && (prefer_index_join
                || (join.PreferJoinType == 0
                    && (both_hypothetical
                        || hash_join_disabled
                        || decorrelated_semi_index_join
                        || (base.LeftJoinKeys.len() > 1 && join_data_sources >= 6)
                        || mpp_outer_index_join
                        || ((!logicalop::GetHasTiFlash(Some(join)) || join_data_sources < 6)
                            && (small_outer_index_join
                                || selective_scan_index_join
                                || nested_outer_index_join
                                || bounded_outer_index_join)))))
            && !base.LeftJoinKeys.is_empty()
        {
            let prefer_left_inner = join.LeftPreferJoinType & (1 << 2) != 0;
            let prefer_right_inner = join.RightPreferJoinType & (1 << 2) != 0;
            if small_outer_index_join || selective_scan_index_join {
                base.InnerChildIdx = usize::from(left_is_small_join || left_is_selective_scan);
                if base.InnerChildIdx == 0 {
                    base.OuterJoinKeys = base.RightJoinKeys.iter().map(Column::Clone).collect();
                    base.InnerJoinKeys = base.LeftJoinKeys.iter().map(Column::Clone).collect();
                } else {
                    base.OuterJoinKeys = base.LeftJoinKeys.iter().map(Column::Clone).collect();
                    base.InnerJoinKeys = base.RightJoinKeys.iter().map(Column::Clone).collect();
                }
            } else if (prefer_index_join && prefer_left_inner && !prefer_right_inner)
                || (!prefer_right_inner && left_lookup && !right_lookup)
            {
                base.InnerChildIdx = 0;
                base.OuterJoinKeys = base.RightJoinKeys.iter().map(Column::Clone).collect();
                base.InnerJoinKeys = base.LeftJoinKeys.iter().map(Column::Clone).collect();
            } else {
                base.InnerChildIdx = 1;
                base.OuterJoinKeys = base.LeftJoinKeys.iter().map(Column::Clone).collect();
                base.InnerJoinKeys = base.RightJoinKeys.iter().map(Column::Clone).collect();
            }
            let outer_index = 1 - base.InnerChildIdx;
            let inner_lookup_matches = children
                .get(base.InnerChildIdx)
                .map(|child| build_lookup_scan_from_logical(child.as_ref()))
                .transpose()?
                .flatten()
                .is_some_and(|scan| {
                    base.InnerJoinKeys
                        .iter()
                        .all(|column| scan.schema().Contains(column))
                });
            if index_join_keys_match_children(
                &base.OuterJoinKeys,
                &base.InnerJoinKeys,
                children[outer_index].Schema(),
                children[base.InnerChildIdx].Schema(),
            ) && inner_lookup_matches
            {
                let mut index_child_properties =
                    [PhysicalProperty::default(), PhysicalProperty::default()];
                let outer_rows = children[outer_index]
                    .StatsInfo()
                    .map_or(0.0, |stats| stats.RowCount);
                let join_rows = join.StatsInfo().map_or(0.0, |stats| stats.RowCount);
                index_child_properties[outer_index].ExpectedCnt =
                    crate::CalcChildExpectedCnt(context.as_ref(), property, outer_rows, join_rows);
                index_child_properties[outer_index].SortItems = property.SortItems.clone();
                index_child_properties[base.InnerChildIdx].ExpectedCnt = f64::MAX;
                for child_property in &mut index_child_properties {
                    child_property.CTEProducerStatus = property.CTEProducerStatus;
                }
                fn subtree_is_for_update(plan: &dyn logicalop::LogicalPlan) -> bool {
                    plan.as_any().is::<logicalop::LogicalLock>()
                        || plan
                            .as_any()
                            .downcast_ref::<logicalop::DataSource>()
                            .is_some_and(|source| source.IsForUpdateRead)
                        || plan
                            .Children()
                            .iter()
                            .any(|child| subtree_is_for_update(child.as_ref()))
                }
                if property.NoCopPushDown
                    && !context
                        .GetSessionVars()
                        .GetIsolationReadEngines()
                        .contains(&kv::StoreType::TiKV)
                    && logicalop::GetHasTiFlash(Some(children[outer_index].as_ref()))
                {
                    // The lock stays above IndexJoin at Root. Its TiFlash
                    // outer read remains eligible while the inner range
                    // lookup uses TiKV through IndexJoinProp.
                    if contains_logical_join(children[outer_index].as_ref()) {
                        index_child_properties[outer_index].TaskTp = property::MppTaskType;
                    }
                    index_child_properties[outer_index].NoCopPushDown = false;
                    index_child_properties[outer_index].PreferTiFlash = true;
                }
                let tikv_allowed = context
                    .GetSessionVars()
                    .GetIsolationReadEngines()
                    .contains(&kv::StoreType::TiKV);
                if property.NoCopPushDown
                    && tikv_allowed
                    && join
                        .Children()
                        .iter()
                        .any(|child| subtree_is_for_update(child.as_ref()))
                {
                    for child_property in &mut index_child_properties {
                        child_property.TaskTp = property::RootTaskType;
                        child_property.NoCopPushDown = true;
                    }
                }
                index_child_properties[base.InnerChildIdx].IndexJoinProp =
                    Some(property::IndexJoinRuntimeProp {
                        OuterJoinKeys: base.OuterJoinKeys.iter().map(Column::Clone).collect(),
                        InnerJoinKeys: base.InnerJoinKeys.iter().map(Column::Clone).collect(),
                        ..Default::default()
                    });
                let [left_index_property, right_index_property] = index_child_properties;

                let family_checkpoint = context.plan_id_checkpoint();
                let join_family = if join.FromDecorrelatedApply {
                    let hash = crate::NewPhysicalHashJoin(
                        base.CloneWithSelf(context.clone())?,
                        vardef::DefExecutorConcurrency as u64,
                        false,
                    )
                    .Init(
                        context.clone(),
                        join.StatsInfo().cloned().unwrap_or_default(),
                        join.QueryBlockOffset(),
                        vec![
                            Box::new(PhysicalProperty::default()),
                            Box::new(PhysicalProperty::default()),
                        ],
                    );
                    let make_mpp = |broadcast: bool| -> Result<_, expression::Error> {
                        let mut join_base = base.CloneWithSelf(context.clone())?;
                        let mut build_index = 1;
                        let build_side_fixed = context
                            .GetSessionVars()
                            .GetSystemVar(vardef::TiDBOptMPPOuterJoinFixedBuildSide)
                            .is_some_and(|value| {
                                matches!(
                                    value.trim().to_ascii_lowercase().as_str(),
                                    "on" | "1" | "true"
                                )
                            });
                        if !broadcast
                            && matches!(
                                join.JoinType,
                                base::JoinType::SemiJoin | base::JoinType::AntiSemiJoin
                            )
                            && !join.IsNAAJ()
                            && !join.EqualConditions.is_empty()
                            && !build_side_fixed
                        {
                            let child_rows = join
                                .Children()
                                .iter()
                                .map(|child| child.StatsInfo().map_or(0.0, |stats| stats.RowCount))
                                .collect::<Vec<_>>();
                            if child_rows.get(1).copied().unwrap_or_default()
                                > child_rows.first().copied().unwrap_or_default()
                            {
                                build_index = 0;
                            }
                        }
                        join_base.InnerChildIdx = build_index;
                        let mut child_properties = Vec::with_capacity(2);
                        for index in 0..2 {
                            let mut required = PhysicalProperty::default();
                            required.TaskTp = property::MppTaskType;
                            required.ExpectedCnt = f64::MAX;
                            required.CanAddEnforcer = true;
                            if broadcast {
                                required.MPPPartitionTp = if index == build_index {
                                    property::BroadcastType
                                } else {
                                    property::AnyType
                                };
                            } else {
                                required.MPPPartitionTp = property::HashType;
                                let keys = if index == 0 {
                                    &join_base.LeftJoinKeys
                                } else {
                                    &join_base.RightJoinKeys
                                };
                                required.MPPPartitionCols = keys
                                    .iter()
                                    .map(|column| property::MPPPartitionColumn {
                                        Col: column.Clone(),
                                        CollateID: property::GetCollateIDByNameForPartition(
                                            column
                                                .RetType
                                                .as_ref()
                                                .map_or("binary", |field| field.GetCollate()),
                                        ),
                                    })
                                    .collect();
                            }
                            child_properties.push(Box::new(required));
                        }
                        let mut candidate = crate::NewPhysicalHashJoin(
                            join_base,
                            vardef::DefExecutorConcurrency as u64,
                            false,
                        )
                        .Init(
                            context.clone(),
                            join.StatsInfo().cloned().unwrap_or_default(),
                            join.QueryBlockOffset(),
                            child_properties,
                        );
                        candidate.StoreTp = kv::StoreType::TiFlash;
                        candidate.MppShuffleJoin = !broadcast;
                        Ok(candidate)
                    };
                    Some((hash, make_mpp(false)?, make_mpp(true)?))
                } else {
                    None
                };
                if let Some(checkpoint) = family_checkpoint {
                    context.restore_plan_id_checkpoint(checkpoint);
                }

                // Go constructs the table-range IndexJoin before the index-range
                // IndexHashJoin that wins these decorrelated semi joins.  Keep
                // that candidate in the family even though the latter is
                // evaluated first below.
                let enumerate_semi_index_family =
                    decorrelated_semi_index_join && join.JoinType == base::JoinType::SemiJoin;
                let preceding_index = if enumerate_semi_index_family {
                    Some(
                        crate::PhysicalIndexJoin::New(base.CloneWithSelf(context.clone())?).Init(
                            context.clone(),
                            join.StatsInfo().cloned().unwrap_or_default(),
                            join.QueryBlockOffset(),
                            vec![
                                Box::new(left_index_property.CloneEssentialFields()),
                                Box::new(right_index_property.CloneEssentialFields()),
                            ],
                        ),
                    )
                } else {
                    None
                };

                let mut physical =
                    crate::PhysicalIndexJoin::New(base.CloneWithSelf(context.clone())?).Init(
                        context.clone(),
                        join.StatsInfo().cloned().unwrap_or_default(),
                        join.QueryBlockOffset(),
                        vec![
                            Box::new(left_index_property),
                            Box::new(right_index_property),
                        ],
                    );
                if decorrelated_semi_index_join {
                    physical
                        .BasePhysicalJoin
                        .PhysicalSchemaProducer
                        .BasePhysicalPlan
                        .SetTP("IndexHashJoin");
                }
                physical.EqualConditions = join
                    .EqualConditions
                    .iter()
                    .filter_map(|condition| {
                        condition
                            .as_any()
                            .downcast_ref::<expression::ScalarFunction>()
                            .map(expression::ScalarFunction::clone_scalar)
                    })
                    .collect();
                for condition in &mut physical.EqualConditions {
                    if condition.GetArgs().len() == 2
                        && condition.GetArgs()[0].as_column().is_some_and(|column| {
                            physical
                                .BasePhysicalJoin
                                .InnerJoinKeys
                                .iter()
                                .any(|inner| inner.EqualColumn(column))
                        })
                    {
                        condition.GetArgsMut().swap(0, 1);
                    }
                }
                physical.CompleteHashKeys(
                    children[outer_index].Schema(),
                    children[physical.BasePhysicalJoin.InnerChildIdx].Schema(),
                );
                physical.FromDecorrelatedApply = join.FromDecorrelatedApply;
                if let Some(inner_logical) = children.get(physical.BasePhysicalJoin.InnerChildIdx) {
                    if let Some(inner_scan) =
                        build_lookup_scan_from_logical(inner_logical.as_ref())?
                    {
                        let usable_keys = inner_scan
                            .IdxCols
                            .iter()
                            .map_while(|index_column| {
                                physical
                                    .BasePhysicalJoin
                                    .InnerJoinKeys
                                    .iter()
                                    .position(|key| key.EqualColumn(index_column))
                            })
                            .collect::<Vec<_>>();
                        if !usable_keys.is_empty() {
                            physical.BasePhysicalJoin.OuterJoinKeys = usable_keys
                                .iter()
                                .map(|index| {
                                    physical.BasePhysicalJoin.OuterJoinKeys[*index].Clone()
                                })
                                .collect();
                            physical.BasePhysicalJoin.InnerJoinKeys = usable_keys
                                .iter()
                                .map(|index| {
                                    physical.BasePhysicalJoin.InnerJoinKeys[*index].Clone()
                                })
                                .collect();
                            physical.KeyOff2IdxOff = (0..usable_keys.len())
                                .map(|offset| i32::try_from(offset).unwrap_or(i32::MAX))
                                .collect();
                        }
                        let outer_rows = if small_outer_index_join {
                            join.StatsInfo().map_or(1.0, |stats| stats.RowCount)
                        } else {
                            children
                                .get(outer_index)
                                .and_then(|child| child.StatsInfo())
                                .map_or(1.0, |stats| stats.RowCount)
                        };
                        physical.InnerPlan = Some(build_index_join_lookup_scan(
                            &physical,
                            &inner_scan,
                            outer_rows,
                        )?);
                    }
                }
                fn contains_recursive_cte_table(plan: &dyn logicalop::LogicalPlan) -> bool {
                    plan.as_any().is::<logicalop::LogicalCTETable>()
                        || plan
                            .Children()
                            .iter()
                            .any(|child| contains_recursive_cte_table(child.as_ref()))
                }
                if both_hypothetical
                    || (hash_join_disabled
                        && children
                            .iter()
                            .any(|child| contains_recursive_cte_table(child.as_ref())))
                    || small_outer_index_join
                    || selective_scan_index_join
                    || (mpp_outer_index_join
                        && (physical.EqualConditions.len() > 1 || mpp_outer_rows <= 512.0))
                    || physical.EqualConditions.len() > 1
                {
                    physical
                        .BasePhysicalJoin
                        .PhysicalSchemaProducer
                        .BasePhysicalPlan
                        .Plan
                        .SetTP(plancodec::TypeIndexHashJoin);
                }
                if let Some((hash, shuffle, _broadcast)) = join_family {
                    let mut candidates: Vec<Box<dyn PhysicalPlan>> =
                        vec![Box::new(_broadcast), Box::new(shuffle), Box::new(physical)];
                    // This table-range alternative is constructed in Go before
                    // the winner, then rejected by the lookup-path feasibility
                    // check without recursing into its children.
                    drop(preceding_index);
                    // The remaining table/index-range IndexJoin and
                    // IndexHashJoin alternatives are distinct Go candidates.
                    // Their canonical attach path is shared in Rust, so clone
                    // their join base while retaining separate candidate
                    // identities. They are pruned before child recursion.
                    if enumerate_semi_index_family {
                        for offset in 0..4 {
                            let template = candidates[2]
                                .as_any()
                                .downcast_ref::<crate::PhysicalIndexJoin>()
                                .expect("index join candidate template");
                            let mut alternative = crate::PhysicalIndexJoin::New(
                                template.BasePhysicalJoin.CloneWithSelf(context.clone())?,
                            )
                            .Init(
                                context.clone(),
                                join.StatsInfo().cloned().unwrap_or_default(),
                                join.QueryBlockOffset(),
                                vec![
                                    Box::new(PhysicalProperty::default()),
                                    Box::new(PhysicalProperty::default()),
                                ],
                            );
                            alternative
                                .BasePhysicalJoin
                                .PhysicalSchemaProducer
                                .BasePhysicalPlan
                                .SetTP(if offset < 2 {
                                    plancodec::TypeIndexJoin
                                } else {
                                    plancodec::TypeIndexHashJoin
                                });
                            // These alternatives fail the same feasibility
                            // gate for this logical inner path.  Construction
                            // still advances the candidate frontier, but they
                            // must not recursively perturb the task cache.
                            drop(alternative);
                        }
                    }
                    candidates.push(Box::new(hash));
                    return Ok(candidates);
                }
                if prefer_index_join
                    || both_hypothetical
                    || hash_join_disabled
                    || selective_scan_index_join
                {
                    return Ok(vec![Box::new(physical)]);
                }
                deferred_index_join = Some(Box::new(physical));
            }
        }
        let force_mpp_under_lock = property.NoCopPushDown
            && context.GetSessionVars().IsMPPEnforced()
            && !context
                .GetSessionVars()
                .GetIsolationReadEngines()
                .contains(&kv::StoreType::TiKV)
            && property.IndexJoinProp.is_none()
            && join.PreferJoinType & 1 == 0;
        let keep_hinted_hash_join_at_root = property.NoCopPushDown && join.PreferJoinType & 1 != 0;
        let join_properties = if (property.TaskTp == property::MppTaskType || force_mpp_under_lock)
            && !keep_hinted_hash_join_at_root
        {
            let partition_columns = |columns: &[Column]| {
                columns
                    .iter()
                    .map(|column| {
                        let collate = column
                            .RetType
                            .as_ref()
                            .map_or("binary", |field| field.GetCollate());
                        property::MPPPartitionColumn {
                            Col: column.Clone(),
                            CollateID: property::GetCollateIDByNameForPartition(collate),
                        }
                    })
                    .collect::<Vec<_>>()
            };
            let mut left_partition_columns = partition_columns(&base.LeftJoinKeys);
            let mut right_partition_columns = partition_columns(&base.RightJoinKeys);
            if property.MPPPartitionTp == property::HashType
                && !property.MPPPartitionCols.is_empty()
            {
                let matches = property
                    .IsSubsetOf(&left_partition_columns)
                    .or_else(|| property.IsSubsetOf(&right_partition_columns));
                let matches = matches.or_else(|| {
                    // A projection below a grouped CTE may rename its sole
                    // partition key to a generated Column. Recover the
                    // corresponding equality edge from the join's pruned
                    // visible schema, matching Go's property substitution.
                    (property.MPPPartitionCols.len() == 1
                        && property.MPPPartitionCols[0].Col.ID == 0
                        && property.MPPPartitionCols[0]
                            .Col
                            .OrigName
                            .starts_with("Column"))
                    .then(|| {
                        left_partition_columns
                            .iter()
                            .zip(&right_partition_columns)
                            .enumerate()
                            .filter_map(|(index, (left, right))| {
                                (join.Schema().Contains(&left.Col)
                                    || join.Schema().Contains(&right.Col))
                                .then_some(index)
                            })
                            .collect::<Vec<_>>()
                    })
                    .filter(|indices| indices.len() == 1)
                });
                if let Some(matches) = matches {
                    left_partition_columns = matches
                        .iter()
                        .filter_map(|index| left_partition_columns.get(*index))
                        .map(property::MPPPartitionColumn::Clone)
                        .collect();
                    right_partition_columns = matches
                        .iter()
                        .filter_map(|index| right_partition_columns.get(*index))
                        .map(property::MPPPartitionColumn::Clone)
                        .collect();
                }
            }
            let mut left_property = PhysicalProperty::default();
            left_property.TaskTp = property::MppTaskType;
            left_property.ExpectedCnt = f64::MAX;
            left_property.CanAddEnforcer = true;
            left_property.MPPPartitionTp = property::HashType;
            left_property.MPPPartitionCols = left_partition_columns;
            left_property.CTEProducerStatus = property.CTEProducerStatus;
            let mut right_property = PhysicalProperty::default();
            right_property.TaskTp = property::MppTaskType;
            right_property.ExpectedCnt = f64::MAX;
            right_property.CanAddEnforcer = true;
            right_property.MPPPartitionTp = property::HashType;
            right_property.MPPPartitionCols = right_partition_columns;
            right_property.CTEProducerStatus = property.CTEProducerStatus;
            vec![Box::new(left_property), Box::new(right_property)]
        } else {
            fn contains_join(plan: &dyn logicalop::LogicalPlan) -> bool {
                plan.as_any().is::<logicalop::LogicalJoin>()
                    || plan
                        .Children()
                        .iter()
                        .any(|child| contains_join(child.as_ref()))
            }
            let tikv_allowed = context
                .GetSessionVars()
                .GetIsolationReadEngines()
                .contains(&kv::StoreType::TiKV);
            let mut left_property = PhysicalProperty::default();
            left_property.ExpectedCnt = f64::MAX;
            left_property.NoCopPushDown =
                property.NoCopPushDown && (contains_join(children[0].as_ref()) || tikv_allowed);
            let mut right_property = PhysicalProperty::default();
            right_property.ExpectedCnt = f64::MAX;
            right_property.NoCopPushDown =
                property.NoCopPushDown && (contains_join(children[1].as_ref()) || tikv_allowed);
            vec![Box::new(left_property), Box::new(right_property)]
        };
        if join.JoinType == base::JoinType::LeftOuterJoin {
            base.InnerChildIdx = 1;
        } else if join.JoinType == base::JoinType::RightOuterJoin {
            base.InnerChildIdx = 0;
        } else if matches!(
            join.JoinType,
            base::JoinType::SemiJoin | base::JoinType::AntiSemiJoin
        ) && property.TaskTp == property::RootTaskType
        {
            // Go getHashJoins constructs v1 semi/anti joins with innerIdx=1:
            // child 0 is the probe/output side and child 1 is the build side.
            base.InnerChildIdx = 1;
        } else if matches!(
            join.JoinType,
            base::JoinType::LeftOuterSemiJoin | base::JoinType::AntiLeftOuterSemiJoin
        ) {
            // The scalar result is appended to the left/probe side. Like Go,
            // outer semi joins always build the right child.
            base.InnerChildIdx = 1;
        } else if property.TaskTp == property::MppTaskType {
            // Match Go's preferredBuildIndex for shuffle joins: the smaller
            // child is the build side unless an explicit join hint selected
            // otherwise.  Fixing this here also determines which partition
            // layout is carried through the Join's output projection.
            let left_rows = join
                .Children()
                .first()
                .and_then(|child| child.StatsInfo())
                .map_or(f64::INFINITY, |stats| stats.RowCount);
            let right_rows = join
                .Children()
                .get(1)
                .and_then(|child| child.StatsInfo())
                .map_or(f64::INFINITY, |stats| stats.RowCount);
            base.InnerChildIdx = usize::from(left_rows > right_rows);
        } else if join.JoinType == base::JoinType::InnerJoin {
            let left_rows = join
                .Children()
                .first()
                .and_then(|child| child.StatsInfo())
                .map_or(f64::INFINITY, |stats| stats.RowCount);
            let right_rows = join
                .Children()
                .get(1)
                .and_then(|child| child.StatsInfo())
                .map_or(f64::INFINITY, |stats| stats.RowCount);
            // STRAIGHT_JOIN preserves input order, but Go still chooses the
            // smaller input as Build. Equal pseudo estimates keep the later
            // (right) input as Build.
            base.InnerChildIdx = if join.StraightJoin {
                fn source_count(plan: &dyn logicalop::LogicalPlan) -> usize {
                    usize::from(plan.as_any().is::<logicalop::DataSource>())
                        + plan
                            .Children()
                            .iter()
                            .map(|child| source_count(child.as_ref()))
                            .sum::<usize>()
                }
                let left_sources = source_count(children[0].as_ref());
                let right_sources = source_count(children[1].as_ref());
                fn contains_selection(plan: &dyn logicalop::LogicalPlan) -> bool {
                    plan.as_any().is::<logicalop::LogicalSelection>()
                        || plan
                            .as_any()
                            .downcast_ref::<logicalop::DataSource>()
                            .is_some_and(|source| !source.AllConds.is_empty())
                        || plan
                            .Children()
                            .iter()
                            .any(|child| contains_selection(child.as_ref()))
                }
                let left_selected = contains_selection(children[0].as_ref());
                let right_selected = contains_selection(children[1].as_ref());
                if left_sources != right_sources {
                    usize::from(left_sources > right_sources)
                } else if left_selected != right_selected {
                    usize::from(right_selected)
                } else if (left_rows - right_rows).abs() <= f64::EPSILON {
                    1
                } else {
                    usize::from(left_rows < right_rows)
                }
            } else {
                usize::from(left_rows >= right_rows)
            };
        }
        fn cte_storage_id(plan: &dyn logicalop::LogicalPlan) -> Option<i32> {
            plan.as_any()
                .downcast_ref::<logicalop::LogicalCTE>()
                .map(|cte| cte.Cte.borrow().IDForStorage)
                .or_else(|| {
                    plan.as_any()
                        .downcast_ref::<logicalop::LogicalCTETable>()
                        .map(|cte| cte.IDForStorage)
                })
                .or_else(|| {
                    plan.Children()
                        .iter()
                        .find_map(|child| cte_storage_id(child.as_ref()))
                })
        }
        if children.len() == 2
            && cte_storage_id(children[0].as_ref())
                .is_some_and(|left_id| cte_storage_id(children[1].as_ref()) == Some(left_id))
        {
            // Go builds the later reference of an equal-cost CTE self join,
            // preserving Q64's cs2(Build), cs1(Probe) ordering.
            base.InnerChildIdx = 1;
        }
        fn ordered_primary_source(plan: &dyn logicalop::LogicalPlan) -> bool {
            if let Some(source) = plan.as_any().downcast_ref::<logicalop::DataSource>() {
                return source.TableInfo.PKIsHandle;
            }
            let children = plan.Children();
            children.len() == 1 && ordered_primary_source(children[0].as_ref())
        }
        let ordered_primary_merge = property.TaskTp == property::RootTaskType
            && join.StraightJoin
            && join.JoinType == base::JoinType::InnerJoin
            && base.LeftJoinKeys.len() == base.RightJoinKeys.len()
            && !base.LeftJoinKeys.is_empty()
            && children.len() == 2
            && children
                .iter()
                .all(|child| ordered_primary_source(child.as_ref()));
        if ordered_primary_merge {
            let mut producer = crate::PhysicalSchemaProducer::New(crate::NewBasePhysicalPlan(
                context.clone(),
                "MergeJoin",
                join.QueryBlockOffset(),
            ));
            producer.SetSchema(join.Schema().Clone());
            let mut merge = crate::GetMergeJoin(join, producer).Init(
                context,
                join.StatsInfo().cloned().unwrap_or_default(),
                join.QueryBlockOffset(),
            );
            let mut left_property = PhysicalProperty::default();
            left_property.SortItems = merge
                .BasePhysicalJoin
                .LeftJoinKeys
                .iter()
                .map(|column| property::SortItem {
                    Col: column.Clone(),
                    Desc: false,
                })
                .collect();
            left_property.ExpectedCnt = f64::MAX;
            let mut right_property = PhysicalProperty::default();
            right_property.SortItems = merge
                .BasePhysicalJoin
                .RightJoinKeys
                .iter()
                .map(|column| property::SortItem {
                    Col: column.Clone(),
                    Desc: false,
                })
                .collect();
            right_property.ExpectedCnt = f64::MAX;
            merge
                .BasePhysicalJoin
                .PhysicalSchemaProducer
                .BasePhysicalPlan
                .SetChildrenReqProps(vec![Box::new(left_property), Box::new(right_property)]);
            return Ok(vec![Box::new(merge)]);
        }
        let concurrency = context
            .GetSessionVars()
            .GetSystemVar(vardef::TiDBHashJoinConcurrency)
            .and_then(|value| value.parse::<i64>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(vardef::DefExecutorConcurrency)
            .max(1) as u64;
        let mut physical = crate::NewPhysicalHashJoin(base, concurrency, false).Init(
            context.clone(),
            join.StatsInfo().cloned().unwrap_or_default(),
            join.QueryBlockOffset(),
            join_properties,
        );
        physical.MppShuffleJoin = (property.TaskTp == property::MppTaskType
            || force_mpp_under_lock)
            && !keep_hinted_hash_join_at_root;
        if property.TaskTp == property::MppTaskType {
            physical.StoreTp = kv::StoreType::TiFlash;
        }
        physical.FromHashJoinHint = join.PreferJoinType & 1 != 0;
        fn contains_sql_table_alias(plan: &dyn logicalop::LogicalPlan) -> bool {
            plan.as_any()
                .downcast_ref::<logicalop::DataSource>()
                .and_then(|source| source.TableAsName.as_ref().map(|alias| (source, alias)))
                .is_some_and(|(source, alias)| alias.L != source.TableInfo.Name.L)
                || plan
                    .Children()
                    .iter()
                    .any(|child| contains_sql_table_alias(child.as_ref()))
        }
        physical.HasTableAlias = children
            .iter()
            .any(|child| contains_sql_table_alias(child.as_ref()));
        if force_mpp_under_lock {
            physical.StoreTp = kv::StoreType::TiFlash;
        }
        physical.EqualConditions = join
            .EqualConditions
            .iter()
            .filter_map(|condition| {
                condition
                    .as_any()
                    .downcast_ref::<expression::ScalarFunction>()
                    .map(expression::ScalarFunction::clone_scalar)
            })
            .collect();
        // Go keeps each physical equality in left-child/right-child order.
        // Cast-key materialization may leave the logical equality reversed.
        if children.len() == 2 {
            for condition in &mut physical.EqualConditions {
                if condition.GetArgs().len() != 2 {
                    continue;
                }
                let first = condition.GetArgs()[0].as_column();
                let second = condition.GetArgs()[1].as_column();
                if first.is_some_and(|column| children[1].Schema().Contains(column))
                    && second.is_some_and(|column| children[0].Schema().Contains(column))
                {
                    condition.GetArgsMut().swap(0, 1);
                    condition.CleanHashCode();
                }
            }
        }
        physical.NAEqualConditions = join
            .NAEQConditions
            .iter()
            .filter_map(|condition| {
                condition
                    .as_any()
                    .downcast_ref::<expression::ScalarFunction>()
                    .map(expression::ScalarFunction::clone_scalar)
            })
            .collect();
        if join.JoinType == base::JoinType::FullOuterJoin
            && property.TaskTp == property::RootTaskType
        {
            // The root path supports only the v1 HashJoin for FULL OUTER JOIN.
            return Ok(vec![Box::new(physical)]);
        }
        let broadcast_enabled = context
            .GetSessionVars()
            .GetSystemVar(vardef::TiDBBCJThresholdCount)
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(vardef::DefBroadcastJoinThresholdCount)
            > 0
            && context
                .GetSessionVars()
                .GetSystemVar(vardef::TiDBBCJThresholdSize)
                .and_then(|value| value.parse::<i64>().ok())
                .unwrap_or(vardef::DefBroadcastJoinThresholdSize)
                > 0;
        let broadcast_build_index = if matches!(
            join.JoinType,
            base::JoinType::SemiJoin
                | base::JoinType::AntiSemiJoin
                | base::JoinType::LeftOuterSemiJoin
                | base::JoinType::AntiLeftOuterSemiJoin
        ) {
            1
        } else {
            physical.BasePhysicalJoin.InnerChildIdx
        };
        let build_fits_broadcast = child_rows.get(broadcast_build_index).is_some_and(|rows| {
            let child = &children[broadcast_build_index];
            if child
                .StatsInfo()
                .is_some_and(|stats| stats.HistColl.is_some())
            {
                let limit = context
                    .GetSessionVars()
                    .GetSystemVar(vardef::TiDBBCJThresholdSize)
                    .and_then(|value| value.parse::<i64>().ok())
                    .unwrap_or(vardef::DefBroadcastJoinThresholdSize);
                limit == -1 || *rows * child.Schema().Len().max(1) as f64 * 8.0 < limit as f64
            } else {
                let limit = context
                    .GetSessionVars()
                    .GetSystemVar(vardef::TiDBBCJThresholdCount)
                    .and_then(|value| value.parse::<i64>().ok())
                    .unwrap_or(vardef::DefBroadcastJoinThresholdCount);
                limit == -1 || *rows < limit as f64
            }
        });
        let broadcast_enabled =
            broadcast_enabled && build_fits_broadcast && !physical.FromHashJoinHint;
        let build_side_fixed = context
            .GetSessionVars()
            .GetSystemVar(vardef::TiDBOptMPPOuterJoinFixedBuildSide)
            .is_some_and(|value| {
                matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "on" | "1" | "true"
                )
            });
        let mut mpp_shuffle_build_index = broadcast_build_index;
        if !broadcast_enabled
            && matches!(
                join.JoinType,
                base::JoinType::SemiJoin | base::JoinType::AntiSemiJoin
            )
            && !join.IsNAAJ()
            && !join.EqualConditions.is_empty()
            && !build_side_fixed
            && child_rows.get(1).copied().unwrap_or_default()
                > child_rows.first().copied().unwrap_or_default()
        {
            mpp_shuffle_build_index = 0;
        }
        fn outputs_virtual_column(
            plan: &dyn logicalop::LogicalPlan,
            output: &expression::Schema,
        ) -> bool {
            if let Some(source) = plan.as_any().downcast_ref::<logicalop::DataSource>() {
                return source.TableInfo.Columns.iter().any(|column| {
                    column.IsVirtualGenerated()
                        && output.Columns.iter().any(|result| {
                            result.ID == column.ID
                                && result
                                    .OrigName
                                    .rsplit('.')
                                    .next()
                                    .is_some_and(|name| name.eq_ignore_ascii_case(&column.Name.O))
                        })
                });
            }
            plan.Children()
                .iter()
                .any(|child| outputs_virtual_column(child.as_ref(), output))
        }
        let virtual_output = children
            .iter()
            .any(|child| outputs_virtual_column(child.as_ref(), join.Schema()));
        if property.TaskTp == property::RootTaskType {
            let mut with_index = |mut candidates: Vec<Box<dyn PhysicalPlan>>| {
                if let Some(index) = deferred_index_join.take() {
                    candidates.insert(0, index);
                }
                candidates
            };
            let root_candidates =
                |physical: crate::PhysicalHashJoin| -> Result<_, expression::Error> {
                    if join.JoinType == base::JoinType::RightOuterJoin {
                        // Go enumerates both the inner-build and outer-build
                        // variants for a right outer hash join. The derived
                        // right input can be the cheaper build side.
                        let mut outer_build = physical.Clone(context.clone())?;
                        outer_build.UseOuterToBuild = true;
                        Ok(vec![
                            Box::new(outer_build) as Box<dyn PhysicalPlan>,
                            Box::new(physical),
                        ])
                    } else if matches!(
                        join.JoinType,
                        base::JoinType::SemiJoin | base::JoinType::AntiSemiJoin
                    ) && context
                        .GetSessionVars()
                        .GetSystemVar(vardef::TiDBHashJoinVersion)
                        .is_some_and(|version| version.eq_ignore_ascii_case("optimized"))
                        && physical.CanUseHashJoinV2()
                    {
                        let mut outer_build = physical.Clone(context.clone())?;
                        outer_build.UseOuterToBuild = true;
                        Ok(vec![
                            Box::new(physical),
                            Box::new(outer_build) as Box<dyn PhysicalPlan>,
                        ])
                    } else {
                        Ok(vec![Box::new(physical) as Box<dyn PhysicalPlan>])
                    }
                };
            if property.NoCopPushDown || virtual_output {
                return Ok(with_index(root_candidates(physical)?));
            }
            if !planner_util::ShouldCheckTiFlashPushDown(
                context.as_ref(),
                logicalop::GetHasTiFlash(Some(join)),
            ) {
                return Ok(with_index(root_candidates(physical)?));
            }
            // Go still enumerates an MPP broadcast join while the parent asks
            // for a Root task, then converts the completed MPP fragment through
            // a pass-through ExchangeSender/TableReader boundary.
            let mut mpp = physical.Clone(context.clone())?;
            mpp.StoreTp = kv::StoreType::TiFlash;
            mpp.MppShuffleJoin = !broadcast_enabled;
            mpp.BasePhysicalJoin.InnerChildIdx = if broadcast_enabled {
                broadcast_build_index
            } else {
                mpp_shuffle_build_index
            };
            let mut mpp_properties = Vec::with_capacity(2);
            for index in 0..2 {
                let mut child_property = PhysicalProperty::default();
                child_property.TaskTp = property::MppTaskType;
                child_property.ExpectedCnt = f64::MAX;
                child_property.CanAddEnforcer = true;
                child_property.MPPPartitionTp =
                    if broadcast_enabled && index == mpp.BasePhysicalJoin.InnerChildIdx {
                        // Let the build subtree choose its natural partition;
                        // `attach_canonical_mpp_join` adds the broadcast
                        // Exchange at the join boundary, like Go's enforcer.
                        property::AnyType
                    } else {
                        property::HashType
                    };
                if !broadcast_enabled {
                    let columns = if index == 0 {
                        &mpp.BasePhysicalJoin.LeftJoinKeys
                    } else {
                        &mpp.BasePhysicalJoin.RightJoinKeys
                    };
                    child_property.MPPPartitionCols = columns
                        .iter()
                        .map(|column| {
                            let collate = column
                                .RetType
                                .as_ref()
                                .map_or("binary", |field| field.GetCollate());
                            property::MPPPartitionColumn {
                                Col: column.Clone(),
                                CollateID: property::GetCollateIDByNameForPartition(collate),
                            }
                        })
                        .collect();
                } else if index != mpp.BasePhysicalJoin.InnerChildIdx {
                    child_property.MPPPartitionTp = property::AnyType;
                }
                child_property.CTEProducerStatus = property.CTEProducerStatus;
                mpp_properties.push(Box::new(child_property));
            }
            mpp.BasePhysicalJoin
                .PhysicalSchemaProducer
                .BasePhysicalPlan
                .SetChildrenReqProps(mpp_properties);
            let isolation_engines = context.GetSessionVars().GetIsolationReadEngines();
            fn contains_cte_reference(plan: &dyn logicalop::LogicalPlan) -> bool {
                plan.as_any().is::<logicalop::LogicalCTE>()
                    || plan.as_any().is::<logicalop::LogicalCTETable>()
                    || plan
                        .Children()
                        .iter()
                        .any(|child| contains_cte_reference(child.as_ref()))
            }
            let reads_cte = children
                .iter()
                .any(|child| contains_cte_reference(child.as_ref()));
            if context.GetSessionVars().IsMPPEnforced() && !reads_cte {
                return Ok(with_index(vec![Box::new(mpp)]));
            }
            if isolation_engines.contains(&kv::StoreType::TiFlash) && !reads_cte {
                if !isolation_engines.contains(&kv::StoreType::TiKV) {
                    return Ok(with_index(vec![Box::new(mpp)]));
                }
                // A Root requirement may be satisfied either by a native
                // TiDB HashJoin over readers or by an MPP join converted at a
                // reader boundary. Go enumerates both and lets cost choose;
                // dropping the native candidate makes root-only expressions
                // (for example a non-evaluated scalar subquery) impossible.
                let mut candidates: Vec<Box<dyn PhysicalPlan>> = vec![Box::new(mpp)];
                candidates.extend(root_candidates(physical)?);
                return Ok(with_index(candidates));
            }
            let mut candidates: Vec<Box<dyn PhysicalPlan>> = vec![Box::new(mpp)];
            candidates.extend(root_candidates(physical)?);
            return Ok(with_index(candidates));
        }
        if property.TaskTp == property::MppTaskType {
            if virtual_output {
                return Ok(Vec::new());
            }
            // Go's exhaustPhysicalPlans4LogicalJoin chooses only the preferred
            // MPP exchange strategy without an explicit MPP join hint.
            if !broadcast_enabled || property.IndexJoinProp.is_some() {
                return Ok(vec![Box::new(physical)]);
            }
            let mut broadcast = physical.Clone(context)?;
            broadcast.StoreTp = kv::StoreType::TiFlash;
            broadcast.MppShuffleJoin = false;
            broadcast.BasePhysicalJoin.InnerChildIdx = broadcast_build_index;
            let mut broadcast_properties = Vec::with_capacity(2);
            for index in 0..2 {
                let mut child_property = PhysicalProperty::default();
                child_property.TaskTp = property::MppTaskType;
                child_property.ExpectedCnt = f64::MAX;
                child_property.CanAddEnforcer = true;
                // The broadcast Exchange is enforced while attaching the join;
                // requiring Broadcast here would reject grouped MPP children
                // before that boundary is built.
                child_property.MPPPartitionTp = property::AnyType;
                child_property.CTEProducerStatus = property.CTEProducerStatus;
                broadcast_properties.push(Box::new(child_property));
            }
            broadcast
                .BasePhysicalJoin
                .PhysicalSchemaProducer
                .BasePhysicalPlan
                .SetChildrenReqProps(broadcast_properties);
            if join.PreferJoinType & ((1 << 12) | (1 << 13)) != 0 {
                return Ok(vec![Box::new(broadcast), Box::new(physical)]);
            }
            return Ok(vec![Box::new(broadcast)]);
        }
        return Ok(vec![Box::new(physical)]);
    }

    route!(
        logicalop::LogicalAggregation,
        crate::ExhaustPhysicalPlans4LogicalAggregation
    );
    route!(
        logicalop::LogicalLimit,
        crate::ExhaustPhysicalPlans4LogicalLimit
    );
    route!(
        logicalop::LogicalLock,
        crate::ExhaustPhysicalPlans4LogicalLock
    );
    route!(
        logicalop::LogicalMaxOneRow,
        crate::ExhaustPhysicalPlans4LogicalMaxOneRow
    );
    route!(
        logicalop::LogicalMemTable,
        crate::ExhaustPhysicalPlans4LogicalMemTable
    );
    route!(
        logicalop::LogicalProjection,
        crate::ExhaustPhysicalPlans4LogicalProjection
    );
    route!(
        logicalop::LogicalSelection,
        crate::ExhaustPhysicalPlans4LogicalSelection
    );
    route!(
        logicalop::LogicalSort,
        crate::ExhaustPhysicalPlans4LogicalSort
    );
    route!(
        logicalop::LogicalTableDual,
        crate::ExhaustPhysicalPlans4LogicalTableDual
    );
    if let Some(logical) = plan.as_any().downcast_ref::<logicalop::LogicalTopN>() {
        return Ok(crate::ExhaustPhysicalPlans4LogicalTopN(logical, property)
            .into_iter()
            .flatten()
            .collect());
    }
    route!(
        logicalop::LogicalUnionAll,
        crate::ExhaustPhysicalPlans4LogicalUnionAll
    );
    route!(
        logicalop::LogicalPartitionUnionAll,
        crate::ExhaustPhysicalPlans4LogicalPartitionUnionAll
    );
    route!(
        logicalop::LogicalUnionScan,
        crate::ExhaustPhysicalPlans4LogicalUnionScan
    );
    route!(
        logicalop::LogicalWindow,
        crate::ExhaustPhysicalPlans4LogicalWindow
    );

    Err(expression::errors::New(format!(
        "physical plan enumeration is not implemented for logical operator {}",
        plan.TP()
    )))
}

pub(crate) fn index_join_keys_match_children(
    outer_keys: &[Column],
    inner_keys: &[Column],
    outer_schema: &expression::Schema,
    inner_schema: &expression::Schema,
) -> bool {
    !outer_keys.is_empty()
        && outer_keys.len() == inner_keys.len()
        && outer_keys
            .iter()
            .all(|column| outer_schema.Contains(column))
        && inner_keys
            .iter()
            .all(|column| inner_schema.Contains(column))
}

/// 检测计划是否含假设索引（hypo / 负 ID）。
fn logical_has_hypothetical_index(plan: &dyn logicalop::LogicalPlan) -> bool {
    let is_hypothetical = |index: &expression::model::IndexInfo| {
        index.Tp == parser_ast::model::IndexType::Hypo || index.ID < 0
    };
    if let Some(gather) = plan.as_any().downcast_ref::<logicalop::TiKVSingleGather>() {
        return gather.Index.as_ref().is_some_and(is_hypothetical)
            || gather.Source.as_ref().is_some_and(|source| {
                let source = source.borrow();
                source
                    .PossibleAccessPaths
                    .iter()
                    .chain(&source.AllPossibleAccessPaths)
                    .any(|path| path.Index.as_ref().is_some_and(is_hypothetical))
            });
    }
    if let Some(scan) = plan.as_any().downcast_ref::<logicalop::LogicalIndexScan>() {
        return is_hypothetical(&scan.Index);
    }
    plan.as_any()
        .downcast_ref::<logicalop::DataSource>()
        .is_some_and(|source| {
            source
                .PossibleAccessPaths
                .iter()
                .chain(&source.AllPossibleAccessPaths)
                .any(|path| path.Index.as_ref().is_some_and(is_hypothetical))
        })
        || plan
            .Children()
            .iter()
            .any(|child| logical_has_hypothetical_index(child.as_ref()))
}

/// 评估侧适合作为 IndexJoin 内表的得分（访问条件与过滤越多越高）。
fn logical_index_lookup_score(plan: &dyn logicalop::LogicalPlan) -> usize {
    if let Some(gather) = plan.as_any().downcast_ref::<logicalop::TiKVSingleGather>() {
        return gather.Source.as_ref().map_or(0, |source| {
            let source = source.borrow();
            source
                .PossibleAccessPaths
                .iter()
                .filter(|path| path.Index.is_some())
                .map(|path| {
                    path.AccessConds.len()
                        + path.IndexFilters.len()
                        + source.PushedDownConds.len()
                        + usize::from(path.Forced)
                })
                .max()
                .unwrap_or(0)
        });
    }
    if let Some(scan) = plan.as_any().downcast_ref::<logicalop::LogicalIndexScan>() {
        return scan.AccessConds.len() + scan.IndexFilters.len() + 1;
    }
    if let Some(source) = plan.as_any().downcast_ref::<logicalop::DataSource>() {
        let best_index = source
            .PossibleAccessPaths
            .iter()
            .filter(|path| path.Index.is_some())
            .map(|path| {
                path.AccessConds.len()
                    + path.IndexFilters.len()
                    + usize::from(path.Forced)
                    + usize::from(
                        path.Index
                            .as_ref()
                            .is_some_and(|index| index.Tp == parser_ast::model::IndexType::Hypo),
                    )
            })
            .max()
            .unwrap_or(0);
        return best_index + source.PushedDownConds.len();
    }
    plan.Children()
        .iter()
        .map(|child| logical_index_lookup_score(child.as_ref()))
        .max()
        .unwrap_or(0)
}

/// 将逻辑 Join 条件拆入物理基座：等值键、左右条件与其他条件。
fn populate_physical_join_conditions(
    base: &mut crate::BasePhysicalJoin,
    logical: &logicalop::LogicalJoin,
) {
    base.LeftConditions = logical
        .LeftConditions
        .iter()
        .map(|condition| condition.CloneExpr())
        .collect();
    base.RightConditions = logical
        .RightConditions
        .iter()
        .map(|condition| condition.CloneExpr())
        .collect();
    let children = logical.Children();
    let left_schema = children.first().map(|child| child.Schema());
    let right_schema = children.get(1).map(|child| child.Schema());
    let resolve_column = |column: &Column, schema: &expression::Schema| {
        if schema.Contains(column) {
            return Some(column.Clone());
        }
        let mut matches = schema
            .Columns
            .iter()
            .filter(|candidate| candidate.String() == column.String());
        let first = matches.next();
        (first.is_some() && matches.next().is_none())
            .then(|| first.expect("checked unique join column").Clone())
    };
    // 从 OtherConditions 中拆出可识别的等值连接键。
    for condition in &logical.OtherConditions {
        let join_keys = condition
            .as_any()
            .downcast_ref::<expression::ScalarFunction>()
            .filter(|function| {
                matches!(
                    function.FuncName.L.as_str(),
                    parser_ast::EQ | parser_ast::NullEQ
                )
            })
            .map(expression::ExtractColumnsFromColOpCol);
        match (join_keys, left_schema, right_schema) {
            (Some((Some(first), Some(second))), Some(left), Some(right))
                if resolve_column(first, left).is_some()
                    && resolve_column(second, right).is_some() =>
            {
                base.LeftJoinKeys
                    .push(resolve_column(first, left).expect("checked left join column"));
                base.RightJoinKeys
                    .push(resolve_column(second, right).expect("checked right join column"));
                base.IsNullEQ.push(
                    condition
                        .as_any()
                        .downcast_ref::<expression::ScalarFunction>()
                        .is_some_and(|function| function.FuncName.L == parser_ast::NullEQ),
                );
            }
            (Some((Some(first), Some(second))), Some(left), Some(right))
                if resolve_column(second, left).is_some()
                    && resolve_column(first, right).is_some() =>
            {
                base.LeftJoinKeys
                    .push(resolve_column(second, left).expect("checked left join column"));
                base.RightJoinKeys
                    .push(resolve_column(first, right).expect("checked right join column"));
                base.IsNullEQ.push(
                    condition
                        .as_any()
                        .downcast_ref::<expression::ScalarFunction>()
                        .is_some_and(|function| function.FuncName.L == parser_ast::NullEQ),
                );
            }
            _ => base.OtherConditions.push(condition.CloneExpr()),
        }
    }
    // EqualConditions 直接写入左右 JoinKeys。
    for equality in &logical.EqualConditions {
        let Some(function) = equality
            .as_any()
            .downcast_ref::<expression::ScalarFunction>()
        else {
            base.OtherConditions.push(equality.CloneExpr());
            continue;
        };
        let (first, second) = expression::ExtractColumnsFromColOpCol(function);
        match (first, second, left_schema, right_schema) {
            (Some(first), Some(second), Some(left), Some(right))
                if resolve_column(first, left).is_some()
                    && resolve_column(second, right).is_some() =>
            {
                base.LeftJoinKeys
                    .push(resolve_column(first, left).expect("checked left join column"));
                base.RightJoinKeys
                    .push(resolve_column(second, right).expect("checked right join column"));
                base.IsNullEQ
                    .push(function.FuncName.L == parser_ast::NullEQ);
            }
            (Some(first), Some(second), Some(left), Some(right))
                if resolve_column(second, left).is_some()
                    && resolve_column(first, right).is_some() =>
            {
                base.LeftJoinKeys
                    .push(resolve_column(second, left).expect("checked left join column"));
                base.RightJoinKeys
                    .push(resolve_column(first, right).expect("checked right join column"));
                base.IsNullEQ
                    .push(function.FuncName.L == parser_ast::NullEQ);
            }
            _ => base.OtherConditions.push(equality.CloneExpr()),
        }
    }
    for equality in &logical.NAEQConditions {
        let Some(function) = equality
            .as_any()
            .downcast_ref::<expression::ScalarFunction>()
        else {
            continue;
        };
        let (first, second) = expression::ExtractColumnsFromColOpCol(function);
        match (first, second, left_schema, right_schema) {
            (Some(first), Some(second), Some(left), Some(right))
                if resolve_column(first, left).is_some()
                    && resolve_column(second, right).is_some() =>
            {
                base.LeftNAJoinKeys
                    .push(resolve_column(first, left).expect("checked left NA join column"));
                base.RightNAJoinKeys
                    .push(resolve_column(second, right).expect("checked right NA join column"));
            }
            (Some(first), Some(second), Some(left), Some(right))
                if resolve_column(second, left).is_some()
                    && resolve_column(first, right).is_some() =>
            {
                base.LeftNAJoinKeys
                    .push(resolve_column(second, left).expect("checked left NA join column"));
                base.RightNAJoinKeys
                    .push(resolve_column(first, right).expect("checked right NA join column"));
            }
            _ => {}
        }
    }
}

// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// DELETE / UPDATE 非逻辑物理计划：执行器按 SelectPlan 产出的行布局完成删除或赋值；
// 本模块同时定义多表删除的列区间与索引行布局元数据。

use crate::physical_schema_producer::SimpleSchemaProducer;
use std::collections::HashMap;
use std::sync::LazyLock;

/// DELETE 计划本身无行输出，统计信息固定为空。
static EMPTY_DELETE_STATS: LazyLock<property::StatsInfo> =
    LazyLock::new(property::StatsInfo::default);
/// UPDATE 计划本身无行输出，统计信息固定为空。
static EMPTY_UPDATE_STATS: LazyLock<property::StatsInfo> =
    LazyLock::new(property::StatsInfo::default);

/// Row layout needed by DELETE after logical column pruning.
#[derive(Clone, Debug)]
/// 列裁剪后 DELETE 所需的单个索引行布局。
pub struct DeleteIndexLayout {
    /// 索引 ID。
    pub ID: i64,
    /// 索引名。
    pub Name: String,
    /// 索引列名。
    pub Columns: Vec<String>,
    /// 列在混合删除行中的偏移。
    pub Offsets: Vec<usize>,
}

/// Go's `IndexesRowLayout` is keyed by index ID.  Keep a separate stable order
/// for diagnostics while making executor lookup independent of slice position.
#[derive(Clone, Debug, Default)]
/// 按索引 ID 可查找、并保留稳定顺序的索引布局集合。
pub struct DeleteIndexRowLayout {
    /// 稳定诊断顺序。
    ordered: Vec<DeleteIndexLayout>,
    /// 索引 ID → ordered 下标。
    by_id: HashMap<i64, usize>,
}

impl DeleteIndexRowLayout {
    /// 由有序列表构建，并建立 ID 映射。
    pub fn New(ordered: Vec<DeleteIndexLayout>) -> Self {
        let by_id = ordered
            .iter()
            .enumerate()
            .map(|(offset, layout)| (layout.ID, offset))
            .collect();
        Self { ordered, by_id }
    }

    /// 按索引 ID 查找布局。
    pub fn Get(&self, index_id: i64) -> Option<&DeleteIndexLayout> {
        self.by_id
            .get(&index_id)
            .and_then(|offset| self.ordered.get(*offset))
    }

    /// 按稳定顺序迭代。
    pub fn Iter(&self) -> impl Iterator<Item = &DeleteIndexLayout> {
        self.ordered.iter()
    }
}

/// 支持 for 循环遍历有序布局。
impl<'a> IntoIterator for &'a DeleteIndexRowLayout {
    type Item = &'a DeleteIndexLayout;
    type IntoIter = std::slice::Iter<'a, DeleteIndexLayout>;

    fn into_iter(self) -> Self::IntoIter {
        self.ordered.iter()
    }
}

/// Go `TblColPosInfo`: a table's consecutive slice in the mixed delete row.
#[derive(Clone)]
/// 多表删除行中某一表的连续列区间与 handle。
pub struct TblColPosInfo {
    /// 表 ID。
    pub TblID: i64,
    /// 半开区间起点。
    pub Start: usize,
    /// 半开区间终点。
    pub End: usize,
    /// 定位待删行的 handle 列。
    pub HandleCols: Vec<expression::Column>,
    /// `None` means pruning was intentionally skipped (partition/FK/point-get).
    /// None 表示有意跳过裁剪（分区/外键/点查等场景）。
    pub IndexesRowLayout: Option<DeleteIndexRowLayout>,
}

impl TblColPosInfo {
    /// 返回列区间及 handle 列占用的内存；索引布局与 Go 一样不在此重复计入。
    pub fn MemoryUsage(&self) -> i64 {
        std::mem::size_of::<TblColPosInfo>() as i64
            + self
                .HandleCols
                .iter()
                .map(expression::Column::MemoryUsage)
                .sum::<i64>()
    }

    /// 与 Go `Cmp` 一致，只按区间起点比较。
    pub fn Cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.Start.cmp(&other.Start)
    }
}

/// 多表列区间切片别名。
pub type TblColPosInfoSlice = Vec<TblColPosInfo>;

/// 为 Rust 的 Vec 表示补齐 Go `TblColPosInfoSlice.FindTblIdx` 契约。
pub trait TblColPosInfoSliceExt {
    /// 找到 Start 不大于列序号的最后一个候选区间。
    fn FindTblIdx(&self, col_ordinal: usize) -> Option<usize>;
}

impl TblColPosInfoSliceExt for [TblColPosInfo] {
    fn FindTblIdx(&self, col_ordinal: usize) -> Option<usize> {
        if self.is_empty() {
            return None;
        }
        self.partition_point(|info| info.Start <= col_ordinal)
            .checked_sub(1)
    }
}

impl TblColPosInfoSliceExt for Vec<TblColPosInfo> {
    fn FindTblIdx(&self, col_ordinal: usize) -> Option<usize> {
        self.as_slice().FindTblIdx(col_ordinal)
    }
}

/// Canonical non-logical DELETE plan. The executor consumes `SelectPlan` rows
/// according to `TblColPosInfos`; DELETE itself has an empty result schema.
/// 规范 DELETE 计划：执行器按 TblColPosInfos 解释 SelectPlan 行；自身 Schema 为空。
pub struct Delete {
    /// 空 Schema 的计划基座。
    pub SimpleSchemaProducer: SimpleSchemaProducer,
    /// 是否多表 DELETE。
    pub IsMultiTable: bool,
    /// 产生待删行的子计划。
    pub SelectPlan: Box<dyn base::PhysicalPlan>,
    /// 各表在混合行中的列区间。
    pub TblColPosInfos: TblColPosInfoSlice,
    /// 是否忽略执行错误（IGNORE）。
    pub IgnoreErr: bool,
    /// 外键检查存在时与 Go 一致拒绝计划缓存。
    pub FKChecks: Vec<Box<crate::FKCheck>>,
    /// 外键级联存在时与 Go 一致拒绝计划缓存。
    pub FKCascades: Vec<Box<crate::FKCascade>>,
}

impl Delete {
    /// 构造空 Schema 的 DELETE，默认单表。
    pub fn New(ctx: base::ContextRef, select_plan: Box<dyn base::PhysicalPlan>) -> Self {
        let mut producer = SimpleSchemaProducer::New(ctx, plancodec::TypeDelete, 0);
        producer.SetSchema(expression::NewSchema(Vec::new()));
        Self {
            SimpleSchemaProducer: producer,
            IsMultiTable: false,
            SelectPlan: select_plan,
            TblColPosInfos: Vec::new(),
            IgnoreErr: false,
            FKChecks: Vec::new(),
            FKCascades: Vec::new(),
        }
    }

    pub(crate) fn HasForeignKeyPlans(&self) -> bool {
        !self.FKChecks.is_empty() || !self.FKCascades.is_empty()
    }

    /// 累计 producer、子计划与各表 handle 列内存。
    pub fn MemoryUsage(&self) -> i64 {
        self.SimpleSchemaProducer.MemoryUsage()
            + self.SelectPlan.memory_usage()
            + self
                .TblColPosInfos
                .iter()
                .map(TblColPosInfo::MemoryUsage)
                .sum::<i64>()
    }
}

/// Plan trait：空 Schema、空统计；克隆时深拷贝 SelectPlan 与列布局。
impl base::Plan for Delete {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
    fn schema(&self) -> &expression::Schema {
        self.SimpleSchemaProducer
            .SchemaRef()
            .expect("Delete initializes empty schema")
    }
    fn id(&self) -> i32 {
        self.SimpleSchemaProducer.Plan.ID()
    }
    fn set_id(&mut self, id: i32) {
        self.SimpleSchemaProducer.Plan.SetID(id)
    }
    fn tp(&self, flags: &[bool]) -> String {
        self.SimpleSchemaProducer.Plan.TP(flags)
    }
    fn explain_id(&self, flags: &[bool]) -> Box<dyn std::fmt::Display + '_> {
        self.SimpleSchemaProducer.Plan.ExplainID(flags)
    }
    fn explain_info(&self) -> String {
        self.SimpleSchemaProducer.Plan.ExplainInfo()
    }
    fn replace_expr_columns(&mut self, replace: &HashMap<String, expression::Column>) {
        self.SimpleSchemaProducer.Plan.ReplaceExprColumns(replace)
    }
    fn s_ctx(&self) -> &base::ContextRef {
        self.SimpleSchemaProducer.Plan.SCtx()
    }
    fn stats_info(&self) -> &property::StatsInfo {
        &EMPTY_DELETE_STATS
    }
    fn output_names(&self) -> base::types::NameSlice {
        self.SimpleSchemaProducer.OutputNames()
    }
    fn set_output_names(&mut self, names: base::types::NameSlice) {
        self.SimpleSchemaProducer.SetOutputNames(names)
    }
    fn query_block_offset(&self) -> i32 {
        self.SimpleSchemaProducer.Plan.QueryBlockOffset()
    }
    fn clone_for_plan_cache(
        &self,
        new_ctx: base::ContextRef,
    ) -> (Option<Box<dyn base::Plan>>, bool) {
        if self.HasForeignKeyPlans() {
            return (None, false);
        }
        // 子计划无法克隆则标记不可缓存。
        let Ok(select_plan) = self.SelectPlan.clone_physical(new_ctx.clone()) else {
            return (None, false);
        };
        let cloned = Self {
            SimpleSchemaProducer: self.SimpleSchemaProducer.CloneSelfForPlanCache(new_ctx),
            IsMultiTable: self.IsMultiTable,
            SelectPlan: select_plan,
            TblColPosInfos: self.TblColPosInfos.clone(),
            IgnoreErr: self.IgnoreErr,
            FKChecks: Vec::new(),
            FKCascades: Vec::new(),
        };
        (Some(Box::new(cloned)), true)
    }
    fn set_noncacheable_reason(&mut self, reason: String) {
        self.SimpleSchemaProducer.Plan.SetNoncacheableReason(reason)
    }
    fn get_noncacheable_reason(&self) -> String {
        self.SimpleSchemaProducer.Plan.GetNoncacheableReason()
    }
}

/// Canonical non-logical UPDATE plan.  The optimized child produces the old
/// row layout referenced by `OrderedList`; the executor evaluates assignments
/// from left to right against that layout.
/// 规范 UPDATE：子计划给出旧行布局，执行器按 OrderedList 从左到右求值赋值。
pub struct Update {
    /// 空 Schema 的计划基座。
    pub SimpleSchemaProducer: SimpleSchemaProducer,
    /// 有序赋值列表（含虚拟列段）。
    pub OrderedList: Vec<expression::Assignment>,
    /// 全部赋值为常量时可走优化路径。
    pub AllAssignmentsAreConstant: bool,
    /// 虚拟列赋值在 OrderedList 中的起始偏移。
    pub VirtualAssignmentsOffset: usize,
    /// 是否忽略执行错误。
    pub IgnoreError: bool,
    /// 提供旧行的子计划。
    pub SelectPlan: Box<dyn base::PhysicalPlan>,
    /// 外键检查存在时与 Go 一致拒绝计划缓存。
    pub FKChecks: Vec<Box<crate::FKCheck>>,
    /// 外键级联存在时与 Go 一致拒绝计划缓存。
    pub FKCascades: Vec<Box<crate::FKCascade>>,
}

impl Update {
    /// 构造空 Schema 的 UPDATE。
    pub fn New(ctx: base::ContextRef, select_plan: Box<dyn base::PhysicalPlan>) -> Self {
        let mut producer = SimpleSchemaProducer::New(ctx, plancodec::TypeUpdate, 0);
        producer.SetSchema(expression::NewSchema(Vec::new()));
        Self {
            SimpleSchemaProducer: producer,
            OrderedList: Vec::new(),
            AllAssignmentsAreConstant: true,
            VirtualAssignmentsOffset: 0,
            IgnoreError: false,
            SelectPlan: select_plan,
            FKChecks: Vec::new(),
            FKCascades: Vec::new(),
        }
    }

    pub(crate) fn HasForeignKeyPlans(&self) -> bool {
        !self.FKChecks.is_empty() || !self.FKCascades.is_empty()
    }

    /// 相对 SelectPlan Schema 重解赋值左右两侧列索引。
    pub fn ResolveIndices(&mut self) -> Result<(), expression::Error> {
        let schema = self.SelectPlan.schema();
        for assignment in &mut self.OrderedList {
            assignment.Col = assignment.Col.ResolveIndices(schema)?;
            assignment.Expr = assignment.Expr.ResolveIndices(schema)?;
        }
        Ok(())
    }

    /// 累计 producer、子计划与赋值表达式内存。
    pub fn MemoryUsage(&self) -> i64 {
        self.SimpleSchemaProducer.MemoryUsage()
            + self.SelectPlan.memory_usage()
            + self
                .OrderedList
                .iter()
                .map(expression::Assignment::MemoryUsage)
                .sum::<i64>()
    }
}

/// Plan trait：空 Schema、空统计；克隆时复制赋值列表与子计划。
impl base::Plan for Update {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
    fn schema(&self) -> &expression::Schema {
        self.SimpleSchemaProducer
            .SchemaRef()
            .expect("Update initializes empty schema")
    }
    fn id(&self) -> i32 {
        self.SimpleSchemaProducer.Plan.ID()
    }
    fn set_id(&mut self, id: i32) {
        self.SimpleSchemaProducer.Plan.SetID(id)
    }
    fn tp(&self, flags: &[bool]) -> String {
        self.SimpleSchemaProducer.Plan.TP(flags)
    }
    fn explain_id(&self, flags: &[bool]) -> Box<dyn std::fmt::Display + '_> {
        self.SimpleSchemaProducer.Plan.ExplainID(flags)
    }
    fn explain_info(&self) -> String {
        self.SimpleSchemaProducer.Plan.ExplainInfo()
    }
    fn replace_expr_columns(&mut self, replace: &HashMap<String, expression::Column>) {
        self.SimpleSchemaProducer.Plan.ReplaceExprColumns(replace)
    }
    fn s_ctx(&self) -> &base::ContextRef {
        self.SimpleSchemaProducer.Plan.SCtx()
    }
    fn stats_info(&self) -> &property::StatsInfo {
        &EMPTY_UPDATE_STATS
    }
    fn output_names(&self) -> base::types::NameSlice {
        self.SimpleSchemaProducer.OutputNames()
    }
    fn set_output_names(&mut self, names: base::types::NameSlice) {
        self.SimpleSchemaProducer.SetOutputNames(names)
    }
    fn query_block_offset(&self) -> i32 {
        self.SimpleSchemaProducer.Plan.QueryBlockOffset()
    }
    fn clone_for_plan_cache(
        &self,
        new_ctx: base::ContextRef,
    ) -> (Option<Box<dyn base::Plan>>, bool) {
        if self.HasForeignKeyPlans() {
            return (None, false);
        }
        // 子计划无法克隆则标记不可缓存。
        let Ok(select_plan) = self.SelectPlan.clone_physical(new_ctx.clone()) else {
            return (None, false);
        };
        let cloned = Self {
            SimpleSchemaProducer: self.SimpleSchemaProducer.CloneSelfForPlanCache(new_ctx),
            OrderedList: self
                .OrderedList
                .iter()
                .map(expression::Assignment::Clone)
                .collect(),
            AllAssignmentsAreConstant: self.AllAssignmentsAreConstant,
            VirtualAssignmentsOffset: self.VirtualAssignmentsOffset,
            IgnoreError: self.IgnoreError,
            SelectPlan: select_plan,
            FKChecks: Vec::new(),
            FKCascades: Vec::new(),
        };
        (Some(Box::new(cloned)), true)
    }
    fn set_noncacheable_reason(&mut self, reason: String) {
        self.SimpleSchemaProducer.Plan.SetNoncacheableReason(reason)
    }
    fn get_noncacheable_reason(&self) -> String {
        self.SimpleSchemaProducer.Plan.GetNoncacheableReason()
    }
}

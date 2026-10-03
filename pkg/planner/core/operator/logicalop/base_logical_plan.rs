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
// 逻辑计划基座：`LogicalPlan` trait、`BaseLogicalPlan` 与公共优化钩子。
//
// 提供谓词下推（Predicate Push Down）、列裁剪、统计推导、任务缓存与
// Coprocessor 下推判定等默认实现；具体算子按需覆写。

// 逻辑计划基类与公共优化契约。
//
// 定义 `LogicalPlan` trait（谓词下推、列裁剪、统计推导等）以及内嵌的
// `BaseLogicalPlan`（子节点、schema、任务缓存、函数依赖 FDSet 等）。
// 具体算子通常只覆盖改变自身表达式或基数的变换；树遍历逻辑集中在基类。

use crate::{Column, Expression, NameSlice, PlannerError, Result, Schema, StatsInfo};
use std::any::Any;
use std::collections::HashMap;
use std::hash::Hasher;

/// 标记 Apply 由 XF decorrelate 规则生成，供后续规则识别。
pub const APPLY_GEN_FROM_XF_DECORRELATE_RULE_FLAG: u64 = 1 << 0;

#[derive(Clone, Debug, Default, PartialEq)]
/// 物理任务缓存条目：代价与对应物理计划 ID。
pub struct TaskRef {
    /// 任务代价估计。
    pub cost: f64,
    /// 关联物理计划节点 ID。
    pub plan_id: i32,
}

/// 逻辑计划树节点的堆分配句柄。
pub type LogicalPlanRef = Box<dyn LogicalPlan>;

/// 对根节点执行谓词下推；若返回替换计划则原地替换 `plan`。
pub fn PredicatePushDownPlan(
    plan: &mut LogicalPlanRef,
    predicates: Vec<Expression>,
) -> Result<Vec<Expression>> {
    let (retained, replacement) = plan.PredicatePushDownRoot(predicates)?;
    if let Some(replacement) = replacement {
        *plan = replacement;
    }
    Ok(retained)
}

/// 将子节点未消化的残差谓词物化为 Selection，挂在该子节点之上（对应 Go `AddSelection`）。
/// Materializes predicates that a child could not consume immediately above
/// that child.  This is the Rust counterpart of Go's `AddSelection` boundary:
/// residual predicates must never disappear merely because the parent is a
/// semantic push-down barrier.
/// 将子节点无法立刻消费的谓词物化为上方 Selection（对应 Go 的 AddSelection）。
pub fn AttachSelectionToPlan(plan: &mut LogicalPlanRef, conditions: Vec<Expression>) -> Result<()> {
    if conditions.is_empty()
        || plan
            .as_any()
            .downcast_ref::<crate::LogicalTableDual>()
            // 空 TableDual 上再挂 Selection 无意义。
            .is_some_and(|dual| dual.RowCount == 0)
    {
        return Ok(());
    }
    let context = plan
        .SCtx()
        .cloned()
        .ok_or_else(|| PlannerError("logical child has no plan context".into()))?;
    let query_block = plan.QueryBlockOffset();
    let schema = plan.Schema().Clone();
    let names = plan.OutputNames().Shallow();
    let child = std::mem::replace(plan, Box::new(crate::LogicalTableDual::default()));
    let mut selection = crate::LogicalSelection {
        Conditions: conditions,
        ..crate::LogicalSelection::default()
    }
    .Init(context, query_block);
    selection.SetSchema(schema);
    selection.SetOutputNames(names);
    selection.SetChildren(vec![child]);
    *plan = Box::new(selection);
    Ok(())
}

/// 逻辑计划公共优化契约：具体算子仅覆盖改变自身表达式/基数的变换。
/// Common optimizer contract.  Concrete operators override only transformations
/// that change their own expressions or cardinality; tree traversal is shared.
/// 逻辑计划公共契约：具体算子只覆写改变自身表达式/基数的变换，遍历默认共享。
pub trait LogicalPlan: Any {
    /// 向下转型入口。
    fn as_any(&self) -> &dyn Any;
    /// 可变向下转型入口。
    fn as_any_mut(&mut self) -> &mut dyn Any;
    /// 访问内嵌基类。
    fn base(&self) -> &BaseLogicalPlan;
    /// 可变访问内嵌基类。
    fn base_mut(&mut self) -> &mut BaseLogicalPlan;

    /// 算子类型名（如 Aggregation、Join）。
    fn TP(&self) -> &str {
        &self.base().tp
    }
    /// 计划节点唯一 ID。
    fn ID(&self) -> i32 {
        self.base().id
    }
    /// 查询块偏移，用于多块 SQL 定位。
    fn QueryBlockOffset(&self) -> i32 {
        self.base().query_block_offset
    }
    /// 会话/规划上下文。
    fn SCtx(&self) -> Option<&base::ContextRef> {
        self.base().ctx.as_ref()
    }
    /// EXPLAIN 附加信息，默认空。
    fn ExplainInfo(&self) -> String {
        String::new()
    }
    /// 节点哈希码，默认基于 ID。
    fn HashCode(&self) -> Vec<u8> {
        self.base().HashCode()
    }
    /// 输出 schema。
    fn Schema(&self) -> &Schema {
        &self.base().schema
    }
    /// 可变输出 schema。
    fn Schema_mut(&mut self) -> &mut Schema {
        &mut self.base_mut().schema
    }
    /// 输出列名切片。
    fn OutputNames(&self) -> &NameSlice {
        &self.base().output_names
    }
    /// 设置输出列名。
    fn SetOutputNames(&mut self, names: NameSlice) {
        self.base_mut().output_names = names;
    }
    /// 基数等统计信息。
    fn StatsInfo(&self) -> Option<&StatsInfo> {
        self.base().stats.as_ref()
    }
    /// 写入统计信息。
    fn SetStats(&mut self, stats: StatsInfo) {
        self.base_mut().stats = Some(stats);
    }
    /// 设置输出 schema。
    fn SetSchema(&mut self, schema: Schema) {
        self.base_mut().schema = schema;
    }
    /// 子计划列表。
    fn Children(&self) -> &[LogicalPlanRef] {
        &self.base().children
    }
    /// 可变子计划切片。
    fn Children_mut(&mut self) -> &mut [LogicalPlanRef] {
        &mut self.base_mut().children
    }
    /// 替换全部子计划。
    fn SetChildren(&mut self, children: Vec<LogicalPlanRef>) {
        self.base_mut().children = children;
    }
    /// 取出并清空子计划。
    fn TakeChildren(&mut self) -> Vec<LogicalPlanRef> {
        self.base_mut().TakeChildren()
    }
    /// 谓词下推：返回仍需留在上层的过滤条件。
    fn PredicatePushDown(&mut self, predicates: Vec<Expression>) -> Result<Vec<Expression>> {
        self.base_mut().PredicatePushDown(predicates)
    }
    /// 根级谓词下推，可附带整棵子树替换。
    fn PredicatePushDownRoot(
        &mut self,
        predicates: Vec<Expression>,
    ) -> Result<(Vec<Expression>, Option<LogicalPlanRef>)> {
        Ok((self.PredicatePushDown(predicates)?, None))
    }
    /// 列裁剪：仅保留父节点仍引用的列。
    fn PruneColumns(&mut self, parent_used_cols: &[Column]) -> Result<()> {
        self.base_mut().PruneColumns(parent_used_cols)
    }
    /// 推导唯一键/主键信息与 MaxOneRow。
    fn BuildKeyInfo(&mut self) {
        self.base_mut().BuildKeyInfo();
    }
    /// 谓词化简，默认递归子节点。
    fn PredicateSimplification(&mut self) {
        for child in self.Children_mut() {
            child.PredicateSimplification();
        }
    }
    /// TopN/Limit 下推。
    fn PushDownTopN(&mut self, top_n: Option<LogicalPlanRef>) -> Option<LogicalPlanRef> {
        self.base_mut().PushDownTopN(top_n)
    }
    /// 推导统计信息；返回 (stats, 是否新计算)。
    fn DeriveStats(&mut self, reload: bool) -> Result<(StatsInfo, bool)> {
        self.base_mut().DeriveStats(reload)
    }
    /// 是否保证最多一行输出。
    fn MaxOneRow(&self) -> bool {
        self.base().max_one_row
    }
    /// 设置 MaxOneRow 标志。
    fn SetMaxOneRow(&mut self, value: bool) {
        self.base_mut().max_one_row = value;
    }
}

/// 所有逻辑算子共享的基类状态。
/// 所有逻辑算子共享的基座字段：上下文、ID、子树、Schema、统计与任务缓存等。
pub struct BaseLogicalPlan {
    ctx: Option<base::ContextRef>,
    tp: String,
    id: i32,
    query_block_offset: i32,
    task_map: HashMap<String, TaskRef>,
    task_map_bak: Vec<(u64, String, Option<TaskRef>)>,
    plan_ids_hash: u64,
    task_map_bak_ts: u64,
    children: Vec<LogicalPlanRef>,
    schema: Schema,
    output_names: NameSlice,
    stats: Option<StatsInfo>,
    max_one_row: bool,
    fd_set: Option<fd::FDSet>,
    has_ti_flash: bool,
    pub Flag: u64,
}

/// 空基类默认值。
impl Default for BaseLogicalPlan {
    fn default() -> Self {
        Self {
            ctx: None,
            tp: String::new(),
            id: 0,
            query_block_offset: 0,
            task_map: HashMap::new(),
            task_map_bak: Vec::new(),
            plan_ids_hash: 0,
            task_map_bak_ts: 0,
            children: Vec::new(),
            schema: expression::NewSchema(Vec::new()),
            output_names: NameSlice(Vec::new()),
            stats: None,
            max_one_row: false,
            fd_set: None,
            has_ti_flash: false,
            Flag: 0,
        }
    }
}

/// 分配新计划 ID 并构造带上下文的基类逻辑计划。
/// 分配计划 ID 并构造带上下文的 `BaseLogicalPlan`。
pub fn NewBaseLogicalPlan(
    ctx: base::ContextRef,
    tp: impl Into<String>,
    query_block_offset: i32,
) -> BaseLogicalPlan {
    let id = ctx.alloc_plan_id();
    BaseLogicalPlan {
        ctx: Some(ctx),
        tp: tp.into(),
        id,
        query_block_offset,
        ..BaseLogicalPlan::default()
    }
}

/// BaseLogicalPlan 上的默认优化与访问器实现。
impl BaseLogicalPlan {
    /// 输出 schema。
    pub fn Schema(&self) -> &Schema {
        &self.schema
    }

    /// 写入节点 ID 的 64 位哈希。
    pub fn Hash64(&self, hasher: &mut dyn Hasher) {
        hasher.write_i32(self.id);
    }

    /// 按 ID 与类型名判定相等。
    pub fn Equals(&self, other: &BaseLogicalPlan) -> bool {
        self.id == other.id && self.tp == other.tp
    }

    /// 规划上下文。
    pub fn SCtx(&self) -> Option<&base::ContextRef> {
        self.ctx.as_ref()
    }

    /// 默认无额外 Explain 信息。
    pub fn ExplainInfo(&self) -> String {
        String::new()
    }

    /// 以大端 ID 字节作为哈希码。
    pub fn HashCode(&self) -> Vec<u8> {
        self.id.to_be_bytes().to_vec()
    }

    /// 默认：下推到唯一子节点，残差谓词挂 Selection。
    pub fn PredicatePushDown(&mut self, predicates: Vec<Expression>) -> Result<Vec<Expression>> {
        let Some(child) = self.children.first_mut() else {
            return Ok(predicates);
        };
        let residual = PredicatePushDownPlan(child, predicates)?;
        AttachSelectionToPlan(child, residual)?;
        Ok(Vec::new())
    }

    /// 取出子计划列表。
    pub fn TakeChildren(&mut self) -> Vec<LogicalPlanRef> {
        std::mem::take(&mut self.children)
    }

    /// 默认把父使用列传给第一个子节点。
    pub fn PruneColumns(&mut self, parent_used_cols: &[Column]) -> Result<()> {
        if let Some(child) = self.children.first_mut() {
            child.PruneColumns(parent_used_cols)?;
        }
        Ok(())
    }

    /// 单子节点时把子树挂到 TopN 下并返回 TopN。
    pub fn PushDownTopN(&mut self, mut top_n: Option<LogicalPlanRef>) -> Option<LogicalPlanRef> {
        if let Some(top_n) = top_n.as_mut()
            && self.children.len() == 1
        {
            let child = self.children.remove(0);
            top_n.SetChildren(vec![child]);
        }
        top_n
    }

    /// 递归触发子节点 DeriveTopN。
    pub fn DeriveTopN(&mut self) {
        for child in &mut self.children {
            child.base_mut().DeriveTopN();
        }
    }

    /// 递归谓词化简。
    pub fn PredicateSimplification(&mut self) {
        for child in &mut self.children {
            child.base_mut().PredicateSimplification();
        }
    }

    /// 常量传播占位，默认递归子节点。
    pub fn ConstantPropagation(&mut self, _predicates: &[Expression]) {
        for child in &mut self.children {
            child.base_mut().ConstantPropagation(&[]);
        }
    }

    /// 上拉常量谓词，默认无。
    pub fn PullUpConstantPredicates(&self) -> Vec<Expression> {
        Vec::new()
    }

    /// 递归子节点后按算子类型推导 MaxOneRow。
    pub fn BuildKeyInfo(&mut self) {
        for child in &mut self.children {
            child.BuildKeyInfo();
        }
        self.max_one_row = HasMaxOneRow(self.tp.as_str(), &self.children);
    }

    /// 先子后父推导统计。
    pub fn RecursiveDeriveStats(&mut self, reload: bool) -> Result<(StatsInfo, bool)> {
        for child in &mut self.children {
            child.DeriveStats(reload)?;
        }
        self.DeriveStats(reload)
    }

    /// 无子节点视为一行；单子节点继承；多子节点须由具体算子实现。
    pub fn DeriveStats(&mut self, reload: bool) -> Result<(StatsInfo, bool)> {
        if !reload && let Some(stats) = &self.stats {
            return Ok((stats.clone(), false));
        }
        let stats = match self.children.as_mut_slice() {
            // 叶子默认基数 1，各列 NDV=1。
            [] => {
                let mut stats = StatsInfo {
                    RowCount: 1.0,
                    ..StatsInfo::default()
                };
                for column in &self.schema.Columns {
                    stats.ColNDVs.insert(column.UniqueID, 1.0);
                }
                stats
            }
            // 单子节点：直接继承子统计。
            [child] => child.DeriveStats(reload)?.0,
            // 多子节点算子必须自行实现 DeriveStats。
            _ => {
                return Err(PlannerError(
                    "multi-child logical operator must implement DeriveStats".to_owned(),
                ));
            }
        };
        self.stats = Some(stats.clone());
        Ok((stats, true))
    }

    /// 提取列组，默认空。
    pub fn ExtractColGroups(&self, _groups: &[Vec<Column>]) -> Vec<Vec<Column>> {
        Vec::new()
    }

    /// 子树均可走 TiFlash 时置 has_ti_flash。
    pub fn PreparePossibleProperties(&mut self, children_have_tiflash: &[bool]) -> bool {
        self.has_ti_flash =
            !children_have_tiflash.is_empty() && children_have_tiflash.iter().all(|value| *value);
        self.has_ti_flash
    }

    /// 读取 has_ti_flash 缓存。
    pub fn PreparePossiblePropertiesValue(&self) -> bool {
        self.has_ti_flash
    }

    /// 提取关联列，默认无。
    pub fn ExtractCorrelatedCols(&self) -> Vec<expression::CorrelatedColumn> {
        Vec::new()
    }

    /// 自身与全部子节点是否可下推到指定存储的 Coprocessor。
    pub fn CanPushToCop(&self, store_type: StoreType) -> bool {
        CanPushToCopImpl(self.tp.as_str(), store_type, &self.children)
    }

    /// 合并子节点函数依赖（Functional Dependency）集合。
    pub fn ExtractFD(&mut self) -> &fd::FDSet {
        if self.fd_set.is_none() {
            let mut result = fd::FDSet::default();
            for child in &mut self.children {
                result.AddFrom(child.base_mut().ExtractFD());
            }
            self.fd_set = Some(result);
        }
        self.fd_set.as_ref().expect("initialized above")
    }

    /// 是否最多一行。
    pub fn MaxOneRow(&self) -> bool {
        self.max_one_row
    }

    /// 设置 MaxOneRow。
    pub fn SetMaxOneRow(&mut self, value: bool) {
        self.max_one_row = value;
    }

    /// 设置子计划。
    pub fn SetChildren(&mut self, children: Vec<LogicalPlanRef>) {
        self.children = children;
    }

    /// 替换指定下标子计划。
    pub fn SetChild(&mut self, index: usize, child: LogicalPlanRef) -> Result<()> {
        let Some(slot) = self.children.get_mut(index) else {
            return Err(PlannerError(format!("child index {index} out of bounds")));
        };
        *slot = child;
        Ok(())
    }

    /// 子节点个数。
    pub fn ChildLen(&self) -> usize {
        self.children.len()
    }

    /// 返回自身（兼容 Go Self 接口）。
    pub fn SelfPlan(&self) -> &BaseLogicalPlan {
        self
    }

    /// 仅更新计划 ID。
    pub fn SetSelf(&mut self, plan_id: i32) {
        self.id = plan_id;
    }

    /// 获取基类引用。
    pub fn GetBaseLogicalPlan(&self) -> &BaseLogicalPlan {
        self
    }

    /// 获取包装后的逻辑计划（此处即自身）。
    pub fn GetWrappedLogicalPlan(&self) -> &BaseLogicalPlan {
        self
    }

    /// 子计划只读切片。
    pub fn Children(&self) -> &[LogicalPlanRef] {
        &self.children
    }

    /// 子计划可变切片。
    pub fn Children_mut(&mut self) -> &mut [LogicalPlanRef] {
        &mut self.children
    }

    /// 设置 schema。
    pub fn SetSchema(&mut self, schema: Schema) {
        self.schema = schema;
    }

    /// 按属性键读取物理任务缓存。
    pub fn GetTask(&self, key: &str) -> Option<TaskRef> {
        self.task_map.get(key).cloned()
    }

    /// 写入任务缓存并记录回滚日志。
    pub fn StoreTask(&mut self, key: String, task: TaskRef) {
        let previous = self.task_map.insert(key.clone(), task);
        self.task_map_bak_ts = self.task_map_bak_ts.wrapping_add(1);
        self.task_map_bak
            .push((self.task_map_bak_ts, key, previous));
    }

    /// 回滚 timestamp 之后的任务缓存变更。
    pub fn RollBackTaskMap(&mut self, timestamp: u64) {
        while self
            .task_map_bak
            .last()
            .is_some_and(|(ts, _, _)| *ts > timestamp)
        {
            let (_, key, previous) = self.task_map_bak.pop().expect("checked above");
            match previous {
                Some(task) => {
                    self.task_map.insert(key, task);
                }
                None => {
                    self.task_map.remove(&key);
                }
            }
        }
    }

    /// 当前任务缓存备份时间戳。
    pub fn TaskMapBakTS(&self) -> u64 {
        self.task_map_bak_ts
    }

    /// 递增并返回逻辑时间戳，供探索性枚举做回滚点。
    pub fn GetLogicalTS4TaskMap(&mut self) -> u64 {
        self.task_map_bak_ts = self.task_map_bak_ts.wrapping_add(1);
        self.task_map_bak_ts
    }

    /// 设置函数依赖集合。
    pub fn SetFDs(&mut self, value: fd::FDSet) {
        self.fd_set = Some(value);
    }

    /// 读取函数依赖集合。
    pub fn FDs(&self) -> Option<&fd::FDSet> {
        self.fd_set.as_ref()
    }

    /// 设置子树计划 ID 哈希。
    pub fn SetPlanIDsHash(&mut self, value: u64) {
        self.plan_ids_hash = value;
    }

    /// 读取子树计划 ID 哈希。
    pub fn PlanIDsHash(&self) -> u64 {
        self.plan_ids_hash
    }

    /// 同 PlanIDsHash。
    pub fn GetPlanIDsHash(&self) -> u64 {
        self.plan_ids_hash
    }

    /// 取第一个子节点的统计与 schema。
    pub fn GetChildStatsAndSchema(&self) -> Option<(&StatsInfo, &Schema)> {
        let child = self.children.first()?;
        Some((child.base().stats.as_ref()?, child.Schema()))
    }

    /// 取 Join 左右子节点的统计与 schema。
    pub fn GetJoinChildStatsAndSchema(
        &self,
    ) -> Option<((&StatsInfo, &Schema), (&StatsInfo, &Schema))> {
        let [left, right] = self.children.as_slice() else {
            return None;
        };
        Some((
            (left.base().stats.as_ref()?, left.Schema()),
            (right.base().stats.as_ref()?, right.Schema()),
        ))
    }

    /// 测试 Flag 位。
    pub fn HasFlag(&self, mask: u64) -> bool {
        self.Flag & mask != 0
    }

    /// 置位 Flag。
    pub fn SetFlag(&mut self, mask: u64) {
        self.Flag |= mask;
    }

    /// Cascades 重分配：换类型、新 ID，并清空任务缓存与 FD。
    pub fn ReAlloc4Cascades(&mut self, tp: impl Into<String>) {
        self.tp = tp.into();
        if let Some(ctx) = &self.ctx {
            self.id = ctx.alloc_plan_id();
        }
        self.task_map.clear();
        self.task_map_bak.clear();
        self.task_map_bak_ts = 0;
        self.max_one_row = false;
        self.fd_set = None;
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Coprocessor 下推目标存储类型。
pub enum StoreType {
    TiKV,
    TiFlash,
}

/// 仅看算子自身类型是否允许下推到 TiKV/TiFlash Coprocessor。
pub fn CanSelfBeingPushedToCopImpl(tp: &str, store_type: StoreType) -> bool {
    match store_type {
        StoreType::TiKV => matches!(tp, "DataSource" | "TableScan" | "IndexScan" | "Selection"),
        StoreType::TiFlash => !matches!(tp, "Limit" | "TopN" | "UnionScan" | "Lock"),
    }
}

/// 自身与全部子节点均可下推时才返回 true。
/// 自身与全部子树均可下推时返回 true。
pub fn CanPushToCopImpl(tp: &str, store_type: StoreType, children: &[LogicalPlanRef]) -> bool {
    CanSelfBeingPushedToCopImpl(tp, store_type)
        && children
            .iter()
            .all(|child| child.base().CanPushToCop(store_type))
}

/// 按算子语义与子节点 MaxOneRow 推导本节点是否最多一行。
/// 按算子类型与子节点 MaxOneRow 推导本节点是否最多一行。
pub fn HasMaxOneRow(tp: &str, children: &[LogicalPlanRef]) -> bool {
    match tp {
        "MaxOneRow" => true,
        "Lock" | "Limit" | "Sort" | "Selection" | "Apply" | "Projection" | "Window"
        | "Aggregation" => children.first().is_some_and(|child| child.MaxOneRow()),
        "Join" => children.len() == 2 && children.iter().all(|child| child.MaxOneRow()),
        _ => false,
    }
}

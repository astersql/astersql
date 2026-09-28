// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0

// Cascades Memo 中的 GroupExpression（组表达式）。
//
// 一个 GroupExpression = 一个逻辑算子 + 若干子等价类（Group）输入。
// Group 强持有表达式，表达式强持有子 Group，回指父 Group 用弱引用，
// 以对齐 Go 指针图并避免循环泄漏。

use crate::{Group, GroupRef};
use cascades_base::Hasher as CascadesHasher;
use logicalop::{LogicalPlan, LogicalPlanRef};
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::hash::Hasher;
use std::rc::{Rc, Weak};

/// `GroupExpression` 的共享可变引用。
pub type GroupExpressionRef = Rc<RefCell<GroupExpression>>;

/// A logical operator plus its child equivalence groups.
///
/// Groups own expressions strongly, expressions own child groups strongly, and
/// the back edge to the owner is weak. This models Go's pointer graph without
/// leaking the owner/expression cycle or requiring raw pointers.
///
/// 逻辑算子加上其子等价类输入；所有权关系见上方英文说明。
pub struct GroupExpression {
    /// 包装的逻辑计划算子。
    pub LogicalPlan: LogicalPlanRef,
    /// 子 Group 输入列表（按算子输入顺序）。
    pub Inputs: Vec<GroupRef>,
    /// 所属 Group 的弱回指。
    pub(crate) group: Weak<RefCell<Group>>,
    /// 预计算的 64 位语义哈希（不可为 0）。
    pub(crate) hash64: u64,
    /// 已应用过的变换规则下标集合（exploration mask）。
    pub(crate) mask: BTreeSet<usize>,
    /// 是否已废弃（abandoned），不再参与后续枚举。
    pub(crate) abandoned: bool,
}

impl GroupExpression {
    /// 构造表达式并立即 `Init` 计算哈希。
    pub(crate) fn new(plan: LogicalPlanRef, inputs: Vec<GroupRef>) -> GroupExpressionRef {
        let result = Rc::new(RefCell::new(Self {
            LogicalPlan: plan,
            Inputs: inputs,
            group: Weak::new(),
            hash64: 0,
            mask: BTreeSet::new(),
            abandoned: false,
        }));
        result.borrow_mut().Init();
        result
    }

    /// 升级弱引用，返回所属 Group（若仍存活）。
    pub fn GetGroup(&self) -> Option<GroupRef> {
        self.group.upgrade()
    }

    /// 调试字符串：`GE:<算子类型>{GID:..., ...}`。
    pub fn String(&self) -> String {
        let children = self
            .Inputs
            .iter()
            .map(|group| format!("GID:{}", group.borrow().GetGroupID()))
            .collect::<Vec<_>>()
            .join(", ");
        format!("GE:{}{{{children}}}", self.LogicalPlan.TP())
    }

    /// 返回预计算哈希；若为 0 则断言失败（Init 保证非零）。
    pub fn GetHash64(&self) -> u64 {
        assert_ne!(self.hash64, 0, "hash64 should not be 0");
        self.hash64
    }

    /// 将算子语义哈希与各子 GroupID 写入 Hasher。
    pub fn Hash64(&self, hasher: &mut dyn CascadesHasher) {
        hasher.HashUint64(hash_logical_plan(self.LogicalPlan.as_ref()));
        for child in &self.Inputs {
            hasher.HashUint64(child.borrow().GetGroupID());
        }
    }

    /// 比较算子语义与子 Group 是否全部相等。
    pub fn Equals(&self, other: &GroupExpression) -> bool {
        self.Inputs.len() == other.Inputs.len()
            && equal_logical_plans(self.LogicalPlan.as_ref(), other.LogicalPlan.as_ref())
            && self
                .Inputs
                .iter()
                .zip(&other.Inputs)
                .all(|(left, right)| left.borrow().Equals(&right.borrow()))
    }

    /// 用 HashEqualer 计算并缓存 hash64；若结果为 0 则强制为 1。
    pub fn Init(&mut self) {
        let mut hasher = cascades_base::NewHashEqualer();
        self.Hash64(hasher.as_mut());
        self.hash64 = hasher.Sum64();
        // FNV's offset is nonzero, but keep the Go invariant even if a future
        // hasher implementation legitimately returns zero.
        // FNV 偏移量非零，但仍保持 Go 不变量：hash64 永不为 0。
        if self.hash64 == 0 {
            self.hash64 = 1;
        }
    }

    /// 指定下标的变换规则是否已探索。
    pub fn IsExplored(&self, index: usize) -> bool {
        self.mask.contains(&index)
    }

    /// 标记指定下标的变换规则已探索。
    pub fn SetExplored(&mut self, index: usize) {
        self.mask.insert(index);
    }

    /// 是否已废弃。
    pub fn IsAbandoned(&self) -> bool {
        self.abandoned
    }

    /// 标记为废弃。
    pub fn SetAbandoned(&mut self) {
        self.abandoned = true;
    }

    /// 返回包装的逻辑算子 trait 对象。
    pub fn GetWrappedLogicalPlan(&self) -> &dyn LogicalPlan {
        self.LogicalPlan.as_ref()
    }

    /// 子输入个数。
    pub fn InputsLen(&self) -> usize {
        self.Inputs.len()
    }

    /// 取第 index 个子 Group 的 schema（列结构）。
    pub fn GetInputSchema(&self, index: usize) -> logicalop::Schema {
        self.Inputs[index]
            .borrow()
            .GetLogicalProperty()
            .and_then(|property| property.Schema.as_deref().map(|schema| schema.Clone()))
            .expect("child group must have a schema")
    }

    /// 单孩子算子：取唯一子节点的统计信息与 schema。
    pub fn GetChildStatsAndSchema(
        &self,
    ) -> (Option<property::StatsInfo>, Option<logicalop::Schema>) {
        assert!(
            !self.LogicalPlan.as_any().is::<logicalop::LogicalJoin>()
                && !self.LogicalPlan.as_any().is::<logicalop::LogicalApply>(),
            "GetChildStatsAndSchema should not be called on join GE, Please use GetJoinChildStatsAndSchema."
        );
        assert!(
            self.Inputs.len() == 1,
            "single-child expression must have one input"
        );
        let child = self.Inputs[0].borrow();
        let property = child.GetLogicalProperty();
        match property {
            Some(property) => (
                property.Stats.as_deref().cloned(),
                property.Schema.as_deref().map(|schema| schema.Clone()),
            ),
            None => (None, None),
        }
    }

    /// Join 等双孩子算子：分别取左右子节点的统计信息与 schema。
    pub fn GetJoinChildStatsAndSchema(
        &self,
    ) -> [(Option<property::StatsInfo>, Option<logicalop::Schema>); 2] {
        assert!(
            self.LogicalPlan.as_any().is::<logicalop::LogicalJoin>()
                || self.LogicalPlan.as_any().is::<logicalop::LogicalApply>(),
            "GetJoinChildStatsAndSchema should not be called on non-join GE, Please use GetChildStatsAndSchema."
        );
        assert_eq!(self.Inputs.len(), 2, "join expression must have two inputs");
        std::array::from_fn(|index| {
            let child = self.Inputs[index].borrow();
            let property = child.GetLogicalProperty();
            match property {
                Some(property) => (
                    property.Stats.as_deref().cloned(),
                    property.Schema.as_deref().map(|schema| schema.Clone()),
                ),
                None => (None, None),
            }
        })
    }

    /// 若所属 Group 尚无逻辑属性，则从本表达式派生并写入。
    pub fn DeriveLogicalProp(this: &GroupExpressionRef) {
        let owner = this
            .borrow()
            .GetGroup()
            .expect("inserted group expression must have an owner");
        if owner.borrow().HasLogicalProperty() {
            return;
        }
        let (child_possible_props, child_has_tiflash) = {
            let expression = this.borrow();
            let mut child_possible_props = Vec::with_capacity(expression.Inputs.len());
            let mut child_has_tiflash = Vec::with_capacity(expression.Inputs.len());
            for child in &expression.Inputs {
                let child = child.borrow();
                let property = child
                    .GetLogicalProperty()
                    .expect("child group must have logical properties");
                child_possible_props.push(property.PossibleProps.clone());
                child_has_tiflash.push(property.HasTiFlash);
            }
            (child_possible_props, child_has_tiflash)
        };
        let mut expression = this.borrow_mut();
        let schema = expression.LogicalPlan.Schema().Clone();
        let stats = expression.LogicalPlan.StatsInfo().cloned().map(Box::new);
        // Memo::CopyIn detaches plan children after bottom-up derivation. ExtractFD
        // therefore returns the cached operator FD when present and initializes an
        // empty FD set for operators whose Go default is the empty set.
        let fd = expression.LogicalPlan.base_mut().ExtractFD().clone();
        let max_one_row = expression.LogicalPlan.MaxOneRow();
        let has_tiflash = if child_has_tiflash.is_empty() {
            expression
                .LogicalPlan
                .base()
                .PreparePossiblePropertiesValue()
        } else {
            expression
                .LogicalPlan
                .base_mut()
                .PreparePossibleProperties(&child_has_tiflash)
        };
        let logical_property = property::LogicalProperty {
            Schema: Some(Box::new(schema)),
            Stats: stats,
            FD: Some(Box::new(fd)),
            MaxOneRow: max_one_row,
            // Base logical operators inherit the first child's ordering.
            PossibleProps: child_possible_props.into_iter().next().unwrap_or_default(),
            HasTiFlash: has_tiflash,
        };
        owner.borrow_mut().SetLogicalProperty(logical_property);
    }

    /// 以 Rc 指针地址作为稳定键，用于父引用表。
    pub(crate) fn addr(this: &GroupExpressionRef) -> usize {
        Rc::as_ptr(this) as usize
    }

    /// 将 source 合并到等价的 target：合并探索 mask，并解除子 Group 父引用。
    pub(crate) fn mergeTo(source: &GroupExpressionRef, target: &GroupExpressionRef) {
        let explored = source.borrow().mask.clone();
        target.borrow_mut().mask.extend(explored);
        let children = source.borrow().Inputs.clone();
        for child in children {
            Group::removeParentGEs(&child, source);
        }
        let mut source = source.borrow_mut();
        source.Inputs.clear();
        source.group = Weak::new();
    }
}

/// Like Go's embedded `base.LogicalPlan`, the memo expression forwards the
/// canonical logical contract to its one wrapped operator.  Methods that a
/// concrete operator can override are forwarded explicitly so routing through
/// the memo never falls back to `BaseLogicalPlan` behavior.
///
/// 对齐 Go 嵌入式 `base.LogicalPlan`：将标准逻辑计划契约转发到包装算子，
/// 显式转发可被覆写的方法，避免经 Memo 路由时回落到 `BaseLogicalPlan` 默认实现。
impl LogicalPlan for GroupExpression {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn base(&self) -> &logicalop::BaseLogicalPlan {
        self.LogicalPlan.base()
    }

    fn base_mut(&mut self) -> &mut logicalop::BaseLogicalPlan {
        self.LogicalPlan.base_mut()
    }

    fn ExplainInfo(&self) -> String {
        self.LogicalPlan.ExplainInfo()
    }

    fn HashCode(&self) -> Vec<u8> {
        self.LogicalPlan.HashCode()
    }

    fn PredicatePushDown(
        &mut self,
        predicates: Vec<logicalop::Expression>,
    ) -> logicalop::Result<Vec<logicalop::Expression>> {
        self.LogicalPlan.PredicatePushDown(predicates)
    }

    fn PruneColumns(&mut self, parent_used_cols: &[logicalop::Column]) -> logicalop::Result<()> {
        self.LogicalPlan.PruneColumns(parent_used_cols)
    }

    fn BuildKeyInfo(&mut self) {
        self.LogicalPlan.BuildKeyInfo();
    }

    fn PushDownTopN(
        &mut self,
        top_n: Option<logicalop::LogicalPlanRef>,
    ) -> Option<logicalop::LogicalPlanRef> {
        self.LogicalPlan.PushDownTopN(top_n)
    }

    fn DeriveStats(&mut self, reload: bool) -> logicalop::Result<(logicalop::StatsInfo, bool)> {
        self.LogicalPlan.DeriveStats(reload)
    }
}

/// Stable semantic hash dispatcher for the formal logical operators currently
/// available to the cascades memo. Unsupported future operators still receive a
/// deterministic key from their public type, explain text, hash code and schema.
///
/// 对 Cascades Memo 当前支持的形式化逻辑算子做稳定语义哈希分发；
/// 未专门处理的算子仍用类型名、explain、HashCode 与 schema 生成确定性键。
pub fn hash_logical_plan(plan: &dyn LogicalPlan) -> u64 {
    let mut hasher = Fnv64::default();
    hasher.write(plan.TP().as_bytes());
    // 优先按具体算子类型调用其 Hash64；命中即返回。
    macro_rules! hash_as {
        ($type:ty) => {
            if let Some(value) = plan.as_any().downcast_ref::<$type>() {
                value.Hash64(&mut hasher);
                return hasher.finish();
            }
        };
    }
    hash_as!(logicalop::LogicalJoin);
    hash_as!(logicalop::LogicalAggregation);
    hash_as!(logicalop::LogicalApply);
    hash_as!(logicalop::LogicalExpand);
    hash_as!(logicalop::LogicalLimit);
    hash_as!(logicalop::LogicalMaxOneRow);
    hash_as!(logicalop::DataSource);
    hash_as!(logicalop::LogicalMemTable);
    hash_as!(logicalop::LogicalUnionAll);
    hash_as!(logicalop::LogicalPartitionUnionAll);
    hash_as!(logicalop::LogicalProjection);
    hash_as!(logicalop::LogicalSelection);
    hash_as!(logicalop::LogicalSequence);
    hash_as!(logicalop::LogicalShow);
    hash_as!(logicalop::LogicalShowDDLJobs);
    hash_as!(logicalop::LogicalSort);
    hash_as!(logicalop::LogicalTableDual);
    hash_as!(logicalop::LogicalTopN);
    hash_as!(logicalop::LogicalUnionScan);
    hash_as!(logicalop::LogicalLock);
    if let Some(value) = plan.as_any().downcast_ref::<logicalop::LogicalWindow>() {
        hasher.write_u64(value.Hash64());
        return hasher.finish();
    }
    hasher.write(&plan.HashCode());
    hasher.write(plan.ExplainInfo().as_bytes());
    // 回退路径：用 HashCode、ExplainInfo 与列 UniqueID/ID 拼确定性哈希。
    for column in &plan.Schema().Columns {
        hasher.write_i64(column.UniqueID);
        hasher.write_i64(column.ID);
    }
    hasher.finish()
}

/// 按算子类型分派 Equals；类型不同或未识别时回退到 HashCode/Explain/Schema 比较。
pub fn equal_logical_plans(left: &dyn LogicalPlan, right: &dyn LogicalPlan) -> bool {
    if left.TP() != right.TP() {
        return false;
    }
    macro_rules! equal_as {
        ($type:ty) => {
            if let Some(left) = left.as_any().downcast_ref::<$type>() {
                return right
                    .as_any()
                    .downcast_ref::<$type>()
                    .is_some_and(|right| left.Equals(right));
            }
        };
    }
    equal_as!(logicalop::LogicalJoin);
    equal_as!(logicalop::LogicalAggregation);
    equal_as!(logicalop::LogicalApply);
    equal_as!(logicalop::LogicalExpand);
    equal_as!(logicalop::LogicalLimit);
    equal_as!(logicalop::LogicalMaxOneRow);
    equal_as!(logicalop::DataSource);
    equal_as!(logicalop::LogicalMemTable);
    equal_as!(logicalop::LogicalUnionAll);
    equal_as!(logicalop::LogicalPartitionUnionAll);
    equal_as!(logicalop::LogicalProjection);
    equal_as!(logicalop::LogicalSelection);
    equal_as!(logicalop::LogicalSequence);
    equal_as!(logicalop::LogicalShow);
    equal_as!(logicalop::LogicalShowDDLJobs);
    equal_as!(logicalop::LogicalSort);
    equal_as!(logicalop::LogicalTableDual);
    equal_as!(logicalop::LogicalTopN);
    equal_as!(logicalop::LogicalUnionScan);
    equal_as!(logicalop::LogicalWindow);
    equal_as!(logicalop::LogicalLock);
    left.HashCode() == right.HashCode()
        && left.ExplainInfo() == right.ExplainInfo()
        && left.Schema().Equal(right.Schema())
}

/// FNV-1a 64 位哈希器，用于逻辑算子语义哈希的稳定基底。
struct Fnv64(u64);

impl Default for Fnv64 {
    fn default() -> Self {
        // FNV offset basis。
        Self(14_695_981_039_346_656_037)
    }
}

impl Hasher for Fnv64 {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        // FNV-1a：逐字节异或后乘以 FNV prime。
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(1_099_511_628_211);
        }
    }
}

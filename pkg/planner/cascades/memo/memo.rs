// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0

// Cascades Memo 图：Group / GroupExpression 的插入、去重、合并与计划枚举。
//
// Memo（记忆化搜索空间）把逻辑等价的表达式放进同一 Group，通过全局哈希去重；
// 发现两个 Group 等价时执行 `mergeGroup`。`IteratorLP` 自根 Group DFS 笛卡尔积
// 展开全部逻辑计划备选（PlanAlternative），供探索阶段遍历。

use crate::{Group, GroupExpression, GroupExpressionRef, GroupID, GroupIDGenerator, GroupRef};
use logicalop::{LogicalPlan, LogicalPlanRef};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

/// Safe Rust ownership implementation of the cascades memo graph.
/// Cascades Memo 图的安全 Rust 所有权实现：管理 Group、组表达式与根组。
pub struct Memo {
    /// 组 ID 生成器。
    groupIDGen: GroupIDGenerator,
    /// 当前根 Group（Init / CopyIn 后设置）。
    rootGroup: Option<GroupRef>,
    /// 全部存活 Group 列表。
    groups: Vec<GroupRef>,
    /// GroupID → Group 索引。
    groupID2Group: HashMap<GroupID, GroupRef>,
    /// 全局组表达式表，用于哈希去重。
    globalExpressions: Vec<GroupExpressionRef>,
}

impl Default for Memo {
    fn default() -> Self {
        Self::NewMemo(&[])
    }
}

impl Memo {
    /// 创建空 Memo；`capacities` 预留参数与 Go 对齐，当前未使用。
    pub fn NewMemo(_capacities: &[u64]) -> Self {
        Self {
            groupIDGen: GroupIDGenerator::default(),
            rootGroup: None,
            groups: Vec::new(),
            groupID2Group: HashMap::new(),
            globalExpressions: Vec::new(),
        }
    }

    /// 清空全部 Group / 表达式，并重置 ID 生成器。
    pub fn Destroy(&mut self) {
        for group in &self.groups {
            group.borrow_mut().Clear();
        }
        self.groupIDGen.reset();
        self.rootGroup = None;
        self.groups.clear();
        self.groupID2Group.clear();
        self.globalExpressions.clear();
    }

    /// 返回 Cascades 哈希/相等比较器（与 Go HashEqualer 对齐）。
    pub fn GetHasher(&self) -> Box<dyn cascades_base::Hasher> {
        cascades_base::NewHashEqualer()
    }

    /// 自底向上把逻辑计划树拷入 Memo：先递归孩子，再插入当前表达式。
    ///
    /// `target` 为 `Some` 时强制插入该 Group；`None` 时新建或复用等价 Group。
    /// 新插入且无指定 target 时会推导逻辑属性（DeriveLogicalProp）。
    pub fn CopyIn(
        &mut self,
        target: Option<GroupRef>,
        mut plan: LogicalPlanRef,
    ) -> logicalop::Result<GroupExpressionRef> {
        // 先自底向上推导统计信息，保证孩子已有 Stats
        derive_tree(plan.as_mut())?;
        let children = plan.TakeChildren();
        let mut child_groups = Vec::with_capacity(children.len());
        for child in children {
            let child_expression = self.CopyIn(None, child)?;
            child_groups.push(
                child_expression
                    .borrow()
                    .GetGroup()
                    .expect("copied child must belong to a group"),
            );
        }
        // 禁止孩子 Group 与目标 Group 相同，避免成环
        if let Some(target) = &target {
            assert!(child_groups.iter().all(|child| !Rc::ptr_eq(child, target)));
        }
        let expression = self.NewGroupExpression(plan, child_groups);
        let create_property = target.is_none();
        let (expression, inserted) = self.InsertGroupExpression(expression, target);
        if inserted && create_property {
            GroupExpression::DeriveLogicalProp(&expression);
        }
        Ok(expression)
    }

    /// 将规则输出以已绑定的 Memo 子 Group 插入目标 Group。
    ///
    /// Cascades 规则通常返回浅拷贝逻辑算子：其逻辑计划子树已被 Memo
    /// 拆出，真正的输入保存在 Binder 的 BoundPlan 中。该入口保留这些
    /// Group 引用，避免把浅拷贝误当成无孩子算子重新建图。
    pub fn CopyInWithGroupChildren(
        &mut self,
        target: Option<GroupRef>,
        plan: LogicalPlanRef,
        child_groups: Vec<GroupRef>,
    ) -> logicalop::Result<GroupExpressionRef> {
        if !plan.Children().is_empty() || child_groups.is_empty() {
            return self.CopyIn(target, plan);
        }

        let create_property = target.is_none();
        let expression = self.NewGroupExpression(plan, child_groups);
        let (expression, inserted) = self.InsertGroupExpression(expression, target);
        if inserted && create_property {
            GroupExpression::DeriveLogicalProp(&expression);
        }
        Ok(expression)
    }

    /// 从指定 Group 移除表达式：删边、清全局表，并标记 abandoned。
    pub fn RemoveOut(&mut self, target: &GroupRef, expression: &GroupExpressionRef) {
        Group::Delete(target, expression);
        self.remove_global(expression);
        for child in expression.borrow().Inputs.clone() {
            Group::removeParentGEs(&child, expression);
        }
        expression.borrow_mut().SetAbandoned();
    }

    /// 返回当前全部 Group 的快照列表。
    pub fn GetGroups(&self) -> Vec<GroupRef> {
        self.groups.clone()
    }

    /// 返回 GroupID → Group 映射快照。
    pub fn GetGroupID2Group(&self) -> HashMap<GroupID, GroupRef> {
        self.groupID2Group.clone()
    }

    /// 返回根 Group（若已 Init）。
    pub fn GetRootGroup(&self) -> Option<GroupRef> {
        self.rootGroup.clone()
    }

    /// 插入组表达式：全局去重；若已存在且指定了不同 target，则合并 Group。
    ///
    /// 返回 `(表达式, 是否新插入)`。
    pub fn InsertGroupExpression(
        &mut self,
        expression: GroupExpressionRef,
        target: Option<GroupRef>,
    ) -> (GroupExpressionRef, bool) {
        // 全局表命中：复用已有表达式，必要时合并所属 Group
        if let Some(existing) = self.find_global(&expression, None) {
            let existing_group = existing.borrow().GetGroup();
            if let (Some(existing_group), Some(target_group)) = (existing_group, target.as_ref()) {
                self.mergeGroup(&existing_group, target_group);
            }
            return (existing, false);
        }

        let target = target.unwrap_or_else(|| self.NewGroup());
        assert!(Group::Insert(&target, expression.clone()));
        self.globalExpressions.push(expression.clone());
        // 在孩子 Group 上登记父表达式回边
        for child in expression.borrow().Inputs.clone() {
            Group::addParentGEs(&child, &expression);
        }
        (expression, true)
    }

    /// 分配新 GroupID 并登记到 groups / groupID2Group。
    pub fn NewGroup(&mut self) -> GroupRef {
        let group = Group::NewGroup(None);
        let id = self.groupIDGen.NextGroupID();
        group.borrow_mut().groupID = id;
        self.groups.push(group.clone());
        self.groupID2Group.insert(id, group.clone());
        group
    }

    /// 用完整逻辑计划树首次初始化 Memo，并设置 rootGroup。
    pub fn Init(&mut self, plan: LogicalPlanRef) -> logicalop::Result<GroupExpressionRef> {
        assert!(self.groups.is_empty(), "memo can only be initialized once");
        let expression = self.CopyIn(None, plan)?;
        self.rootGroup = expression.borrow().GetGroup();
        Ok(expression)
    }

    /// 遍历全部 Group；callback 返回 false 时提前停止。
    pub fn ForEachGroup(&self, mut callback: impl FnMut(&GroupRef) -> bool) {
        for group in &self.groups {
            if !callback(group) {
                break;
            }
        }
    }

    /// 由逻辑算子与子 Group 输入构造新的 GroupExpression（尚未插入）。
    pub fn NewGroupExpression(
        &self,
        plan: LogicalPlanRef,
        inputs: Vec<GroupRef>,
    ) -> GroupExpressionRef {
        GroupExpression::new(plan, inputs)
    }

    /// 从根 Group 构造逻辑计划备选迭代器。
    pub fn NewIterator(&self) -> IteratorLP {
        IteratorLP::new(self.rootGroup.clone())
    }

    /// 将 source Group 合并进 target：迁移父表达式、重写孩子指针，必要时递归合并。
    pub(crate) fn mergeGroup(&mut self, source: &GroupRef, target: &GroupRef) {
        if Rc::ptr_eq(source, target) {
            return;
        }
        let source_id = source.borrow().GetGroupID();
        if !self.groupID2Group.contains_key(&source_id) {
            return;
        }

        // 从索引中摘掉 source
        self.groups.retain(|group| !Rc::ptr_eq(group, source));
        self.groupID2Group.remove(&source_id);
        if self
            .rootGroup
            .as_ref()
            .is_some_and(|root| Rc::ptr_eq(root, source))
        {
            self.rootGroup = Some(target.clone());
        }

        // 收集仍存活的父表达式，重写其指向 source 的输入为 target
        let parents = source
            .borrow()
            .parentExpressions
            .values()
            .filter_map(|parent| parent.upgrade())
            .collect::<Vec<_>>();
        let mut deferred_merges = Vec::new();
        for parent in parents {
            let Some(owner) = parent.borrow().GetGroup() else {
                continue;
            };
            if Rc::ptr_eq(&owner, target) {
                continue;
            }
            self.remove_global(&parent);
            Group::Delete(&owner, &parent);
            self.replaceGEChild(&parent, source, target);
            assert!(Group::Insert(&owner, parent.clone()));

            // 重写后若与全局已有表达式冲突，则合并或推迟合并所属 Group
            if let Some(existing) = self.find_global(&parent, Some(&parent)) {
                parent.borrow_mut().SetAbandoned();
                let existing_owner = existing
                    .borrow()
                    .GetGroup()
                    .expect("global expression must have an owner");
                if Rc::ptr_eq(&existing_owner, &owner) {
                    Group::Delete(&owner, &parent);
                    GroupExpression::mergeTo(&parent, &existing);
                } else {
                    deferred_merges.push((owner.clone(), existing_owner));
                }
            } else {
                self.globalExpressions.push(parent);
            }
        }

        Group::mergeTo(source, target);
        // 冲突导致的跨 Group 合并延后递归处理，避免迭代中途改结构
        for (source, target) in deferred_merges {
            self.mergeGroup(&source, &target);
        }
    }

    /// 把表达式输入中的 older Group 全部替换为 newer，并维护父子回边。
    pub(crate) fn replaceGEChild(
        &mut self,
        expression: &GroupExpressionRef,
        older: &GroupRef,
        newer: &GroupRef,
    ) {
        Group::removeParentGEs(older, expression);
        {
            let mut expression = expression.borrow_mut();
            for input in &mut expression.Inputs {
                if Rc::ptr_eq(input, older) {
                    *input = newer.clone();
                }
            }
            expression.Init();
        }
        Group::addParentGEs(newer, expression);
    }

    /// 在全局表达式表中按哈希与 Equals 查找等价项；`exclude` 用于跳过自身。
    fn find_global(
        &self,
        expression: &GroupExpressionRef,
        exclude: Option<&GroupExpressionRef>,
    ) -> Option<GroupExpressionRef> {
        self.globalExpressions
            .iter()
            .find(|candidate| {
                exclude.is_none_or(|exclude| !Rc::ptr_eq(candidate, exclude))
                    && candidate.borrow().GetHash64() == expression.borrow().GetHash64()
                    && candidate.borrow().Equals(&expression.borrow())
            })
            .cloned()
    }

    /// 从全局表移除指定表达式。
    fn remove_global(&mut self, expression: &GroupExpressionRef) {
        self.globalExpressions
            .retain(|candidate| !Rc::ptr_eq(candidate, expression));
    }

    /// 测试辅助：全局表达式数量。
    #[cfg(test)]
    pub(crate) fn global_expression_count(&self) -> usize {
        self.globalExpressions.len()
    }

    /// 测试辅助：直接设置根 Group。
    #[cfg(test)]
    pub(crate) fn set_root_group(&mut self, group: GroupRef) {
        self.rootGroup = Some(group);
    }
}

/// 包级构造函数，转发到 `Memo::NewMemo`。
pub fn NewMemo(capacities: &[u64]) -> Memo {
    Memo::NewMemo(capacities)
}

/// 自底向上对逻辑计划树调用 DeriveStats。
fn derive_tree(plan: &mut dyn LogicalPlan) -> logicalop::Result<()> {
    for child in plan.Children_mut() {
        derive_tree(child.as_mut())?;
    }
    plan.DeriveStats(true)?;
    Ok(())
}

/// One materialized choice from the memo forest. It retains the exact group
/// expression handles and the bottom-up plan-ID hash without cloning operators.
/// Memo 森林中的一条具体逻辑计划备选：保留组表达式句柄与自底向上的 PlanID 哈希。
#[derive(Clone)]
pub struct PlanAlternative {
    /// 本节点选用的组表达式。
    pub Expression: GroupExpressionRef,
    /// 各孩子 Group 上选定的备选子树。
    pub Children: Vec<PlanAlternative>,
    /// 由孩子 PlanIDsHash 与本算子 ID 汇聚的指纹。
    pub PlanIDsHash: u64,
}

impl PlanAlternative {
    /// 返回逻辑算子类型名字符串。
    pub fn TP(&self) -> String {
        self.Expression.borrow().LogicalPlan.TP().to_owned()
    }

    /// 返回逻辑算子计划节点 ID。
    pub fn ID(&self) -> i32 {
        self.Expression.borrow().LogicalPlan.ID()
    }
}

/// 自根 Group DFS 展开全部逻辑计划备选的迭代器。
pub struct IteratorLP {
    /// 预计算的全部备选。
    alternatives: Vec<PlanAlternative>,
    /// Next / Each 的游标。
    cursor: usize,
    /// DFS 路径上的 GroupID 栈（调试/追踪用）。
    stackInfo: Vec<GroupID>,
    /// 当前追踪深度。
    traceID: isize,
}

impl IteratorLP {
    /// 从根 Group 预计算全部备选；无根则空迭代器。
    fn new(root: Option<GroupRef>) -> Self {
        let mut iterator = Self {
            alternatives: Vec::new(),
            cursor: 0,
            stackInfo: Vec::new(),
            traceID: -1,
        };
        if let Some(root) = root {
            iterator.alternatives = iterator.dfs(&root, &mut HashSet::new());
        }
        iterator.stackInfo.clear();
        iterator.traceID = -1;
        iterator
    }

    /// 依次回调每条备选；callback 返回 false 时停止。
    pub fn Each(&mut self, mut callback: impl FnMut(&PlanAlternative) -> bool) {
        while let Some(alternative) = self.Next() {
            if !callback(&alternative) {
                break;
            }
        }
    }

    /// 取出下一条备选；耗尽返回 None。
    pub fn Next(&mut self) -> Option<PlanAlternative> {
        let alternative = self.alternatives.get(self.cursor).cloned();
        if alternative.is_some() {
            self.cursor += 1;
        }
        alternative
    }

    /// 对目标 Group 做 DFS：枚举每个逻辑表达式，并对孩子备选做笛卡尔积。
    fn dfs(&mut self, target: &GroupRef, path: &mut HashSet<GroupID>) -> Vec<PlanAlternative> {
        let id = target.borrow().GetGroupID();
        // 路径上出现重复 GroupID 说明成环，备选图必须无环
        assert!(
            path.insert(id),
            "memo alternatives must form an acyclic plan graph"
        );
        self.traceIn(target);
        let mut result = Vec::new();
        for index in 0..target.borrow().logicalExpressions.len() {
            let expression = self
                .pickGroupExpression(target, index)
                .expect("expression index came from the group length");
            let child_groups = expression.borrow().Inputs.clone();
            let child_alternatives = child_groups
                .iter()
                .map(|child| self.dfs(child, path))
                .collect::<Vec<_>>();
            // 孩子备选笛卡尔积，再与本表达式组合并计算 PlanIDsHash
            for children in cartesian_product(&child_alternatives) {
                let mut hasher = cascades_base::NewHashEqualer();
                for child in &children {
                    hasher.HashUint64(child.PlanIDsHash);
                }
                hasher.HashInt(expression.borrow().LogicalPlan.ID() as isize);
                result.push(PlanAlternative {
                    Expression: expression.clone(),
                    Children: children,
                    PlanIDsHash: hasher.Sum64(),
                });
            }
        }
        path.remove(&id);
        self.stackInfo.pop();
        self.traceID -= 1;
        result
    }

    /// 进入 Group 时更新追踪栈。
    fn traceIn(&mut self, group: &GroupRef) {
        self.traceID += 1;
        self.stackInfo.push(group.borrow().GetGroupID());
    }

    /// 按索引取出 Group 内的逻辑表达式。
    fn pickGroupExpression(&self, group: &GroupRef, index: usize) -> Option<GroupExpressionRef> {
        group.borrow().logicalExpressions.get(index).cloned()
    }
}

/// 多路选择的笛卡尔积，用于组合各孩子 Group 的备选子树。
fn cartesian_product(inputs: &[Vec<PlanAlternative>]) -> Vec<Vec<PlanAlternative>> {
    let mut result = vec![Vec::new()];
    for choices in inputs {
        let mut next = Vec::new();
        for prefix in result {
            for choice in choices {
                let mut item = prefix.clone();
                item.push(choice.clone());
                next.push(item);
            }
        }
        result = next;
    }
    result
}

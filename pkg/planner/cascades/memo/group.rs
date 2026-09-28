// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0

// Cascades Memo 中的等价类 Group。
//
// Group 持有语义等价的一组 `GroupExpression`（逻辑算子 + 子 Group），
// 维护按算子类型的首条索引、父表达式弱引用、逻辑属性，以及按物理属性
// （PhysicalProperty）缓存的最优物理任务（best task）。

use crate::{GroupExpression, GroupExpressionRef, GroupID};
use cascades_base::Hasher;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

/// `Group` 的共享可变引用（Rc + RefCell），对齐 Go 中的指针共享图。
pub type GroupRef = Rc<RefCell<Group>>;

/// One equivalence class in the cascades memo.
/// Cascades Memo 中的一个等价类：同一 Group 内表达式逻辑语义等价。
pub struct Group {
    /// 本 Group 的唯一标识。
    pub(crate) groupID: GroupID,
    /// 组内全部逻辑表达式（GroupExpression）。
    pub(crate) logicalExpressions: Vec<GroupExpressionRef>,
    /// 算子类型（operand）→ 该类型在 `logicalExpressions` 中首次出现的下标。
    pub Operand2FirstExpr: HashMap<String, usize>,
    /// 指向以本 Group 为输入的父 GroupExpression 的弱引用表（键为地址）。
    pub(crate) parentExpressions: HashMap<usize, Weak<RefCell<GroupExpression>>>,
    /// 派生出的逻辑属性（schema、统计信息等）。
    pub(crate) logicalProp: Option<property::LogicalProperty>,
    /// 是否已完成探索（exploration，应用变换规则）。
    explored: bool,
    /// 按物理属性哈希码缓存的最优物理执行任务。
    bestPhysicalMap: HashMap<Vec<u8>, Box<dyn core_base::Task>>,
}

impl Group {
    /// 创建空 Group，可选地附带初始逻辑属性。
    pub fn NewGroup(property: Option<property::LogicalProperty>) -> GroupRef {
        Rc::new(RefCell::new(Self {
            groupID: 0,
            logicalExpressions: Vec::new(),
            Operand2FirstExpr: HashMap::new(),
            parentExpressions: HashMap::new(),
            logicalProp: property,
            explored: false,
            bestPhysicalMap: HashMap::new(),
        }))
    }

    /// 将 groupID 写入 Hasher，用于等价类层面的哈希。
    pub fn Hash64(&self, hasher: &mut dyn Hasher) {
        hasher.HashUint64(self.groupID);
    }

    /// 仅比较 groupID 是否相等。
    pub fn Equals(&self, other: &Group) -> bool {
        self.groupID == other.groupID
    }

    /// 向 Group 插入表达式；若已存在等价表达式则返回 false。
    /// 插入位置尽量靠近同 operand 的首次出现之后，并重建索引。
    pub fn Insert(group: &GroupRef, expression: GroupExpressionRef) -> bool {
        // 已存在语义等价表达式则拒绝插入。
        if group
            .borrow()
            .logicalExpressions
            .iter()
            .any(|existing| existing.borrow().Equals(&expression.borrow()))
        {
            return false;
        }
        let operand = expression.borrow().LogicalPlan.TP().to_owned();
        // 同类型算子插在首次出现之后，保持 Operand2FirstExpr 语义。
        let index = group.borrow().Operand2FirstExpr.get(&operand).map_or_else(
            || group.borrow().logicalExpressions.len(),
            |first| first + 1,
        );
        expression.borrow_mut().group = Rc::downgrade(group);
        group
            .borrow_mut()
            .logicalExpressions
            .insert(index, expression);
        group.borrow_mut().rebuild_operand_index();
        true
    }

    /// 从 Group 删除指定表达式，并断开其回指弱引用。
    pub fn Delete(group: &GroupRef, expression: &GroupExpressionRef) {
        let position = {
            let group = group.borrow();
            group.logicalExpressions.iter().position(|existing| {
                Rc::ptr_eq(existing, expression) || existing.borrow().Equals(&expression.borrow())
            })
        };
        let Some(position) = position else {
            return;
        };
        let removed = group.borrow_mut().logicalExpressions.remove(position);
        removed.borrow_mut().group = Weak::new();
        group.borrow_mut().rebuild_operand_index();
    }

    /// 返回本 Group 的 ID。
    pub fn GetGroupID(&self) -> GroupID {
        self.groupID
    }

    /// 克隆返回组内全部逻辑表达式句柄。
    pub fn GetLogicalExpressions(&self) -> Vec<GroupExpressionRef> {
        self.logicalExpressions.clone()
    }

    /// 取组内第一条表达式，或指定 operand 类型的第一条。
    pub fn GetFirstElem(&self, operand: Option<&str>) -> Option<GroupExpressionRef> {
        match operand {
            None => self.logicalExpressions.first().cloned(),
            Some(operand) => self
                .Operand2FirstExpr
                .get(operand)
                .and_then(|index| self.logicalExpressions.get(*index))
                .cloned(),
        }
    }

    /// 是否已派生逻辑属性。
    pub fn HasLogicalProperty(&self) -> bool {
        self.logicalProp.is_some()
    }

    /// 获取已派生的逻辑属性引用。
    pub fn GetLogicalProperty(&self) -> Option<&property::LogicalProperty> {
        self.logicalProp.as_ref()
    }

    /// 设置（覆盖）逻辑属性。
    pub fn SetLogicalProperty(&mut self, property: property::LogicalProperty) {
        self.logicalProp = Some(property);
    }

    /// 是否已完成探索。
    pub fn IsExplored(&self) -> bool {
        self.explored
    }

    /// 标记本 Group 已探索完毕。
    pub fn SetExplored(&mut self) {
        self.explored = true;
    }

    /// 调试用字符串：`GID:<id>`。
    pub fn String(&self) -> String {
        format!("GID:{}", self.groupID)
    }

    /// 遍历组内全部表达式；回调返回 false 时提前结束。
    pub fn ForEachGE(group: &GroupRef, mut callback: impl FnMut(&GroupExpressionRef) -> bool) {
        // Snapshot the Rc handles so the callback may migrate/delete the current
        // expression without invalidating the traversal cursor.
        // 先快照 Rc，允许回调迁移/删除当前表达式而不破坏遍历游标。
        for expression in group.borrow().logicalExpressions.clone() {
            if !callback(&expression) {
                break;
            }
        }
    }

    /// 从父表达式引用表中移除指定父 GE。
    pub(crate) fn removeParentGEs(group: &GroupRef, parent: &GroupExpressionRef) {
        let removed = group
            .borrow_mut()
            .parentExpressions
            .remove(&GroupExpression::addr(parent));
        assert!(removed.is_some(), "parent expression reference is missing");
    }

    /// 登记以本 Group 为输入的父 GroupExpression（弱引用）。
    pub(crate) fn addParentGEs(group: &GroupRef, parent: &GroupExpressionRef) {
        let previous = group
            .borrow_mut()
            .parentExpressions
            .insert(GroupExpression::addr(parent), Rc::downgrade(parent));
        assert!(previous.is_none(), "parent expression registered twice");
    }

    /// 将 source Group 合并入 target：迁移父引用与表达式，重复则 merge 表达式。
    pub(crate) fn mergeTo(source: &GroupRef, target: &GroupRef) {
        // 先迁移父表达式弱引用表。
        for (address, parent) in source.borrow().parentExpressions.clone() {
            target
                .borrow_mut()
                .parentExpressions
                .insert(address, parent);
        }
        let expressions = source.borrow().logicalExpressions.clone();
        for expression in expressions {
            let equivalent = target
                .borrow()
                .logicalExpressions
                .iter()
                .find(|candidate| candidate.borrow().Equals(&expression.borrow()))
                .cloned();
            if let Some(equivalent) = equivalent {
                // 目标已有等价表达式：合并探索 mask 等状态后丢弃 source 侧。
                Group::Delete(source, &expression);
                GroupExpression::mergeTo(&expression, &equivalent);
            } else {
                Group::Delete(source, &expression);
                assert!(Group::Insert(target, expression));
            }
        }
        source.borrow_mut().Clear();
    }

    /// 清空组内表达式、索引、父引用与逻辑属性。
    pub fn Clear(&mut self) {
        for expression in &self.logicalExpressions {
            expression.borrow_mut().group = Weak::new();
        }
        self.logicalExpressions.clear();
        self.Operand2FirstExpr.clear();
        self.parentExpressions.clear();
        self.logicalProp = None;
    }

    /// 不变量检查：groupID、operand 索引与父表达式输入边一致性。
    pub fn Check(group: &GroupRef) {
        let group = group.borrow();
        assert!(group.groupID > 0);
        for (operand, first) in &group.Operand2FirstExpr {
            let expression = &group.logicalExpressions[*first];
            assert_eq!(expression.borrow().LogicalPlan.TP(), operand);
        }
        for parent in group.parentExpressions.values() {
            let parent = parent
                .upgrade()
                .expect("parent expression must remain live");
            assert!(
                parent
                    .borrow()
                    .Inputs
                    .iter()
                    .any(|child| child.borrow().groupID == group.groupID)
            );
        }
    }

    /// 缓存给定物理属性下的最优任务。
    pub fn SetBestTask(
        &mut self,
        property: &property::PhysicalProperty,
        task: Box<dyn core_base::Task>,
    ) {
        self.bestPhysicalMap.insert(property.HashCode(), task);
    }

    /// 查询给定物理属性下已缓存的最优任务。
    pub fn GetBestTask(
        &self,
        property: &property::PhysicalProperty,
    ) -> Option<&dyn core_base::Task> {
        self.bestPhysicalMap
            .get(&property.HashCode())
            .map(Box::as_ref)
    }

    /// 测试用：返回登记的父表达式数量。
    #[cfg(test)]
    pub(crate) fn parent_count(&self) -> usize {
        self.parentExpressions.len()
    }

    /// 根据当前表达式列表重建 Operand → 首次下标索引。
    fn rebuild_operand_index(&mut self) {
        self.Operand2FirstExpr.clear();
        for (index, expression) in self.logicalExpressions.iter().enumerate() {
            self.Operand2FirstExpr
                .entry(expression.borrow().LogicalPlan.TP().to_owned())
                .or_insert(index);
        }
    }
}

/// 便捷构造：等价于 `Group::NewGroup`。
pub fn NewGroup(property: Option<property::LogicalProperty>) -> GroupRef {
    Group::NewGroup(property)
}

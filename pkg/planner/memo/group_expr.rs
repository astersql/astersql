// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// GroupExpr：Memo 中「逻辑算子 + 子 Group」形态的表达式节点。
//
// 与普通 LogicalPlan 树不同，子节点不是计划节点而是 Group（等价类）；
// 指纹用于 Group 内去重，ExploreMark / appliedRuleSet 记录变换探索状态。

use crate::{ExploreMark, Group, GroupRef};
use astersql_expression::Schema;
use astersql_planner_core_operator_logicalop::{LogicalPlan, LogicalPlanRef};
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::{Rc, Weak};

/// GroupExpr 的共享可变引用。
pub type GroupExprRef = Rc<RefCell<GroupExpr>>;

/// A logical operator whose children are memo groups rather than plan nodes.
/// 逻辑算子，子节点为 Memo Group 而非计划节点。
pub struct GroupExpr {
    /// 本表达式对应的逻辑算子节点。
    pub ExprNode: LogicalPlanRef,
    /// 子输入各自所属的等价类 Group。
    pub Children: Vec<GroupRef>,
    /// 所属 Group 的弱引用（插入 Group 后回填，避免循环强引用）。
    pub Group: Weak<RefCell<Group>>,
    /// 各探索轮次是否已探索的位标记。
    pub ExploreMark: ExploreMark,
    /// 缓存的自指纹；子节点变更后需清空重算。
    selfFingerprint: Vec<u8>,
    /// 已对本表达式应用过的变换规则 ID 集合，避免重复应用。
    appliedRuleSet: HashSet<u64>,
}

/// 用给定逻辑算子构造空子节点、未归属 Group 的 GroupExpr。
pub fn NewGroupExpr(node: LogicalPlanRef) -> GroupExprRef {
    Rc::new(RefCell::new(GroupExpr {
        ExprNode: node,
        Children: Vec::new(),
        Group: Weak::new(),
        ExploreMark: ExploreMark::default(),
        selfFingerprint: Vec::new(),
        appliedRuleSet: HashSet::new(),
    }))
}

impl GroupExpr {
    /// 标记指定探索轮次已探索。
    pub fn SetExplored(&mut self, round: usize) {
        self.ExploreMark.SetExplored(round);
    }

    /// 清除指定探索轮次的已探索标记。
    pub fn SetUnexplored(&mut self, round: usize) {
        self.ExploreMark.SetUnexplored(round);
    }

    /// 查询指定探索轮次是否已探索。
    pub fn Explored(&self, round: usize) -> bool {
        self.ExploreMark.Explored(round)
    }

    /// The encoding is lossless: child count, stable child-group IDs, plan hash.
    /// 无损编码：子节点数量 + 各子 Group 稳定 ID + 算子 HashCode；结果缓存。
    pub fn FingerPrint(&mut self) -> Vec<u8> {
        if self.selfFingerprint.is_empty() {
            // 大端 u16 子数量，再拼接每个子 Group 的 u64 ID，最后是算子哈希。
            let mut fingerprint = (self.Children.len() as u16).to_be_bytes().to_vec();
            for child in &self.Children {
                fingerprint.extend_from_slice(&child.borrow().ID().to_be_bytes());
            }
            fingerprint.extend_from_slice(&self.ExprNode.HashCode());
            self.selfFingerprint = fingerprint;
        }
        self.selfFingerprint.clone()
    }

    /// 设置子 Group 并清空指纹缓存，迫使下次 FingerPrint 重算。
    pub fn SetChildren(&mut self, children: Vec<GroupRef>) {
        self.Children = children;
        self.selfFingerprint.clear();
    }

    /// 从所属 Group 的逻辑属性中取 Schema；尚未挂入 Group 时返回 None。
    pub fn Schema(&self) -> Option<Schema> {
        self.Group
            .upgrade()
            .and_then(|group| group.borrow().Prop.Schema.as_deref().map(Schema::Clone))
    }

    /// 记录某变换规则已对本表达式应用。
    pub fn AddAppliedRule(&mut self, rule_id: u64) {
        self.appliedRuleSet.insert(rule_id);
    }

    /// 查询某变换规则是否已对本表达式应用过。
    pub fn HasAppliedRule(&self, rule_id: u64) -> bool {
        self.appliedRuleSet.contains(&rule_id)
    }
}

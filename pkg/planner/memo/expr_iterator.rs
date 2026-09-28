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

// ExprIter：按 Cascades Pattern 在 Memo Group 中枚举匹配的等价表达式。
//
// 迭代顺序与 Go memo 迭代器一致：先推进最右子迭代器，再重置右侧兄弟，
// 最后在本 Group 的 Equivalents 中按 Operand/引擎过滤推进 Element。

use crate::{GroupExprRef, GroupRef};
use astersql_planner_cascades_pattern::{self as pattern, Pattern};

/// Enumerates expressions in exactly the order used by the Go memo iterator.
/// 按与 Go memo 迭代器完全相同的顺序枚举匹配表达式。
pub struct ExprIter {
    /// 当前绑定的 Group；根迭代器在部分构造路径上可能为 None。
    pub Group: Option<GroupRef>,
    /// 当前匹配的 Equivalents 下标。
    pub Element: Option<usize>,
    /// 最近一次 Next/Reset 是否仍处于匹配状态。
    matched: bool,
    /// 要匹配的算子模式（Operand、引擎集合、子 Pattern）。
    pub Pattern: Pattern,
    /// 与 Pattern.Children 一一对应的子迭代器。
    pub Children: Vec<ExprIter>,
    /// 根迭代器的固定表达式。Go 根迭代器不绑定 Group，但仍能返回 GetExpr。
    root_expression: Option<GroupExprRef>,
}

impl ExprIter {
    /// 推进到下一个匹配组合；无更多匹配时将 matched 置 false 并返回 false。
    pub fn Next(&mut self) -> bool {
        // 先尝试从最右子迭代器推进（字典序式组合枚举）。
        for index in (0..self.Children.len()).rev() {
            if !self.Children[index].Next() {
                continue;
            }
            // 右侧兄弟需 Reset 到各自的第一个匹配。
            for child in self.Children.iter_mut().skip(index + 1) {
                child.Reset();
            }
            self.matched = true;
            return true;
        }

        let Some(group) = self.Group.clone() else {
            self.matched = false;
            return false;
        };
        // OperandAny 表示只匹配引擎、不枚举具体表达式，Next 直接结束。
        if self.Pattern.Operand == pattern::OperandAny {
            self.matched = false;
            return false;
        }

        // 从当前 Element 的下一个下标起扫描本 Group。
        let mut index = self.Element.map_or(0, |element| element + 1);
        loop {
            let Some(expression) = group.borrow().Equivalents.get(index).cloned() else {
                self.matched = false;
                return false;
            };
            let (operand, child_groups) = {
                let expression = expression.borrow();
                (
                    pattern::GetOperand(expression.ExprNode.as_ref()),
                    expression.Children.clone(),
                )
            };
            // Operand 不匹配则整条链结束（Equivalents 按 Operand 聚簇，后续不会再匹配）。
            if !self.Pattern.Operand.Match(operand) {
                self.matched = false;
                return false;
            }
            if self.Children.is_empty() {
                self.Element = Some(index);
                self.matched = true;
                return true;
            }
            // 子 Pattern 数量与子 Group 一致，且全部 Reset 成功才算匹配。
            if self.Children.len() == child_groups.len()
                && reset_children(&mut self.Children, &child_groups)
            {
                self.Element = Some(index);
                self.matched = true;
                return true;
            }
            index += 1;
        }
    }

    /// 当前是否仍匹配。
    pub fn Matched(&self) -> bool {
        self.matched
    }

    /// 重置到本 Group 中第一个满足 Pattern（含引擎）的表达式组合。
    pub fn Reset(&mut self) -> bool {
        let Some(group) = self.Group.clone() else {
            self.matched = false;
            return false;
        };
        let engine = group.borrow().EngineType;
        // OperandAny：不绑定具体 Element，整 Group 视为一次匹配。
        if self.Pattern.MatchOperandAny(engine) {
            self.Element = None;
            self.matched = true;
            return true;
        }

        let Some(mut index) = group.borrow().GetFirstElem(self.Pattern.Operand) else {
            self.matched = false;
            return false;
        };
        loop {
            let Some(expression) = group.borrow().Equivalents.get(index).cloned() else {
                self.matched = false;
                return false;
            };
            let (operand, child_groups) = {
                let expression = expression.borrow();
                (
                    pattern::GetOperand(expression.ExprNode.as_ref()),
                    expression.Children.clone(),
                )
            };
            if !self.Pattern.Match(operand, engine) {
                self.matched = false;
                return false;
            }
            if self.Children.is_empty() {
                self.Element = Some(index);
                self.matched = true;
                return true;
            }
            if self.Children.len() == child_groups.len()
                && reset_children(&mut self.Children, &child_groups)
            {
                self.Element = Some(index);
                self.matched = true;
                return true;
            }
            index += 1;
        }
    }

    /// 取当前 Element 对应的 GroupExpr；Group/Element 缺失时返回 None。
    pub fn GetExpr(&self) -> Option<GroupExprRef> {
        if self.Group.is_none() {
            return self.root_expression.clone();
        }
        let group = self.Group.as_ref()?;
        group.borrow().Equivalents.get(self.Element?).cloned()
    }
}

/// 把子迭代器绑定到对应子 Group 并各自 Reset；任一失败则整体失败。
fn reset_children(iterators: &mut [ExprIter], groups: &[GroupRef]) -> bool {
    for (iterator, group) in iterators.iter_mut().zip(groups) {
        iterator.Group = Some(group.clone());
        if !iterator.Reset() {
            return false;
        }
    }
    true
}

/// 从 Group 的指定 element 出发，若该表达式匹配 Pattern 则构造已定位的迭代器。
pub fn NewExprIterFromGroupElem(
    group: &GroupRef,
    element: usize,
    expression_pattern: &Pattern,
) -> Option<ExprIter> {
    let expression = group.borrow().Equivalents.get(element).cloned()?;
    let engine = group.borrow().EngineType;
    if !expression_pattern.Match(
        pattern::GetOperand(expression.borrow().ExprNode.as_ref()),
        engine,
    ) {
        return None;
    }
    let mut iterator = newExprIterFromGroupExpr(&expression, expression_pattern)?;
    // Go 只绑定根 Element；根 Group 保持 nil，避免 Next 扫描同 Group 的其它根式。
    iterator.root_expression = Some(expression);
    iterator.Element = Some(element);
    Some(iterator)
}

/// 由单个 GroupExpr 与 Pattern 构造子迭代器树；子数量不一致则失败。
fn newExprIterFromGroupExpr(
    expression: &GroupExprRef,
    expression_pattern: &Pattern,
) -> Option<ExprIter> {
    let child_groups = expression.borrow().Children.clone();
    if !expression_pattern.Children.is_empty()
        && expression_pattern.Children.len() != child_groups.len()
    {
        return None;
    }
    let mut iterator = ExprIter {
        Group: None,
        Element: None,
        matched: true,
        Pattern: expression_pattern.clone(),
        Children: Vec::with_capacity(expression_pattern.Children.len()),
        root_expression: None,
    };
    for (child_group, child_pattern) in child_groups.iter().zip(&expression_pattern.Children) {
        iterator
            .Children
            .push(newExprIterFromGroup(child_group, child_pattern)?);
    }
    Some(iterator)
}

/// 在 Group 中找第一个匹配 Pattern 的表达式并构造迭代器；OperandAny 则整组匹配。
fn newExprIterFromGroup(group: &GroupRef, expression_pattern: &Pattern) -> Option<ExprIter> {
    let engine = group.borrow().EngineType;
    if expression_pattern.MatchOperandAny(engine) {
        return Some(ExprIter {
            Group: Some(group.clone()),
            Element: None,
            matched: true,
            Pattern: expression_pattern.clone(),
            Children: Vec::new(),
            root_expression: None,
        });
    }

    let mut index = group.borrow().GetFirstElem(expression_pattern.Operand)?;
    loop {
        let expression = group.borrow().Equivalents.get(index).cloned()?;
        if !expression_pattern.Match(
            pattern::GetOperand(expression.borrow().ExprNode.as_ref()),
            engine,
        ) {
            return None;
        }
        if let Some(mut iterator) = newExprIterFromGroupExpr(&expression, expression_pattern) {
            iterator.Group = Some(group.clone());
            iterator.Element = Some(index);
            return Some(iterator);
        }
        index += 1;
    }
}

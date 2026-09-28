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

// Cascades 规则绑定器（Binder）：将 Pattern 与 Memo 中的 GroupExpression 对齐。
//
// Binder 深度优先枚举所有满足 Pattern 的绑定树（BoundPlan）。Go 侧可让
// GroupExpression 直接实现 LogicalPlan；Rust 用显式 BoundPlan 适配，使 Memo
// 仍由 Rc/RefCell 持有，同时规则拿到统一的算子/孩子视图。OperandAny 在同一
// Group 内只取首个已匹配表达式，避免组合爆炸。

use cascades_memo::{GroupExpressionRef, GroupRef};
use cascades_pattern::{GetOperand, OperandAny, Pattern};

/// A bound memo expression tree. Go can make GroupExpression implement
/// LogicalPlan directly; Rust keeps this explicit adapter so the memo graph
/// remains Rc/RefCell-owned while rules receive the same operator/child view.
/// 已绑定的 memo 表达式树：根表达式引用 + 按 Pattern 对齐的孩子 BoundPlan。
#[derive(Clone)]
pub struct BoundPlan {
    /// 当前绑定的组表达式。
    expression: GroupExpressionRef,
    /// 与 Pattern 子节点一一对应的绑定子树。
    children: Vec<BoundPlan>,
}

impl BoundPlan {
    /// 取得绑定的组表达式引用。
    pub fn Expression(&self) -> &GroupExpressionRef {
        &self.expression
    }
    /// 取得已绑定的孩子 BoundPlan 切片。
    pub fn Children(&self) -> &[BoundPlan] {
        &self.children
    }

    /// 返回该绑定的 Memo 子 Group，供浅拷贝规则输出重新接回 Memo 图。
    pub fn ChildGroups(&self) -> Vec<GroupRef> {
        self.children
            .iter()
            .map(|child| {
                child
                    .expression
                    .borrow()
                    .GetGroup()
                    .expect("bound expression must belong to a group")
            })
            .collect()
    }

    /// 在持有表达式借用的前提下，对包装的逻辑计划执行回调。
    pub fn WithWrappedLogicalPlan<T>(
        &self,
        callback: impl FnOnce(&dyn logicalop::LogicalPlan) -> T,
    ) -> T {
        let expression = self.expression.borrow();
        callback(expression.GetWrappedLogicalPlan())
    }
}

/// Pattern 与根 GroupExpression 的绑定枚举器：可依次取出所有匹配的 BoundPlan。
pub struct Binder {
    /// 预计算得到的全部匹配结果。
    matches: Vec<BoundPlan>,
    /// 下一次 Next 将返回的下标。
    next: usize,
    /// 当前绑定（Holder）；构造时为根表达式，Next 成功后为最新匹配。
    holder: Option<BoundPlan>,
}

/// 用 Pattern 与根表达式构造 Binder，并立即 DFS 收集全部匹配。
pub fn NewBinder(pattern: Pattern, expression: GroupExpressionRef) -> Binder {
    Binder {
        matches: dfsMatch(&pattern, &expression),
        next: 0,
        // Go initializes holder with the supplied root expression. Keep that
        // observable state even when the pattern never yields a match.
        holder: Some(BoundPlan {
            expression,
            children: Vec::new(),
        }),
    }
}

/// 判断单个表达式是否与 Pattern 节点的 Operand 匹配（Any 恒真）。
fn r#match(pattern: &Pattern, expression: &GroupExpressionRef) -> bool {
    if pattern.Operand == OperandAny {
        return true;
    }
    let expression = expression.borrow();
    GetOperand(expression.GetWrappedLogicalPlan()).Match(pattern.Operand)
}

/// 深度优先匹配：根匹配后，对每个子 Pattern 与对应 Group 做笛卡尔组合。
fn dfsMatch(pattern: &Pattern, expression: &GroupExpressionRef) -> Vec<BoundPlan> {
    if !r#match(pattern, expression) {
        return Vec::new();
    }
    let inputs = expression.borrow().Inputs.clone();
    // 无子模式：仅绑定当前节点。
    if pattern.Children.is_empty() {
        return vec![BoundPlan {
            expression: expression.clone(),
            children: Vec::new(),
        }];
    }
    // 子模式个数必须与输入 Group 个数一致。
    if pattern.Children.len() != inputs.len() {
        return Vec::new();
    }

    // 逐子位扩展前缀组合，得到全部孩子绑定排列。
    let mut combinations: Vec<Vec<BoundPlan>> = vec![Vec::new()];
    for (child_pattern, group) in pattern.Children.iter().zip(inputs) {
        let candidates = pickGroupExpression(child_pattern, &group);
        if candidates.is_empty() {
            return Vec::new();
        }
        let mut expanded = Vec::new();
        for prefix in combinations {
            for candidate in &candidates {
                let mut next = prefix.clone();
                next.push(candidate.clone());
                expanded.push(next);
            }
        }
        combinations = expanded;
    }

    combinations
        .into_iter()
        .map(|children| BoundPlan {
            expression: expression.clone(),
            children,
        })
        .collect()
}

/// OperandAny 且已跳过首个候选时返回 true，用于截断同组后续枚举。
fn anyHasBeenMatched(pattern: &Pattern, index: usize) -> bool {
    pattern.Operand == OperandAny && index > 0
}

/// 在 Group 的逻辑表达式中挑选并递归匹配，生成候选 BoundPlan 列表。
fn pickGroupExpression(pattern: &Pattern, group: &GroupRef) -> Vec<BoundPlan> {
    group
        .borrow()
        .GetLogicalExpressions()
        .into_iter()
        .enumerate()
        // Any 只保留组内第一个表达式，避免与所有等价式做组合爆炸。
        .take_while(|(index, _)| !anyHasBeenMatched(pattern, *index))
        .filter(|(_, expression)| r#match(pattern, expression))
        .flat_map(|(_, expression)| dfsMatch(pattern, &expression))
        .collect()
}

impl Binder {
    /// 取出下一个匹配的 BoundPlan；耗尽后返回 None。
    pub fn Next(&mut self) -> Option<BoundPlan> {
        let result = self.matches.get(self.next).cloned();
        if result.is_some() {
            self.next += 1;
            self.holder = result.clone();
        }
        result
    }

    /// 取得最近一次成功 Next 返回的绑定引用。
    pub fn GetHolder(&self) -> Option<&BoundPlan> {
        self.holder.as_ref()
    }
}

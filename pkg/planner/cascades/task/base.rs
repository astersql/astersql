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

// Cascades 任务基类与优化器上下文契约。
//
// Cascades 用任务栈驱动探索：每个 Task 只做一小步（应用规则、优化 Group 等），
// 通过 Context 入队后续任务、向 Memo 写入/移除表达式，以及查询可用规则。
// BaseTask 封装共享 Context，Push 仅调度、不内联执行。

use crate::{GroupExpressionRef, GroupRef, RuleRef, TaskError};
use cascades_base::Task;
use cascades_pattern::Operand;
use logicalop::LogicalPlanRef;
use std::cell::RefCell;
use std::rc::Rc;

/// Shared optimizer services needed by stack tasks.
/// 栈式任务所需的共享优化器服务：入队、Memo 写入/移除、规则查询与开关。
pub trait Context {
    /// 将任务压入调度栈。
    fn PushTask(&mut self, task: Box<dyn Task>);
    /// 将逻辑表达式写入目标 Group（CopyIn）。
    fn CopyIn(
        &mut self,
        target: &GroupRef,
        expression: LogicalPlanRef,
    ) -> Result<GroupExpressionRef, TaskError>;
    /// 将规则输出写入目标 Group；默认实现兼容普通逻辑计划子树。
    fn CopyInWithChildren(
        &mut self,
        target: &GroupRef,
        expression: LogicalPlanRef,
        _child_groups: Vec<GroupRef>,
    ) -> Result<GroupExpressionRef, TaskError> {
        self.CopyIn(target, expression)
    }
    /// 从目标 Group 移除表达式（RemoveOut）。
    fn RemoveOut(&mut self, target: &GroupRef, expression: &GroupExpressionRef);
    /// 按 Operand 名称取得适用规则列表。
    fn RulesFor(&self, operand: Operand) -> Vec<RuleRef>;
    /// 查询指定规则 ID 是否启用。
    fn RuleEnabled(&self, rule_id: usize) -> bool;
}

/// 共享 Context 的引用计数句柄（Rc + RefCell）。
pub type ContextRef = Rc<RefCell<dyn Context>>;

/// Common task wrapper. `Push` only schedules work; it never executes inline.
/// 通用任务包装：持有 Context，Push 只入队、不内联执行。
#[derive(Clone)]
pub struct BaseTask {
    /// 共享优化器上下文。
    pub ctx: ContextRef,
}

impl BaseTask {
    /// 用给定 Context 构造 BaseTask。
    pub fn New(ctx: ContextRef) -> Self {
        Self { ctx }
    }

    /// 将子任务压入 Context 的调度栈。
    pub fn Push(&self, task: Box<dyn Task>) {
        self.ctx.borrow_mut().PushTask(task);
    }
}

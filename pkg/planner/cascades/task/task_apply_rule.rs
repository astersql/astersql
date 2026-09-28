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

// Cascades 中「对组表达式应用变换规则」的任务实现。
//
// Cascades/Volcano 风格优化器把逻辑计划存在 Memo（等价类记忆表）里。
// 本模块定义 Rule（变换规则）、Binder（绑定器，把规则模式匹配到具体表达式）
// 以及 ApplyRuleTask：对某个 GroupExpression 的每个绑定跑 PreCheck / XForm，
// 再把新逻辑表达式 CopyIn 回目标 Group，并调度后续 OptGroupExpression 任务。

use crate::{BaseTask, ContextRef, GroupExpressionRef, NewOptGroupExpressionTask, TaskError};
use cascades_base::Task;
use cascades_base::util::StrBufferWriter;
use cascades_rule::{BoundPlan, NewBinder, Rule};
use cascades_util::StrBufferWriter as CascadesStrBufferWriter;
use std::rc::Rc;

/// Rule 的引用计数句柄，便于在任务与 Context 间共享同一规则实例。
pub type RuleRef = Rc<dyn Rule>;

/// Apply one rule to every binding of one memo group expression.
/// 对 Memo 中某个组表达式的每个绑定应用一条变换规则。
pub struct ApplyRuleTask {
    pub BaseTask: BaseTask,
    /// 待应用规则的目标组表达式。
    pub gE: GroupExpressionRef,
    /// 要应用的变换规则。
    pub rule: RuleRef,
}

/// 构造 ApplyRuleTask 并装箱为调度器可压栈的 Task。
pub fn NewApplyRuleTask(
    ctx: ContextRef,
    expression: GroupExpressionRef,
    rule: RuleRef,
) -> Box<dyn Task> {
    Box::new(ApplyRuleTask {
        BaseTask: BaseTask::New(ctx),
        gE: expression,
        rule,
    })
}

impl Task for ApplyRuleTask {
    fn Execute(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let rule_id = self.rule.ID();
        // 已对该规则探索过，或表达式已被标记废弃（abandoned），则跳过。
        if self.gE.borrow().IsExplored(rule_id) || self.gE.borrow().IsAbandoned() {
            return Ok(());
        }

        // 遍历全部绑定：PreCheck 失败则跳过；XForm 成功则 CopyIn 并调度优化。
        let mut binder = NewBinder(self.rule.Pattern().clone(), self.gE.clone());
        while let Some(holder) = binder.Next() {
            if !self.rule.PreCheck(&holder) {
                continue;
            }
            let (new_expressions, remove) = self
                .rule
                .XForm(&holder)
                .map_err(|error| TaskError::New(error.to_string()))?;
            let child_groups = holder.ChildGroups();
            for expression in new_expressions {
                // CopyIn：把新逻辑表达式插入同一等价组（Group），必要时去重。
                let target = self
                    .gE
                    .borrow()
                    .GetGroup()
                    .expect("applied group expression must belong to a group");
                let new_group_expression = self.BaseTask.ctx.borrow_mut().CopyInWithChildren(
                    &target,
                    expression,
                    child_groups.clone(),
                )?;
                self.BaseTask.Push(NewOptGroupExpressionTask(
                    self.BaseTask.ctx.clone(),
                    new_group_expression,
                ));
            }
            // remove 为真时从 Group 摘掉原表达式（规则要求替换而非并存）。
            if remove {
                let target = self
                    .gE
                    .borrow()
                    .GetGroup()
                    .expect("applied group expression must belong to a group");
                self.BaseTask.ctx.borrow_mut().RemoveOut(&target, &self.gE);
            }
        }
        // 标记本规则已在该组表达式上探索完毕，避免重复调度。
        self.gE.borrow_mut().SetExplored(rule_id);
        Ok(())
    }

    fn Desc(&self, writer: &mut dyn StrBufferWriter) {
        writer.WriteString("ApplyRuleTask{gE:");
        writer.WriteString(&self.gE.borrow().String());
        writer.WriteString(", rule:");
        let mut adapter = RuleWriter(writer);
        self.rule.String(&mut adapter);
        writer.WriteString("}");
    }
}

/// 任务描述使用 `cascades_base` 的 writer，而真实 Rule 使用 `cascades_util` 的 writer。
struct RuleWriter<'a>(&'a mut dyn StrBufferWriter);

impl CascadesStrBufferWriter for RuleWriter<'_> {
    fn WriteString(&mut self, text: &str) {
        self.0.WriteString(text);
    }

    fn Flush(&mut self) {
        self.0.Flush();
    }
}

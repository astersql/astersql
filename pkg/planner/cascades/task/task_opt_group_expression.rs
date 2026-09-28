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

// Cascades 组表达式（GroupExpression）及其优化任务。
//
// GroupExpression 把一条 LogicalExpression 挂到某个 Memo Group 上，并记录
// 已探索的规则集与 abandoned 状态。OptGroupExpressionTask 会：
// 1）为匹配且启用的规则压 ApplyRuleTask；
// 2）按 LIFO 调度器要求逆序压入子 Group 的 OptGroupTask，使第 0 个子组先执行。

use crate::{BaseTask, ContextRef, GroupExpressionRef, NewApplyRuleTask, NewOptGroupTask};
use cascades_base::Task;
use cascades_base::util::StrBufferWriter;
use cascades_pattern::GetOperand;

/// OptGroupExpressionTask：优化单个组表达式。
pub struct OptGroupExpressionTask {
    pub BaseTask: BaseTask,
    /// 待优化的组表达式。
    pub groupExpression: GroupExpressionRef,
}

/// 构造 OptGroupExpressionTask 并装箱为可调度 Task。
pub fn NewOptGroupExpressionTask(ctx: ContextRef, expression: GroupExpressionRef) -> Box<dyn Task> {
    Box::new(OptGroupExpressionTask {
        BaseTask: BaseTask::New(ctx),
        groupExpression: expression,
    })
}

impl OptGroupExpressionTask {
    /// 收集当前表达式上 Match 成功且已启用的规则列表。
    fn getValidRules(&self) -> Vec<crate::RuleRef> {
        let expression = self.groupExpression.borrow();
        let operand = GetOperand(expression.GetWrappedLogicalPlan());
        self.BaseTask
            .ctx
            .borrow()
            .RulesFor(operand)
            .into_iter()
            .filter(|rule| {
                rule.Pattern().Operand == operand
                    && self.BaseTask.ctx.borrow().RuleEnabled(rule.ID())
            })
            .collect()
    }
}

impl Task for OptGroupExpressionTask {
    fn Execute(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        // 先为每条有效规则压 ApplyRuleTask。
        for rule in self.getValidRules() {
            self.BaseTask.Push(NewApplyRuleTask(
                self.BaseTask.ctx.clone(),
                self.groupExpression.clone(),
                rule,
            ));
        }

        // Reverse push is required by the LIFO scheduler: child zero executes first.
        // 逆序压子组任务：LIFO 下最后压入的最先执行，从而保证第 0 个子组先跑。
        let inputs = self.groupExpression.borrow().Inputs.clone();
        for group in inputs.into_iter().rev() {
            self.BaseTask
                .Push(NewOptGroupTask(self.BaseTask.ctx.clone(), group));
        }
        Ok(())
    }

    fn Desc(&self, writer: &mut dyn StrBufferWriter) {
        writer.WriteString("OptGroupExpressionTask{ge:");
        writer.WriteString(&self.groupExpression.borrow().String());
        writer.WriteString("}");
    }
}

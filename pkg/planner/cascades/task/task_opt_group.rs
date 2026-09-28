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

// Cascades Memo 中的等价组（Group）及其优化任务。
//
// Group 是 Memo 里一组语义等价的逻辑计划表达式集合；OptGroupTask
// 负责对该组做探索（exploration）：为组内每个 GroupExpression 调度
// OptGroupExpressionTask，从而驱动规则应用与子组递归优化。

use crate::{BaseTask, ContextRef, NewOptGroupExpressionTask};
use cascades_base::Task;
use cascades_base::util::StrBufferWriter;
use cascades_memo::GroupRef;

/// OptGroupTask：对单个 Memo Group 发起探索优化。
pub struct OptGroupTask {
    pub BaseTask: BaseTask,
    /// 待优化的等价组。
    pub group: GroupRef,
}

/// 构造 OptGroupTask 并装箱为可调度 Task。
pub fn NewOptGroupTask(ctx: ContextRef, group: GroupRef) -> Box<dyn Task> {
    Box::new(OptGroupTask {
        BaseTask: BaseTask::New(ctx),
        group,
    })
}

impl Task for OptGroupTask {
    fn Execute(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        // 已探索过则幂等返回，避免重复压栈。
        if self.group.borrow().IsExplored() {
            return Ok(());
        }

        // Snapshot handles like Go's ForEachGE so task creation cannot invalidate traversal.
        // 先快照表达式句柄（对齐 Go ForEachGE），再压 OptGroupExpressionTask，避免遍历中改组。
        for expression in self.group.borrow().GetLogicalExpressions() {
            self.BaseTask.Push(NewOptGroupExpressionTask(
                self.BaseTask.ctx.clone(),
                expression,
            ));
        }
        self.group.borrow_mut().SetExplored();
        Ok(())
    }

    fn Desc(&self, writer: &mut dyn StrBufferWriter) {
        writer.WriteString("OptGroupTask{group:");
        writer.WriteString(&self.group.borrow().String());
        writer.WriteString("}");
    }
}

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

// Cascades Memo 优化使用的串行 LIFO 任务调度器。
//
// SimpleTaskScheduler 持有一个任务栈（Stack）：PushTask 压入，
// ExecuteTasks 循环弹出并执行，直到栈空。Destroy 归还/清空栈资源。
// LIFO（后进先出）决定了派生任务的执行顺序（见 OptGroupExpression 的逆序压栈）。

use crate::{Stack, takeTaskStack};
use cascades_base::{Scheduler, Task};

/// Serial LIFO scheduler used by cascades memo optimization.
/// Cascades Memo 优化使用的串行后进先出（LIFO）调度器。
pub struct SimpleTaskScheduler {
    /// 任务栈；Destroy 后变为 None，防止重复使用。
    stack: Option<Stack>,
}

impl Scheduler for SimpleTaskScheduler {
    /// 循环弹出并执行栈顶任务，直到栈空；任一任务失败则短路返回。
    fn ExecuteTasks(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let stack = self
            .stack
            .as_mut()
            .expect("ExecuteTasks called after scheduler destruction");
        while !stack.Empty() {
            // Empty was checked in the same exclusive borrow.
            // 同一独占借用内已确认非空，再 Pop 必有任务。
            let mut task = stack.Pop().expect("non-empty stack must pop a task");
            task.Execute()?;
        }
        Ok(())
    }

    /// 取出并销毁内部栈，释放可复用池资源。
    fn Destroy(&mut self) {
        if let Some(mut stack) = self.stack.take() {
            stack.Destroy();
        }
    }

    /// 将新任务压入栈顶，供后续 ExecuteTasks 弹出执行。
    fn PushTask(&mut self, task: Box<dyn Task>) {
        self.stack
            .as_mut()
            .expect("PushTask called after scheduler destruction")
            .Push(task);
    }
}

/// 从栈池取出 Stack，构造新的 SimpleTaskScheduler。
pub fn NewSimpleTaskScheduler() -> Box<dyn Scheduler> {
    Box::new(SimpleTaskScheduler {
        stack: Some(takeTaskStack()),
    })
}

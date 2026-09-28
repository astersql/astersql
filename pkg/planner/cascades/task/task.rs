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

// Cascades 优化任务栈与任务错误类型。
//
// 优化过程用 LIFO 栈存放待执行 Task；线程本地栈池（STACK_POOL）复用
// 已清空的栈以降低分配。Destroy 清空任务后归还池中；测试可故意不 Destroy
// 而直接 put 回脏栈以覆盖复用路径。

use cascades_base::Task;
use cascades_base::util::StrBufferWriter;
use std::cell::RefCell;
use std::error::Error;
use std::fmt;

/// 任务执行错误；载荷为可读消息。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskError(pub String);

impl TaskError {
    /// 由任意可转成 String 的消息构造错误。
    pub fn New(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for TaskError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for TaskError {}

// 线程本地任务栈对象池，避免频繁分配。
thread_local! {
    static STACK_POOL: RefCell<Vec<Stack>> = const { RefCell::new(Vec::new()) };
}

/// LIFO storage for optimizer tasks created before or during optimization.
/// 优化前/优化中创建的任务的 LIFO 存储。
pub struct Stack {
    /// 栈内任务（末尾为栈顶）。
    tasks: Vec<Box<dyn Task>>,
}

/// 构造默认容量（4）的新任务栈。
pub fn newTaskStack() -> Stack {
    Stack {
        tasks: Vec::with_capacity(4),
    }
}

/// 按指定容量构造任务栈。
pub fn newTaskStackWithCap(capacity: usize) -> Stack {
    Stack {
        tasks: Vec::with_capacity(capacity),
    }
}

/// 从线程本地池取出可复用栈；池空则新建。
pub(crate) fn takeTaskStack() -> Stack {
    STACK_POOL.with(|pool| pool.borrow_mut().pop().unwrap_or_else(newTaskStack))
}

/// putTaskStack 对应 Go `stackPool.Put`：测试里可故意不 Destroy、原样归还脏栈。
#[cfg(test)]
pub(crate) fn putTaskStack(stack: Stack) {
    STACK_POOL.with(|pool| pool.borrow_mut().push(stack));
}

impl Stack {
    /// 清空任务并将空栈归还线程本地池。
    pub fn Destroy(&mut self) {
        self.tasks.clear();
        let reusable = std::mem::replace(self, newTaskStack());
        STACK_POOL.with(|pool| pool.borrow_mut().push(reusable));
    }

    /// 将栈中各任务的描述写入缓冲区，任务之间换行。
    pub fn Desc(&self, writer: &mut dyn StrBufferWriter) {
        for task in &self.tasks {
            task.Desc(writer);
            writer.WriteString("\n");
        }
    }

    /// 当前栈中任务个数。
    pub fn Len(&self) -> usize {
        self.tasks.len()
    }

    /// Cap 对应 Go `cap(ts.tasks)`，供栈池复用测试断言初始/复用容量。
    pub fn Cap(&self) -> usize {
        self.tasks.capacity()
    }

    /// 弹出栈顶任务；空栈返回 None。
    pub fn Pop(&mut self) -> Option<Box<dyn Task>> {
        self.tasks.pop()
    }

    /// 压入任务到栈顶。
    pub fn Push(&mut self, task: Box<dyn Task>) {
        self.tasks.push(task);
    }

    /// 栈是否为空。
    pub fn Empty(&self) -> bool {
        self.Len() == 0
    }
}

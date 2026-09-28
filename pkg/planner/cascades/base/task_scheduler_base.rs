// Copyright 2024 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// Cascades 任务调度器（Scheduler）基础接口。
//
// 优化过程被拆成可入队的 Task；Scheduler 负责驱动执行循环，并可在任务运行中
// 继续接收派生任务。具体实现可以是单线程串行栈，也可以是多线程并发队列。

// 本文件由 pkg/planner/cascades/base/task_scheduler_base.go 迁移而来，保留 Go 接口方法顺序。

use std::error::Error;

/// Scheduler 对应 Go 的调度接口，可由单线程串行实现或多线程并发实现。
pub trait Scheduler {
    /// ExecuteTasks 启动实现内部的调度循环。
    // / Go 返回 `error`；用动态 Error 保留“成功或调度失败”的返回形状。
    #[allow(non_snake_case)]
    fn ExecuteTasks(&mut self) -> Result<(), Box<dyn Error>>;

    /// Destroy 释放调度器持有的队列、工作线程或其他资源（如果具体实现拥有这些资源）。
    #[allow(non_snake_case)]
    fn Destroy(&mut self);

    /// PushTask 是外部任务入队入口；正在运行的任务也可继续派生并压入后续任务。
    /// Box 表示 Go 接口值的动态分发与所有权转交，实际同步策略由具体调度器决定。
    #[allow(non_snake_case)]
    fn PushTask(&mut self, task: Box<dyn Task>);
}

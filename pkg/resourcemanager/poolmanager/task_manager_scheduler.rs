// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 任务管理器对外的调度入口：Overclock / Downclock。
//
// 封装迭代器中的 `getBoostTask` / `pauseTask`，供资源管理器在 CPU 调度决策后
// 提升或降低具体任务的并发度。

impl TaskManager {
    /// 选出可加速的任务，返回 (taskID, Meta)；无候选时 Meta 为 None。
    pub fn Overclock(&self) -> (u64, Option<Meta>) {
        self.getBoostTask()
    }

    /// Rust 风格：仅返回可加速任务的 Meta。
    pub fn overclock(&self) -> Option<Meta> {
        self.Overclock().1
    }

    /// 对适合降并发的任务发送暂停信号（Downclock）。
    pub fn Downclock(&self) {
        self.pauseTask();
    }

    /// Rust 风格 Downclock 别名。
    pub fn downclock(&self) {
        self.Downclock();
    }
}

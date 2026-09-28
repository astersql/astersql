// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//	 http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 调度器抽象：调容命令枚举与 Scheduler trait。
//
// 对应 Go 的 scheduler 接口；各具体调度器（如 CPUScheduler）实现 `Tune`，
// 根据组件类型与 goroutine 池状态返回 Downclock / Hold / Overclock。

#![allow(non_snake_case)]

use crate::util;

// Command 是调度器返回的并发度调整命令。
/// 调度器返回的并发度调整命令。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    // Downclock 表示减少并发数。
    /// 减少池并发数。
    Downclock,
    // Hold 表示保持并发数不变。
    /// 保持并发数不变。
    Hold,
    // Overclock 表示增加并发数。
    /// 增加池并发数。
    Overclock,
}

// Scheduler 对应 Go interface；Tune 根据组件和 goroutine 池状态给出命令。
/// 调度器接口：按组件与池状态给出调容命令。
pub trait Scheduler {
    /// 根据组件类型与池当前状态返回调容命令。
    fn Tune(&self, component: util::Component, pool: &dyn util::GoroutinePool) -> Command;
}

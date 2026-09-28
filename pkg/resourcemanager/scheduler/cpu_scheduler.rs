// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 基于 CPU 使用率的调度器实现。
//
// 对应 Go 的 CPUScheduler：在距上次调容超过最小间隔后采样 CPU；
// 使用率 < 0.5 升容，> 0.7 降容，否则保持；采样不支持时返回 Hold。

#![allow(non_snake_case)]

use super::scheduler::{Command, Scheduler};
use crate::{cpu, util};

// CPUScheduler 对应 Go 的 CPU 调度器空结构体。
/// 基于 CPU 使用率决策的调度器（无额外状态）。
pub struct CPUScheduler;

// NewCPUScheduler 创建 CPU 调度器，保留 Go 构造函数返回指针的语义。
/// 创建 CPU 调度器实例。
pub fn NewCPUScheduler() -> CPUScheduler {
    CPUScheduler
}

/// Converts the sampled CPU state to the same command as the Go scheduler.
/// 将采样到的 CPU 使用率映射为调度命令（阈值与 Go 一致）。
pub fn command_for_cpu_usage(value: f64, unsupported: bool) -> Command {
    if unsupported {
        return Command::Hold;
    }
    if value < 0.5 {
        return Command::Overclock;
    }
    if value > 0.7 {
        return Command::Downclock;
    }
    Command::Hold
}

impl CPUScheduler {
    // Tune 按最近调节时间和 CPU 使用率决定扩容、缩容或保持不变。
    /// 按最近调容时间与 CPU 使用率返回 Overclock/Downclock/Hold。
    pub fn Tune(&self, _component: util::Component, pool: &dyn util::GoroutinePool) -> Command {
        let since_last_tune = pool.LastTunerTs().elapsed().unwrap_or_default();
        // 未满最小调度间隔则直接保持，避免过于频繁采样与调容。
        if since_last_tune < util::MinSchedulerInterval.Load() {
            return Command::Hold;
        }
        let (value, unsupported) = cpu::GetCPUUsage();
        command_for_cpu_usage(value, unsupported)
    }
}

impl Scheduler for CPUScheduler {
    /// 将 trait 方法委托给 CPUScheduler::Tune。
    fn Tune(&self, component: util::Component, pool: &dyn util::GoroutinePool) -> Command {
        CPUScheduler::Tune(self, component, pool)
    }
}

/// Free-function compatibility wrapper retained for translated callers.
/// 自由函数包装，供机械翻译侧的调用方兼容使用。
pub fn Tune(
    scheduler: &CPUScheduler,
    component: util::Component,
    pool: &dyn util::GoroutinePool,
) -> Command {
    scheduler.Tune(component, pool)
}

// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

//! Resource concurrency limiter matching `br/pkg/backup/limit.go`.
//!
//! 这个文件实现备份阶段的资源并发限流器。
//! Go 版本用 `sync.Cond` 控制“当前资源占用”是否超过阈值，Rust 这里保持同样语义。
//! 它限制的是累计资源量，而不是线程数量。
//! 因此一次 `Acquire(200)` 可能让占用值超过阈值，只要进入临界区前当前值还没越线。
//! 这种行为会影响测试里允许出现的峰值，需要在注释里明确下来。

use std::sync::{Condvar, Mutex};

/// ResourceConcurrentLimiter mirrors Go `ResourceConcurrentLimiter`.
/// 记录当前已占用资源，并在达到阈值后阻塞后续申请者。
pub struct ResourceConcurrentLimiter {
    cond: Condvar,
    current: Mutex<isize>,
    threshold: isize,
}

/// NewResourceMemoryLimiter mirrors Go `NewResourceMemoryLimiter`.
/// 创建一个以“资源量”为单位的限流器，而不是以任务数限流。
pub fn NewResourceMemoryLimiter(threshold: isize) -> ResourceConcurrentLimiter {
    ResourceConcurrentLimiter {
        cond: Condvar::new(),
        current: Mutex::new(0),
        threshold,
    }
}

impl ResourceConcurrentLimiter {
    /// Acquire waits while `current >= threshold`, then adds `resource`.
    /// 这里先检查“进入前”的当前值，再把本次请求量整体累加进去。
    /// 这样可以保持和 Go 一致的宽松阈值语义，避免拆分单次大请求。
    pub fn Acquire(&self, resource: isize) -> isize {
        let mut current = self
            .current
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while *current >= self.threshold {
            current = self
                .cond
                .wait(current)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        *current += resource;
        *current
    }

    /// Release subtracts `resource` and wakes all waiters.
    /// 释放后广播唤醒全部等待者，让它们重新竞争当前额度。
    pub fn Release(&self, resource: isize) {
        let mut current = self
            .current
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *current -= resource;
        self.cond.notify_all();
    }
}

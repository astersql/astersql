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

// Hash Join 的 OOM spill（落盘）动作。
//
// 当内存 Tracker（内存配额跟踪器）超限时，优先将 spill helper 状态设为 `NeedSpill`，
// 并记录触发时的内存快照；若数据量不足或不允许 spill，则交给 fallback OOM action。
// Spill：把内存中的分区数据写出到磁盘以降低内存占用。

use crate::hash_join_spill_helper::{HashJoinSpillHelper, MemoryTracker, SpillStatus};
use std::sync::{Arc, Mutex};

/// Spill 触发时写入日志/诊断的提示文案。
pub const SPILL_INFO: &str = "memory exceeds quota, spill to disk now.";
/// 默认 OOM action 优先级（数值越小通常越先执行，具体语义与内存子系统约定一致）。
pub const DEFAULT_SPILL_PRIORITY: i64 = 2;

/// OOM（内存超限）动作接口：返回优先级并在 Tracker 超限时执行。
pub trait OomAction: Send + Sync {
    /// 动作优先级。
    fn priority(&self) -> i64;
    /// 对给定内存 Tracker 执行动作。
    fn action(&self, tracker: &MemoryTracker);
}

/// Hash Join 专用 spill OOM action：尝试置位 needSpill，否则回退 fallback。
pub struct HashJoinSpillAction {
    /// 与执行器共享的 spill 状态机与分区落盘辅助对象。
    spill_helper: Arc<HashJoinSpillHelper>,
    /// 无法 spill 时调用的回退动作（如直接报 OOM）。
    fallback: Option<Arc<dyn OomAction>>,
    /// 对齐 Go 在 cond 锁内完成等待、检查和置位，避免并发 action 重复成功。
    action_lock: Mutex<()>,
}

impl HashJoinSpillAction {
    /// 构造仅含 spill helper、无 fallback 的动作。
    pub fn new(spill_helper: Arc<HashJoinSpillHelper>) -> Self {
        Self {
            spill_helper,
            fallback: None,
            action_lock: Mutex::new(()),
        }
    }

    /// 链式设置 fallback OOM action。
    pub fn with_fallback(mut self, fallback: Arc<dyn OomAction>) -> Self {
        self.fallback = Some(fallback);
        self
    }

    /// 访问内部 spill helper。
    pub fn spill_helper(&self) -> &Arc<HashJoinSpillHelper> {
        &self.spill_helper
    }

    /// 触发 fallback action（若已配置）。
    pub fn trigger_fallback_action(&self, tracker: &MemoryTracker) {
        if let Some(fallback) = &self.fallback {
            fallback.action(tracker);
        }
    }

    /// 主逻辑：等待当前 spill 轮次结束，在未 spill 且数据足够时置 needSpill 并记录内存快照。
    pub fn action_impl(&self, tracker: &MemoryTracker) -> bool {
        let _action_guard = self.action_lock.lock().expect("spill action poisoned");
        // 正在 spill 时等待，避免并发重复置位。
        self.spill_helper.wait_while_spilling();
        if tracker.check_exceed()
            && self.spill_helper.status() == SpillStatus::NotSpilled
            && has_enough_data_to_spill(&self.spill_helper.memory_tracker, tracker)
            && self.spill_helper.can_spill()
        {
            // 切到 inSpilling 前记录器可能仍在跑，因此记录触发时刻的内存快照。
            self.spill_helper
                .set_need_spill(tracker.bytes_consumed(), tracker.bytes_limit());
            return true;
        }
        false
    }
}

impl OomAction for HashJoinSpillAction {
    fn priority(&self) -> i64 {
        DEFAULT_SPILL_PRIORITY
    }

    fn action(&self, tracker: &MemoryTracker) {
        // 成功置位 needSpill 则本轮由 spill 路径消化内存压力。
        if self.action_impl(tracker) {
            return;
        }
        // 仍超限且无法/不值得 spill 时交给 fallback。
        if tracker.check_exceed()
            && (!has_enough_data_to_spill(&self.spill_helper.memory_tracker, tracker)
                || !self.spill_helper.can_spill())
        {
            self.trigger_fallback_action(tracker);
        }
    }
}

/// 判断是否有足够数据值得 spill：hash join 已消费字节 ≥ 触发 Tracker 配额的 1/20。
pub fn has_enough_data_to_spill(
    hash_join_tracker: &MemoryTracker,
    passed_in_tracker: &MemoryTracker,
) -> bool {
    hash_join_tracker.bytes_consumed() >= passed_in_tracker.bytes_limit() / 20
}

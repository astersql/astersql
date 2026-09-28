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

//! stats 文件测试共用的全局配置保护器。
//!
//! 测试会临时缩小写文件与内联阈值以覆盖不同分支；本模块负责串行化这类修改，
//! 并在每次测试前后恢复 Go 兼容的默认值，避免并发测试或 panic 污染后续用例。

use std::sync::atomic::Ordering;
use std::sync::{Mutex, MutexGuard};

use crate::statsfile::{inlineSize, maxStatsJsonTableSize};

const DEFAULT_MAX_STATS_JSON_TABLE_SIZE: usize = 32 * 1024 * 1024;
const DEFAULT_INLINE_SIZE: usize = 8 * 1024;

// 两个阈值属于包级共享状态，同一时刻只能有一个测试改写它们。
static STATS_CONFIG_TEST_LOCK: Mutex<()> = Mutex::new(());

/// 独占 stats 阈值配置的测试守卫。
///
/// 持有互斥锁期间，其他使用该守卫的测试无法改写阈值；离开作用域时即使因断言失败
/// 发生栈展开，也会通过 `Drop` 恢复默认值。
pub(crate) struct StatsConfigTestGuard {
    _guard: MutexGuard<'static, ()>,
}

impl StatsConfigTestGuard {
    /// 获取全局测试锁，并先清理可能由此前失败用例遗留的阈值。
    pub(crate) fn acquire() -> Self {
        let guard = STATS_CONFIG_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        maxStatsJsonTableSize.store(DEFAULT_MAX_STATS_JSON_TABLE_SIZE, Ordering::SeqCst);
        inlineSize.store(DEFAULT_INLINE_SIZE, Ordering::SeqCst);
        Self { _guard: guard }
    }
}

impl Drop for StatsConfigTestGuard {
    /// 在释放锁之前恢复默认阈值，确保下一个测试看到稳定的初始配置。
    fn drop(&mut self) {
        maxStatsJsonTableSize.store(DEFAULT_MAX_STATS_JSON_TABLE_SIZE, Ordering::SeqCst);
        inlineSize.store(DEFAULT_INLINE_SIZE, Ordering::SeqCst);
    }
}

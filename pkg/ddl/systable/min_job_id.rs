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

// 最小 DDL job_id 后台刷新器。
//
// 周期性查询系统表中仍存在的最小 job_id，并用 `fetch_max` 单调推进缓存值。
// 该值常用于判断历史作业是否已全部完成，以及是否存在后续 FLASHBACK 等作业。

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use crate::{Context, Manager};

/// 刷新间隔：默认每 10 秒查询一次最小 job_id。
pub const REFRESH_INTERVAL: Duration = Duration::from_secs(10);

/// 可跨线程通知的取消令牌，供刷新循环优雅退出。
#[derive(Clone, Default)]
pub struct Cancellation {
    state: Arc<(Mutex<bool>, Condvar)>,
}

impl Cancellation {
    /// 置位取消标志并唤醒等待中的刷新循环。
    pub fn cancel(&self) {
        let (lock, changed) = &*self.state;
        *lock.lock().expect("cancellation mutex poisoned") = true;
        changed.notify_all();
    }

    /// 是否已收到取消信号。
    pub fn is_cancelled(&self) -> bool {
        *self.state.0.lock().expect("cancellation mutex poisoned")
    }

    /// 等待超时或取消；若已取消或等待期间被取消则返回 true。
    fn wait_timeout(&self, timeout: Duration) -> bool {
        let (lock, changed) = &*self.state;
        let cancelled = lock.lock().expect("cancellation mutex poisoned");
        if *cancelled {
            return true;
        }
        *changed
            .wait_timeout(cancelled, timeout)
            .expect("cancellation mutex poisoned")
            .0
    }
}

/// 持有 Manager 引用，缓存当前已知的最小 job_id，并暴露刷新循环。
pub struct MinJobIdRefresher {
    system_table_manager: Arc<dyn Manager>,
    current_min_job_id: AtomicI64,
    running: AtomicBool,
}

/// 构造初始最小 job_id 为 0 的刷新器。
pub fn new_min_job_id_refresher(manager: Arc<dyn Manager>) -> MinJobIdRefresher {
    MinJobIdRefresher {
        system_table_manager: manager,
        current_min_job_id: AtomicI64::new(0),
        running: AtomicBool::new(false),
    }
}

impl MinJobIdRefresher {
    /// 返回当前缓存的最小 job_id。
    pub fn current_min_job_id(&self) -> i64 {
        self.current_min_job_id.load(Ordering::Acquire)
    }

    /// Refreshes immediately, then waits for either cancellation or the next
    /// interval. Query failures are ignored so the following tick can recover.
    /// 立即刷新一次，随后按间隔等待；查询失败被忽略以便下轮恢复。
    pub fn start(&self, context: &Context, cancellation: &Cancellation) {
        self.running.store(true, Ordering::Release);
        loop {
            self.refresh(context);
            if cancellation.wait_timeout(REFRESH_INTERVAL) {
                break;
            }
        }
        self.running.store(false, Ordering::Release);
    }

    /// 向系统表查询最小 job_id，并用较大值更新缓存（永不回退）。
    pub fn refresh(&self, context: &Context) {
        let current = self.current_min_job_id.load(Ordering::Acquire);
        let Ok(next) = self.system_table_manager.get_min_job_id(context, current) else {
            return;
        };
        // fetch_max：作业清空返回 0 时保持原值，避免最小 id 回退。
        self.current_min_job_id.fetch_max(next, Ordering::AcqRel);
    }

    /// 刷新循环是否仍在运行。
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }
}

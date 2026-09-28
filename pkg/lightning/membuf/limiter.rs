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

// Lightning membuf 内存配额限制器（Limiter）。
//
// 用互斥锁与条件变量实现 FIFO 排队的 Acquire/Release：当已占用配额超过上限时，
// 后续获取请求阻塞；释放时按入队顺序唤醒可满足的等待者。TryAcquire 在已有排队者时失败，
// 避免后来者插队。对应 Go `membuf.Limiter` 的公平限流语义。

use std::{
    backtrace::Backtrace,
    collections::VecDeque,
    sync::{Arc, Condvar, Mutex},
};

/// ErrCannotAcquireMemory 对应非阻塞获取无法立即满足时返回的固定错误。
pub const ErrCannotAcquireMemory: &str = "cannot acquire memory from membuf limiter";

/// Waiter 对应 Go 中同一索引上的 waitNums 与 waitChs 项。
/// 把数量和唤醒信号绑定在一起，避免两个并行切片发生错位。
struct Waiter {
    n: usize,
    ready: Arc<(Mutex<bool>, Condvar)>,
}

/// LimiterState 是由 Go `mu` 保护的全部可变状态。
struct LimiterState {
    limit: usize,
    waiters: VecDeque<Waiter>,
}

/// Limiter 在已获取且未释放的配额超过上限时阻塞后续 Acquire。
pub struct Limiter {
    initLimit: usize,
    state: Mutex<LimiterState>,
}

/// NewLimiter 使用给定初始配额创建 Limiter。
pub fn NewLimiter(limit: usize) -> Arc<Limiter> {
    Arc::new(Limiter {
        initLimit: limit,
        state: Mutex::new(LimiterState {
            limit,
            waiters: VecDeque::new(),
        }),
    })
}

impl Limiter {
    /// Acquire 获取 n 个配额；当前余额不足时进入 FIFO 队列并阻塞，直到 Release 明确唤醒。
    pub fn Acquire(&self, n: usize) {
        let ready = {
            let mut state = self.state.lock().unwrap();
            if state.limit >= n {
                state.limit -= n;
                return;
            }

            // 每个 Condvar 对应 Go 为等待者创建的无缓冲 waitCh；入队顺序决定释放时的公平性。
            let ready = Arc::new((Mutex::new(false), Condvar::new()));
            state.waiters.push_back(Waiter {
                n,
                ready: Arc::clone(&ready),
            });
            ready
        };

        let (flag, condvar) = &*ready;
        let mut awakened = flag.lock().unwrap();
        while !*awakened {
            // 用循环抵御 Condvar 的伪唤醒；Go 的 `<-waitCh` 只在 channel 被关闭后继续。
            awakened = condvar.wait(awakened).unwrap();
        }
    }

    /// TryAcquire 不阻塞。只要已有排队者或余额不足就失败，避免后来者越过 FIFO 队首。
    pub fn TryAcquire(&self, n: usize) -> bool {
        let mut state = self.state.lock().unwrap();
        if !state.waiters.is_empty() || state.limit < n {
            return false;
        }
        state.limit -= n;
        true
    }

    /// Release 归还 n 个配额，并从队首开始连续唤醒所有当前可满足的等待者。
    pub fn Release(&self, n: usize) {
        let mut ready_to_wake = Vec::new();
        {
            let mut state = self.state.lock().unwrap();
            state.limit += n;
            if state.limit > self.initLimit {
                // Go 使用 pingcap/log + zap 输出 limit、initLimit 和调用栈。
                log::error!(
                    "limit overflow; limit={}, initLimit={}, stack={}",
                    state.limit,
                    self.initLimit,
                    Backtrace::force_capture(),
                );
            }

            loop {
                let Some(waiter) = state.waiters.front() else {
                    break;
                };
                if state.limit < waiter.n {
                    // 严格 FIFO：即使后面的较小请求能满足，也不能越过当前队首。
                    break;
                }
                state.limit -= waiter.n;
                ready_to_wake.push(state.waiters.pop_front().unwrap().ready);
            }
        }

        // 先释放状态锁再通知，避免被唤醒线程立即争用同一把锁；配额已经在锁内扣除。
        for ready in ready_to_wake {
            let (flag, condvar) = &*ready;
            *flag.lock().unwrap() = true;
            condvar.notify_all();
        }
    }
}

#[cfg(test)]
#[path = "limiter_test.rs"]
mod limiter_test;

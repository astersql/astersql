// Copyright 2019-present PingCAP, Inc.
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

// 悲观锁等待队列：按 keyHash 管理 Waiter，支持唤醒、延迟唤醒与死锁通知。
//
// 对应 TiKV/unistore 中锁冲突时的等待语义：同一键上多个事务排队，
// 锁持有者提交后优先唤醒最早等待者（按 startTS），其余进入短暂延迟再重试；
// 死锁检测器发现环路时通过 DeadlockResponse 精准唤醒对应等待者。

use crate::config;
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, bounded};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Special pessimistic lock wait value; `-1` means no wait.
/// 特殊的悲观锁等待取值；`-1` 表示不等待（立即失败）。
pub const LockNoWait: i64 = -1;

/// The deadlock entry fields consumed by this package.
/// 死锁等待边：本事务 Txn 等待 WaitForTxn，落在 KeyHash 对应键上。
#[allow(non_snake_case)]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WaitForEntry {
    /// 等待方事务的 startTS。
    pub Txn: u64,
    /// 被等待方事务的 startTS（持锁者）。
    pub WaitForTxn: u64,
    /// 冲突键的哈希，用于定位等待队列。
    pub KeyHash: u64,
}

/// Deadlock notification delivered to a matching waiter.
/// 发给匹配等待者的死锁通知，携带等待边与触发环路的键哈希。
#[allow(non_snake_case)]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DeadlockResponse {
    /// 死锁等待边。
    pub Entry: WaitForEntry,
    /// 参与死锁环路的键哈希。
    pub DeadlockKeyHash: u64,
}

/// 单个键上的等待队列（多个 Waiter）。
#[derive(Default)]
struct Queue {
    waiters: Vec<Arc<Waiter>>,
}

impl Queue {
    /// 按 startTS 排序后取出最早等待者（FIFO 优先最早事务）。
    fn get_oldest_waiter(&mut self) -> Arc<Waiter> {
        self.waiters.sort_by_key(|waiter| waiter.startTS);
        self.waiters.remove(0)
    }

    /// 按 Arc 指针相等从队列中移除指定等待者。
    fn remove_waiter(&mut self, waiter: &Arc<Waiter>) {
        if let Some(index) = self
            .waiters
            .iter()
            .position(|current| Arc::ptr_eq(current, waiter))
        {
            self.waiters.remove(index);
        }
    }
}

/// Manager owns the per-key lock wait queues.
/// 按 keyHash 维护锁等待队列的管理器。
#[allow(non_snake_case)]
pub struct Manager {
    /// keyHash → 该键上的等待队列。
    waitingQueues: Mutex<HashMap<u64, Queue>>,
    /// 非最早等待者被唤醒后额外延迟（毫秒），来自悲观事务配置。
    wakeUpDelayDuration: i64,
}

/// Constructs a lock waiter manager from the unistore configuration.
/// 根据 unistore 配置构造锁等待管理器。
#[allow(non_snake_case)]
pub fn NewManager(conf: &config::Config) -> Manager {
    Manager {
        waitingQueues: Mutex::new(HashMap::new()),
        wakeUpDelayDuration: conf.PessimisticTxn.WakeUpDelayDuration,
    }
}

/// 唤醒等待时间语义：超时 / 立即本等待者 / 延迟后再试。
pub type WakeupWaitTime = i32;

/// 等待超时（未收到正常唤醒）。
pub const WaitTimeout: WakeupWaitTime = -1;
/// 本等待者被立即唤醒（最早等待者或死锁匹配）。
pub const WakeUpThisWaiter: WakeupWaitTime = 0;
/// 延迟唤醒：需再等一小段时间后重试获取锁。
pub const WakeupDelayTimeout: WakeupWaitTime = 1;

/// Result returned by [`Waiter::Wait`].
/// Wait 的返回结果：可能含死锁响应、唤醒类型与提交时间戳。
#[allow(non_snake_case)]
#[derive(Clone, Debug)]
pub struct WaitResult {
    /// 若因死锁被唤醒则非空。
    pub DeadlockResp: Option<Arc<DeadlockResponse>>,
    /// 唤醒/超时类型，见 WakeupWaitTime 常量。
    pub WakeupSleepTime: WakeupWaitTime,
    /// 锁持有者提交时间戳（commitTS）；超时或死锁时通常为 0。
    pub CommitTS: u64,
}

impl WaitResult {
    /// 构造超时结果；若处于延迟唤醒阶段则带上 commit_ts。
    fn timeout(wakeup_delayed: bool, commit_ts: u64) -> Self {
        Self {
            DeadlockResp: None,
            WakeupSleepTime: if wakeup_delayed {
                WakeupDelayTimeout
            } else {
                WaitTimeout
            },
            CommitTS: if wakeup_delayed { commit_ts } else { 0 },
        }
    }
}

/// A single pessimistic lock waiter.
/// 单个悲观锁等待者：通过 channel 接收唤醒，并带有截止时间。
#[allow(non_snake_case)]
pub struct Waiter {
    /// 原始等待截止时刻。
    deadlineTime: Instant,
    /// 接收唤醒结果的通道端。
    receiver: Receiver<WaitResult>,
    /// 管理器侧发送唤醒结果的通道端。
    sender: Sender<WaitResult>,
    /// 延迟唤醒时长（毫秒）。
    wakeUpDelayDuration: i64,
    /// 本事务 startTS，用于排队与死锁匹配。
    startTS: u64,
    /// 持锁事务的时间戳（被等待方）。
    pub LockTS: u64,
    /// 冲突键哈希。
    pub KeyHash: u64,
}

impl Waiter {
    /// Blocks until normal wake-up, deadlock notification, or timeout.
    /// 阻塞直到普通唤醒、死锁通知或超时。
    #[allow(non_snake_case)]
    pub fn Wait(&self) -> WaitResult {
        let mut active_deadline = self.deadlineTime;
        let mut commit_ts = 0;
        let mut wakeup_delayed = false;

        loop {
            let now = Instant::now();
            // 已超过当前有效截止时间：按是否处于延迟唤醒返回对应超时结果。
            if now >= active_deadline {
                return WaitResult::timeout(wakeup_delayed, commit_ts);
            }

            match self.receiver.recv_timeout(active_deadline - now) {
                // 延迟唤醒：记录 commitTS，并在原截止时间内缩短下一次等待窗口。
                Ok(result) if result.WakeupSleepTime == WakeupDelayTimeout => {
                    commit_ts = result.CommitTS;
                    wakeup_delayed = true;
                    let delay_ms = self.wakeUpDelayDuration.max(0) as u64;
                    let delayed_deadline = Instant::now() + Duration::from_millis(delay_ms);
                    // Go resets the timer only when the delay remains inside the original deadline.
                    // Go 仅在延迟截止仍早于原始 deadline 时重置定时器。
                    if delayed_deadline < self.deadlineTime {
                        active_deadline = delayed_deadline;
                    }
                }
                Ok(result) => return result,
                Err(RecvTimeoutError::Timeout) => {
                    return WaitResult::timeout(wakeup_delayed, commit_ts);
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return WaitResult::timeout(wakeup_delayed, commit_ts);
                }
            }
        }
    }

    /// Drains wake-up notifications already queued for this waiter.
    /// 清空本等待者通道中已排队的唤醒通知，避免残留干扰后续逻辑。
    #[allow(non_snake_case)]
    pub fn DrainCh(&self) {
        while self.receiver.try_recv().is_ok() {}
    }
}

impl Manager {
    /// Registers a waiter under `keyHash`.
    /// 在 keyHash 对应队列中注册新等待者并返回句柄。
    #[allow(non_snake_case)]
    pub fn NewWaiter(
        &self,
        startTS: u64,
        lockTS: u64,
        keyHash: u64,
        timeout: Duration,
    ) -> Arc<Waiter> {
        // Allocate before locking, matching the Go critical-section behavior.
        // 先分配 channel/Waiter 再加锁，与 Go 临界区行为一致，缩短持锁时间。
        let (sender, receiver) = bounded(32);
        let waiter = Arc::new(Waiter {
            deadlineTime: Instant::now() + timeout,
            receiver,
            sender,
            wakeUpDelayDuration: self.wakeUpDelayDuration,
            startTS,
            LockTS: lockTS,
            KeyHash: keyHash,
        });

        self.waitingQueues
            .lock()
            .unwrap()
            .entry(keyHash)
            .or_default()
            .waiters
            .push(waiter.clone());
        waiter
    }

    /// Wakes the oldest waiter for each key and delay-notifies the remaining waiters.
    /// 对每个键唤醒最早等待者，并向剩余等待者发送延迟唤醒通知。
    #[allow(non_snake_case)]
    pub fn WakeUp(&self, _txn: u64, commitTS: u64, keyHashes: &[u64]) {
        // 在锁内收集需立即唤醒与延迟唤醒的等待者，锁外再发消息，避免阻塞。
        let (waiters, delayed_waiters) = {
            let mut queues = self.waitingQueues.lock().unwrap();
            let mut waiters = Vec::with_capacity(keyHashes.len());
            let mut delayed_waiters = Vec::new();

            for key_hash in keyHashes {
                let mut remove_queue = false;
                if let Some(queue) = queues.get_mut(key_hash) {
                    waiters.push(queue.get_oldest_waiter());
                    if queue.waiters.is_empty() {
                        remove_queue = true;
                    } else {
                        delayed_waiters.extend(queue.waiters.iter().cloned());
                    }
                }
                if remove_queue {
                    queues.remove(key_hash);
                }
            }
            (waiters, delayed_waiters)
        };

        for waiter in waiters {
            let _ = waiter.sender.try_send(WaitResult {
                DeadlockResp: None,
                WakeupSleepTime: WakeUpThisWaiter,
                CommitTS: commitTS,
            });
        }
        for waiter in delayed_waiters {
            let _ = waiter.sender.try_send(WaitResult {
                DeadlockResp: None,
                WakeupSleepTime: WakeupDelayTimeout,
                CommitTS: commitTS,
            });
        }
    }

    /// Removes a timed-out or cancelled waiter and drains pending notifications.
    /// 移除超时或取消的等待者，并排空其通道中的待处理通知。
    #[allow(non_snake_case)]
    pub fn CleanUp(&self, waiter: &Arc<Waiter>) {
        let mut queues = self.waitingQueues.lock().unwrap();
        let mut remove_queue = false;
        if let Some(queue) = queues.get_mut(&waiter.KeyHash) {
            queue.remove_waiter(waiter);
            remove_queue = queue.waiters.is_empty();
        }
        if remove_queue {
            queues.remove(&waiter.KeyHash);
        }
        drop(queues);
        waiter.DrainCh();
    }

    /// Removes and wakes the waiter identified by a deadlock response.
    /// 按死锁响应定位并移除匹配等待者，再向其发送死锁唤醒结果。
    #[allow(non_snake_case)]
    pub fn WakeUpForDeadlock(&self, response: Arc<DeadlockResponse>) {
        let entry = &response.Entry;
        let waiter = {
            let mut queues = self.waitingQueues.lock().unwrap();
            let mut matched = None;
            let mut remove_queue = false;
            if let Some(queue) = queues.get_mut(&entry.KeyHash) {
                // 以 startTS + KeyHash 匹配目标等待者。
                if let Some(index) = queue.waiters.iter().position(|waiter| {
                    waiter.startTS == entry.Txn && waiter.KeyHash == entry.KeyHash
                }) {
                    matched = Some(queue.waiters.remove(index));
                }
                remove_queue = queue.waiters.is_empty();
            }
            if remove_queue {
                queues.remove(&entry.KeyHash);
            }
            matched
        };

        if let Some(waiter) = waiter {
            let _ = waiter.sender.send(WaitResult {
                DeadlockResp: Some(response),
                WakeupSleepTime: WakeUpThisWaiter,
                CommitTS: 0,
            });
        }
    }

    /// 测试用：返回指定 keyHash 队列中的等待者数量。
    #[cfg(test)]
    pub(crate) fn waiter_count(&self, key_hash: u64) -> usize {
        self.waitingQueues
            .lock()
            .unwrap()
            .get(&key_hash)
            .map_or(0, |queue| queue.waiters.len())
    }
}

/// 锁等待管理器单元测试。
#[cfg(test)]
#[path = "lockwaiter_test.rs"]
mod lockwaiter_test;

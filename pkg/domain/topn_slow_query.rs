// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// Top-N 慢查询（slow query）收集与查询。
//
// 维护：FIFO 最近队列、用户慢查询小根堆、内部（internal）慢查询小根堆。
// 默认 N≈30、窗口约 7 天。供 `SHOW SLOW` 等按最近/Top/内部/全部查询。
// 慢查询：执行耗时超过阈值的语句记录。
//
// 文件前半为贴近 Go 的机械翻译草稿（块注释内），后半为线程安全的 `TopNSlowQueries`。

// use std::cmp::Ordering;
// use std::time::{Duration, SystemTime};
//
// slowQueryHeap 对应 Go 里的 heap.Interface 实现；data 按 Duration 维护小根堆语义。
// pub struct slowQueryHeap {
//     pub data: Vec<SlowQueryInfo>,
// }
//
// impl slowQueryHeap {
//     pub fn Len(&self) -> usize {
//         self.data.len()
//     }
//
//     pub fn Less(&self, i: usize, j: usize) -> bool {
//         self.data[i].Duration < self.data[j].Duration
//     }
//
//     pub fn Swap(&mut self, i: usize, j: usize) {
//         self.data.swap(i, j);
//     }
//
// Push 对应 Go 的 heap.Push 回调，把 any 断言为 *SlowQueryInfo 后追加。
//     pub fn Push(&mut self, x: SlowQueryInfo) {
//         self.data.push(x);
//         self.rebuild_heap();
//     }
//
// Pop 对应 Go 的 heap.Pop 回调：弹出底层 slice 最后一项。
//     pub fn Pop(&mut self) -> Option<SlowQueryInfo> {
//         self.data.pop()
//     }
//
//     pub fn RemoveExpired(&mut self, now: SystemTime, period: Duration) {
// Remove outdated slow query element.
//         let old_len = self.data.len();
//         let mut retained = Vec::with_capacity(self.data.len());
//         for info in self.data.drain(..) {
//             let outdateTime = info.Start + period;
//             if outdateTime > now {
//                 retained.push(info);
//             }
//         }
//
//         if retained.len() == old_len {
//             self.data = retained;
//             return;
//         }
//
// Rebuild the heap.
//         self.data = retained;
//         self.rebuild_heap();
//     }
//
//     pub fn Query(&mut self, count: usize) -> Vec<SlowQueryInfo> {
// The sorted array still maintains the heap property.
//         self.data.sort_by(compare_duration);
//
// The result should be in decrease order.
//         takeLastN(&self.data, count)
//     }
//
//     fn rebuild_heap(&mut self) {
// Go 使用 heap.Init；这里排序成 Duration 升序，保留堆顶为最短慢查询的语义。
//         self.data.sort_by(compare_duration);
//     }
// }
//
// slowQueryQueue 对应 Go 的定长 FIFO recent queue。
// pub struct slowQueryQueue {
//     pub data: Vec<SlowQueryInfo>,
//     pub size: usize,
// }
//
// impl slowQueryQueue {
//     pub fn Enqueue(&mut self, info: SlowQueryInfo) {
//         if self.data.len() < self.size {
//             self.data.push(info);
//             return;
//         }
//
// Go 使用 append(q.data, info)[1:] 丢弃最旧元素；这里显式 remove(0) 保留同样 FIFO 语义。
//         self.data.push(info);
//         if !self.data.is_empty() {
//             self.data.remove(0);
//         }
//     }
//
//     pub fn Query(&self, count: usize) -> Vec<SlowQueryInfo> {
// Queue is empty.
//         if self.data.is_empty() {
//             return Vec::new();
//         }
//         takeLastN(&self.data, count)
//     }
// }
//
// pub fn takeLastN(data: &[SlowQueryInfo], mut count: usize) -> Vec<SlowQueryInfo> {
//     if count > data.len() {
//         count = data.len();
//     }
//     let mut ret = Vec::with_capacity(count);
//     for info in data.iter().rev() {
//         if ret.len() >= count {
//             break;
//         }
//         ret.push(info.clone());
//     }
//     ret
// }
//
// topNSlowQueries maintains two heaps to store recent slow queries: one for user's and one for internal.
// N = 30, period = 7 days by default.
// It also maintains a recent queue, in a FIFO manner.
// topNSlowQueries 对应 Go 结构体：recent 保存最近队列，user/internal 分别保存用户和内部慢查询 topN。
// pub struct topNSlowQueries {
//     pub recent: slowQueryQueue,
//     pub user: slowQueryHeap,
//     pub internal: slowQueryHeap,
//     pub topN: usize,
//     pub period: Duration,
//     pub ch: Vec<SlowQueryInfo>,
//     pub msgCh: Vec<showSlowMessage>,
//     pub mu: topNSlowQueriesMu,
// }
//
// Go 的匿名 mu 结构体嵌入 sync.RWMutex 并携带 closed 标记；这里只保留 closed 状态。
// pub struct topNSlowQueriesMu {
//     pub closed: bool,
// }
//
// pub fn newTopNSlowQueries(topN: usize, period: Duration, queueSize: usize) -> topNSlowQueries {
//     let mut ret = topNSlowQueries {
//         topN,
//         period,
//         ch: Vec::with_capacity(1000),
//         msgCh: Vec::with_capacity(10),
//         recent: slowQueryQueue {
//             size: queueSize,
//             data: Vec::with_capacity(queueSize),
//         },
//         user: slowQueryHeap {
//             data: Vec::with_capacity(topN),
//         },
//         internal: slowQueryHeap {
//             data: Vec::with_capacity(topN),
//         },
//         mu: topNSlowQueriesMu { closed: false },
//     };
// Go 在构造后显式初始化三个 slice 容量；上面的字段初始化已按相同容量完成。
//     ret
// }
//
// impl topNSlowQueries {
//     pub fn Append(&mut self, info: SlowQueryInfo) {
// Put into the recent queue.
//         self.recent.Enqueue(info.clone());
//
//         let h = if info.Internal {
//             &mut self.internal
//         } else {
//             &mut self.user
//         };
//
// Heap is not full.
//         if h.data.len() < self.topN {
//             h.Push(info);
//             return;
//         }
//
// Replace the heap top.
//         if !h.data.is_empty() && info.Duration > h.data[0].Duration {
//             h.Pop();
//             h.Push(info);
//         }
//     }
//
//     pub fn QueryAll(&self) -> Vec<SlowQueryInfo> {
//         self.recent.data.clone()
//     }
//
//     pub fn RemoveExpired(&mut self, now: SystemTime) {
//         self.user.RemoveExpired(now, self.period);
//         self.internal.RemoveExpired(now, self.period);
//     }
// }
//
// pub struct showSlowMessage {
//     pub request: ShowSlow,
//     pub result: Vec<SlowQueryInfo>,
//     pub WaitGroup: WaitGroup,
// }
//
// impl topNSlowQueries {
//     pub fn QueryRecent(&self, count: usize) -> Vec<SlowQueryInfo> {
//         self.recent.Query(count)
//     }
//
//     pub fn QueryTop(&mut self, count: usize, kind: ShowSlowKind) -> Vec<SlowQueryInfo> {
//         let ret = match kind {
//             ShowSlowKind::Default => self.user.Query(count),
//             ShowSlowKind::Internal => self.internal.Query(count),
//             ShowSlowKind::All => {
//                 let mut tmp = Vec::with_capacity(self.user.data.len() + self.internal.data.len());
//                 tmp.extend(self.user.data.iter().cloned());
//                 tmp.extend(self.internal.data.iter().cloned());
//                 tmp.sort_by(compare_duration);
//                 takeLastN(&tmp, count)
//             }
//         };
//         ret
//     }
//
//     pub fn Close(&mut self) {
//         self.mu.closed = true;
//
// Go close(q.ch) 会唤醒 channel 接收方；用清空 Vec 表示不再接收新的慢查询。
//         self.ch.clear();
//     }
// }
//
// SlowQueryInfo is a struct to record slow query info.
// SlowQueryInfo 按 Go 字段顺序记录慢查询信息。
// #[derive(Clone)]
// pub struct SlowQueryInfo {
//     pub SQL: String,
//     pub Start: SystemTime,
//     pub Duration: Duration,
//     pub Detail: ExecDetails,
//     pub ConnID: u64,
//     pub SessAlias: String,
//     pub TxnTS: u64,
//     pub User: String,
//     pub DB: String,
//     pub TableIDs: String,
//     pub IndexNames: String,
//     pub Digest: String,
//     pub Internal: bool,
//     pub Succ: bool,
// }
//
// #[derive(Clone)]
// pub struct ExecDetails;
//
// pub struct ShowSlow;
//
// pub struct WaitGroup;
//
// pub enum ShowSlowKind {
//     Default,
//     Internal,
//     All,
// }
//
// fn compare_duration(left: &SlowQueryInfo, right: &SlowQueryInfo) -> Ordering {
//     left.Duration.cmp(&right.Duration)
// }
// */
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::time::{Duration, SystemTime};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 执行明细：进程时间、等待、backoff（重试退避）与请求次数等。
pub struct ExecDetails {
    /// 实际处理耗时。
    pub process_time: Duration,
    /// 等待耗时（如锁/调度）。
    pub wait_time: Duration,
    /// 退避重试累计时间。
    pub backoff_time: Duration,
    /// 底层请求次数。
    pub request_count: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 一条慢查询记录（SQL、起止、用户/库表、digest、是否内部语句等）。
pub struct SlowQueryInfo {
    /// SQL 文本。
    pub sql: String,
    /// 开始时间（用于过期淘汰）。
    pub start: SystemTime,
    /// 执行时长（堆按此比较）。
    pub duration: Duration,
    /// 执行明细。
    pub detail: ExecDetails,
    /// 连接 ID。
    pub connection_id: u64,
    /// session 别名。
    pub session_alias: String,
    /// 事务时间戳（TxnTS）。
    pub transaction_ts: u64,
    /// 用户名。
    pub user: String,
    /// 当前库名。
    pub database: String,
    /// 涉及表 ID 列表（字符串形式）。
    pub table_ids: String,
    /// 涉及索引名。
    pub index_names: String,
    /// SQL digest（归一化指纹，用于聚合同类语句）。
    pub digest: String,
    /// 是否内部语句（系统后台 SQL）。
    pub internal: bool,
    /// 是否执行成功。
    pub success: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// `SHOW SLOW` 查询种类：默认用户 Top、内部 Top、或合并全部。
pub enum ShowSlowKind {
    /// 用户慢查询 Top-N。
    Default,
    /// 内部慢查询 Top-N。
    Internal,
    /// 用户与内部合并后按耗时取 Top。
    All,
}

#[derive(Debug, Default)]
/// 按 duration 维护的小根堆：堆顶为当前 Top-N 中最短的一条，便于替换。
struct SlowQueryHeap {
    data: Vec<Arc<SlowQueryInfo>>,
}

impl SlowQueryHeap {
    /// 未满则入堆；已满且新记录更慢则替换堆顶后重建。
    fn push_top_n(&mut self, info: Arc<SlowQueryInfo>, capacity: usize) {
        if capacity == 0 {
            return;
        }
        if self.data.len() < capacity {
            self.data.push(info);
            self.rebuild();
            return;
        }
        if self
            .data
            .first()
            .is_some_and(|shortest| info.duration > shortest.duration)
        {
            // 去掉最短项，再插入更长的新慢查询，保持 Top-N 为最慢的一批。
            self.data.swap_remove(0);
            self.data.push(info);
            self.rebuild();
        }
    }

    /// 按 duration 升序排序，使 first() 为最短。
    fn rebuild(&mut self) {
        self.data.sort_by_key(|info| info.duration);
    }

    /// 剔除 start+period ≤ now 的过期项后重建堆。
    fn remove_expired(&mut self, now: SystemTime, period: Duration) {
        self.data.retain(|info| {
            info.start
                .checked_add(period)
                .is_some_and(|expires| expires > now)
        });
        self.rebuild();
    }

    /// 取耗时最长的 count 条（从排序后的尾部倒序取）。
    fn query(&self, count: usize) -> Vec<Arc<SlowQueryInfo>> {
        take_last_n(&self.data, count)
    }
}

#[derive(Debug)]
/// 定长 FIFO 最近慢查询队列。
struct SlowQueryQueue {
    data: VecDeque<Arc<SlowQueryInfo>>,
    capacity: usize,
}

impl SlowQueryQueue {
    /// 构造指定容量的队列。
    fn new(capacity: usize) -> Self {
        Self {
            data: VecDeque::with_capacity(capacity),
            capacity,
        }
    }
    /// 满则弹出队首再追加，保持最近 capacity 条。
    fn enqueue(&mut self, info: Arc<SlowQueryInfo>) {
        if self.capacity == 0 {
            return;
        }
        if self.data.len() == self.capacity {
            self.data.pop_front();
        }
        self.data.push_back(info);
    }
    /// 从最新到最旧取 count 条。
    fn query(&self, count: usize) -> Vec<Arc<SlowQueryInfo>> {
        self.data.iter().rev().take(count).cloned().collect()
    }
}

/// 从切片尾部倒序取最多 count 个元素（堆已按 duration 升序时即取最慢的）。
fn take_last_n(data: &[Arc<SlowQueryInfo>], count: usize) -> Vec<Arc<SlowQueryInfo>> {
    data.iter().rev().take(count).cloned().collect()
}

#[derive(Debug)]
/// 受 RwLock 保护的可变状态：最近队列、用户/内部堆、关闭标记。
struct SlowQueryState {
    recent: SlowQueryQueue,
    user: SlowQueryHeap,
    internal: SlowQueryHeap,
    closed: bool,
}

/// 线程安全的 Top-N 慢查询容器（对齐 Go 单 owner channel worker：写串行、读可廉价克隆 Arc）。
/// Thread-safe counterpart of Go's single-owner channel worker. Mutations are
/// serialized while readers can cheaply clone Arc-backed query records.
pub struct TopNSlowQueries {
    top_n: usize,
    period: Duration,
    state: RwLock<SlowQueryState>,
    close_event: (Mutex<bool>, Condvar),
}

impl TopNSlowQueries {
    /// 创建收集器：top_n 堆容量、过期窗口 period、最近队列长度 queue_size。
    pub fn new(top_n: usize, period: Duration, queue_size: usize) -> Self {
        Self {
            top_n,
            period,
            state: RwLock::new(SlowQueryState {
                recent: SlowQueryQueue::new(queue_size),
                user: SlowQueryHeap::default(),
                internal: SlowQueryHeap::default(),
                closed: false,
            }),
            close_event: (Mutex::new(false), Condvar::new()),
        }
    }

    /// 追加一条慢查询；已 close 则返回 false（对齐已关闭的 Go channel）。
    /// Returns false after close, matching a stopped Go channel consumer.
    pub fn append(&self, info: SlowQueryInfo) -> bool {
        let mut state = self.state.write().expect("slow query state poisoned");
        if state.closed {
            return false;
        }
        let info = Arc::new(info);
        // 始终进入最近队列；再按 internal 标志写入对应 Top-N 堆。
        state.recent.enqueue(info.clone());
        if info.internal {
            state.internal.push_top_n(info, self.top_n);
        } else {
            state.user.push_top_n(info, self.top_n);
        }
        true
    }

    /// 返回最近队列全部记录（FIFO 顺序）。
    pub fn query_all(&self) -> Vec<Arc<SlowQueryInfo>> {
        self.state
            .read()
            .expect("slow query state poisoned")
            .recent
            .data
            .iter()
            .cloned()
            .collect()
    }

    /// 查询最近 count 条（新→旧）。
    pub fn query_recent(&self, count: usize) -> Vec<Arc<SlowQueryInfo>> {
        self.state
            .read()
            .expect("slow query state poisoned")
            .recent
            .query(count)
    }

    /// 按种类查询 Top count；All 时合并两堆再按耗时取最慢。
    pub fn query_top(&self, count: usize, kind: ShowSlowKind) -> Vec<Arc<SlowQueryInfo>> {
        let state = self.state.read().expect("slow query state poisoned");
        match kind {
            ShowSlowKind::Default => state.user.query(count),
            ShowSlowKind::Internal => state.internal.query(count),
            ShowSlowKind::All => {
                let mut all = state
                    .user
                    .data
                    .iter()
                    .chain(&state.internal.data)
                    .cloned()
                    .collect::<Vec<_>>();
                // 合并后升序，再 take_last_n 得到降序的最慢若干条。
                all.sort_by_key(|info| info.duration);
                take_last_n(&all, count)
            }
        }
    }

    /// 按 period 清理用户与内部堆中的过期记录。
    pub fn remove_expired(&self, now: SystemTime) {
        let mut state = self.state.write().expect("slow query state poisoned");
        state.user.remove_expired(now, self.period);
        state.internal.remove_expired(now, self.period);
    }

    /// 关闭收集器并唤醒等待 close 的线程。
    pub fn close(&self) {
        self.state
            .write()
            .expect("slow query state poisoned")
            .closed = true;
        *self
            .close_event
            .0
            .lock()
            .expect("slow query close event poisoned") = true;
        self.close_event.1.notify_all();
    }

    /// 是否已关闭。
    pub fn is_closed(&self) -> bool {
        self.state.read().expect("slow query state poisoned").closed
    }
}

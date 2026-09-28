// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
// Copyright 2019-present PingCAP, Inc.
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

// 基于等待图的死锁检测器。
//
// 维护 txn → 若干 Edge（指向其等待的事务）的邻接表；
// `detect` 时沿边 DFS，若回到源事务则判定死锁并返回等待链。
// 边带 TTL，并可在图过大时主动过期清理。

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 诊断上下文：记录锁键与资源组标签，便于排查死锁。
pub struct DiagnosticContext {
    /// 被等待的锁键。
    pub key: Vec<u8>,
    /// 资源组标签（Resource Control 相关）。
    pub resource_group_tag: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 一条等待边的对外表示。
pub struct WaitForEntry {
    /// 等待方事务 start_ts。
    pub txn: u64,
    /// 被等待方事务 start_ts。
    pub wait_for_txn: u64,
    /// 锁键哈希，用于区分同事务对不同键的等待。
    pub key_hash: u64,
    /// 原始锁键（诊断用）。
    pub key: Vec<u8>,
    /// 资源组标签。
    pub resource_group_tag: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 检测到死锁时的错误载荷。
pub struct DeadlockError {
    /// 闭环关键边上的 key hash。
    pub deadlock_key_hash: u64,
    /// 等待链：每项等待下一项，最终成环。
    pub wait_chain: Vec<WaitForEntry>,
}

#[derive(Clone, Debug)]
/// 邻接表中的一条出边。
struct Edge {
    /// 被等待的事务。
    txn: u64,
    /// 对应锁键哈希。
    key_hash: u64,
    /// 注册时间，用于 TTL 过期。
    registered: Instant,
    /// 诊断信息快照。
    diagnostic: DiagnosticContext,
}

/// Detector 可变状态（受 Mutex 保护）。
struct DetectorState {
    /// 等待图：txn → 其等待的边列表。
    wait_for: HashMap<u64, Vec<Edge>>,
    /// 当前边总数。
    total_size: u64,
    /// 上次主动过期清理的时间。
    last_active_expire: Instant,
}

/// 死锁检测器：TTL、紧急容量与过期间隔可配置。
pub struct Detector {
    /// 受锁保护的等待图状态。
    state: Mutex<DetectorState>,
    /// 单条边的存活时间。
    entry_ttl: Duration,
    /// 边数达到该阈值后才允许主动全图过期。
    urgent_size: u64,
    /// 两次主动过期之间的最小间隔。
    expire_interval: Duration,
}

/// Detector 公共与内部方法。
impl Detector {
    /// 创建空等待图的检测器。
    pub fn new(entry_ttl: Duration, urgent_size: u64, expire_interval: Duration) -> Self {
        Self {
            state: Mutex::new(DetectorState {
                wait_for: HashMap::new(),
                total_size: 0,
                last_active_expire: Instant::now(),
            }),
            entry_ttl,
            urgent_size,
            expire_interval,
        }
    }

    /// 尝试加入 source→wait_for 边；若已成环则返回死锁错误而不落边。
    /// 无环时登记新边并返回 None。
    pub fn detect(
        &self,
        source_txn: u64,
        wait_for_txn: u64,
        key_hash: u64,
        diagnostic: DiagnosticContext,
    ) -> Option<DeadlockError> {
        let mut state = self.state.lock().expect("deadlock detector lock poisoned");
        let now = Instant::now();
        // 图过大时先清理过期边，再做环检测。
        self.active_expire(&mut state, now);
        let mut visited = HashSet::new();
        // 成环：反转 DFS 收集的链，并追加当前触发边。
        if let Some(mut error) =
            self.do_detect(&mut state, now, source_txn, wait_for_txn, &mut visited)
        {
            error.wait_chain.reverse();
            error.wait_chain.push(WaitForEntry {
                txn: source_txn,
                wait_for_txn,
                key_hash,
                key: diagnostic.key,
                resource_group_tag: diagnostic.resource_group_tag,
            });
            Some(error)
        } else {
            // 无环：若尚无相同 (wait_for, key_hash) 边则追加。
            let edges = state.wait_for.entry(source_txn).or_default();
            if !edges
                .iter()
                .any(|edge| edge.txn == wait_for_txn && edge.key_hash == key_hash)
            {
                edges.push(Edge {
                    txn: wait_for_txn,
                    key_hash,
                    registered: now,
                    diagnostic,
                });
                state.total_size += 1;
            }
            None
        }
    }

    /// 从 current 出发 DFS，寻找回到 source 的环；沿途剔除过期边。
    fn do_detect(
        &self,
        state: &mut DetectorState,
        now: Instant,
        source: u64,
        current: u64,
        visited: &mut HashSet<u64>,
    ) -> Option<DeadlockError> {
        // 已访问节点剪枝，避免重复搜索。
        if !visited.insert(current) {
            return None;
        }
        let mut edges = state.wait_for.remove(&current).unwrap_or_default();
        let before = edges.len();
        // 惰性删除 current 上的过期出边。
        edges.retain(|edge| now.duration_since(edge.registered) <= self.entry_ttl);
        state.total_size -= (before - edges.len()) as u64;
        let snapshot = edges.clone();
        if !edges.is_empty() {
            state.wait_for.insert(current, edges);
        }
        for edge in snapshot {
            let entry = WaitForEntry {
                txn: current,
                wait_for_txn: edge.txn,
                key_hash: edge.key_hash,
                key: edge.diagnostic.key.clone(),
                resource_group_tag: edge.diagnostic.resource_group_tag.clone(),
            };
            // 回到源事务：找到死锁环。
            if edge.txn == source {
                return Some(DeadlockError {
                    deadlock_key_hash: edge.key_hash,
                    wait_chain: vec![entry],
                });
            }
            if let Some(mut error) = self.do_detect(state, now, source, edge.txn, visited) {
                error.wait_chain.push(entry);
                return Some(error);
            }
        }
        None
    }

    /// 移除 txn 的全部出边。
    pub fn clean_up(&self, txn: u64) {
        let mut state = self.state.lock().expect("deadlock detector lock poisoned");
        if let Some(edges) = state.wait_for.remove(&txn) {
            state.total_size -= edges.len() as u64;
        }
    }

    /// 移除 txn 上匹配 (wait_for_txn, key_hash) 的单条边。
    pub fn clean_up_wait_for(&self, txn: u64, wait_for_txn: u64, key_hash: u64) {
        let mut state = self.state.lock().expect("deadlock detector lock poisoned");
        let mut removed = 0;
        let mut empty = false;
        if let Some(edges) = state.wait_for.get_mut(&txn) {
            let old = edges.len();
            if let Some(position) = edges
                .iter()
                .position(|edge| edge.txn == wait_for_txn && edge.key_hash == key_hash)
            {
                edges.remove(position);
            }
            removed = old - edges.len();
            empty = edges.is_empty();
        }
        state.total_size -= removed as u64;
        if empty {
            state.wait_for.remove(&txn);
        }
    }

    /// 当边数紧急且距上次清理已超过间隔时，全图剔除过期边。
    fn active_expire(&self, state: &mut DetectorState, now: Instant) {
        if now.duration_since(state.last_active_expire) <= self.expire_interval
            || state.total_size < self.urgent_size
        {
            return;
        }
        let mut removed = 0;
        state.wait_for.retain(|_, edges| {
            let old = edges.len();
            edges.retain(|edge| now.duration_since(edge.registered) <= self.entry_ttl);
            removed += old - edges.len();
            !edges.is_empty()
        });
        state.total_size -= removed as u64;
        state.last_active_expire = now;
    }

    /// 当前边总数（测试与观测用）。
    pub fn edge_count(&self) -> u64 {
        self.state
            .lock()
            .expect("deadlock detector lock poisoned")
            .total_size
    }
}

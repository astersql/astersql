// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

//! 具体会话的事务运行时状态。
//!
//! 本模块集中维护事务可见数据与保存点快照、悲观行锁及等待图、死锁历史、
//! 事务观测信息和提交版本。进程级注册表均按域或会话所有者隔离，供同一运行时的
//! 控制、DML 与诊断路径共享。

use super::*;

#[cfg(test)]
#[path = "transaction_test.rs"]
mod tests;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
/// 行锁的全局键；域标识用于隔离不同运行时中相同的存储键。
pub(super) struct RuntimeRowLockKey {
    pub(super) domain_id: usize,
    pub(super) key: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 运行时支持的共享锁与排他锁模式。
pub(super) enum RuntimeRowLockMode {
    Shared,
    Exclusive,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 排队中的锁请求，记录事务所有者及其所需锁模式。
pub(super) struct RuntimeRowLockWaiter {
    pub(super) owner: u64,
    pub(super) mode: RuntimeRowLockMode,
}

#[derive(Default)]
/// 全局行锁表及单出边等待图。
///
/// `wait_for` 将等待者指向当前阻塞它的持有者，用于检测等待环；`connections`
/// 则关联锁所有者和会话连接，便于事务结束时统一清理。
pub(super) struct RuntimeRowLockState {
    pub(super) holders: HashMap<RuntimeRowLockKey, HashMap<u64, RuntimeRowLockMode>>,
    pub(super) wait_for: HashMap<u64, u64>,
    pub(super) waiters: HashMap<RuntimeRowLockKey, VecDeque<RuntimeRowLockWaiter>>,
    pub(super) connections: HashMap<u64, u64>,
}

#[derive(Clone)]
/// 保存点创建时的事务可见数据与已持有锁快照。
pub(super) struct RuntimeSavepoint {
    pub(super) name: String,
    pub(super) visible: BTreeMap<Vec<u8>, Vec<u8>>,
    pub(super) held_locks: HashSet<RuntimeRowLockKey>,
    pub(super) deferred_optimistic_constraint_errors: BTreeMap<Vec<u8>, String>,
    pub(super) pending_ttl_insert_rows: usize,
}

#[derive(Clone, Debug)]
/// 暴露给事务诊断视图的会话事务快照。
pub(super) struct RuntimeTxnInfo {
    pub(super) domain_id: usize,
    pub(super) start_ts: u64,
    pub(super) current_sql_digest: String,
    pub(super) state: String,
    pub(super) waiting_start_time: Option<Instant>,
    pub(super) mem_buffer_keys: u64,
    pub(super) mem_buffer_bytes: u64,
    pub(super) session_id: u64,
    pub(super) database: String,
    pub(super) all_sql_digests: Vec<String>,
}

pub(super) static NEXT_ROW_LOCK_OWNER: AtomicU64 = AtomicU64::new(1);
static NEXT_DEADLOCK_ID: AtomicU64 = AtomicU64::new(1);
/// Mirrors Go `PessimisticTxn.DeadlockHistoryCapacity`'s default event count.
const DEFAULT_DEADLOCK_HISTORY_CAPACITY: usize = 10;
/// 进程内共享的行锁状态；条件变量在释放锁或移除等待者后唤醒竞争者。
pub(super) static RUNTIME_ROW_LOCKS: LazyLock<(Mutex<RuntimeRowLockState>, Condvar)> =
    LazyLock::new(|| (Mutex::new(RuntimeRowLockState::default()), Condvar::new()));
/// 按锁所有者登记当前活跃事务的诊断信息。
pub(super) static RUNTIME_TXN_INFOS: LazyLock<Mutex<HashMap<u64, RuntimeTxnInfo>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
pub(super) static NEXT_RUNTIME_COMMIT_EPOCH: AtomicU64 = AtomicU64::new(1);
/// 每个运行时键最近一次提交的逻辑纪元，用于判断事务期间是否出现写冲突。
pub(super) static RUNTIME_KEY_COMMIT_EPOCHS: LazyLock<Mutex<HashMap<RuntimeRowLockKey, u64>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
pub(super) const DEFAULT_TXN_ENTRY_SIZE_LIMIT: usize = 6_291_456;
/// 按域覆盖默认事务单条写入大小上限。
pub(super) static RUNTIME_GLOBAL_TXN_ENTRY_SIZE_LIMITS: LazyLock<Mutex<HashMap<u64, usize>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
type RuntimePendingIndexKey = (u64, String, String);
/// 按域、库和表登记尚在构建但写路径必须维护的索引。
pub(super) static RUNTIME_PENDING_WRITE_INDEXES: LazyLock<
    Mutex<HashMap<RuntimePendingIndexKey, Vec<astersql_meta_model::IndexInfo>>>,
> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// 待写索引的作用域守卫，离开作用域时自动撤销登记。
pub(super) struct RuntimePendingWriteIndexGuard {
    key: RuntimePendingIndexKey,
    index_name: String,
}

impl RuntimePendingWriteIndexGuard {
    /// 使用规范化的库表名发布索引，使并发写入路径能够找到它。
    pub(super) fn register(
        domain: &Arc<Domain>,
        database: &str,
        table: &str,
        index: astersql_meta_model::IndexInfo,
    ) -> Self {
        let key = (
            runtime_domain_id(domain),
            database.to_ascii_lowercase(),
            table.to_ascii_lowercase(),
        );
        let index_name = index.Name.L.clone();
        RUNTIME_PENDING_WRITE_INDEXES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(key.clone())
            .or_default()
            .push(index);
        Self { key, index_name }
    }
}

impl Drop for RuntimePendingWriteIndexGuard {
    fn drop(&mut self) {
        let mut pending = RUNTIME_PENDING_WRITE_INDEXES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(indexes) = pending.get_mut(&self.key) {
            indexes.retain(|index| index.Name.L != self.index_name);
            if indexes.is_empty() {
                pending.remove(&self.key);
            }
        }
    }
}

/// 活跃事务诊断信息的作用域守卫。
pub(super) struct RuntimeTxnInfoGuard {
    pub(super) owner: u64,
}

impl Drop for RuntimeTxnInfoGuard {
    fn drop(&mut self) {
        RUNTIME_TXN_INFOS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.owner);
    }
}

/// 从当前持有者沿等待边前进，判断新增等待关系是否会回到等待者。
pub(super) fn runtime_lock_cycle(state: &RuntimeRowLockState, waiter: u64, holder: u64) -> bool {
    let mut current = holder;
    let mut visited = HashSet::new();
    while visited.insert(current) {
        if current == waiter {
            return true;
        }
        let Some(next) = state.wait_for.get(&current).copied() else {
            return false;
        };
        current = next;
    }
    false
}

/// 将一个等待环的每条边记录为同一死锁，并保留最近十个完整死锁事件。
pub(super) fn record_runtime_deadlock(state: &RuntimeRowLockState, waiter: u64) {
    let deadlock_id = NEXT_DEADLOCK_ID.fetch_add(1, Ordering::Relaxed);
    let mut history = RUNTIME_DEADLOCK_HISTORY
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut current = waiter;
    let mut visited = HashSet::new();
    while visited.insert(current) {
        let Some(holder) = state.wait_for.get(&current).copied() else {
            break;
        };
        history.push_back(RuntimeDeadlockRecord {
            deadlock_id,
            try_lock_trx_id: current,
            trx_holding_lock: holder,
        });
        current = holder;
        if current == waiter {
            break;
        }
    }
    let event_count = history
        .iter()
        .map(|record| record.deadlock_id)
        .collect::<HashSet<_>>()
        .len();
    if event_count > DEFAULT_DEADLOCK_HISTORY_CAPACITY {
        let oldest_id = history
            .front()
            .expect("deadlock history with events is non-empty")
            .deadlock_id;
        while history
            .front()
            .is_some_and(|record| record.deadlock_id == oldest_id)
        {
            history.pop_front();
        }
    }
}

/// 同时从等待图和指定锁的等待队列中移除所有者，保持两处状态一致。
pub(super) fn remove_runtime_waiter(
    state: &mut RuntimeRowLockState,
    key: &RuntimeRowLockKey,
    owner: u64,
) {
    state.wait_for.remove(&owner);
    if let Some(queue) = state.waiters.get_mut(key) {
        queue.retain(|candidate| candidate.owner != owner);
        if queue.is_empty() {
            state.waiters.remove(key);
        }
    }
}

/// 释放所有者持有的指定锁或全部锁，并唤醒等待者重新竞争。
///
/// 全量释放还会解除所有者与连接的关联；保存点回滚传入键集合时保留该关联。
pub(super) fn release_runtime_row_locks(owner: u64, keys: Option<&HashSet<RuntimeRowLockKey>>) {
    let (mutex, available) = &*RUNTIME_ROW_LOCKS;
    let mut state = mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for (key, holders) in &mut state.holders {
        if keys.is_none_or(|keys| keys.contains(key)) {
            holders.remove(&owner);
        }
    }
    state.holders.retain(|_, holders| !holders.is_empty());
    state.wait_for.remove(&owner);
    for queue in state.waiters.values_mut() {
        queue.retain(|candidate| candidate.owner != owner);
    }
    state.waiters.retain(|_, queue| !queue.is_empty());
    if keys.is_none() {
        state.connections.remove(&owner);
    }
    available.notify_all();
}

/// 从存储标签提取事务作用域；缺少区域标签时按全局作用域处理。
pub(super) fn runtime_txn_scope(labels: &HashMap<String, String>) -> String {
    labels
        .get("zone")
        .cloned()
        .unwrap_or_else(|| "global".to_owned())
}

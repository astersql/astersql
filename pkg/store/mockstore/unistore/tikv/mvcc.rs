// Copyright 2026 AsterSQL.
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

// 内存版 MVCC 存储：两阶段提交、悲观锁、读校验与 GC。
//
// 对应 unistore mock TiKV 的核心键值引擎。每个键维护可选锁（Lock）、
// 按 commit_ts 索引的写记录（Write）以及长值 default 映射；提供预写/提交/
// 回滚、悲观锁、事务心跳、CheckTxnStatus、点查/扫描与按 safe point 回收。

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

/// 系统最大时间戳；用作“无穷大”水位。
pub const MAX_SYSTEM_TS: u64 = u64::MAX;
/// 短值阈值；超过则写入 defaults 映射。
pub const SHORT_VALUE_MAX_LEN: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 变更操作类型（Put/Delete/Lock/Insert/检查不存在/悲观锁）。
pub enum MutationOp {
    Put,
    Delete,
    Lock,
    Insert,
    CheckNotExists,
    PessimisticLock,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 单次变更：操作、键值及是否伴随悲观锁。
pub struct Mutation {
    pub op: MutationOp,
    pub key: Vec<u8>,
    pub value: Vec<u8>,
    pub is_pessimistic_lock: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 已提交写的种类。
pub enum WriteKind {
    Put,
    Delete,
    Lock,
    Rollback,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 未提交事务持有的锁：主键、TTL、异步提交次键等。
pub struct Lock {
    pub primary: Vec<u8>,
    pub start_ts: u64,
    pub ttl: u64,
    pub op: MutationOp,
    pub value: Vec<u8>,
    pub for_update_ts: u64,
    pub min_commit_ts: u64,
    pub use_async_commit: bool,
    pub secondaries: Vec<Vec<u8>>,
    pub rollback_ts: Vec<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 已提交（或回滚）的写记录。
pub struct Write {
    pub start_ts: u64,
    pub commit_ts: u64,
    pub kind: WriteKind,
    pub value: Vec<u8>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 单键状态：当前锁、按 commit_ts 的写历史与长值。
pub struct KeyState {
    pub lock: Option<Lock>,
    pub writes: BTreeMap<u64, Write>,
    pub defaults: BTreeMap<u64, Vec<u8>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// MVCC 操作错误（锁冲突、写冲突、已提交、GC 过早等）。
pub enum MvccError {
    Deadlock {
        lock_key: Vec<u8>,
        lock_ts: u64,
        deadlock_key_hash: u64,
    },
    KeyLocked {
        key: Vec<u8>,
        lock: Lock,
    },
    WriteConflict {
        key: Vec<u8>,
        start_ts: u64,
        conflict_start_ts: u64,
        conflict_commit_ts: u64,
    },
    AlreadyExists(Vec<u8>),
    PrimaryMismatch {
        key: Vec<u8>,
        primary: Vec<u8>,
    },
    PessimisticLockNotFound(Vec<u8>),
    TxnNotFound {
        key: Vec<u8>,
        start_ts: u64,
    },
    AlreadyCommitted(u64),
    CommitTsExpired {
        key: Vec<u8>,
        start_ts: u64,
        attempted: u64,
        min_commit_ts: u64,
    },
    InvalidRequest(String),
    GcTooEarly {
        safe_point: u64,
        txn_safe_point: u64,
    },
}

impl fmt::Display for MvccError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for MvccError {}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 预写（两阶段提交第一阶段）请求。
pub struct PrewriteRequest {
    pub mutations: Vec<Mutation>,
    pub primary_lock: Vec<u8>,
    pub start_ts: u64,
    pub lock_ttl: u64,
    pub for_update_ts: u64,
    pub min_commit_ts: u64,
    pub max_commit_ts: u64,
    pub use_async_commit: bool,
    pub secondaries: Vec<Vec<u8>>,
    pub try_one_pc: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 预写结果：min_commit_ts 与可选的一阶段提交时间戳。
pub struct PrewriteResult {
    pub min_commit_ts: u64,
    pub one_pc_commit_ts: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 悲观锁请求。
pub struct PessimisticLockRequest {
    pub mutations: Vec<Mutation>,
    pub primary_lock: Vec<u8>,
    pub start_ts: u64,
    pub for_update_ts: u64,
    pub lock_ttl: u64,
    pub return_values: bool,
    pub check_existence: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 悲观锁结果：可选返回旧值与存在性。
pub struct PessimisticLockResult {
    pub values: Vec<Option<Vec<u8>>>,
    pub existence: Vec<bool>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// CheckTxnStatus 采取的动作（回滚或推高 min_commit_ts）。
pub enum Action {
    NoAction,
    TtlExpireRollback,
    LockNotExistRollback,
    MinCommitTsPushed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 事务状态查询结果。
pub struct TxnStatus {
    pub ttl: u64,
    pub commit_ts: u64,
    pub action: Action,
    pub lock_info: Option<Lock>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 异步提交次键锁检查结果。
pub struct SecondaryLocksStatus {
    pub locks: Vec<Lock>,
    pub commit_ts: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 键与其锁（及锁内值）的配对。
pub struct LockPair {
    pub key: Vec<u8>,
    pub value: Vec<u8>,
    pub lock: Lock,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 键值对或读错误。
pub struct KvPair {
    pub key: Vec<u8>,
    pub value: Vec<u8>,
    pub error: Option<MvccError>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 单键完整 MVCC 调试视图。
pub struct MvccInfo {
    pub lock: Option<Lock>,
    pub writes: Vec<Write>,
    pub values: Vec<(u64, Vec<u8>)>,
}

#[derive(Default)]
/// 全库键状态表。
struct StoreState {
    keys: BTreeMap<Vec<u8>, KeyState>,
}

/// GC safe point：低于该水位的读视为过早。
pub struct SafePoint {
    ts: AtomicU64,
    changed: AtomicBool,
}
impl SafePoint {
    /// 以初始时间戳创建 safe point。
    pub fn new(ts: u64) -> Self {
        Self {
            ts: AtomicU64::new(ts),
            changed: AtomicBool::new(false),
        }
    }
    /// 更新 safe point 并标记已变更。
    pub fn update_ts(&self, ts: u64) {
        self.ts.store(ts, Ordering::Release);
        self.changed.store(true, Ordering::Release);
    }
    /// 读取当前 safe point。
    pub fn ts(&self) -> u64 {
        self.ts.load(Ordering::Acquire)
    }
    /// 取出并清除变更标志。
    pub fn take_changed(&self) -> bool {
        self.changed.swap(false, Ordering::AcqRel)
    }
}

/// 内存 MVCC 存储引擎。
pub struct MvccStore {
    state: RwLock<StoreState>,
    latest_ts: AtomicU64,
    safe_point: Arc<SafePoint>,
    closed: AtomicBool,
}

impl MvccStore {
    /// 创建空存储并绑定 safe point。
    pub fn new(safe_point: Arc<SafePoint>) -> Self {
        Self {
            state: RwLock::new(StoreState::default()),
            latest_ts: AtomicU64::new(0),
            safe_point,
            closed: AtomicBool::new(false),
        }
    }
    /// 标记存储关闭。
    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
    }
    /// 是否已关闭。
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }
    /// 返回观测到的最大时间戳。
    pub fn latest_ts(&self) -> u64 {
        self.latest_ts.load(Ordering::Acquire)
    }
    /// 原子推高 latest_ts。
    fn update_latest_ts(&self, ts: u64) {
        self.latest_ts.fetch_max(ts, Ordering::AcqRel);
    }

    /// 对变更集加悲观锁；冲突或写冲突则失败。
    pub fn pessimistic_lock(
        &self,
        request: &PessimisticLockRequest,
    ) -> Result<PessimisticLockResult, MvccError> {
        let mut state = self.state.write().expect("MVCC store lock poisoned");
        let mut result = PessimisticLockResult::default();
        let mut mutations = request.mutations.clone();
        // 按键排序以降低死锁风险并稳定加锁顺序。
        mutations.sort_by(|a, b| a.key.cmp(&b.key));

        // 先完成全部冲突检查和返回值收集；Go 只在整个请求构造完
        // WriteBatch 后写入，不能让后续冲突留下前序锁。
        let mut pending = Vec::with_capacity(mutations.len());
        for mutation in &mutations {
            let key_state = state.keys.entry(mutation.key.clone()).or_default();
            if let Some(lock) = &key_state.lock {
                if lock.start_ts != request.start_ts {
                    return Err(MvccError::KeyLocked {
                        key: mutation.key.clone(),
                        lock: lock.clone(),
                    });
                }
            }
            if let Some(write) = latest_non_rollback_write(key_state) {
                if write.commit_ts > request.for_update_ts {
                    return Err(MvccError::WriteConflict {
                        key: mutation.key.clone(),
                        start_ts: request.start_ts,
                        conflict_start_ts: write.start_ts,
                        conflict_commit_ts: write.commit_ts,
                    });
                }
            }
            let old_value = visible_value(key_state, request.for_update_ts);
            if request.return_values {
                result.values.push(old_value.clone());
            }
            if request.check_existence {
                result.existence.push(old_value.is_some());
            }
            let min_commit_ts = request
                .for_update_ts
                .max(request.start_ts)
                .saturating_add(1);
            pending.push((mutation.key.clone(), min_commit_ts));
        }
        for (key, min_commit_ts) in pending {
            let key_state = state.keys.get_mut(&key).expect("preflight inserted key");
            key_state.lock = Some(Lock {
                primary: request.primary_lock.clone(),
                start_ts: request.start_ts,
                ttl: request.lock_ttl,
                op: MutationOp::PessimisticLock,
                value: Vec::new(),
                for_update_ts: request.for_update_ts,
                min_commit_ts,
                use_async_commit: false,
                secondaries: Vec::new(),
                rollback_ts: Vec::new(),
            });
        }
        self.update_latest_ts(request.for_update_ts);
        Ok(result)
    }

    /// 条件匹配时清除悲观锁。
    pub fn pessimistic_rollback(&self, keys: &[Vec<u8>], start_ts: u64, for_update_ts: u64) {
        let mut state = self.state.write().expect("MVCC store lock poisoned");
        for key in keys {
            if let Some(key_state) = state.keys.get_mut(key)
                && key_state.lock.as_ref().is_some_and(|lock| {
                    lock.start_ts == start_ts
                        && lock.for_update_ts <= for_update_ts
                        && lock.op == MutationOp::PessimisticLock
                })
            {
                key_state.lock = None;
            }
        }
    }

    /// 延长主键锁 TTL（事务心跳）。
    pub fn txn_heartbeat(
        &self,
        primary: &[u8],
        start_ts: u64,
        advise_ttl: u64,
    ) -> Result<u64, MvccError> {
        let mut state = self.state.write().expect("MVCC store lock poisoned");
        let key_state = state
            .keys
            .get_mut(primary)
            .ok_or_else(|| MvccError::TxnNotFound {
                key: primary.to_vec(),
                start_ts,
            })?;
        let lock = key_state
            .lock
            .as_mut()
            .filter(|lock| lock.start_ts == start_ts)
            .ok_or_else(|| MvccError::TxnNotFound {
                key: primary.to_vec(),
                start_ts,
            })?;
        lock.ttl = lock.ttl.max(advise_ttl);
        Ok(lock.ttl)
    }

    /// 预写：写入锁；可选一阶段提交（1PC）直接落提交写。
    pub fn prewrite(&self, request: &PrewriteRequest) -> Result<PrewriteResult, MvccError> {
        let mut state = self.state.write().expect("MVCC store lock poisoned");
        let mut mutations = request.mutations.clone();
        // 悲观锁键优先，其余按键排序。
        mutations.sort_by(|left, right| {
            match (left.is_pessimistic_lock, right.is_pessimistic_lock) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => left.key.cmp(&right.key),
            }
        });
        let min_commit_ts = request
            .min_commit_ts
            .max(request.start_ts.saturating_add(1))
            .max(request.for_update_ts.saturating_add(1));
        // 1PC：校验通过后直接提交，跳过显式第二阶段。
        if request.try_one_pc && request.max_commit_ts >= min_commit_ts {
            for mutation in &mutations {
                check_prewrite(&state, mutation, request)?;
            }
            for mutation in mutations {
                if mutation.op != MutationOp::CheckNotExists {
                    apply_committed_mutation(&mut state, mutation, request.start_ts, min_commit_ts);
                }
            }
            self.update_latest_ts(min_commit_ts);
            return Ok(PrewriteResult {
                min_commit_ts,
                one_pc_commit_ts: min_commit_ts,
            });
        }
        for mutation in &mutations {
            check_prewrite(&state, mutation, request)?;
        }
        for mutation in mutations {
            // CheckNotExists is an assertion-only mutation in Go's MVCC path;
            // it validates absence but never creates a lock.
            if mutation.op == MutationOp::CheckNotExists {
                continue;
            }
            let key_state = state.keys.entry(mutation.key.clone()).or_default();
            // 长值先写入 defaults，锁内仅保留短值路径所需信息。
            if mutation.value.len() > SHORT_VALUE_MAX_LEN {
                key_state
                    .defaults
                    .insert(request.start_ts, mutation.value.clone());
            }
            key_state.lock = Some(Lock {
                primary: request.primary_lock.clone(),
                start_ts: request.start_ts,
                ttl: request.lock_ttl,
                op: mutation.op,
                value: mutation.value,
                for_update_ts: request.for_update_ts,
                min_commit_ts,
                use_async_commit: request.use_async_commit,
                secondaries: request.secondaries.clone(),
                rollback_ts: Vec::new(),
            });
        }
        self.update_latest_ts(request.start_ts);
        Ok(PrewriteResult {
            min_commit_ts,
            one_pc_commit_ts: 0,
        })
    }

    /// Flush 语义与预写相同（兼容上层调用）。
    pub fn flush(&self, request: &PrewriteRequest) -> Result<PrewriteResult, MvccError> {
        self.prewrite(request)
    }

    /// 提交：将匹配 start_ts 的锁转为 Write；幂等处理已提交。
    pub fn commit(&self, keys: &[Vec<u8>], start_ts: u64, commit_ts: u64) -> Result<(), MvccError> {
        let mut state = self.state.write().expect("MVCC store lock poisoned");
        // Go accumulates the whole request in a WriteBatch and only writes it
        // after every key has passed validation. Preflight first so an error
        // on a later key cannot leave earlier keys partially committed.
        for key in keys {
            let Some(key_state) = state.keys.get(key) else {
                return Err(MvccError::TxnNotFound {
                    key: key.clone(),
                    start_ts,
                });
            };
            if let Some(lock) = key_state
                .lock
                .as_ref()
                .filter(|lock| lock.start_ts == start_ts)
            {
                if commit_ts < lock.min_commit_ts {
                    return Err(MvccError::CommitTsExpired {
                        key: key.clone(),
                        start_ts,
                        attempted: commit_ts,
                        min_commit_ts: lock.min_commit_ts,
                    });
                }
                continue;
            }
            if let Some(write) = key_state
                .writes
                .values()
                .find(|write| write.start_ts == start_ts)
            {
                if write.kind == WriteKind::Rollback {
                    return Err(MvccError::TxnNotFound {
                        key: key.clone(),
                        start_ts,
                    });
                }
                continue;
            }
            return Err(MvccError::TxnNotFound {
                key: key.clone(),
                start_ts,
            });
        }
        for key in keys {
            let key_state = state
                .keys
                .get_mut(key)
                .expect("commit preflight checked key");
            if let Some(lock) = key_state
                .lock
                .clone()
                .filter(|lock| lock.start_ts == start_ts)
            {
                commit_lock(key_state, lock, commit_ts);
            }
        }
        self.update_latest_ts(commit_ts);
        Ok(())
    }

    /// 回滚：清锁并写入 Rollback 记录；已提交则报错。
    pub fn rollback(&self, keys: &[Vec<u8>], start_ts: u64) -> Result<(), MvccError> {
        let mut state = self.state.write().expect("MVCC store lock poisoned");
        // As with Go's WriteBatch, validate the complete request before
        // deleting locks or materializing rollback records.
        for key in keys {
            if let Some(write) = state
                .keys
                .get(key)
                .into_iter()
                .flat_map(|key_state| key_state.writes.values())
                .find(|write| write.start_ts == start_ts && write.kind != WriteKind::Rollback)
            {
                return Err(MvccError::AlreadyCommitted(write.commit_ts));
            }
        }
        for key in keys {
            let key_state = state.keys.entry(key.clone()).or_default();
            if key_state
                .lock
                .as_ref()
                .is_some_and(|lock| lock.start_ts == start_ts)
            {
                key_state.lock = None;
                key_state.defaults.remove(&start_ts);
            }
            key_state.writes.entry(start_ts).or_insert(Write {
                start_ts,
                commit_ts: start_ts,
                kind: WriteKind::Rollback,
                value: Vec::new(),
            });
        }
        Ok(())
    }

    /// 查询/推进主键事务状态：过期回滚或推高 min_commit_ts。
    pub fn check_txn_status(
        &self,
        primary: &[u8],
        start_ts: u64,
        caller_start_ts: u64,
        current_ts: u64,
        rollback_if_not_exist: bool,
    ) -> Result<TxnStatus, MvccError> {
        let mut state = self.state.write().expect("MVCC store lock poisoned");
        let key_state = state.keys.entry(primary.to_vec()).or_default();
        if let Some(write) = key_state
            .writes
            .values()
            .find(|write| write.start_ts == start_ts)
        {
            return Ok(TxnStatus {
                ttl: 0,
                commit_ts: if write.kind == WriteKind::Rollback {
                    0
                } else {
                    write.commit_ts
                },
                action: Action::NoAction,
                lock_info: None,
            });
        }
        if let Some(lock) = key_state
            .lock
            .as_mut()
            .filter(|lock| lock.start_ts == start_ts)
        {
            if lock.primary != primary {
                return Err(MvccError::PrimaryMismatch {
                    key: primary.to_vec(),
                    primary: lock.primary.clone(),
                });
            }
            // Async-commit locks are neither expired nor pushed by the
            // ordinary CheckTxnStatus path in Go.
            if lock.use_async_commit {
                return Ok(TxnStatus {
                    ttl: lock.ttl,
                    commit_ts: 0,
                    action: Action::NoAction,
                    lock_info: Some(lock.clone()),
                });
            }
            // TTL 过期则回滚锁。
            if physical_ts(start_ts).saturating_add(lock.ttl) < physical_ts(current_ts) {
                key_state.lock = None;
                key_state.defaults.remove(&start_ts);
                key_state.writes.insert(
                    start_ts,
                    Write {
                        start_ts,
                        commit_ts: start_ts,
                        kind: WriteKind::Rollback,
                        value: Vec::new(),
                    },
                );
                return Ok(TxnStatus {
                    ttl: 0,
                    commit_ts: 0,
                    action: Action::TtlExpireRollback,
                    lock_info: None,
                });
            }
            // 按调用方 start_ts 推高 min_commit_ts，减少后续冲突。
            let candidate = caller_start_ts.saturating_add(1);
            let action = if caller_start_ts == MAX_SYSTEM_TS {
                Action::MinCommitTsPushed
            } else if candidate > lock.min_commit_ts {
                lock.min_commit_ts = candidate;
                Action::MinCommitTsPushed
            } else {
                Action::NoAction
            };
            return Ok(TxnStatus {
                ttl: lock.ttl,
                commit_ts: 0,
                action,
                lock_info: Some(lock.clone()),
            });
        }
        if rollback_if_not_exist {
            key_state.writes.insert(
                start_ts,
                Write {
                    start_ts,
                    commit_ts: start_ts,
                    kind: WriteKind::Rollback,
                    value: Vec::new(),
                },
            );
            Ok(TxnStatus {
                ttl: 0,
                commit_ts: 0,
                action: Action::LockNotExistRollback,
                lock_info: None,
            })
        } else {
            Err(MvccError::TxnNotFound {
                key: primary.to_vec(),
                start_ts,
            })
        }
    }

    /// 检查异步提交次键：收集仍持有的锁或已提交时间戳。
    pub fn check_secondary_locks(&self, keys: &[Vec<u8>], start_ts: u64) -> SecondaryLocksStatus {
        let state = self.state.read().expect("MVCC store lock poisoned");
        let mut status = SecondaryLocksStatus::default();
        for key in keys {
            let Some(key_state) = state.keys.get(key) else {
                continue;
            };
            if let Some(lock) = key_state
                .lock
                .clone()
                .filter(|lock| lock.start_ts == start_ts)
            {
                status.locks.push(lock);
                continue;
            }
            if let Some(write) = key_state
                .writes
                .values()
                .find(|write| write.start_ts == start_ts && write.kind != WriteKind::Rollback)
            {
                status.commit_ts = write.commit_ts;
            }
        }
        status
    }

    /// 按版本点查；低于 safe point 则 GC 过早错误。
    pub fn get(
        &self,
        key: &[u8],
        version: u64,
        resolved: &[u64],
    ) -> Result<Option<Vec<u8>>, MvccError> {
        if version < self.safe_point.ts() {
            return Err(MvccError::GcTooEarly {
                safe_point: version,
                txn_safe_point: self.safe_point.ts(),
            });
        }
        let state = self.state.read().expect("MVCC store lock poisoned");
        let Some(key_state) = state.keys.get(key) else {
            return Ok(None);
        };
        check_read_lock(key, key_state.lock.as_ref(), version, resolved, &[])?;
        Ok(visible_value(key_state, version))
    }

    /// 批量点查，将错误编码进 KvPair。
    pub fn batch_get(&self, keys: &[Vec<u8>], version: u64, resolved: &[u64]) -> Vec<KvPair> {
        keys.iter()
            .filter_map(|key| match self.get(key, version, resolved) {
                Ok(Some(value)) => Some(KvPair {
                    key: key.clone(),
                    value,
                    error: None,
                }),
                Ok(None) => None,
                Err(error) => Some(KvPair {
                    key: key.clone(),
                    value: Vec::new(),
                    error: Some(error),
                }),
            })
            .collect()
    }

    /// 范围扫描；空 end 表示正无穷。
    pub fn scan(
        &self,
        start: &[u8],
        end: &[u8],
        version: u64,
        limit: usize,
        reverse: bool,
        key_only: bool,
        resolved: &[u64],
    ) -> Vec<KvPair> {
        if version < self.safe_point.ts() {
            return vec![KvPair {
                key: start.to_vec(),
                value: Vec::new(),
                error: Some(MvccError::GcTooEarly {
                    safe_point: version,
                    txn_safe_point: self.safe_point.ts(),
                }),
            }];
        }
        let state = self.state.read().expect("MVCC store lock poisoned");
        // Empty end means +inf, matching Go MVCCStore.Scan(nil endKey).
        let mut keys = if end.is_empty() {
            state
                .keys
                .range(start.to_vec()..)
                .map(|(key, _)| key.clone())
                .collect::<Vec<_>>()
        } else {
            state
                .keys
                .range(start.to_vec()..end.to_vec())
                .map(|(key, _)| key.clone())
                .collect::<Vec<_>>()
        };
        if reverse {
            keys.reverse();
        }
        keys.into_iter()
            .filter_map(|key| {
                let key_state = state.keys.get(&key).unwrap();
                match check_read_lock(&key, key_state.lock.as_ref(), version, resolved, &[]) {
                    Err(error) => Some(KvPair {
                        key,
                        value: Vec::new(),
                        error: Some(error),
                    }),
                    Ok(_) => visible_value(key_state, version).map(|value| KvPair {
                        key,
                        value: if key_only { Vec::new() } else { value },
                        error: None,
                    }),
                }
            })
            .take(limit)
            .collect()
    }

    /// 检查指定键上对 start_ts 可见的锁，返回锁对列表。
    pub fn check_keys_lock(
        &self,
        start_ts: u64,
        resolved: &[u64],
        committed: &[u64],
        keys: &[Vec<u8>],
    ) -> Result<Vec<LockPair>, MvccError> {
        let state = self.state.read().expect("MVCC store lock poisoned");
        let mut result = Vec::new();
        for key in keys {
            if let Some(key_state) = state.keys.get(key) {
                if let Some(lock) =
                    check_read_lock(key, key_state.lock.as_ref(), start_ts, resolved, committed)?
                {
                    result.push(LockPair {
                        key: key.clone(),
                        value: lock.value.clone(),
                        lock,
                    });
                }
            }
        }
        Ok(result)
    }

    /// 检查范围内是否存在阻塞读的锁。
    pub fn check_range_lock(
        &self,
        start_ts: u64,
        start: &[u8],
        end: &[u8],
        resolved: &[u64],
    ) -> Result<(), MvccError> {
        let state = self.state.read().expect("MVCC store lock poisoned");
        for (key, key_state) in state.keys.range(start.to_vec()..end.to_vec()) {
            check_read_lock(key, key_state.lock.as_ref(), start_ts, resolved, &[])?;
        }
        Ok(())
    }

    /// 从本事务 Put/Insert 锁中读取缓冲值。
    pub fn read_buffer_from_lock(&self, start_ts: u64, keys: &[Vec<u8>]) -> Vec<KvPair> {
        let state = self.state.read().expect("MVCC store lock poisoned");
        keys.iter()
            .filter_map(|key| {
                state
                    .keys
                    .get(key)?
                    .lock
                    .as_ref()
                    .filter(|lock| {
                        lock.start_ts == start_ts
                            && matches!(lock.op, MutationOp::Put | MutationOp::Insert)
                    })
                    .map(|lock| KvPair {
                        key: key.clone(),
                        value: lock.value.clone(),
                        error: None,
                    })
            })
            .collect()
    }

    /// 清理主键：委托 CheckTxnStatus；若已提交则失败。
    pub fn cleanup(&self, key: &[u8], start_ts: u64, current_ts: u64) -> Result<(), MvccError> {
        let status = self.check_txn_status(key, start_ts, current_ts, current_ts, true)?;
        if status.commit_ts > 0 {
            Err(MvccError::AlreadyCommitted(status.commit_ts))
        } else {
            Ok(())
        }
    }

    /// 扫描 start_ts ≤ max_ts 的锁。
    pub fn scan_lock(
        &self,
        start: &[u8],
        end: &[u8],
        max_ts: u64,
        limit: usize,
    ) -> Vec<(Vec<u8>, Lock)> {
        let state = self.state.read().expect("MVCC store lock poisoned");
        state
            .keys
            .range(start.to_vec()..end.to_vec())
            .filter_map(|(key, state)| {
                state
                    .lock
                    .clone()
                    .filter(|lock| lock.start_ts <= max_ts)
                    .map(|lock| (key.clone(), lock))
            })
            .take(limit)
            .collect()
    }

    /// 按 start_ts 解析全部相关锁：commit_ts=0 则回滚，否则提交。
    pub fn resolve_lock(&self, start_ts: u64, commit_ts: u64) -> Result<(), MvccError> {
        let keys = {
            let state = self.state.read().expect("MVCC store lock poisoned");
            state
                .keys
                .iter()
                .filter_map(|(key, state)| {
                    state
                        .lock
                        .as_ref()
                        .filter(|lock| lock.start_ts == start_ts)
                        .map(|_| key.clone())
                })
                .collect::<Vec<_>>()
        };
        if commit_ts == 0 {
            self.rollback(&keys, start_ts)
        } else {
            self.commit(&keys, start_ts, commit_ts)
        }
    }

    /// 按键导出完整 MVCC 信息。
    pub fn mvcc_get_by_key(&self, key: &[u8]) -> MvccInfo {
        let state = self.state.read().expect("MVCC store lock poisoned");
        state
            .keys
            .get(key)
            .map(|key_state| MvccInfo {
                lock: key_state.lock.clone(),
                writes: key_state.writes.values().cloned().collect(),
                values: key_state
                    .defaults
                    .iter()
                    .map(|(ts, value)| (*ts, value.clone()))
                    .collect(),
            })
            .unwrap_or_default()
    }

    /// 按 start_ts 查找首个相关键及其 MVCC 信息。
    pub fn mvcc_get_by_start_ts(&self, start_ts: u64) -> Option<(Vec<u8>, MvccInfo)> {
        let state = self.state.read().expect("MVCC store lock poisoned");
        state
            .keys
            .iter()
            .find(|(_, key_state)| {
                key_state
                    .lock
                    .as_ref()
                    .is_some_and(|lock| lock.start_ts == start_ts)
                    || key_state
                        .writes
                        .values()
                        .any(|write| write.start_ts == start_ts)
            })
            .map(|(key, state)| {
                (
                    key.clone(),
                    MvccInfo {
                        lock: state.lock.clone(),
                        writes: state.writes.values().cloned().collect(),
                        values: state
                            .defaults
                            .iter()
                            .map(|(ts, value)| (*ts, value.clone()))
                            .collect(),
                    },
                )
            })
    }

    /// 删除范围内全部键状态（模拟删文件）。
    pub fn delete_file_in_range(&self, start: &[u8], end: &[u8]) {
        let mut state = self.state.write().expect("MVCC store lock poisoned");
        let keys = state
            .keys
            .range(start.to_vec()..end.to_vec())
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        for key in keys {
            state.keys.remove(&key);
        }
    }

    /// 更新 GC safe point。
    pub fn update_safe_point(&self, safe_point: u64) {
        self.safe_point.update_ts(safe_point);
    }

    /// 按 safe point 回收旧版本，保留可见锚点写与仍存活的长值。
    pub fn gc(&self) {
        let safe_point = self.safe_point.ts();
        let mut state = self.state.write().expect("MVCC store lock poisoned");
        for key_state in state.keys.values_mut() {
            // 保留 safe point 前最后一个非 Rollback 写，保证读可见性。
            let anchor = key_state
                .writes
                .range(..safe_point)
                .rev()
                .find(|(_, write)| write.kind != WriteKind::Rollback)
                .map(|(ts, _)| *ts);
            key_state.writes.retain(|commit_ts, write| {
                *commit_ts >= safe_point
                    || Some(*commit_ts) == anchor
                    || write.kind == WriteKind::Rollback && *commit_ts >= safe_point
            });
            let live_start_ts = key_state
                .writes
                .values()
                .map(|write| write.start_ts)
                .collect::<BTreeSet<_>>();
            key_state
                .defaults
                .retain(|start_ts, _| live_start_ts.contains(start_ts));
        }
    }
}

/// 预写前冲突检查：锁、写冲突、Insert/CheckNotExists 存在性。
fn check_prewrite(
    state: &StoreState,
    mutation: &Mutation,
    request: &PrewriteRequest,
) -> Result<(), MvccError> {
    let Some(key_state) = state.keys.get(&mutation.key) else {
        if mutation.is_pessimistic_lock {
            return Err(MvccError::PessimisticLockNotFound(mutation.key.clone()));
        }
        return Ok(());
    };
    if let Some(lock) = &key_state.lock {
        if lock.start_ts != request.start_ts {
            return Err(MvccError::KeyLocked {
                key: mutation.key.clone(),
                lock: lock.clone(),
            });
        }
        if mutation.is_pessimistic_lock && lock.op != MutationOp::PessimisticLock {
            return Err(MvccError::PessimisticLockNotFound(mutation.key.clone()));
        }
    } else if mutation.is_pessimistic_lock {
        return Err(MvccError::PessimisticLockNotFound(mutation.key.clone()));
    }
    if let Some(write) = key_state
        .writes
        .range(request.start_ts..)
        .next_back()
        .map(|(_, write)| write)
        .filter(|write| write.kind != WriteKind::Rollback && write.start_ts != request.start_ts)
    {
        return Err(MvccError::WriteConflict {
            key: mutation.key.clone(),
            start_ts: request.start_ts,
            conflict_start_ts: write.start_ts,
            conflict_commit_ts: write.commit_ts,
        });
    }
    if matches!(mutation.op, MutationOp::Insert | MutationOp::CheckNotExists)
        && visible_value(key_state, request.start_ts).is_some()
    {
        return Err(MvccError::AlreadyExists(mutation.key.clone()));
    }
    Ok(())
}

/// 1PC 路径：直接写入已提交 Write 并清锁。
fn apply_committed_mutation(
    state: &mut StoreState,
    mutation: Mutation,
    start_ts: u64,
    commit_ts: u64,
) {
    let key_state = state.keys.entry(mutation.key).or_default();
    let kind = match mutation.op {
        MutationOp::Put | MutationOp::Insert => WriteKind::Put,
        MutationOp::Delete => WriteKind::Delete,
        _ => WriteKind::Lock,
    };
    key_state.writes.insert(
        commit_ts,
        Write {
            start_ts,
            commit_ts,
            kind,
            value: mutation.value,
        },
    );
    key_state.lock = None;
}

/// 将锁转为 Write；长值优先从 defaults 取回。
fn commit_lock(key_state: &mut KeyState, lock: Lock, commit_ts: u64) {
    let kind = match lock.op {
        MutationOp::Put | MutationOp::Insert => WriteKind::Put,
        MutationOp::Delete => WriteKind::Delete,
        _ => WriteKind::Lock,
    };
    let value = if lock.value.len() <= SHORT_VALUE_MAX_LEN {
        lock.value
    } else {
        key_state
            .defaults
            .get(&lock.start_ts)
            .cloned()
            .unwrap_or(lock.value)
    };
    key_state.writes.insert(
        commit_ts,
        Write {
            start_ts: lock.start_ts,
            commit_ts,
            kind,
            value,
        },
    );
    key_state.lock = None;
}

/// 取最新非回滚写。
fn latest_non_rollback_write(state: &KeyState) -> Option<&Write> {
    state
        .writes
        .values()
        .rev()
        .find(|write| write.kind != WriteKind::Rollback)
}

/// 在 version 可见的最新 Put/Delete 值。
fn visible_value(state: &KeyState, version: u64) -> Option<Vec<u8>> {
    for write in state.writes.range(..=version).rev().map(|(_, write)| write) {
        match write.kind {
            WriteKind::Put => return Some(write.value.clone()),
            WriteKind::Delete => return None,
            WriteKind::Lock | WriteKind::Rollback => continue,
        }
    }
    None
}

/// 读路径锁检查：忽略已 resolved 或纯 Lock 操作；否则报 KeyLocked。
fn check_read_lock(
    key: &[u8],
    lock: Option<&Lock>,
    start_ts: u64,
    resolved: &[u64],
    committed: &[u64],
) -> Result<Option<Lock>, MvccError> {
    let Some(lock) = lock else {
        return Ok(None);
    };
    if lock.start_ts > start_ts || resolved.contains(&lock.start_ts) {
        return Ok(None);
    }
    let is_write_lock = matches!(lock.op, MutationOp::Put | MutationOp::Delete);
    let is_primary_get =
        start_ts == MAX_SYSTEM_TS && lock.primary.as_slice() == key && !lock.use_async_commit;
    if !is_write_lock || is_primary_get {
        return Ok(None);
    }
    if committed.contains(&lock.start_ts) {
        return Ok(Some(lock.clone()));
    }
    Err(MvccError::KeyLocked {
        key: key.to_vec(),
        lock: lock.clone(),
    })
}

/// 从混合逻辑时钟取出物理部分（右移 18 位）。
fn physical_ts(ts: u64) -> u64 {
    ts >> 18
}

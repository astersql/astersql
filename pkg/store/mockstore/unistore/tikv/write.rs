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

// 内存写后端与 WriteBatch：把 MVCC 预写/提交/回滚落成 DB 与锁表变更。
//
// 两阶段提交（2PC）中 Prewrite 写锁、Commit 写版本并删锁；Rollback 写事务状态并可选删锁。
// `DbWriter` 保证先应用 DB 条目再删锁，避免读到半提交状态。
use crate::mvcc::{Lock, MAX_SYSTEM_TS, MutationOp};
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

/// 写批处理通道容量。
pub const BATCH_CHANNEL_SIZE: usize = 1024;
/// 按范围删除时每批移除的条目上限。
pub const DELETE_RANGE_BATCH_SIZE: usize = 4096;

#[derive(Clone, Debug, Eq, PartialEq)]
/// 写入 DB 的一条版本化键值（含是否删除标记）。
pub struct DbEntry {
    pub key: Vec<u8>,
    pub version: u64,
    pub value: Vec<u8>,
    pub user_meta: Vec<u8>,
    pub delete: bool,
}
#[derive(Clone, Debug, Eq, PartialEq)]
/// 锁表变更：设置锁或删除锁。
pub enum LockEntry {
    Set(Vec<u8>, Lock),
    Delete(Vec<u8>),
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 一次事务写批：记录 start/commit 时间戳及待落盘的 DB/锁条目。
pub struct WriteBatch {
    pub start_ts: u64,
    pub commit_ts: u64,
    pub db_entries: Vec<DbEntry>,
    pub lock_entries: Vec<LockEntry>,
}

impl WriteBatch {
    /// Prewrite：仅写入锁条目。
    pub fn prewrite(&mut self, key: Vec<u8>, lock: Lock) {
        self.lock_entries.push(LockEntry::Set(key, lock));
    }
    /// Commit：按锁类型写出版本数据或事务状态，并删除锁。
    pub fn commit(&mut self, key: Vec<u8>, lock: Lock) {
        // 普通 Put/Delete：以 commit_ts 写入版本值。
        if lock.op != MutationOp::PessimisticLock && lock.op != MutationOp::Lock {
            self.db_entries.push(DbEntry {
                key: key.clone(),
                version: self.commit_ts,
                value: lock.value,
                user_meta: encode_user_meta(self.start_ts, self.commit_ts),
                delete: false,
            });
        // Op::Lock 且为主键：写 txn_status 记录而非业务值。
        } else if lock.op == MutationOp::Lock && key == lock.primary {
            self.db_entries.push(DbEntry {
                key: extra_txn_status_key(&key, self.start_ts),
                version: self.start_ts,
                value: Vec::new(),
                user_meta: encode_user_meta(self.start_ts, self.commit_ts),
                delete: false,
            });
        }
        self.lock_entries.push(LockEntry::Delete(key));
    }
    /// Rollback：写入 rollback 事务状态记录，可选删除锁。
    pub fn rollback(&mut self, key: Vec<u8>, delete_lock: bool) {
        self.db_entries.push(DbEntry {
            key: extra_txn_status_key(&key, self.start_ts),
            version: self.start_ts,
            value: Vec::new(),
            user_meta: encode_user_meta(self.start_ts, 0),
            delete: false,
        });
        if delete_lock {
            self.lock_entries.push(LockEntry::Delete(key));
        }
    }
    /// 悲观锁：写入锁条目。
    pub fn pessimistic_lock(&mut self, key: Vec<u8>, lock: Lock) {
        self.lock_entries.push(LockEntry::Set(key, lock));
    }
    /// 悲观锁回滚：删除锁条目。
    pub fn pessimistic_rollback(&mut self, key: Vec<u8>) {
        self.lock_entries.push(LockEntry::Delete(key));
    }
}

#[derive(Default)]
/// 内存 DB（按 key+version 有序）与锁表后端。
pub struct MemoryWriteBackend {
    db: RwLock<BTreeMap<(Vec<u8>, u64), DbEntry>>,
    locks: RwLock<HashMap<Vec<u8>, Lock>>,
    max_lock_entry_size: usize,
}
impl MemoryWriteBackend {
    /// 创建后端；`max_lock_entry_size` 为 0 表示不限制锁条目大小。
    pub fn new(max_lock_entry_size: usize) -> Self {
        Self {
            max_lock_entry_size,
            ..Self::default()
        }
    }
    /// 查询指定 key 上的锁。
    pub fn get_lock(&self, key: &[u8]) -> Option<Lock> {
        self.locks
            .read()
            .expect("lock backend poisoned")
            .get(key)
            .cloned()
    }
    /// 返回某 key 的全部版本条目。
    pub fn versions(&self, key: &[u8]) -> Vec<DbEntry> {
        self.db
            .read()
            .expect("DB backend poisoned")
            .range((key.to_vec(), 0)..=(key.to_vec(), u64::MAX))
            .map(|(_, entry)| entry.clone())
            .collect()
    }
    /// 应用 DB 条目：删除或插入版本。
    fn apply_db(&self, entries: &[DbEntry]) {
        let mut db = self.db.write().expect("DB backend poisoned");
        for entry in entries {
            if entry.delete {
                db.remove(&(entry.key.clone(), entry.version));
            } else {
                db.insert((entry.key.clone(), entry.version), entry.clone());
            }
        }
    }
    /// 应用锁条目；超大锁条目返回错误。
    fn apply_locks(&self, entries: &[LockEntry]) -> Result<(), String> {
        let mut locks = self.locks.write().expect("lock backend poisoned");
        for entry in entries {
            match entry {
                LockEntry::Set(key, lock) => {
                    // Go checks the complete lockstore entry: user key plus
                    // Lock.MarshalBinary(), including its fixed 40-byte header
                    // and the length-prefixed async-commit secondary keys.
                    let marshaled_lock_size = 40
                        + lock.primary.len()
                        + lock.value.len()
                        + lock
                            .secondaries
                            .iter()
                            .map(|secondary| 2 + secondary.len())
                            .sum::<usize>();
                    let size = key.len() + marshaled_lock_size;
                    if self.max_lock_entry_size > 0 && size > self.max_lock_entry_size {
                        return Err(format!(
                            "unistore lock entry too big {size} > {}",
                            self.max_lock_entry_size
                        ));
                    }
                    locks.insert(key.clone(), lock.clone());
                }
                LockEntry::Delete(key) => {
                    locks.remove(key);
                }
            }
        }
        Ok(())
    }
}

/// 对 `MemoryWriteBackend` 的写入口，跟踪最新时间戳与开闭状态。
pub struct DbWriter {
    backend: Arc<MemoryWriteBackend>,
    latest_ts: AtomicU64,
    open: AtomicBool,
}
impl DbWriter {
    /// 创建未打开的写入口。
    pub fn new(backend: Arc<MemoryWriteBackend>) -> Self {
        Self {
            backend,
            latest_ts: AtomicU64::new(0),
            open: AtomicBool::new(false),
        }
    }
    /// 标记写入口可用。
    pub fn open(&self) {
        self.open.store(true, Ordering::Release);
    }
    /// 关闭写入口，后续 `write` 将失败。
    pub fn close(&self) {
        self.open.store(false, Ordering::Release);
    }
    /// 是否处于打开状态。
    pub fn is_open(&self) -> bool {
        self.open.load(Ordering::Acquire)
    }
    /// 创建写批并刷新观测到的最新时间戳。
    pub fn new_write_batch(&self, start_ts: u64, commit_ts: u64) -> WriteBatch {
        self.update_latest_ts(if commit_ts > 0 { commit_ts } else { start_ts });
        WriteBatch {
            start_ts,
            commit_ts,
            ..WriteBatch::default()
        }
    }
    /// 落盘写批：先 DB 后锁，保证提交可见性。
    pub fn write(&self, batch: WriteBatch) -> Result<(), String> {
        if !self.is_open() {
            return Err("DB writer is closed".into());
        }
        // 数据库变更必须先于删锁落盘，避免读到半提交状态。
        // Database mutations must become durable before lock deletion.
        self.backend.apply_db(&batch.db_entries);
        self.backend.apply_locks(&batch.lock_entries)
    }
    /// 返回写路径观测到的最大时间戳。
    pub fn latest_ts(&self) -> u64 {
        self.latest_ts.load(Ordering::Acquire)
    }
    /// 更新最新时间戳（忽略系统最大时间戳哨兵值）。
    fn update_latest_ts(&self, ts: u64) {
        if ts != MAX_SYSTEM_TS {
            self.latest_ts.fetch_max(ts, Ordering::AcqRel);
        }
    }
    /// 删除 `[start, end)` 范围内的全部版本；`end` 不可为空。
    pub fn delete_range(&self, start: &[u8], end: &[u8]) -> Result<(), String> {
        if end.is_empty() {
            panic!("invalid end key");
        }
        let keys = {
            let db = self.backend.db.read().expect("DB backend poisoned");
            db.keys()
                .filter(|(key, _)| key.as_slice() >= start && key.as_slice() < end)
                .cloned()
                .collect::<Vec<_>>()
        };
        // 分批持有写锁删除，降低长时间阻塞。
        for chunk in keys.chunks(DELETE_RANGE_BATCH_SIZE) {
            let mut db = self.backend.db.write().expect("DB backend poisoned");
            for key in chunk {
                db.remove(key);
            }
        }
        Ok(())
    }
}

/// 将 start_ts/commit_ts 编码为用户元数据（匹配 Go 原生小端布局）。
fn encode_user_meta(start_ts: u64, commit_ts: u64) -> Vec<u8> {
    let mut value = start_ts.to_le_bytes().to_vec();
    value.extend_from_slice(&commit_ts.to_le_bytes());
    value
}
/// 构造额外事务状态键，记录 rollback/Lock 类型提交等辅助信息。
fn extra_txn_status_key(key: &[u8], start_ts: u64) -> Vec<u8> {
    // Go appends codec.EncodeUintDesc(startTS), then increments the first
    // user-key byte so the status record sorts outside the ordinary key.
    let mut value = key.to_vec();
    value.extend_from_slice(&(!start_ts).to_be_bytes());
    value[0] = value[0].wrapping_add(1);
    value
}

// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// 内存 mock KV Storage：事务、快照、PD 客户端与协处理器 Store 替身。
//
// 用多版本（commit_ts）键值表模拟 MVCC 可见性，供测试在无真实 TiKV 时验证
// Begin/Commit/Snapshot 与 Storage 接口行为。

use std::any::Any;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// 可跨线程共享的任意类型值（选项与钩子常用）。
pub type AnyValue = Arc<dyn Any + Send + Sync>;
/// 本模块统一结果类型。
pub type Result<T> = std::result::Result<T, MockStorageError>;

#[derive(Clone, Debug, Eq, PartialEq)]
/// mock storage 侧错误：关闭、Begin 失败、协处理器/PD 注入失败等。
pub enum MockStorageError {
    NotImplemented,
    Closed,
    WriteConflict,
    Begin(String),
    Coprocessor(String),
    Pd(String),
}

impl fmt::Display for MockStorageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotImplemented => formatter.write_str("not implemented"),
            Self::Closed => formatter.write_str("mock storage is closed"),
            Self::WriteConflict => formatter.write_str("mock storage write conflict"),
            Self::Begin(message) => write!(formatter, "begin transaction: {message}"),
            Self::Coprocessor(message) => write!(formatter, "create coprocessor store: {message}"),
            Self::Pd(message) => write!(formatter, "pd client: {message}"),
        }
    }
}

impl std::error::Error for MockStorageError {}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 请求上下文占位（对齐 kv::Context 简化版）。
pub struct Context;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// TLS 配置占位。
pub struct TLSConfig;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 死锁检测用的锁等待条目。
pub struct WaitForEntry {
    pub txn: u64,
    pub wait_for_txn: u64,
    pub key_hash: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// Keyspace（多租户键空间）元数据：名称与 ID。
pub struct KeyspaceMeta {
    pub Name: String,
    pub Id: u32,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
/// Storage 选项键的可哈希表示。
pub enum OptionKey {
    String(String),
    Signed(i64),
    Unsigned(u64),
    Bool(bool),
}

impl From<&str> for OptionKey {
    fn from(value: &str) -> Self {
        Self::String(value.to_owned())
    }
}

impl From<String> for OptionKey {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<i64> for OptionKey {
    fn from(value: i64) -> Self {
        Self::Signed(value)
    }
}

impl From<u64> for OptionKey {
    fn from(value: u64) -> Self {
        Self::Unsigned(value)
    }
}

impl From<bool> for OptionKey {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

/// 将具体值装箱为共享 `AnyValue`。
pub fn NewOptionValue<T: Any + Send + Sync>(value: T) -> AnyValue {
    Arc::new(value)
}

/// PD（Placement Driver）客户端最小接口：按名加载 keyspace。
pub trait PdClient: Send + Sync {
    fn LoadKeyspace(&self, context: &Context, name: &str) -> Result<Option<KeyspaceMeta>>;
}

#[derive(Default)]
/// 内存 PD 客户端：可注入 keyspace、返回 None 或下次失败。
pub struct MemoryPdClient {
    keyspaces: RwLock<HashMap<String, KeyspaceMeta>>,
    return_none: AtomicBool,
    failure: Mutex<Option<String>>,
}

impl MemoryPdClient {
    /// 登记/覆盖一个 keyspace。
    pub fn StoreKeyspace(&self, meta: KeyspaceMeta) {
        self.keyspaces
            .write()
            .expect("keyspace map lock poisoned")
            .insert(meta.Name.clone(), meta);
    }

    /// 设置 LoadKeyspace 是否强制返回 None。
    pub fn SetReturnNone(&self, return_none: bool) {
        self.return_none.store(return_none, Ordering::Release);
    }

    /// 使下一次 LoadKeyspace 返回指定 PD 错误。
    pub fn FailNextLoad(&self, message: impl Into<String>) {
        *self.failure.lock().expect("pd failure lock poisoned") = Some(message.into());
    }
}

impl PdClient for MemoryPdClient {
    fn LoadKeyspace(&self, _context: &Context, name: &str) -> Result<Option<KeyspaceMeta>> {
        if let Some(message) = self
            .failure
            .lock()
            .expect("pd failure lock poisoned")
            .take()
        {
            return Err(MockStorageError::Pd(message));
        }
        if self.return_none.load(Ordering::Acquire) {
            return Ok(None);
        }
        Ok(self
            .keyspaces
            .read()
            .expect("keyspace map lock poisoned")
            .get(name)
            .cloned())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 编解码模式（当前仅 Txn）。
pub enum Mode {
    Txn,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// API 编解码版本：V1 或带 keyspace 的 V2。
pub enum Codec {
    ApiV1,
    ApiV2 {
        keyspace_name: String,
        keyspace_id: u32,
    },
}

/// 包装编解码策略的 PD 客户端视图。
pub struct CodecPDClient {
    codec: Codec,
}

impl CodecPDClient {
    /// 返回当前编解码策略。
    pub fn GetCodec(&self) -> Codec {
        self.codec.clone()
    }
}

/// 创建默认 ApiV1 编解码客户端。
pub fn NewCodecPDClient(_mode: Mode, _client: Arc<dyn PdClient>) -> CodecPDClient {
    CodecPDClient {
        codec: Codec::ApiV1,
    }
}

/// 按 keyspace 名从 PD 加载元数据并构造 ApiV2 编解码。
pub fn NewCodecPDClientWithKeyspace(
    _mode: Mode,
    client: Arc<dyn PdClient>,
    name: &str,
) -> Result<CodecPDClient> {
    let meta = client
        .LoadKeyspace(&Context, name)?
        .ok_or_else(|| MockStorageError::Pd(format!("keyspace {name:?} was not found")))?;
    Ok(CodecPDClient {
        codec: Codec::ApiV2 {
            keyspace_name: meta.Name,
            keyspace_id: meta.Id,
        },
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 开启事务时的选项（作用域、是否流水线）。
pub enum TxnOption {
    Scope(String),
    Pipelined(bool),
    StartTS(u64),
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 某一提交时间戳下的版本值；`None` 表示删除。
struct VersionedValue {
    commit_ts: u64,
    value: Option<Vec<u8>>,
}

/// KVStore 内部共享状态。
struct KVStoreInner {
    data: RwLock<BTreeMap<Vec<u8>, Vec<VersionedValue>>>,
    current_ts: AtomicU64,
    tso_request_count: AtomicU64,
    wall_clock_tso: bool,
    closed: AtomicBool,
    close_count: AtomicUsize,
    pd_client: Arc<dyn PdClient>,
    begin_failure: Mutex<Option<String>>,
    copr_failure: Mutex<Option<String>>,
    uuid: String,
}

#[derive(Clone)]
/// 可克隆的内存 KV 存储句柄。
pub struct KVStore {
    inner: Arc<KVStoreInner>,
}

impl KVStore {
    fn new_with_clock(pd_client: Arc<dyn PdClient>, wall_clock_tso: bool) -> Self {
        static NEXT_STORE_ID: AtomicU64 = AtomicU64::new(1);
        let id = NEXT_STORE_ID.fetch_add(1, Ordering::Relaxed);
        Self {
            inner: Arc::new(KVStoreInner {
                data: RwLock::new(BTreeMap::new()),
                current_ts: AtomicU64::new(0),
                tso_request_count: AtomicU64::new(0),
                wall_clock_tso,
                closed: AtomicBool::new(false),
                close_count: AtomicUsize::new(0),
                pd_client,
                begin_failure: Mutex::new(None),
                copr_failure: Mutex::new(None),
                uuid: format!("mock-kv-{id}"),
            }),
        }
    }

    /// 绑定 PD 客户端并分配唯一 UUID。
    pub fn New(pd_client: Arc<dyn PdClient>) -> Self {
        Self::new_with_clock(pd_client, false)
    }

    /// 使用默认内存 PD 客户端创建。
    pub fn NewMemory() -> Self {
        Self::New(Arc::new(MemoryPdClient::default()))
    }

    /// Create a session-facing mock store whose oracle preserves TiDB TSO
    /// physical time while remaining strictly monotonic.
    pub fn NewMemoryWithWallClockTSO() -> Self {
        Self::new_with_clock(Arc::new(MemoryPdClient::default()), true)
    }

    /// 开启事务：分配 start_ts（事务开始时间戳）。
    pub fn Begin(&self, opts: &[TxnOption]) -> Result<KVTxn> {
        if self.Closed() {
            return Err(MockStorageError::Closed);
        }
        if let Some(message) = self
            .inner
            .begin_failure
            .lock()
            .expect("begin failure lock poisoned")
            .take()
        {
            return Err(MockStorageError::Begin(message));
        }
        let start_ts = opts
            .iter()
            .find_map(|option| match option {
                TxnOption::StartTS(start_ts) => Some(*start_ts),
                TxnOption::Scope(_) | TxnOption::Pipelined(_) => None,
            })
            .map_or_else(|| self.CurrentTimestamp("global"), Ok)?;
        Ok(KVTxn {
            store: self.clone(),
            start_ts,
            opts: opts.to_vec(),
            writes: BTreeMap::new(),
            buffered_write_size: 0,
            valid: true,
            flags: HashMap::new(),
            stages: Vec::new(),
            next_stage: 1,
            pessimistic: false,
        })
    }

    /// 获取指定版本快照；过大版本钳制到当前时间戳。
    pub fn GetSnapshot(&self, version: u64) -> Snapshot {
        let current = self.inner.current_ts.load(Ordering::Acquire);
        Snapshot {
            store: self.clone(),
            version: if version == u64::MAX || version > current {
                current
            } else {
                version
            },
            options: HashMap::new(),
            cache: RefCell::new(HashMap::new()),
        }
    }

    /// 单调递增分配新时间戳。
    pub fn CurrentTimestamp(&self, _txn_scope: &str) -> Result<u64> {
        if self.Closed() {
            return Err(MockStorageError::Closed);
        }
        // Timestamp allocation and commit publication share the data lock. A
        // transaction that starts after a commit timestamp is allocated must
        // also observe that commit's versions; otherwise a fixed MVCC snapshot
        // can change between two reads while the commit is being published.
        let _data = self.inner.data.read().expect("kv data lock poisoned");
        Ok(self.allocate_timestamp())
    }

    fn allocate_timestamp(&self) -> u64 {
        self.inner.tso_request_count.fetch_add(1, Ordering::AcqRel);
        if !self.inner.wall_clock_tso {
            return self.inner.current_ts.fetch_add(1, Ordering::AcqRel) + 1;
        }
        let wall_tso = (SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64)
            << 18;
        let previous = self
            .inner
            .current_ts
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                Some(current.saturating_add(1).max(wall_tso))
            })
            .unwrap_or_else(|current| current);
        previous.saturating_add(1).max(wall_tso)
    }

    /// Return the number of successful oracle timestamp requests.
    pub fn TSORequestCount(&self) -> u64 {
        self.inner.tso_request_count.load(Ordering::Acquire)
    }

    /// 返回绑定的 PD 客户端。
    pub fn GetPDClient(&self) -> Arc<dyn PdClient> {
        Arc::clone(&self.inner.pd_client)
    }

    /// 存储是否已关闭。
    pub fn Closed(&self) -> bool {
        self.inner.closed.load(Ordering::Acquire)
    }

    /// 标记关闭（幂等，仅首次增加 close_count）。
    pub fn Close(&self) -> Result<()> {
        if !self.inner.closed.swap(true, Ordering::AcqRel) {
            self.inner.close_count.fetch_add(1, Ordering::Relaxed);
        }
        Ok(())
    }

    /// 存储实例 UUID。
    pub fn UUID(&self) -> String {
        self.inner.uuid.clone()
    }

    /// 关闭次数（测试用）。
    pub fn CloseCount(&self) -> usize {
        self.inner.close_count.load(Ordering::Relaxed)
    }

    /// 注入下一次 Begin 失败。
    pub fn FailNextBegin(&self, message: impl Into<String>) {
        *self
            .inner
            .begin_failure
            .lock()
            .expect("begin failure lock poisoned") = Some(message.into());
    }

    /// 注入下一次创建 CoprStore 失败。
    pub fn FailNextCoprocessorStore(&self, message: impl Into<String>) {
        *self
            .inner
            .copr_failure
            .lock()
            .expect("coprocessor failure lock poisoned") = Some(message.into());
    }

    /// 取出并清除协处理器失败注入。
    fn take_copr_failure(&self) -> Option<String> {
        self.inner
            .copr_failure
            .lock()
            .expect("coprocessor failure lock poisoned")
            .take()
    }

    /// 在给定版本读取键的值（忽略删除）。
    fn read_at(&self, key: &[u8], version: u64) -> Option<Vec<u8>> {
        self.read_entry_at(key, version).map(|entry| entry.0)
    }

    /// MVCC 读：取 commit_ts <= version 的最新非删除版本。
    pub(crate) fn read_entry_at(&self, key: &[u8], version: u64) -> Option<(Vec<u8>, u64)> {
        self.inner
            .data
            .read()
            .expect("kv data lock poisoned")
            .get(key)
            // 从新到旧找第一个可见版本。
            .and_then(|versions| {
                versions
                    .iter()
                    .rev()
                    .find(|entry| entry.commit_ts <= version)
            })
            .and_then(|entry| entry.value.clone().map(|value| (value, entry.commit_ts)))
    }

    /// 在版本上按 [lower, upper) 扫描可见键值，可选逆序。
    pub(crate) fn scan_at(
        &self,
        version: u64,
        lower_bound: Option<&[u8]>,
        upper_bound: Option<&[u8]>,
        reverse: bool,
    ) -> Vec<(Vec<u8>, Vec<u8>)> {
        let data = self.inner.data.read().expect("kv data lock poisoned");
        let mut entries = data
            .iter()
            .filter(|(key, _)| {
                lower_bound.is_none_or(|lower| key.as_slice() >= lower)
                    && upper_bound.is_none_or(|upper| key.as_slice() < upper)
            })
            .filter_map(|(key, versions)| {
                versions
                    .iter()
                    .rev()
                    .find(|entry| entry.commit_ts <= version)
                    .and_then(|entry| entry.value.clone())
                    .map(|value| (key.clone(), value))
            })
            .collect::<Vec<_>>();
        if reverse {
            entries.reverse();
        }
        entries
    }

    /// 分配 commit_ts 并将写集追加为新版本。
    fn commit(&self, start_ts: u64, writes: BTreeMap<Vec<u8>, Option<Vec<u8>>>) -> Result<u64> {
        self.commit_with_allocated_timestamp(start_ts, writes, true)
    }

    /// 使用已分配的 oracle 时间戳提交。Async Commit 的 minCommitTS
    /// 可以复用 prewrite 期间已推进的 oracle 上界。
    fn commit_at(
        &self,
        start_ts: u64,
        writes: BTreeMap<Vec<u8>, Option<Vec<u8>>>,
        commit_ts: u64,
    ) -> Result<u64> {
        self.commit_at_with_conflict_check(start_ts, writes, commit_ts, true)
    }

    /// 提交已通过悲观锁排序的写集。
    ///
    /// 悲观事务可能在开启快照之后等待并取得行锁；此时写集合法地包含
    /// 等待期间提交的新版本，不能再按旧快照执行乐观写冲突检查。
    fn commit_at_with_conflict_check(
        &self,
        start_ts: u64,
        writes: BTreeMap<Vec<u8>, Option<Vec<u8>>>,
        commit_ts: u64,
        check_conflicts: bool,
    ) -> Result<u64> {
        let mut data = self.inner.data.write().expect("kv data lock poisoned");
        if check_conflicts
            && writes.keys().any(|key| {
                data.get(key)
                    .and_then(|versions| versions.last())
                    .is_some_and(|entry| entry.commit_ts > start_ts)
            })
        {
            return Err(MockStorageError::WriteConflict);
        }
        self.inner.current_ts.fetch_max(commit_ts, Ordering::AcqRel);
        for (key, value) in writes {
            data.entry(key)
                .or_default()
                .push(VersionedValue { commit_ts, value });
        }
        Ok(commit_ts)
    }

    fn commit_with_allocated_timestamp(
        &self,
        start_ts: u64,
        writes: BTreeMap<Vec<u8>, Option<Vec<u8>>>,
        check_conflicts: bool,
    ) -> Result<u64> {
        let mut data = self.inner.data.write().expect("kv data lock poisoned");
        if check_conflicts
            && writes.keys().any(|key| {
                data.get(key)
                    .and_then(|versions| versions.last())
                    .is_some_and(|entry| entry.commit_ts > start_ts)
            })
        {
            return Err(MockStorageError::WriteConflict);
        }
        let commit_ts = self.allocate_timestamp();
        for (key, value) in writes {
            data.entry(key)
                .or_default()
                .push(VersionedValue { commit_ts, value });
        }
        Ok(commit_ts)
    }
}

/// 底层 KV 事务：本地写缓冲 + staging（可嵌套暂存）。
pub struct KVTxn {
    pub(crate) store: KVStore,
    pub(crate) start_ts: u64,
    pub(crate) opts: Vec<TxnOption>,
    pub(crate) writes: BTreeMap<Vec<u8>, Option<Vec<u8>>>,
    pub(crate) buffered_write_size: usize,
    pub(crate) valid: bool,
    pub(crate) flags: HashMap<Vec<u8>, u8>,
    pub(crate) stages: Vec<(
        i32,
        BTreeMap<Vec<u8>, Option<Vec<u8>>>,
        HashMap<Vec<u8>, u8>,
    )>,
    pub(crate) next_stage: i32,
    pub(crate) pessimistic: bool,
}

impl KVTxn {
    /// 先读本地写缓冲，否则按 start_ts 读存储。
    pub fn Get(&self, key: &[u8]) -> Option<Vec<u8>> {
        self.writes
            .get(key)
            .cloned()
            .unwrap_or_else(|| self.store.read_at(key, self.start_ts))
    }

    /// 缓冲 Put。
    pub fn Set(&mut self, key: impl Into<Vec<u8>>, value: impl Into<Vec<u8>>) {
        let key = key.into();
        let value = value.into();
        let previous_size = self
            .writes
            .get(&key)
            .map_or(0, |value| key.len() + value.as_ref().map_or(0, Vec::len));
        self.buffered_write_size = self
            .buffered_write_size
            .saturating_sub(previous_size)
            .saturating_add(key.len() + value.len());
        self.writes.insert(key, Some(value));
    }

    /// 缓冲删除（值为 None）。
    pub fn Delete(&mut self, key: impl Into<Vec<u8>>) {
        let key = key.into();
        let previous_size = self
            .writes
            .get(&key)
            .map_or(0, |value| key.len() + value.as_ref().map_or(0, Vec::len));
        self.buffered_write_size = self
            .buffered_write_size
            .saturating_sub(previous_size)
            .saturating_add(key.len());
        self.writes.insert(key, None);
    }

    /// 提交写集并失效本事务。
    pub fn Commit(&mut self) -> Result<u64> {
        if !self.valid {
            return Err(MockStorageError::Closed);
        }
        let writes = std::mem::take(&mut self.writes);
        self.buffered_write_size = 0;
        let commit_ts =
            self.store
                .commit_with_allocated_timestamp(self.start_ts, writes, !self.pessimistic)?;
        self.valid = false;
        Ok(commit_ts)
    }

    /// Async Commit uses the maximum TSO already allocated during prewrite,
    /// while still keeping commit_ts strictly above start_ts.
    pub fn CommitAsync(&mut self) -> Result<u64> {
        if !self.valid {
            return Err(MockStorageError::Closed);
        }
        let writes = std::mem::take(&mut self.writes);
        self.buffered_write_size = 0;
        let commit_ts = self
            .store
            .inner
            .current_ts
            .load(Ordering::Acquire)
            .max(self.start_ts.saturating_add(1));
        let commit_ts = self.store.commit_at_with_conflict_check(
            self.start_ts,
            writes,
            commit_ts,
            !self.pessimistic,
        )?;
        self.valid = false;
        Ok(commit_ts)
    }

    /// 清空缓冲并失效本事务。
    pub fn Rollback(&mut self) -> Result<()> {
        if !self.valid {
            return Ok(());
        }
        self.writes.clear();
        self.buffered_write_size = 0;
        self.flags.clear();
        self.stages.clear();
        self.valid = false;
        Ok(())
    }

    /// 快照扫描与本地写缓冲合并后的可见结果。
    pub(crate) fn scan(
        &self,
        lower_bound: Option<&[u8]>,
        upper_bound: Option<&[u8]>,
        reverse: bool,
    ) -> Vec<(Vec<u8>, Vec<u8>)> {
        let mut entries = self
            .store
            .scan_at(self.start_ts, lower_bound, upper_bound, false)
            .into_iter()
            .collect::<BTreeMap<_, _>>();
        // 用本地写覆盖快照扫描结果（含删除）。
        for (key, value) in &self.writes {
            if lower_bound.is_some_and(|lower| key.as_slice() < lower)
                || upper_bound.is_some_and(|upper| key.as_slice() >= upper)
            {
                continue;
            }
            if let Some(value) = value {
                entries.insert(key.clone(), value.clone());
            } else {
                entries.remove(key);
            }
        }
        let mut entries = entries.into_iter().collect::<Vec<_>>();
        if reverse {
            entries.reverse();
        }
        entries
    }

    /// 事务开始时间戳。
    pub fn StartTS(&self) -> u64 {
        self.start_ts
    }

    /// 开启时传入的事务选项。
    pub fn Options(&self) -> &[TxnOption] {
        &self.opts
    }

    /// Mark this mock transaction as pessimistic after the SQL session selects
    /// pessimistic mode.
    pub fn SetPessimistic(&mut self, pessimistic: bool) {
        self.pessimistic = pessimistic;
    }
}

/// 面向上层的事务包装：含快照、选项、内存钩子等。
pub struct Transaction {
    pub(crate) inner: KVTxn,
    pub(crate) snapshot: Snapshot,
    pub(crate) commit_ts: u64,
    pub(crate) options: HashMap<i32, Box<dyn Any>>,
    pub(crate) vars: Box<dyn Any>,
    pub(crate) table_info: HashMap<i64, astersql_kv::model::TableInfo>,
    pub(crate) checkpoint: astersql_kv::tikv::MemDBCheckpoint,
    pub(crate) memory_hook: Option<Box<dyn Fn(u64)>>,
    pub(crate) fair_locking: bool,
}

impl Transaction {
    /// 委托内层 Get。
    pub fn Get(&self, key: &[u8]) -> Option<Vec<u8>> {
        self.inner.Get(key)
    }

    /// 委托内层 Set。
    pub fn Set(&mut self, key: impl Into<Vec<u8>>, value: impl Into<Vec<u8>>) {
        self.inner.Set(key, value);
    }

    /// 委托内层 Delete。
    pub fn Delete(&mut self, key: impl Into<Vec<u8>>) {
        self.inner.Delete(key);
    }

    /// 提交并记录 commit_ts。
    pub fn Commit(&mut self) -> Result<u64> {
        let commit_ts = self.inner.Commit()?;
        self.commit_ts = commit_ts;
        Ok(commit_ts)
    }

    /// 委托内层 StartTS。
    pub fn StartTS(&self) -> u64 {
        self.inner.StartTS()
    }
}

/// 将 KVTxn 包装为带快照的 Transaction。
pub fn newTiKVTxn(transaction: Result<KVTxn>) -> Result<Transaction> {
    transaction.map(|inner| Transaction {
        snapshot: inner.store.GetSnapshot(inner.start_ts),
        inner,
        commit_ts: 0,
        options: HashMap::new(),
        vars: Box::new(()),
        table_info: HashMap::new(),
        checkpoint: astersql_kv::tikv::MemDBCheckpoint,
        memory_hook: None,
        fair_locking: false,
    })
}

/// 只读快照：固定 version 下的可见性视图。
pub struct Snapshot {
    pub(crate) store: KVStore,
    pub(crate) version: u64,
    pub(crate) options: HashMap<i32, Box<dyn Any>>,
    pub(crate) cache: RefCell<HashMap<Vec<u8>, Option<astersql_kv::ValueEntry>>>,
}

impl Snapshot {
    /// 按快照版本读键。
    pub fn Get(&self, key: &[u8]) -> Option<Vec<u8>> {
        self.store.read_at(key, self.version)
    }

    /// 快照版本号。
    pub fn Version(&self) -> u64 {
        self.version
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 版本号包装（对齐 Go Version）。
pub struct Version {
    pub Ver: u64,
}

/// 构造 Version。
pub fn NewVersion(version: u64) -> Version {
    Version { Ver: version }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 协处理器缓存配置占位。
pub struct CoprCacheConfig;

/// 默认空配置。
pub fn DefaultCoprCacheConfig() -> CoprCacheConfig {
    CoprCacheConfig
}

/// mock 协处理器 Store：可关闭并计数。
pub struct CoprStore {
    closed: AtomicBool,
    close_count: AtomicUsize,
}

impl CoprStore {
    /// 创建 CoprStore；若有失败注入则返回错误。
    pub fn NewStore(store: &KVStore, _config: &CoprCacheConfig) -> Result<Self> {
        if let Some(message) = store.take_copr_failure() {
            return Err(MockStorageError::Coprocessor(message));
        }
        Ok(Self {
            closed: AtomicBool::new(false),
            close_count: AtomicUsize::new(0),
        })
    }

    /// 标记关闭（幂等）。
    pub fn Close(&self) {
        if !self.closed.swap(true, Ordering::AcqRel) {
            self.close_count.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// 是否已关闭。
    pub fn Closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// 关闭次数。
    pub fn CloseCount(&self) -> usize {
        self.close_count.load(Ordering::Relaxed)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// KV 客户端占位类型。
pub struct KvClient;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// MPP（大规模并行处理）客户端占位。
pub struct MppClient;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 时间戳 Oracle 占位。
pub struct Oracle;

#[derive(Default)]
/// 按表 ID 缓存键值的内存管理器。
pub struct MemManager {
    values: RwLock<HashMap<(i64, Vec<u8>), Vec<u8>>>,
}

impl MemManager {
    /// 按默认 table_id=0 读取缓存。
    pub fn Get(&self, key: &[u8]) -> Option<Vec<u8>> {
        self.values
            .read()
            .expect("memory cache lock poisoned")
            .get(&(0, key.to_vec()))
            .cloned()
    }

    /// 按默认 table_id=0 写入缓存。
    pub fn Set(&self, key: impl Into<Vec<u8>>, value: impl Into<Vec<u8>>) {
        self.values
            .write()
            .expect("memory cache lock poisoned")
            .insert((0, key.into()), value.into());
    }

    /// 按默认 table_id=0 删除缓存。
    pub fn Delete(&self, key: &[u8]) {
        self.values
            .write()
            .expect("memory cache lock poisoned")
            .remove(&(0, key.to_vec()));
    }

    /// 按表 ID 读缓存。
    pub(crate) fn get_for_table(&self, table_id: i64, key: &[u8]) -> Option<Vec<u8>> {
        self.values
            .read()
            .expect("memory cache lock poisoned")
            .get(&(table_id, key.to_vec()))
            .cloned()
    }

    /// 按表 ID 写缓存。
    pub(crate) fn set_for_table(&self, table_id: i64, key: &[u8], value: &[u8]) {
        self.values
            .write()
            .expect("memory cache lock poisoned")
            .insert((table_id, key.to_vec()), value.to_vec());
    }

    /// 删除整表缓存条目。
    pub(crate) fn delete_table(&self, table_id: i64) {
        self.values
            .write()
            .expect("memory cache lock poisoned")
            .retain(|(cached_table_id, _), _| *cached_table_id != table_id);
    }
}

/// mock Storage 门面：组合 KVStore、CoprStore、选项与内存缓存。
pub struct mockStorage {
    pub KVStore: KVStore,
    pub Store: Arc<CoprStore>,
    opts: RwLock<HashMap<OptionKey, AnyValue>>,
    memCache: Arc<MemManager>,
    LockWaits: RwLock<Vec<WaitForEntry>>,
    keyspaceMeta: Option<KeyspaceMeta>,
    pub(crate) canonicalOptions: RwLock<HashMap<OptionKey, &'static (dyn Any + Send + Sync)>>,
}

/// 对外类型别名。
pub type MockStorage = mockStorage;

/// 创建 mockStorage；CoprStore 创建失败则整体失败。
pub fn NewMockStorage(
    tikvStore: KVStore,
    keyspaceMeta: Option<KeyspaceMeta>,
) -> Result<Arc<mockStorage>> {
    let coprConfig = DefaultCoprCacheConfig();
    let coprStore = CoprStore::NewStore(&tikvStore, &coprConfig)?;
    Ok(Arc::new(mockStorage {
        KVStore: tikvStore,
        Store: Arc::new(coprStore),
        opts: RwLock::new(HashMap::new()),
        memCache: Arc::new(MemManager::default()),
        LockWaits: RwLock::new(Vec::new()),
        keyspaceMeta,
        canonicalOptions: RwLock::new(HashMap::new()),
    }))
}

impl mockStorage {
    /// 读取选项，返回 (值, 是否存在)。
    pub fn GetOption(&self, key: &OptionKey) -> (Option<AnyValue>, bool) {
        let value = self
            .opts
            .read()
            .expect("storage options lock poisoned")
            .get(key)
            .cloned();
        let exists = value.is_some();
        (value, exists)
    }

    /// 设置或删除选项。
    pub fn SetOption(&self, key: OptionKey, value: Option<AnyValue>) {
        let mut options = self.opts.write().expect("storage options lock poisoned");
        if let Some(value) = value {
            options.insert(key, value);
        } else {
            options.remove(&key);
        }
    }

    /// etcd 地址列表（mock 为空）。
    pub fn EtcdAddrs(&self) -> Result<Vec<String>> {
        Ok(Vec::new())
    }

    /// PD 地址列表（mock 为空）。
    pub fn GetPDAddrs(&self) -> Result<Vec<String>> {
        Ok(Vec::new())
    }

    /// TLS 配置（mock 为 None）。
    pub fn TLSConfig(&self) -> Option<TLSConfig> {
        None
    }

    /// 返回共享内存缓存。
    pub fn GetMemCache(&self) -> Arc<MemManager> {
        Arc::clone(&self.memCache)
    }

    /// 规范 Storage 接口用的 MemManager 引用。
    pub(crate) fn canonical_mem_cache(&self) -> &MemManager {
        self.memCache.as_ref()
    }

    /// 启动 GC worker（mock 为空操作）。
    pub fn StartGCWorker(&self) -> Result<()> {
        Ok(())
    }

    /// 存储名称。
    pub fn Name(&self) -> String {
        "mock-storage".to_owned()
    }

    /// 描述信息（空串）。
    pub fn Describe(&self) -> String {
        String::new()
    }

    /// 开启上层事务。
    pub fn Begin(&self, opts: &[TxnOption]) -> Result<Transaction> {
        newTiKVTxn(self.KVStore.Begin(opts))
    }

    /// ShowStatus 未实现。
    pub fn ShowStatus(&self, _context: &Context, _key: &str) -> Result<AnyValue> {
        Err(MockStorageError::NotImplemented)
    }

    /// 按 Version 取快照。
    pub fn GetSnapshot(&self, version: Version) -> Snapshot {
        self.KVStore.GetSnapshot(version.Ver)
    }

    /// 分配当前版本。
    pub fn CurrentVersion(&self, txnScope: &str) -> Result<Version> {
        self.KVStore.CurrentTimestamp(txnScope).map(NewVersion)
    }

    /// Return the number of timestamp requests made through this storage.
    pub fn TSORequestCount(&self) -> u64 {
        self.KVStore.TSORequestCount()
    }

    /// 最小安全时间戳（mock 恒为 0）。
    pub fn GetMinSafeTS(&self, _txnScope: &str) -> u64 {
        0
    }

    /// 返回当前锁等待列表。
    pub fn GetLockWaits(&self) -> Result<Vec<WaitForEntry>> {
        Ok(self
            .LockWaits
            .read()
            .expect("lock waits lock poisoned")
            .clone())
    }

    /// 关闭 CoprStore 与 KVStore。
    pub fn Close(&self) -> Result<()> {
        if self.KVStore.Closed() {
            return Ok(());
        }
        self.Store.Close();
        self.KVStore.Close()
    }

    /// 按 keyspaceMeta 解析编解码；缺失时用包装 PD 客户端回退。
    pub fn GetCodec(&self) -> Codec {
        let pdClient = self.KVStore.GetPDClient();
        // 无 keyspace 元数据时走 ApiV1。
        let Some(keyspaceMeta) = &self.keyspaceMeta else {
            return NewCodecPDClient(Mode::Txn, pdClient).GetCodec();
        };

        let keyspace = pdClient
            .LoadKeyspace(&Context, &keyspaceMeta.Name)
            .unwrap_or_else(|error| panic!("{error}"));
        let pdClient: Arc<dyn PdClient> = if keyspace.is_none() {
            Arc::new(pdCliWithCodec {
                Client: pdClient,
                ksMeta: keyspaceMeta.clone(),
            })
        } else {
            pdClient
        };
        NewCodecPDClientWithKeyspace(Mode::Txn, pdClient, &keyspaceMeta.Name)
            .unwrap_or_else(|error| panic!("{error}"))
            .GetCodec()
    }

    /// 测试注入锁等待列表。
    pub fn SetMockLockWaits(&self, lockWaits: Vec<WaitForEntry>) {
        *self.LockWaits.write().expect("lock waits lock poisoned") = lockWaits;
    }

    /// 集群 ID（mock 为 1）。
    pub fn GetClusterID(&self) -> u64 {
        1
    }

    /// 当前 keyspace 名。
    pub fn GetKeyspace(&self) -> String {
        self.keyspaceMeta
            .as_ref()
            .map(|meta| meta.Name.clone())
            .unwrap_or_default()
    }

    /// 返回空 KvClient。
    pub fn GetClient(&self) -> KvClient {
        KvClient
    }

    /// 返回空 MppClient。
    pub fn GetMPPClient(&self) -> MppClient {
        MppClient
    }

    /// 委托 KVStore UUID。
    pub fn UUID(&self) -> String {
        self.KVStore.UUID()
    }

    /// 返回占位 Oracle。
    pub fn GetOracle(&self) -> Oracle {
        Oracle
    }

    /// 是否支持 DeleteRange（mock 为 true）。
    pub fn SupportDeleteRange(&self) -> bool {
        true
    }
}

/// 强制 LoadKeyspace 返回固定 ksMeta 的 PD 包装。
pub struct pdCliWithCodec {
    pub Client: Arc<dyn PdClient>,
    pub ksMeta: KeyspaceMeta,
}

impl PdClient for pdCliWithCodec {
    fn LoadKeyspace(&self, _context: &Context, _name: &str) -> Result<Option<KeyspaceMeta>> {
        Ok(Some(self.ksMeta.clone()))
    }
}

/// 可注入锁等待列表的测试接口。
pub trait MockLockWaitSetter {
    fn SetMockLockWaits(&self, lockWaits: Vec<WaitForEntry>);
}

impl MockLockWaitSetter for mockStorage {
    fn SetMockLockWaits(&self, lockWaits: Vec<WaitForEntry>) {
        mockStorage::SetMockLockWaits(self, lockWaits);
    }
}

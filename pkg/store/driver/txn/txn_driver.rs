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

// TiKV 事务驱动（`tikvTxn`）核心适配。
//
// 封装本地 memBuffer（脏写缓冲）与不可变 snapshot，提供 Get/Set/Delete、范围扫描、
// 悲观锁相关错误转换与提交路径。提交对应两阶段提交（2PC）在驱动侧的简化落地：
// 将缓冲写入刷入共享存储并失效事务。

use std::any::Any;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use astersql_kv as canonical_kv;
use tikv_client::{CheckLevel, TransactionOptions as ClientTransactionOptions};

use crate::{
    BatchBufferGetter, BatchGetOption, BatchGetter, DriverError, ExtractKeyExistsErrFromHandle,
    ExtractKeyExistsErrFromIndex, GetOption, IndexInfo, Key, KeyFlags, KvIterator, MemDBCheckpoint,
    NewBufferBatchGetter, NewSnapshot, NewUnionIter, ReplicaReadType, ResourceGroupTagger,
    SnapshotInterceptor, SnapshotOption, SnapshotRuntimeStats, TableInfo, ValueEntry,
    WriteConflict, decode_table_key_head, memBuffer, newWriteConflictError, prettyWriteKey,
    tikvSnapshot,
};

/// 官方 client-rust 事务模式。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClientTransactionMode {
    Optimistic,
    Pessimistic,
}

impl ClientTransactionMode {
    /// 构造对应 client-rust 模式，并关闭未提交 Drop 检查。
    ///
    /// adapter 在错误返回、Store 关闭或未显式提交时会丢弃句柄；真正的资源释放由
    /// client-rust rollback/Drop 完成，不应因正常错误路径触发 panic。
    pub fn options(self) -> ClientTransactionOptions {
        match self {
            Self::Optimistic => ClientTransactionOptions::new_optimistic(),
            Self::Pessimistic => ClientTransactionOptions::new_pessimistic(),
        }
        .drop_check(CheckLevel::None)
    }

    /// 稳定的用户可见模式名。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Optimistic => "optimistic",
            Self::Pessimistic => "pessimistic",
        }
    }
}

/// 将 client-rust 错误映射到 canonical `pkg/kv` 错误边界。
pub fn map_client_error(error: tikv_client::Error) -> canonical_kv::errors::SharedError {
    fn conflict(error: &tikv_client::Error) -> Option<&tikv_client::proto::kvrpcpb::WriteConflict> {
        match error {
            tikv_client::Error::KeyError(error) => error.conflict.as_ref(),
            tikv_client::Error::ExtractedErrors(errors)
            | tikv_client::Error::MultipleKeyErrors(errors) => errors.iter().find_map(conflict),
            tikv_client::Error::PessimisticLockError { inner, .. } => conflict(inner),
            _ => None,
        }
    }
    if let Some(conflict) = conflict(&error) {
        return newWriteConflictError(Some(WriteConflict {
            start_ts: conflict.start_ts,
            conflict_ts: conflict.conflict_ts,
            conflict_commit_ts: conflict.conflict_commit_ts,
            key: conflict.key.clone(),
            primary: conflict.primary.clone(),
            reason: conflict.reason().as_str_name().to_owned(),
            ..Default::default()
        }));
    }
    canonical_kv::errors::NewNoStackError(error.to_string())
}

/// 将 client-rust 未命中值映射成 TiDB `ErrNotExist`。
pub fn canonical_value(
    value: Option<Vec<u8>>,
) -> Result<canonical_kv::ValueEntry, canonical_kv::errors::SharedError> {
    value
        .map(|value| canonical_kv::NewValueEntry(value, 0))
        .ok_or_else(|| canonical_kv::ErrNotExist.FastGenByArgs(&[]))
}

/// `LockKeys` 返回给上层的单键结果：含值与冲突时间戳。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ReturnedValue {
    pub value: ValueEntry,
    pub locked_with_conflict_ts: u64,
}

/// 加锁上下文：聚合冲突时间戳、已返回值，以及可注入的后端错误。
#[derive(Clone, Debug, Default)]
pub struct LockContext {
    pub max_locked_with_conflict_ts: u64,
    pub values: HashMap<Key, ReturnedValue>,
    pub backend_error: Option<DriverError>,
}

/// 断言（Assertion）检查严格程度：提交前校验键是否存在/不存在。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AssertionLevel {
    Off,
    Fast,
    #[default]
    Strict,
}

/// Prewrite（预写，2PC 第一阶段）遇到锁时的策略：尝试解析或直接失败。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PrewriteEncounterLockPolicy {
    #[default]
    TryResolve,
    NoResolve,
}

/// 后台协程生命周期钩子名称占位（对齐 Go 回调注册点）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LifecycleHooks {
    pub before_start: Option<String>,
    pub after_finish: Option<String>,
}

/// 单条目与整事务体积上限，防止超大事务拖垮 Region / 内存。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxnSizeLimits {
    pub entry: usize,
    pub total: usize,
}

impl Default for TxnSizeLimits {
    fn default() -> Self {
        Self {
            entry: 6 * 1024 * 1024,
            total: 100 * 1024 * 1024,
        }
    }
}

/// 提交完成后的回调钩子。
pub type CommitHook = Arc<dyn Fn(&str, Option<&DriverError>) + Send + Sync>;
/// 提交时间戳上界检查：返回 false 则拒绝该 commit_ts。
pub type CommitTsUpperBoundCheck = Arc<dyn Fn(u64) -> bool + Send + Sync>;

/// 事务级选项，部分会转发到底层 snapshot。
pub enum TxnOption {
    SchemaChecker(String),
    IsolationLevel(crate::IsoLevel),
    Priority(i32),
    NotFillCache(bool),
    Pessimistic(bool),
    SnapshotTS(u64),
    ReplicaRead(ReplicaReadType),
    TaskID(u64),
    InfoSchema(i64),
    CollectRuntimeStats(Option<SnapshotRuntimeStats>),
    SampleStep(u32),
    CommitHook(CommitHook),
    EnableAsyncCommit(bool),
    Enable1PC(bool),
    GuaranteeLinearizability(bool),
    TxnScope(String),
    IsStalenessReadOnly(bool),
    MatchStoreLabels(Vec<(String, String)>),
    ResourceGroupTag(Vec<u8>),
    ResourceGroupTagger(ResourceGroupTagger),
    KVFilter(TiDBKVFilter),
    SnapInterceptor(Arc<dyn SnapshotInterceptor>),
    CommitTSUpperBoundCheck(CommitTsUpperBoundCheck),
    RPCInterceptor(String),
    AssertionLevel(AssertionLevel),
    TableToColumnMaps(Arc<dyn Any + Send + Sync>),
    RequestSourceInternal(bool),
    RequestSourceType(String),
    ExplicitRequestSourceType(String),
    ReplicaReadAdjuster(String),
    TxnSource(u64),
    ResourceGroupName(String),
    LoadBasedReplicaReadThreshold(Duration),
    TiKVClientReadTimeout(u64),
    SizeLimits(TxnSizeLimits),
    SessionID(u64),
    BackgroundGoroutineLifecycleHooks(LifecycleHooks),
    PrewriteEncounterLockPolicy(PrewriteEncounterLockPolicy),
}

/// `GetOption` 查询返回的选项值包装。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TxnOptionValue {
    Bool(bool),
    String(String),
    RequestSourceInternal(bool),
    RequestSourceType(String),
}

/// 事务内部设置缓存（悲观、异步提交、1PC、资源组等）。
#[derive(Clone, Debug)]
struct TxnSettings {
    schema_checker: Option<String>,
    pessimistic: bool,
    info_schema_version: Option<i64>,
    enable_async_commit: bool,
    enable_1pc: bool,
    causal_consistency: bool,
    txn_scope: String,
    resource_group_tag: Vec<u8>,
    resource_group_tagger: Option<ResourceGroupTagger>,
    rpc_interceptors: Vec<String>,
    assertion_level: AssertionLevel,
    request_source_internal: bool,
    request_source_type: String,
    explicit_request_source_type: String,
    txn_source: u64,
    resource_group_name: String,
    size_limits: TxnSizeLimits,
    session_id: u64,
    lifecycle_hooks: LifecycleHooks,
    prewrite_lock_policy: PrewriteEncounterLockPolicy,
}

impl Default for TxnSettings {
    fn default() -> Self {
        Self {
            schema_checker: None,
            pessimistic: false,
            info_schema_version: None,
            enable_async_commit: false,
            enable_1pc: false,
            causal_consistency: false,
            txn_scope: "global".to_owned(),
            resource_group_tag: Vec::new(),
            resource_group_tagger: None,
            rpc_interceptors: Vec::new(),
            assertion_level: AssertionLevel::Strict,
            request_source_internal: false,
            request_source_type: String::new(),
            explicit_request_source_type: String::new(),
            txn_source: 0,
            resource_group_name: String::new(),
            size_limits: TxnSizeLimits::default(),
            session_id: 0,
            lifecycle_hooks: LifecycleHooks::default(),
            prewrite_lock_policy: PrewriteEncounterLockPolicy::TryResolve,
        }
    }
}

/// Rust transaction adapter retaining TiDB's buffer/snapshot/option behavior.
/// TiDB 事务适配器：脏写缓冲 + 快照读 + 选项/提交钩子。
pub struct tikvTxn {
    storage: Arc<RwLock<BTreeMap<Key, ValueEntry>>>,
    snapshot: tikvSnapshot,
    mem_buffer: Arc<memBuffer>,
    idx_name_cache: HashMap<i64, TableInfo>,
    column_maps_cache: Option<Arc<dyn Any + Send + Sync>>,
    is_committer_working: AtomicBool,
    settings: TxnSettings,
    commit_hook: Option<CommitHook>,
    commit_ts_upper_bound_check: Option<CommitTsUpperBoundCheck>,
    vars: Option<String>,
    start_ts: u64,
    commit_ts: u64,
    disk_full_option: Option<u32>,
    pipelined: bool,
    valid: bool,
    fair_locking: bool,
    next_gen_kernel: bool,
}

/// 创建事务：共享 storage 上挂快照，并按 `pipelined` 初始化 memBuffer。
pub fn NewTiKVTxn(
    storage: Arc<RwLock<BTreeMap<Key, ValueEntry>>>,
    start_ts: u64,
    pipelined: bool,
) -> tikvTxn {
    let snapshot = NewSnapshot(Arc::clone(&storage));
    tikvTxn {
        storage,
        snapshot,
        mem_buffer: Arc::new(memBuffer::empty(pipelined)),
        idx_name_cache: HashMap::new(),
        column_maps_cache: None,
        is_committer_working: AtomicBool::new(false),
        settings: TxnSettings::default(),
        commit_hook: None,
        commit_ts_upper_bound_check: None,
        vars: None,
        start_ts,
        commit_ts: 0,
        disk_full_option: None,
        pipelined,
        valid: true,
        fair_locking: false,
        next_gen_kernel: false,
    }
}

impl tikvTxn {
    /// 由初始键值集合构造事务（测试便捷入口）。
    pub fn from_entries(
        entries: impl IntoIterator<Item = (Key, ValueEntry)>,
        start_ts: u64,
        pipelined: bool,
    ) -> Self {
        NewTiKVTxn(
            Arc::new(RwLock::new(entries.into_iter().collect())),
            start_ts,
            pipelined,
        )
    }

    /// 查询已缓存的表元信息（用于唯一键冲突错误美化）。
    pub fn GetTableInfo(&self, id: i64) -> Option<&TableInfo> {
        self.idx_name_cache.get(&id)
    }

    /// 设置磁盘满时的处理级别选项。
    pub fn SetDiskFullOpt(&mut self, level: u32) {
        self.disk_full_option = Some(level);
    }

    /// 清除磁盘满选项。
    pub fn ClearDiskFullOpt(&mut self) {
        self.disk_full_option = None;
    }

    /// 缓存表信息；若 `info.id != id` 则双键写入，便于按物理/逻辑 id 查找。
    pub fn CacheTableInfo(&mut self, id: i64, info: Option<TableInfo>) {
        let Some(info) = info else {
            // Go stores a nil map value here. Removing the entry preserves the
            // observable cache-miss semantics and, importantly, clears stale data.
            self.idx_name_cache.remove(&id);
            return;
        };
        self.idx_name_cache.insert(id, info.clone());
        if info.ID != id {
            self.idx_name_cache.insert(info.ID, info);
        }
    }

    /// 悲观锁加锁路径：转换后端错误或根据 LockedWithConflict 生成写冲突。
    pub fn LockKeys(
        &mut self,
        lock_context: &mut LockContext,
        _keys: &[Key],
    ) -> Result<(), DriverError> {
        // 标记 committer 忙碌，阻止并发 SetOption。
        self.is_committer_working.store(true, Ordering::SeqCst);
        let result = if let Some(error) = lock_context.backend_error.take() {
            Err(self.extract_key_error(error))
        } else {
            self.generate_write_conflict_for_locked_with_conflict(lock_context)
        };
        self.is_committer_working.store(false, Ordering::SeqCst);
        result
    }

    /// 先执行调用方注入函数再加锁，对应 Go 的 LockKeysFunc 钩子点。
    pub fn LockKeysFunc<F>(
        &mut self,
        lock_context: &mut LockContext,
        function: F,
        keys: &[Key],
    ) -> Result<(), DriverError>
    where
        F: FnOnce(),
    {
        function();
        self.LockKeys(lock_context, keys)
    }

    /// 提交事务：刷入存储后调用可选 CommitHook。
    pub fn Commit(&mut self, commit_ts: u64) -> Result<(), DriverError> {
        self.is_committer_working.store(true, Ordering::SeqCst);
        let result = self.commit_inner(commit_ts);
        self.is_committer_working.store(false, Ordering::SeqCst);
        if let Some(hook) = &self.commit_hook {
            hook("commit", result.as_ref().err());
        }
        result
    }

    /// 核心提交：校验有效性与 commit_ts 上界，过滤无用索引键，空值作删除。
    fn commit_inner(&mut self, commit_ts: u64) -> Result<(), DriverError> {
        if !self.valid {
            return Err(DriverError::Backend("transaction is invalid".to_owned()));
        }
        if let Some(check) = &self.commit_ts_upper_bound_check
            && !check(commit_ts)
        {
            return Err(DriverError::Backend(
                "commit timestamp exceeds upper bound".to_owned(),
            ));
        }
        let entries = self.mem_buffer.entries();
        // 提交前过滤 untouched 索引等无需写入的键值。
        for (key, (value, flags)) in &entries {
            TiDBKVFilter.IsUnnecessaryKeyValue(key, value, *flags)?;
        }
        let mut storage = self
            .storage
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for (key, (value, _)) in entries {
            // 空 value 表示删除 tombstone。
            if value.is_empty() {
                storage.remove(&key);
            } else {
                storage.insert(key, ValueEntry::new(value, commit_ts));
            }
        }
        self.commit_ts = commit_ts;
        self.valid = false;
        Ok(())
    }

    /// 获取 memBuffer 检查点，供语句级回滚。
    pub fn GetMemDBCheckpoint(&self) -> MemDBCheckpoint {
        self.mem_buffer.checkpoint()
    }

    /// 将 memBuffer 回滚到指定检查点。
    pub fn RollbackMemDBToCheckpoint(&self, checkpoint: &MemDBCheckpoint) {
        self.mem_buffer.revert_to_checkpoint(checkpoint);
    }

    /// 返回事务开始时的快照副本。
    pub fn GetSnapshot(&self) -> tikvSnapshot {
        self.snapshot.clone()
    }

    /// 正向扫描：合并 dirty memBuffer 与 snapshot（UnionIter）。
    pub fn Iter(
        &self,
        key: &[u8],
        upper_bound: Option<&[u8]>,
    ) -> Result<Box<dyn KvIterator>, DriverError> {
        let mut dirty = self.mem_buffer.Iter(key, upper_bound)?;
        let snapshot = match self.snapshot.Iter(key, upper_bound) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                // 快照失败时关闭已打开的 dirty 迭代器，避免泄漏。
                dirty.close();
                return Err(error);
            }
        };
        match NewUnionIter(dirty, snapshot, false) {
            Ok(iterator) => Ok(Box::new(iterator)),
            Err(error) => Err(error),
        }
    }

    /// 反向扫描：同样合并 dirty 与 snapshot。
    pub fn IterReverse(
        &self,
        key: Option<&[u8]>,
        lower_bound: Option<&[u8]>,
    ) -> Result<Box<dyn KvIterator>, DriverError> {
        let mut dirty = self.mem_buffer.IterReverse(key, lower_bound)?;
        let snapshot = match self.snapshot.IterReverse(key, lower_bound) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                dirty.close();
                return Err(error);
            }
        };
        match NewUnionIter(dirty, snapshot, true) {
            Ok(iterator) => Ok(Box::new(iterator)),
            Err(error) => Err(error),
        }
    }

    /// 批量读：先看缓冲再回落快照，并按选项回填 commit_ts。
    pub fn BatchGet(
        &self,
        keys: &[Key],
        options: &[BatchGetOption],
    ) -> Result<HashMap<Key, ValueEntry>, DriverError> {
        let buffer: Arc<dyn BatchBufferGetter> = self.mem_buffer.clone();
        let snapshot: Arc<dyn BatchGetter> = Arc::new(self.snapshot.clone());
        let mut values = NewBufferBatchGetter(buffer, None, snapshot).BatchGet(keys, options)?;
        for value in values.values_mut() {
            *value = crate::apply_commit_ts_option_batch(value.clone(), options);
        }
        Ok(values)
    }

    /// 在缓冲中写入删除（空值 tombstone）。
    pub fn Delete(&self, key: Key) -> Result<(), DriverError> {
        self.mem_buffer.Delete(key)
    }

    /// 单键读：缓冲优先；缓冲空值视为删除；未命中再读快照。
    pub fn Get(&self, key: &[u8], options: &[GetOption]) -> Result<ValueEntry, DriverError> {
        let value = match self.mem_buffer.Get(key, options) {
            Ok(value) => value,
            Err(error) if error.is_not_found() => self.snapshot.Get(key, options)?,
            Err(error) => return Err(error),
        };
        // 空值 tombstone 对上层表现为 NotFound。
        if value.is_value_empty() {
            Err(DriverError::NotFound)
        } else {
            Ok(crate::apply_commit_ts_option(value, options))
        }
    }

    /// 写入缓冲前检查单条目与事务总大小限制。
    pub fn Set(&self, key: Key, value: Vec<u8>) -> Result<(), DriverError> {
        if key.len() + value.len() > self.settings.size_limits.entry {
            return Err(DriverError::Backend(
                "transaction entry is too large".to_owned(),
            ));
        }
        if self.mem_buffer.Size() + key.len() + value.len() > self.settings.size_limits.total {
            return Err(DriverError::Backend("transaction is too large".to_owned()));
        }
        self.mem_buffer.Set(key, value)
    }

    /// 暴露事务本地 memBuffer。
    pub fn GetMemBuffer(&self) -> Arc<memBuffer> {
        Arc::clone(&self.mem_buffer)
    }

    /// 设置事务选项；committer 工作时拒绝修改。
    pub fn SetOption(&mut self, option: TxnOption) -> Result<(), DriverError> {
        if self.is_committer_working.load(Ordering::SeqCst) {
            return Err(DriverError::CommitterWorking);
        }
        match option {
            TxnOption::SchemaChecker(value) => self.settings.schema_checker = Some(value),
            TxnOption::IsolationLevel(value) => self
                .snapshot
                .SetOption(SnapshotOption::IsolationLevel(value)),
            TxnOption::Priority(value) => self.snapshot.SetOption(SnapshotOption::Priority(value)),
            TxnOption::NotFillCache(value) => {
                self.snapshot.SetOption(SnapshotOption::NotFillCache(value))
            }
            TxnOption::Pessimistic(value) => self.settings.pessimistic = value,
            TxnOption::SnapshotTS(value) => {
                self.snapshot.SetOption(SnapshotOption::SnapshotTS(value))
            }
            TxnOption::ReplicaRead(value) => {
                self.snapshot.SetOption(SnapshotOption::ReplicaRead(value))
            }
            TxnOption::TaskID(value) => self.snapshot.SetOption(SnapshotOption::TaskID(value)),
            TxnOption::InfoSchema(value) => self.settings.info_schema_version = Some(value),
            TxnOption::CollectRuntimeStats(value) => self
                .snapshot
                .SetOption(SnapshotOption::CollectRuntimeStats(value)),
            TxnOption::SampleStep(value) => {
                self.snapshot.SetOption(SnapshotOption::SampleStep(value))
            }
            TxnOption::CommitHook(value) => self.commit_hook = Some(value),
            TxnOption::EnableAsyncCommit(value) => self.settings.enable_async_commit = value,
            TxnOption::Enable1PC(value) => self.settings.enable_1pc = value,
            TxnOption::GuaranteeLinearizability(value) => self.settings.causal_consistency = !value,
            TxnOption::TxnScope(value) => self.settings.txn_scope = value,
            TxnOption::IsStalenessReadOnly(value) => self
                .snapshot
                .SetOption(SnapshotOption::IsStalenessReadOnly(value)),
            TxnOption::MatchStoreLabels(value) => self
                .snapshot
                .SetOption(SnapshotOption::MatchStoreLabels(value)),
            TxnOption::ResourceGroupTag(value) => self.settings.resource_group_tag = value,
            TxnOption::ResourceGroupTagger(value) => {
                self.settings.resource_group_tagger = Some(value)
            }
            TxnOption::KVFilter(_) => {}
            TxnOption::SnapInterceptor(value) => self
                .snapshot
                .SetOption(SnapshotOption::SnapInterceptor(value)),
            TxnOption::CommitTSUpperBoundCheck(value) => {
                self.commit_ts_upper_bound_check = Some(value)
            }
            TxnOption::RPCInterceptor(value) => self.settings.rpc_interceptors.push(value),
            TxnOption::AssertionLevel(value) => self.settings.assertion_level = value,
            TxnOption::TableToColumnMaps(value) => self.column_maps_cache = Some(value),
            TxnOption::RequestSourceInternal(value) => {
                self.settings.request_source_internal = value
            }
            TxnOption::RequestSourceType(value) => self.settings.request_source_type = value,
            TxnOption::ExplicitRequestSourceType(value) => {
                self.settings.explicit_request_source_type = value
            }
            TxnOption::ReplicaReadAdjuster(value) => self
                .snapshot
                .SetOption(SnapshotOption::ReplicaReadAdjuster(value)),
            TxnOption::TxnSource(value) => self.settings.txn_source = value,
            TxnOption::ResourceGroupName(value) => self.settings.resource_group_name = value,
            TxnOption::LoadBasedReplicaReadThreshold(value) => self
                .snapshot
                .SetOption(SnapshotOption::LoadBasedReplicaReadThreshold(value)),
            TxnOption::TiKVClientReadTimeout(value) => self
                .snapshot
                .SetOption(SnapshotOption::TiKVClientReadTimeout(value)),
            TxnOption::SizeLimits(value) => self.settings.size_limits = value,
            TxnOption::SessionID(value) => self.settings.session_id = value,
            TxnOption::BackgroundGoroutineLifecycleHooks(value) => {
                self.settings.lifecycle_hooks = value
            }
            TxnOption::PrewriteEncounterLockPolicy(value) => {
                self.settings.prewrite_lock_policy = value
            }
        }
        Ok(())
    }

    /// 按名称查询部分事务选项（线性一致性、TxnScope、请求来源等）。
    pub fn GetOption(&self, option: &str) -> Option<TxnOptionValue> {
        match option {
            "GuaranteeLinearizability" => {
                Some(TxnOptionValue::Bool(!self.settings.causal_consistency))
            }
            "TxnScope" => Some(TxnOptionValue::String(self.settings.txn_scope.clone())),
            "RequestSourceInternal" => Some(TxnOptionValue::RequestSourceInternal(
                self.settings.request_source_internal,
            )),
            "RequestSourceType" => Some(TxnOptionValue::RequestSourceType(
                self.settings.request_source_type.clone(),
            )),
            _ => None,
        }
    }

    /// 返回表到列映射缓存（类型擦除）。
    pub fn TableToColumnMaps(&self) -> Option<&Arc<dyn Any + Send + Sync>> {
        self.column_maps_cache.as_ref()
    }

    /// 设置会话变量占位（当前仅接受 String）。
    pub fn SetVars(&mut self, variables: Box<dyn Any>) {
        if let Ok(variables) = variables.downcast::<String>() {
            self.vars = Some(*variables);
        }
    }

    /// 读取已设置的会话变量字符串。
    pub fn GetVars(&self) -> Option<&str> {
        self.vars.as_deref()
    }

    /// 将后端 KeyExists / WriteConflict 转为带表名信息的驱动错误。
    fn extract_key_error(&self, error: DriverError) -> DriverError {
        match error {
            DriverError::BackendKeyExists { key, value } => {
                self.extract_key_exists_error(&key, &value)
            }
            DriverError::WriteConflict(conflict) => {
                DriverError::Backend(newWriteConflictError(Some(conflict)).to_string())
            }
            error => error,
        }
    }

    /// 根据表键头解码 table/index，生成友好的唯一约束冲突错误。
    fn extract_key_exists_error(&self, key: &[u8], backend_value: &[u8]) -> DriverError {
        let Some((table_id, index_id, is_record)) = decode_table_key_head(key) else {
            return DriverError::KeyExists {
                value: format!("{key:?}"),
                name: "UNKNOWN".to_owned(),
            };
        };
        let Some(table) = self.GetTableInfo(table_id) else {
            return DriverError::KeyExists {
                value: format!("{key:?}"),
                name: "UNKNOWN".to_owned(),
            };
        };
        let value = if self.pipelined {
            backend_value.to_vec()
        } else {
            self.mem_buffer
                .Get(key, &[])
                .map(|entry| entry.value)
                .unwrap_or_default()
        };
        if value.is_empty() {
            return DriverError::KeyExists {
                value: format!("{key:?}"),
                name: "UNKNOWN".to_owned(),
            };
        }
        if is_record {
            ExtractKeyExistsErrFromHandle(key, &value, table)
        } else {
            ExtractKeyExistsErrFromIndex(
                key,
                &value,
                table,
                index_id & astersql_tablecodec::IndexIDMask,
            )
        }
    }

    /// 当存在 LockedWithConflict 时间戳时构造写冲突错误。
    fn generate_write_conflict_for_locked_with_conflict(
        &self,
        lock_context: &LockContext,
    ) -> Result<(), DriverError> {
        if lock_context.max_locked_with_conflict_ts == 0 {
            return Ok(());
        }
        let key = lock_context
            .values
            .iter()
            .find(|(_, value)| {
                value.locked_with_conflict_ts >= lock_context.max_locked_with_conflict_ts
            })
            .map(|(key, _)| key.clone())
            .unwrap_or_default();
        let (key_table_id, key_rest) = if key.is_empty() {
            ("<unknown>".to_owned(), String::new())
        } else {
            prettyWriteKey(&key)
        };
        Err(DriverError::WriteConflict(WriteConflict {
            start_ts: self.start_ts,
            conflict_ts: 0,
            conflict_commit_ts: lock_context.max_locked_with_conflict_ts,
            key,
            primary: Vec::new(),
            reason: "LockedWithConflict".to_owned(),
            key_table_id,
            key_rest,
            primary_table_id: " primary=<unknown>".to_owned(),
            primary_rest: String::new(),
        }))
    }

    /// 进入公平锁（Fair Locking）模式；next-gen kernel 上未实现。
    pub fn StartFairLocking(&mut self) -> Result<(), DriverError> {
        if self.next_gen_kernel {
            return Err(DriverError::NotImplemented);
        }
        self.fair_locking = true;
        Ok(())
    }

    /// 重试公平锁流程，重新打开 fair_locking 标志。
    pub fn RetryFairLocking(&mut self) -> Result<(), DriverError> {
        self.fair_locking = true;
        Ok(())
    }

    /// 取消公平锁模式。
    pub fn CancelFairLocking(&mut self) -> Result<(), DriverError> {
        self.fair_locking = false;
        Ok(())
    }

    /// 完成公平锁模式。
    pub fn DoneFairLocking(&mut self) -> Result<(), DriverError> {
        self.fair_locking = false;
        Ok(())
    }

    /// 是否处于公平锁模式。
    pub fn IsInFairLockingMode(&self) -> bool {
        self.fair_locking
    }

    /// 测试用：标记下一代 kernel，以触发 FairLocking 未实现分支。
    pub fn set_next_gen_kernel(&mut self, value: bool) {
        self.next_gen_kernel = value;
    }

    /// 流水线 DML 下尝试 Flush 缓冲；非 pipelined 直接成功。
    pub fn MayFlush(&self) -> Result<(), DriverError> {
        if !self.pipelined {
            return Ok(());
        }
        self.is_committer_working.store(true, Ordering::SeqCst);
        let result = self.mem_buffer.Flush().map(|_| ());
        self.is_committer_working.store(false, Ordering::SeqCst);
        result
    }

    /// 事务开始时间戳（start_ts）。
    pub fn StartTS(&self) -> u64 {
        self.start_ts
    }

    /// 提交时间戳（commit_ts）；未提交前为 0。
    pub fn CommitTS(&self) -> u64 {
        self.commit_ts
    }

    /// 是否启用流水线 DML。
    pub fn IsPipelined(&self) -> bool {
        self.pipelined
    }

    /// 事务是否仍有效（提交后为 false）。
    pub fn Valid(&self) -> bool {
        self.valid
    }
}

/// TiDB commit filter for untouched index key/value pairs.
/// 提交过滤器：识别未修改（untouched）索引键值，避免无意义写入。
#[derive(Clone, Copy, Debug, Default)]
pub struct TiDBKVFilter;

impl TiDBKVFilter {
    /// 判断键值是否无需提交；untouched 且带 PresumeKeyNotExists 则报错。
    pub fn IsUnnecessaryKeyValue(
        self,
        key: &[u8],
        value: &[u8],
        flags: KeyFlags,
    ) -> Result<bool, DriverError> {
        let is_untouched = is_untouched_index_key_value(key, value);
        if is_untouched && flags.has_presume_key_not_exists() {
            return Err(DriverError::Backend(format!(
                "unexpected untouched key={key:?} value={value:?} contains PresumeKeyNotExists flag keyFlags={}",
                flags.bits()
            )));
        }
        Ok(is_untouched)
    }
}

/// 按 TiDB 索引编码规则检测 untouched 标记（值尾部 '1' 等）。
fn is_untouched_index_key_value(key: &[u8], value: &[u8]) -> bool {
    if key.len() <= 11 || key.first() != Some(&b't') || key.get(10) != Some(&b'i') {
        return false;
    }
    let value_length = value.len();
    if value_length <= 9 {
        return matches!(value_length, 1 | 4 | 9) && value.last() == Some(&b'1');
    }
    let tail_length = value.first().copied().unwrap_or_default() as usize;
    if tail_length < 8 {
        tail_length >= 1 && value.last() == Some(&b'1')
    } else {
        tail_length == 9
    }
}

/// Helper used by callers constructing unique-index metadata.
/// 构造带单个索引的 TableInfo，便于测试与错误路径。
pub fn table_with_index(id: i64, name: impl Into<String>, index: IndexInfo) -> TableInfo {
    let name = name.into();
    let mut table = TableInfo::default();
    table.ID = id;
    table.Name.O = name.clone();
    table.Name.L = name.to_lowercase();
    table.Indices = vec![index];
    table
}

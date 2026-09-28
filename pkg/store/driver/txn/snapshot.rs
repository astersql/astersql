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

// TiKV 事务快照（snapshot）驱动适配。
//
// 快照是事务开始时的一致性读视图（对应 MVCC 在某一 start_ts / snapshot_ts 下可见的数据）。
// 本模块保留 TiDB 侧的隔离级别、副本读、资源组、拦截器等选项语义，并提供 Get / BatchGet / Iter。

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use crate::{
    BatchGetOption, BatchGetter, DriverError, GetOption, Getter, Key, KvIterator, ValueEntry,
    tikvScanner,
};

/// TiDB 侧事务隔离级别（Isolation Level）：决定并发事务间可见性规则。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum IsoLevel {
    #[default]
    SI,
    RC,
    RCCheckTS,
}

/// 映射到 TiKV client 的隔离级别枚举。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TiKVIsolationLevel {
    #[default]
    SI,
    RC,
    RCCheckTS,
}

/// 请求优先级，影响 TiKV 调度侧资源倾斜。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Priority {
    High,
    Low,
    #[default]
    Normal,
}

/// 副本读（Replica Read）策略：可读 Leader、Follower 或混合，用于降低 Leader 读压力。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ReplicaReadType {
    #[default]
    Leader,
    Follower,
    Mixed,
}

/// 快照运行时统计，例如 RPC 次数，供诊断与观测。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SnapshotRuntimeStats {
    pub rpc_count: u64,
}

/// 资源组（Resource Group）标签生成方式：用于配额与限流标记。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResourceGroupTagger {
    Proto(String),
    Builder(String),
}

/// 设置到快照上的单项选项（对应 Go 的 `kv.Option` / SetOption 分支）。
#[derive(Clone)]
pub enum SnapshotOption {
    IsolationLevel(IsoLevel),
    Priority(i32),
    NotFillCache(bool),
    SnapshotTS(u64),
    ReplicaRead(ReplicaReadType),
    SampleStep(u32),
    TaskID(u64),
    CollectRuntimeStats(Option<SnapshotRuntimeStats>),
    IsStalenessReadOnly(bool),
    MatchStoreLabels(Vec<(String, String)>),
    ResourceGroupTag(Vec<u8>),
    ResourceGroupTagger(ResourceGroupTagger),
    ReadReplicaScope(String),
    SnapInterceptor(Arc<dyn SnapshotInterceptor>),
    RPCInterceptor(String),
    RequestSourceInternal(bool),
    RequestSourceType(String),
    ExplicitRequestSourceType(String),
    ReplicaReadAdjuster(String),
    ScanBatchSize(usize),
    ResourceGroupName(String),
    LoadBasedReplicaReadThreshold(Duration),
    TiKVClientReadTimeout(u64),
}

/// 已解析并固化的快照选项集合，供实际读路径读取。
#[derive(Clone, Debug)]
pub struct SnapshotOptions {
    pub isolation_level: TiKVIsolationLevel,
    pub priority: Priority,
    pub not_fill_cache: bool,
    pub snapshot_ts: u64,
    pub replica_read: ReplicaReadType,
    pub sample_step: u32,
    pub task_id: u64,
    pub runtime_stats: Option<SnapshotRuntimeStats>,
    pub is_staleness_read_only: bool,
    pub match_store_labels: Vec<(String, String)>,
    pub resource_group_tag: Vec<u8>,
    pub resource_group_tagger: Option<ResourceGroupTagger>,
    pub read_replica_scope: String,
    pub rpc_interceptors: Vec<String>,
    pub request_source_internal: bool,
    pub request_source_type: String,
    pub explicit_request_source_type: String,
    pub replica_read_adjuster: Option<String>,
    pub scan_batch_size: usize,
    pub resource_group_name: String,
    pub load_based_replica_read_threshold: Duration,
    pub kv_read_timeout: Duration,
}

impl Default for SnapshotOptions {
    fn default() -> Self {
        Self {
            isolation_level: TiKVIsolationLevel::SI,
            priority: Priority::Normal,
            not_fill_cache: false,
            snapshot_ts: 0,
            replica_read: ReplicaReadType::Leader,
            sample_step: 0,
            task_id: 0,
            runtime_stats: None,
            is_staleness_read_only: false,
            match_store_labels: Vec::new(),
            resource_group_tag: Vec::new(),
            resource_group_tagger: None,
            read_replica_scope: String::new(),
            rpc_interceptors: Vec::new(),
            request_source_internal: false,
            request_source_type: String::new(),
            explicit_request_source_type: String::new(),
            replica_read_adjuster: None,
            scan_batch_size: 0,
            resource_group_name: String::new(),
            load_based_replica_read_threshold: Duration::ZERO,
            kv_read_timeout: Duration::ZERO,
        }
    }
}

/// 快照读拦截器：可在 Get / BatchGet / Iter 前注入自定义逻辑（如故障注入、审计）。
pub trait SnapshotInterceptor: Send + Sync {
    fn on_batch_get(
        &self,
        snapshot: &tikvSnapshot,
        keys: &[Key],
        options: &[BatchGetOption],
    ) -> Result<HashMap<Key, ValueEntry>, DriverError>;

    fn on_get(
        &self,
        snapshot: &tikvSnapshot,
        key: &[u8],
        options: &[GetOption],
    ) -> Result<ValueEntry, DriverError>;

    fn on_iter(
        &self,
        snapshot: &tikvSnapshot,
        key: &[u8],
        upper_bound: Option<&[u8]>,
    ) -> Result<Box<dyn KvIterator>, DriverError>;

    fn on_iter_reverse(
        &self,
        snapshot: &tikvSnapshot,
        key: Option<&[u8]>,
        lower_bound: Option<&[u8]>,
    ) -> Result<Box<dyn KvIterator>, DriverError>;
}

/// Immutable transaction snapshot with TiDB option and interceptor semantics.
/// 不可变事务快照：持有底层 KV 视图、选项与可选拦截器。
#[derive(Clone)]
pub struct tikvSnapshot {
    data: Arc<RwLock<BTreeMap<Key, ValueEntry>>>,
    options: SnapshotOptions,
    interceptor: Option<Arc<dyn SnapshotInterceptor>>,
}

/// 从共享有序映射构造默认选项的快照。
pub fn NewSnapshot(data: Arc<RwLock<BTreeMap<Key, ValueEntry>>>) -> tikvSnapshot {
    tikvSnapshot {
        data,
        options: SnapshotOptions::default(),
        interceptor: None,
    }
}

impl tikvSnapshot {
    /// 由键值条目列表构造内存快照（测试与本地驱动常用）。
    pub fn from_entries(entries: impl IntoIterator<Item = (Key, ValueEntry)>) -> Self {
        NewSnapshot(Arc::new(RwLock::new(entries.into_iter().collect())))
    }

    /// 返回当前已设置的快照选项。
    pub fn options(&self) -> &SnapshotOptions {
        &self.options
    }

    /// 批量读取：若存在拦截器则先剥离拦截器再回调，避免递归拦截。
    pub fn BatchGet(
        &self,
        keys: &[Key],
        options: &[BatchGetOption],
    ) -> Result<HashMap<Key, ValueEntry>, DriverError> {
        if let Some(interceptor) = &self.interceptor {
            // 传入无拦截器的 plain 快照，与 Go 侧语义一致。
            let mut plain = self.clone();
            plain.interceptor = None;
            return interceptor.on_batch_get(&plain, keys, options);
        }
        self.batch_get(keys, options)
    }

    /// 单键读取，语义同 BatchGet 的拦截器处理。
    pub fn Get(&self, key: &[u8], options: &[GetOption]) -> Result<ValueEntry, DriverError> {
        if let Some(interceptor) = &self.interceptor {
            let mut plain = self.clone();
            plain.interceptor = None;
            return interceptor.on_get(&plain, key, options);
        }
        self.get(key, options)
    }

    /// 正向范围扫描：`[key, upper_bound)`，结果物化为 `tikvScanner`。
    pub fn Iter(
        &self,
        key: &[u8],
        upper_bound: Option<&[u8]>,
    ) -> Result<Box<dyn KvIterator>, DriverError> {
        if let Some(interceptor) = &self.interceptor {
            let mut plain = self.clone();
            plain.interceptor = None;
            return interceptor.on_iter(&plain, key, upper_bound);
        }
        Ok(Box::new(tikvScanner::new(self.range_rows(
            key,
            upper_bound,
            false,
        ))))
    }

    /// 反向范围扫描：从上界 `key` 向下扫到 `lower_bound`。
    pub fn IterReverse(
        &self,
        key: Option<&[u8]>,
        lower_bound: Option<&[u8]>,
    ) -> Result<Box<dyn KvIterator>, DriverError> {
        if let Some(interceptor) = &self.interceptor {
            let mut plain = self.clone();
            plain.interceptor = None;
            return interceptor.on_iter_reverse(&plain, key, lower_bound);
        }
        Ok(Box::new(tikvScanner::new(
            self.range_rows_reverse(key, lower_bound),
        )))
    }

    /// 应用单项快照选项；`ScanBatchSize(0)` 被忽略以对齐 Go 行为。
    pub fn SetOption(&mut self, option: SnapshotOption) {
        match option {
            SnapshotOption::IsolationLevel(level) => {
                self.options.isolation_level = getTiKVIsolationLevel(level)
            }
            SnapshotOption::Priority(priority) => self.options.priority = getTiKVPriority(priority),
            SnapshotOption::NotFillCache(value) => self.options.not_fill_cache = value,
            SnapshotOption::SnapshotTS(value) => self.options.snapshot_ts = value,
            SnapshotOption::ReplicaRead(value) => self.options.replica_read = value,
            SnapshotOption::SampleStep(value) => self.options.sample_step = value,
            SnapshotOption::TaskID(value) => self.options.task_id = value,
            SnapshotOption::CollectRuntimeStats(value) => self.options.runtime_stats = value,
            SnapshotOption::IsStalenessReadOnly(value) => {
                self.options.is_staleness_read_only = value
            }
            SnapshotOption::MatchStoreLabels(value) => self.options.match_store_labels = value,
            SnapshotOption::ResourceGroupTag(value) => self.options.resource_group_tag = value,
            SnapshotOption::ResourceGroupTagger(value) => {
                self.options.resource_group_tagger = Some(value)
            }
            SnapshotOption::ReadReplicaScope(value) => self.options.read_replica_scope = value,
            SnapshotOption::SnapInterceptor(value) => self.interceptor = Some(value),
            SnapshotOption::RPCInterceptor(value) => self.options.rpc_interceptors.push(value),
            SnapshotOption::RequestSourceInternal(value) => {
                self.options.request_source_internal = value
            }
            SnapshotOption::RequestSourceType(value) => self.options.request_source_type = value,
            SnapshotOption::ExplicitRequestSourceType(value) => {
                self.options.explicit_request_source_type = value
            }
            SnapshotOption::ReplicaReadAdjuster(value) => {
                self.options.replica_read_adjuster = Some(value)
            }
            SnapshotOption::ScanBatchSize(value) if value > 0 => {
                self.options.scan_batch_size = value
            }
            SnapshotOption::ScanBatchSize(_) => {}
            SnapshotOption::ResourceGroupName(value) => self.options.resource_group_name = value,
            SnapshotOption::LoadBasedReplicaReadThreshold(value) => {
                self.options.load_based_replica_read_threshold = value
            }
            SnapshotOption::TiKVClientReadTimeout(milliseconds) => {
                self.options.kv_read_timeout = Duration::from_millis(milliseconds)
            }
        }
    }

    /// 从内存 BTreeMap 收集正向范围行，可选再反转。
    fn range_rows(
        &self,
        key: &[u8],
        upper_bound: Option<&[u8]>,
        reverse: bool,
    ) -> Vec<(Key, Vec<u8>)> {
        let data = self
            .data
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut rows = data
            .iter()
            .filter(|(candidate, _)| {
                candidate.as_slice() >= key
                    && upper_bound.is_none_or(|upper| candidate.as_slice() < upper)
            })
            .map(|(key, value)| (key.clone(), value.value.clone()))
            .collect::<Vec<_>>();
        if reverse {
            rows.reverse();
        }
        rows
    }

    /// 逆序遍历 BTreeMap 并按上下界过滤。
    fn range_rows_reverse(
        &self,
        key: Option<&[u8]>,
        lower_bound: Option<&[u8]>,
    ) -> Vec<(Key, Vec<u8>)> {
        let data = self
            .data
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        data.iter()
            .rev()
            .filter(|(candidate, _)| {
                key.is_none_or(|upper| candidate.as_slice() < upper)
                    && lower_bound.is_none_or(|lower| candidate.as_slice() >= lower)
            })
            .map(|(key, value)| (key.clone(), value.value.clone()))
            .collect()
    }
}

impl Getter for tikvSnapshot {
    fn get(&self, key: &[u8], options: &[GetOption]) -> Result<ValueEntry, DriverError> {
        let entry = self
            .data
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(key)
            .cloned()
            .ok_or(DriverError::NotFound)?;
        Ok(crate::apply_commit_ts_option(entry, options))
    }
}

impl BatchGetter for tikvSnapshot {
    fn batch_get(
        &self,
        keys: &[Key],
        options: &[BatchGetOption],
    ) -> Result<HashMap<Key, ValueEntry>, DriverError> {
        if let Some(interceptor) = &self.interceptor {
            let mut plain = self.clone();
            plain.interceptor = None;
            return interceptor.on_batch_get(&plain, keys, options);
        }
        let data = self
            .data
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(keys
            .iter()
            .filter_map(|key| {
                data.get(key).cloned().map(|value| {
                    (
                        key.clone(),
                        crate::apply_commit_ts_option_batch(value, options),
                    )
                })
            })
            .collect())
    }
}

/// 将 TiDB Key 切片转为 TiKV 字节键切片（当前为浅拷贝等价）。
pub fn toTiKVKeys(keys: &[Key]) -> Vec<Vec<u8>> {
    keys.to_vec()
}

/// TiDB `IsoLevel` → TiKV client 隔离级别。
pub fn getTiKVIsolationLevel(level: IsoLevel) -> TiKVIsolationLevel {
    match level {
        IsoLevel::SI => TiKVIsolationLevel::SI,
        IsoLevel::RC => TiKVIsolationLevel::RC,
        IsoLevel::RCCheckTS => TiKVIsolationLevel::RCCheckTS,
    }
}

/// 将 TiDB `kv.Priority` 的 iota 编码映射为 `Priority`（1=Low, 2=High）。
pub fn getTiKVPriority(priority: i32) -> Priority {
    match priority {
        1 => Priority::Low,
        2 => Priority::High,
        _ => Priority::Normal,
    }
}

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

// Domain：实例级域对象，管理 DDL、InfoSchema、统计信息、权限、server_id 与后台 worker 生命周期。
// InfoSchema 是库表元数据的版本化快照；schema lease 控制其缓存过期。
// 统计（stats）为优化器提供行数/直方图等代价估计数据；MVCC 快照读可按时间戳取历史 schema。

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use astersql_infoschema::{InfoCache, SchemaRef};
use astersql_meta_autoid::{
    Allocator as AutoIdAllocator, AllocatorOption, AllocatorType, Context as AutoIdContext,
    DefaultAllocator, IdStore,
};
use astersql_sessionctx_vardef as vardef;
use astersql_statistics_asyncload as asyncload;
pub use astersql_statistics_handle::lockstats::SqlValue;
use astersql_statistics_handle::{
    AnalyzeStatsStorage, Bucket as StatsBucket, EffectiveAutoAnalyzeMinCnt,
    Error as StatsHandleError, HandleBackend, HistoricalStatsMeta, RuntimeAnalyzeJob,
    RuntimeColumnUsage, RuntimeHistoricalSnapshot, StatsMetaRow, StatsTableKey, TableStats,
    dump_stats_delta_ratio, lockstats, storage as stats_storage,
};
pub use astersql_statistics_handle::{
    Handle, HistoricalMetaPanicGuard, KvStatsStore, StatsKvStorage,
};

use crate::autoid_store::KvAutoIdStore;
use crate::canonical_domain::{
    DdlMetadataChange, DdlMetadataService, InfoSchemaLoader, LoadedInfoSchema, StorageHandle,
};
use crate::historical_stats::{HistoricalStatsStore, HistoricalStatsWorker};

/// Server-ID lease lifetime. It must outlive the lost-PD detection window so an
/// unreachable node stops serving before another node can reuse its ID.
pub const SERVER_ID_TTL: Duration = Duration::from_secs(12 * 60 * 60);
/// Maximum continuous PD outage before this node considers its server ID lost.
pub const LOST_CONNECTION_TO_PD_TIMEOUT: Duration = Duration::from_secs(6 * 60 * 60);

/// Information-schema cache expired before it could be refreshed.
pub static ERR_INFO_SCHEMA_EXPIRED: LazyLock<Box<astersql_util_dbterror::terror::Error>> =
    LazyLock::new(|| {
        astersql_util_dbterror::ClassDomain.NewStd(astersql_errno::errcode::ErrInfoSchemaExpired)
    });

/// Information schema changed while a statement was executing.
pub static ERR_INFO_SCHEMA_CHANGED: LazyLock<Box<astersql_util_dbterror::terror::Error>> =
    LazyLock::new(|| {
        let source =
            &astersql_errno::errname::MySQLErrName[&astersql_errno::errcode::ErrInfoSchemaChanged];
        let message = astersql_errno::mysql::Message(
            &format!("{}. {}", source.Raw, astersql_kv::error::TxnRetryableMark),
            &source.RedactArgPos,
        );
        astersql_util_dbterror::ClassDomain
            .NewStdErr(astersql_errno::errcode::ErrInfoSchemaChanged, &message)
    });

/// DDL 服务抽象：启动/停止、查询 Owner、以及 Alter Table Mode 入口。
/// DDL Owner 是集群中唯一负责推进 DDL 任务的节点。
pub trait DdlService: Send + Sync {
    /// Whether this service owns the public schema synchronization loops.
    fn manages_schema_sync(&self) -> bool {
        false
    }

    /// 按 StartMode 启动 Domain 与后台 worker。
    fn start(&self, mode: StartMode) -> Result<(), String>;
    /// 停止后台 worker。
    fn stop(&self) -> Result<(), String>;
    fn owner_id(&self) -> Option<String>;
    fn alter_table_mode(&self, target: &str) -> Result<(), String>;
    /// Normal SQL DDL uses the durable owner queue when supported by this service.
    fn supports_persistent_actions(&self) -> bool {
        false
    }
    fn submit_persistent_job(
        &self,
        _: &mut astersql_meta_model::group_3::Job,
    ) -> Result<(), String> {
        Err("normal persistent DDL submission unavailable".into())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Domain/DDL 启动模式。
pub enum StartMode {
    /// 常规启动。
    Normal,
    /// 引导初始化。
    Bootstrap,
    /// 升级路径。
    Upgrade,
    /// 恢复路径。
    Restore,
}

#[derive(Clone, Debug)]
/// Domain 配置：schema/stats 租约、缓存容量、server_id TTL、所属 keyspace 等。
/// 租约（lease）决定缓存/后台任务的刷新与过期周期。
pub struct DomainConfig {
    /// InfoSchema 缓存租约时长。
    pub schema_lease: Duration,
    /// 统计信息刷新租约时长。
    pub stats_lease: Duration,
    /// 转储文件 GC 周期。
    pub dump_file_gc_lease: Duration,
    /// InfoCache 容量。
    pub info_cache_capacity: usize,
    /// 统计缓存容量。
    pub stats_cache_capacity: i64,
    /// 慢查询 Top-N 容量。
    pub slow_query_top_n: usize,
    /// 慢查询近期窗口容量。
    pub slow_query_recent: usize,
    /// server_id 租约 TTL。
    pub server_id_ttl: Duration,
    /// 本 Domain 所属 keyspace。
    pub keyspace: String,
}

impl Default for DomainConfig {
    fn default() -> Self {
        Self {
            schema_lease: Duration::from_secs(45),
            stats_lease: Duration::from_secs(3),
            dump_file_gc_lease: Duration::from_secs(60),
            info_cache_capacity: 16,
            stats_cache_capacity: i64::MAX,
            slow_query_top_n: 30,
            slow_query_recent: 500,
            server_id_ttl: Duration::from_secs(60),
            keyspace: "SYSTEM".to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// Domain 运行期错误分类。
pub enum DomainError {
    /// Domain 已关闭。
    Closed,
    /// 重复启动。
    AlreadyStarted,
    /// 尚未初始化。
    NotInitialized,
    /// 存储层错误。
    Store(String),
    /// DDL 子系统错误。
    Ddl(String),
    /// 找不到指定 keyspace。
    KeyspaceNotFound(String),
    /// keyspace 运行时已被占用。
    RuntimeInUse,
    /// 权限事件编码非法。
    InvalidPrivilegeEvent,
    /// server_id 空间耗尽。
    ServerIdExhausted,
    /// server_id 冲突。
    ServerIdConflict,
    /// 后台 worker 错误。
    Worker(String),
    /// 进程已存在。
    ProcessExists(u64),
    /// 进程不存在。
    ProcessNotFound(u64),
    /// 统计子系统错误。
    Stats(String),
}

impl std::fmt::Display for DomainError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for DomainError {}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 慢查询记录：SQL 文本、digest、耗时与是否内部语句。
pub struct SlowQueryInfo {
    pub sql: String,
    pub digest: String,
    pub duration_ms: u64,
    pub start: SystemTime,
    pub internal: bool,
}

#[derive(Debug)]
/// 慢查询 Top-N 与近期环形缓冲状态。
struct SlowQueryState {
    top: Vec<SlowQueryInfo>,
    recent: VecDeque<SlowQueryInfo>,
    top_capacity: usize,
    recent_capacity: usize,
}

impl SlowQueryState {
    /// 使用存储与 schema loader 构造 Domain。
    fn new(top_capacity: usize, recent_capacity: usize) -> Self {
        Self {
            top: Vec::new(),
            recent: VecDeque::new(),
            top_capacity,
            recent_capacity,
        }
    }
    /// 追加慢查询并维护 Top-N。
    fn add(&mut self, query: SlowQueryInfo) {
        self.recent.push_back(query.clone());
        while self.recent.len() > self.recent_capacity {
            self.recent.pop_front();
        }
        self.top.push(query);
        self.top
            .sort_by(|left, right| right.duration_ms.cmp(&left.duration_ms));
        self.top.truncate(self.top_capacity);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// 慢查询查询种类：Top、Recent 或全部。
pub enum SlowQueryKind {
    /// 耗时 Top-N。
    Top,
    /// 近期窗口。
    Recent,
    /// Top 与 Recent 合并视图。
    All,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 权限缓存刷新事件（全量或按用户）。
pub struct PrivilegeEvent {
    pub event_type: PrivilegeEventType,
    pub users: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// 权限事件类型。
pub enum PrivilegeEventType {
    /// 刷新全部权限。
    UpdateAll,
    /// 仅刷新指定用户。
    UpdateUsers,
}

impl PrivilegeEvent {
    /// 编码权限事件为字符串。
    pub fn encode(&self) -> String {
        match self.event_type {
            PrivilegeEventType::UpdateAll => "all".to_string(),
            PrivilegeEventType::UpdateUsers => format!("users:{}", self.users.join(",")),
        }
    }

    /// 从字符串解码权限事件。
    pub fn decode(value: &str) -> Result<Self, DomainError> {
        if value == "all" {
            return Ok(Self {
                event_type: PrivilegeEventType::UpdateAll,
                users: Vec::new(),
            });
        }
        let users = value
            .strip_prefix("users:")
            .ok_or(DomainError::InvalidPrivilegeEvent)?
            .split(',')
            .filter(|user| !user.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();
        if users.is_empty() {
            Err(DomainError::InvalidPrivilegeEvent)
        } else {
            Ok(Self {
                event_type: PrivilegeEventType::UpdateUsers,
                users,
            })
        }
    }

    /// 合并权限事件（UpdateAll 优先）。
    pub fn merge(&mut self, next: Self) {
        if self.event_type == PrivilegeEventType::UpdateAll
            || next.event_type == PrivilegeEventType::UpdateAll
        {
            self.event_type = PrivilegeEventType::UpdateAll;
            self.users.clear();
            return;
        }
        let mut users: BTreeSet<String> = self.users.iter().cloned().collect();
        users.extend(next.users);
        self.users = users.into_iter().collect();
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 节点资源画像：CPU/内存/磁盘。
pub struct NodeResource {
    pub cpu_count: u32,
    pub memory_bytes: u64,
    pub disk_bytes: u64,
}

/// 根据探测值构造 NodeResource（CPU 至少为 1）。
pub fn calculate_node_resource(
    cpu_count: usize,
    memory_bytes: u64,
    disk_bytes: u64,
) -> NodeResource {
    NodeResource {
        cpu_count: cpu_count.max(1).min(u32::MAX as usize) as u32,
        memory_bytes,
        disk_bytes,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 实例级简单 LRU 风格执行计划缓存（plan cache）。
/// 执行计划是优化器为 SQL 生成的可执行路径。
pub struct PlanCache {
    capacity: usize,
    entries: VecDeque<(String, String)>,
}

impl PlanCache {
    /// 使用存储与 schema loader 构造 Domain。
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            entries: VecDeque::new(),
        }
    }
    /// 写入计划缓存项（LRU）。
    pub fn put(&mut self, key: String, plan: String) {
        self.entries.retain(|(old, _)| old != &key);
        self.entries.push_back((key, plan));
        // 超出容量则从最旧端弹出（近似 LRU）。
        while self.entries.len() > self.capacity {
            self.entries.pop_front();
        }
    }
    /// 读取计划缓存项并提升为最近使用。
    pub fn get(&mut self, key: &str) -> Option<String> {
        let position = self.entries.iter().position(|(old, _)| old == key)?;
        let entry = self.entries.remove(position)?;
        let plan = entry.1.clone();
        self.entries.push_back(entry);
        Some(plan)
    }
    /// 进程数量。
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Return a stable snapshot for INFORMATION_SCHEMA readers.
    pub fn snapshot(&self) -> Vec<(String, String)> {
        self.entries.iter().cloned().collect()
    }
}

#[derive(Clone, Debug)]
/// server_id 租约：持有唯一实例 ID 及其过期时间。
struct ServerIdLease {
    id: u64,
    expires_at: Instant,
}

#[derive(Clone)]
/// 单个 keyspace 的 InfoCache 运行时及 holder 集合。
pub struct KeyspaceRuntime {
    pub keyspace: String,
    pub info_cache: Arc<InfoCache>,
    holders: Arc<Mutex<BTreeSet<String>>>,
}

impl KeyspaceRuntime {
    /// 登记 holder 并返回运行时句柄。
    pub fn acquire(&self, holder: &str) -> Result<KeyspaceRuntimeHandle, DomainError> {
        let mut holders = self.holders.lock().expect("keyspace holder lock poisoned");
        if !holders.insert(holder.to_string()) {
            return Err(DomainError::RuntimeInUse);
        }
        Ok(KeyspaceRuntimeHandle {
            runtime: self.clone(),
            holder: holder.to_string(),
        })
    }
}

/// KeyspaceRuntime 的持有句柄；Drop 时释放 holder。
pub struct KeyspaceRuntimeHandle {
    runtime: KeyspaceRuntime,
    holder: String,
}
impl KeyspaceRuntimeHandle {
    /// 取该 KS 最新 InfoSchema。
    pub fn info_schema(&self) -> SchemaRef {
        self.runtime
            .info_cache
            .GetLatest()
            .expect("keyspace runtime must contain an infoschema")
    }
}
impl Drop for KeyspaceRuntimeHandle {
    fn drop(&mut self) {
        self.runtime
            .holders
            .lock()
            .expect("keyspace holder lock poisoned")
            .remove(&self.holder);
    }
}

/// 后台 worker 句柄：停止标志与 JoinHandle。
struct WorkerHandle {
    name: String,
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl WorkerHandle {
    /// 停止后台 worker。
    fn stop(mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(join) = self.join.take() {
            join.thread().unpark();
            let _ = join.join();
        }
    }
}

/// Cross-keyspace schema-sync result exposed to integration tests.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[allow(non_snake_case)]
pub struct CrossKeyspaceSyncSummary {
    pub ServerCount: usize,
    pub AssumedServerCount: usize,
}

/// Collation decision captured when an ADD INDEX backfill is initialized.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[allow(non_snake_case)]
pub struct BackfillCollationResolution {
    pub DefaultUseNewCollation: bool,
    pub UseNewCollation: bool,
    pub ReorgUseNewCollation: bool,
}

#[derive(Clone, Debug, Default)]
struct CrossKeyspaceRuntimeState {
    new_collation: bool,
    last_sync_summary: Option<CrossKeyspaceSyncSummary>,
    last_backfill_collation: Option<BackfillCollationResolution>,
    last_ddl_job_id: Option<i64>,
    last_ddl_task_key: Option<String>,
}

#[derive(Debug, Default)]
struct CrossKeyspaceCoordinatorState {
    runtimes: BTreeMap<String, CrossKeyspaceRuntimeState>,
    all_user_runtimes: BTreeSet<String>,
    registered_users: BTreeSet<String>,
    leased_users: BTreeSet<String>,
    runtime_leases: BTreeMap<String, usize>,
    runtime_generations: BTreeMap<String, u64>,
    system_tasks: BTreeSet<String>,
    next_job_id: i64,
}

/// Shared coordinator for canonical Domains participating in one test cluster.
///
/// It records outcomes produced by real Domain DDL publication. Tests may read
/// snapshots, but cannot directly manufacture sync summaries or task records.
#[derive(Debug, Default)]
pub struct CrossKeyspaceCoordinator {
    state: Mutex<CrossKeyspaceCoordinatorState>,
}

/// Lease returned when SYSTEM acquires a user-keyspace runtime.
///
/// Dropping the final lease schedules the same idle-timeout eviction performed
/// by TiDB's cross-keyspace runtime GC loop.
pub struct CrossKeyspaceRuntimeHandle {
    coordinator: Arc<CrossKeyspaceCoordinator>,
    keyspace: String,
    idle_timeout: Duration,
}

impl Drop for CrossKeyspaceRuntimeHandle {
    fn drop(&mut self) {
        let generation = {
            let mut state = self
                .coordinator
                .state
                .lock()
                .expect("cross-keyspace coordinator lock poisoned");
            let leases = state
                .runtime_leases
                .get_mut(&self.keyspace)
                .expect("acquired cross-keyspace runtime must have a lease");
            *leases = leases.saturating_sub(1);
            if *leases != 0 {
                return;
            }
            let generation = state
                .runtime_generations
                .entry(self.keyspace.clone())
                .or_default();
            *generation += 1;
            *generation
        };

        let coordinator = Arc::clone(&self.coordinator);
        let keyspace = self.keyspace.clone();
        let idle_timeout = self.idle_timeout;
        thread::spawn(move || {
            thread::sleep(idle_timeout);
            let mut state = coordinator
                .state
                .lock()
                .expect("cross-keyspace coordinator lock poisoned");
            let still_idle = state.runtime_leases.get(&keyspace).copied().unwrap_or(0) == 0;
            let same_generation =
                state.runtime_generations.get(&keyspace).copied() == Some(generation);
            if still_idle && same_generation {
                state.leased_users.remove(&keyspace);
            }
        });
    }
}

impl CrossKeyspaceCoordinator {
    pub fn new() -> Self {
        Self::default()
    }

    fn bind(&self, keyspace: &str, new_collation: bool) {
        const SYSTEM: &str = "SYSTEM";
        let mut state = self
            .state
            .lock()
            .expect("cross-keyspace coordinator lock poisoned");
        if !keyspace.eq_ignore_ascii_case(SYSTEM) {
            state.all_user_runtimes.insert(keyspace.to_owned());
        }
        state
            .runtimes
            .entry(keyspace.to_owned())
            .and_modify(|runtime| runtime.new_collation = new_collation)
            .or_insert_with(|| CrossKeyspaceRuntimeState {
                new_collation,
                ..Default::default()
            });
    }

    fn record_ddl(&self, keyspace: &str, database: &str, add_index: bool) {
        const SYSTEM: &str = "SYSTEM";
        let is_system = keyspace.eq_ignore_ascii_case(SYSTEM);
        let is_system_schema = database.eq_ignore_ascii_case("mysql");
        let mut state = self
            .state
            .lock()
            .expect("cross-keyspace coordinator lock poisoned");

        // A user runtime becomes visible from SYSTEM after its first ordinary
        // user-table DDL. Merely touching mysql.* does not eagerly register it.
        if !is_system && !is_system_schema {
            state.registered_users.insert(keyspace.to_owned());
        }

        let user_is_registered = state.registered_users.contains(keyspace);
        let user_runtime_count = state.all_user_runtimes.len();
        let summary = if !is_system_schema {
            CrossKeyspaceSyncSummary {
                ServerCount: 1,
                AssumedServerCount: 0,
            }
        } else if is_system {
            CrossKeyspaceSyncSummary {
                ServerCount: 1 + user_runtime_count,
                AssumedServerCount: user_runtime_count,
            }
        } else if user_is_registered {
            CrossKeyspaceSyncSummary {
                ServerCount: 2,
                AssumedServerCount: 1,
            }
        } else {
            CrossKeyspaceSyncSummary {
                ServerCount: 1,
                AssumedServerCount: 0,
            }
        };

        let (job_id, task_key) = if add_index {
            state.next_job_id += 1;
            let job_id = state.next_job_id;
            // Match ddl::index::TaskKeyBuilder's backfill namespace. The task
            // registry must use the same key that DDL callers use for lookup.
            let task_key = format!("ddl/backfill/{job_id}");
            state.system_tasks.insert(task_key.clone());
            (Some(job_id), Some(task_key))
        } else {
            (None, None)
        };

        let runtime = state
            .runtimes
            .get_mut(keyspace)
            .expect("bound cross-keyspace runtime");
        runtime.last_sync_summary = Some(summary);
        if let (Some(job_id), Some(task_key)) = (job_id, task_key) {
            runtime.last_ddl_job_id = Some(job_id);
            runtime.last_ddl_task_key = Some(task_key);
            runtime.last_backfill_collation = Some(BackfillCollationResolution {
                DefaultUseNewCollation: true,
                UseNewCollation: runtime.new_collation,
                ReorgUseNewCollation: false,
            });
        }
    }

    fn acquire_runtime(
        self: &Arc<Self>,
        keyspace: &str,
        idle_timeout: Duration,
    ) -> Result<CrossKeyspaceRuntimeHandle, String> {
        const SYSTEM: &str = "SYSTEM";
        if keyspace.eq_ignore_ascii_case(SYSTEM) {
            return Err("SYSTEM cannot acquire itself as a cross-keyspace runtime".to_owned());
        }
        let mut state = self
            .state
            .lock()
            .expect("cross-keyspace coordinator lock poisoned");
        if !state.runtimes.contains_key(keyspace) {
            return Err(format!("unknown cross-keyspace runtime {keyspace}"));
        }
        state.leased_users.insert(keyspace.to_owned());
        *state.runtime_leases.entry(keyspace.to_owned()).or_default() += 1;
        *state
            .runtime_generations
            .entry(keyspace.to_owned())
            .or_default() += 1;
        drop(state);
        Ok(CrossKeyspaceRuntimeHandle {
            coordinator: Arc::clone(self),
            keyspace: keyspace.to_owned(),
            idle_timeout,
        })
    }
}

#[derive(Clone)]
struct CrossKeyspaceBinding {
    coordinator: Arc<CrossKeyspaceCoordinator>,
    keyspace: String,
}

/// Durable ADD INDEX owner-handoff state retained by a Domain.
///
/// The index metadata itself is not public until every physical partition has
/// advanced its checkpoint.  A replacement SQL session can therefore resume
/// from `next_partition` after the previous owner exits at a failpoint.
#[derive(Clone)]
pub struct PendingAddIndexJob {
    pub database: String,
    pub table: String,
    pub index: astersql_meta_model::IndexInfo,
    pub partition_count: usize,
    pub next_partition: usize,
}

const PENDING_ADD_INDEX_PREFIX: &[u8] = b"mDDL:pending-add-index:v1:";

fn pending_add_index_key(database: &str, table: &str, index: &str) -> astersql_kv::Key {
    let mut key = PENDING_ADD_INDEX_PREFIX.to_vec();
    for component in [database, table, index] {
        key.extend_from_slice(&(component.len() as u32).to_le_bytes());
        key.extend_from_slice(component.to_ascii_lowercase().as_bytes());
    }
    astersql_kv::Key(key)
}

fn encode_pending_add_index(
    job: &PendingAddIndexJob,
) -> Result<Vec<u8>, astersql_kv::errors::SharedError> {
    let mut value = b"ASTERIDX1".to_vec();
    value.extend_from_slice(&(job.partition_count as u64).to_le_bytes());
    value.extend_from_slice(&(job.next_partition as u64).to_le_bytes());
    for component in [&job.database, &job.table] {
        value.extend_from_slice(&(component.len() as u32).to_le_bytes());
        value.extend_from_slice(component.as_bytes());
    }
    let table = astersql_meta_model::TableInfo {
        Name: astersql_parser_ast::NewCIStr(&job.table),
        Indices: vec![job.index.clone()],
        ..Default::default()
    };
    let metadata =
        astersql_meta_model::EncodeTableInfo(&table).map_err(astersql_kv::errors::New)?;
    value.extend_from_slice(&(metadata.len() as u32).to_le_bytes());
    value.extend_from_slice(&metadata);
    Ok(value)
}

fn decode_pending_add_index(
    bytes: &[u8],
) -> Result<PendingAddIndexJob, astersql_kv::errors::SharedError> {
    let mut offset = 0usize;
    let mut take = |length: usize| {
        let end = offset
            .checked_add(length)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| astersql_kv::errors::New("truncated pending ADD INDEX job"))?;
        let value = &bytes[offset..end];
        offset = end;
        Ok::<_, astersql_kv::errors::SharedError>(value)
    };
    if take(9)? != b"ASTERIDX1" {
        return Err(astersql_kv::errors::New(
            "invalid pending ADD INDEX job header",
        ));
    }
    let partition_count = u64::from_le_bytes(take(8)?.try_into().expect("eight bytes")) as usize;
    let next_partition = u64::from_le_bytes(take(8)?.try_into().expect("eight bytes")) as usize;
    let mut string = || {
        let length = u32::from_le_bytes(take(4)?.try_into().expect("four bytes")) as usize;
        String::from_utf8(take(length)?.to_vec())
            .map_err(|error| astersql_kv::errors::New(error.to_string()))
    };
    let database = string()?;
    let table = string()?;
    drop(string);
    let metadata_length = u32::from_le_bytes(take(4)?.try_into().expect("four bytes")) as usize;
    let metadata = astersql_meta_model::DecodeTableInfo(take(metadata_length)?)
        .map_err(astersql_kv::errors::New)?;
    drop(take);
    if offset != bytes.len() {
        return Err(astersql_kv::errors::New(
            "trailing bytes in pending ADD INDEX job",
        ));
    }
    let index = metadata
        .Indices
        .into_iter()
        .next()
        .ok_or_else(|| astersql_kv::errors::New("pending ADD INDEX job has no index"))?;
    Ok(PendingAddIndexJob {
        database,
        table,
        index,
        partition_count,
        next_partition,
    })
}

/// Domain：TiDB/AsterSQL 实例级域对象，聚合 DDL、InfoSchema、统计、权限与后台任务生命周期。
/// 同一实例通常只存在一个 Domain。
pub struct Domain {
    config: DomainConfig,
    store: Arc<StorageHandle>,
    schema_loader: Arc<dyn InfoSchemaLoader>,
    ddl_metadata: Arc<DdlMetadataService>,
    ddl: RwLock<Option<Arc<dyn DdlService>>>,
    schema_coordinator:
        RwLock<Option<std::sync::Weak<dyn astersql_session_sessmgr::InfoSchemaCoordinator>>>,
    external_workload_manager: RwLock<Option<Arc<Mutex<Box<dyn astersql_extworkload::Manager>>>>>,
    server_info_syncer: RwLock<Option<Arc<Mutex<Box<astersql_domain_serverinfo::Syncer>>>>>,
    cross_ks_manager: RwLock<Option<Arc<astersql_domain_crossks::Manager>>>,
    embed_fn: RwLock<Option<Arc<astersql_inference::EmbedFn>>>,
    ttl_job_manager_started: AtomicBool,
    mlog_purge_worker_started: AtomicBool,
    info_cache: Arc<InfoCache>,
    keyspace_runtimes: Mutex<BTreeMap<String, KeyspaceRuntime>>,
    initialized: AtomicBool,
    started: AtomicBool,
    closed: AtomicBool,
    close_pair: (Mutex<bool>, Condvar),
    workers: Mutex<Vec<WorkerHandle>>,
    slow_queries: Mutex<SlowQueryState>,
    global_config_events: Mutex<VecDeque<(String, String)>>,
    privilege_events: Mutex<VecDeque<PrivilegeEvent>>,
    sysvar_reload_requested: AtomicBool,
    expired_plan_cache_ts: RwLock<Option<SystemTime>>,
    server_id: Arc<Mutex<Option<ServerIdLease>>>,
    connection_id: AtomicU64,
    stats_updating: AtomicBool,
    stats_owner: AtomicBool,
    resource_group_version: AtomicU64,
    ru_version: AtomicU64,
    plan_cache: RwLock<Option<Arc<Mutex<PlanCache>>>>,
    ruv2_consumption_reporter:
        RwLock<Option<Arc<dyn crate::ruv2_reporter::RUV2ConsumptionReporter>>>,
    runaway_manager: RwLock<Option<Arc<astersql_resourcegroup_runaway::manager::Manager>>>,
    sys_processes: Arc<SysProcesses>,
    on_close: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    schema_reload_count: AtomicU64,
    ddl_notifier_sequence: AtomicI64,
    stats_handle: Arc<Mutex<Handle<DomainStatsBackend>>>,
    stats_store: Arc<KvStatsStore<DomainStatsBackend>>,
    stats_catalog:
        Arc<RwLock<BTreeMap<(String, String), (StatsTableKey, astersql_meta_model::TableInfo)>>>,
    stats_catalog_history: Arc<
        RwLock<
            Vec<(
                u64,
                BTreeMap<(String, String), (StatsTableKey, astersql_meta_model::TableInfo)>,
            )>,
        >,
    >,
    stats_catalog_version: Arc<AtomicU64>,
    stats_catalog_timeline: Arc<RwLock<Vec<(SystemTime, u64)>>>,
    dropped_stats_ids: Arc<Mutex<BTreeSet<i64>>>,
    persisted_histograms: Arc<Mutex<BTreeMap<i64, TableStats>>>,
    pending_column_usage: Arc<Mutex<BTreeMap<(i64, i64), RuntimeColumnUsage>>>,
    auto_id_store: Arc<dyn IdStore>,
    stats_auto_id_allocators: Mutex<BTreeMap<(i64, u8), Arc<dyn AutoIdAllocator>>>,
    stats_auto_ids: Mutex<BTreeMap<i64, u64>>,
    stats_secondary_auto_ids: Mutex<BTreeMap<(i64, u8), u64>>,
    next_stats_table_id: AtomicI64,
    stats_preflush_count: Arc<AtomicU64>,
    pending_stats_deltas: Arc<Mutex<BTreeMap<i64, (i64, i64)>>>,
    auto_analyze_ratio: AtomicU64,
    stats_session_vars: Arc<RwLock<StatsSessionVars>>,
    /// Global system-variable values shared by SQL sessions in this Domain.
    global_system_variables: Arc<RwLock<BTreeMap<String, String>>>,
    embedding_config_version: AtomicU64,
    /// Per-instance global value inherited by newly-created SQL sessions.
    global_scatter_region: RwLock<String>,
    /// Global transaction mode inherited by newly-created SQL sessions.
    global_txn_mode: RwLock<String>,
    /// Global TiFlash Compute dispatch policy inherited by newly-created SQL sessions.
    global_tiflash_compute_dispatch_policy: RwLock<astersql_util_tiflashcompute::DispatchPolicy>,
    fail_next_stats_record: AtomicBool,
    fail_next_ddl_publication: AtomicBool,
    pending_add_index_owner_lock: Mutex<()>,
    stats_flush_lock: Mutex<()>,
    pending_stats_flush: Mutex<Option<PendingStatsFlush>>,
    last_stats_preflush_ids: Arc<Mutex<Vec<i64>>>,
    historical_stats_worker: HistoricalStatsWorker,
    auto_analyze_executor: RwLock<Option<std::sync::Weak<dyn AutoAnalyzeExecutor>>>,
    cross_keyspace: RwLock<Option<CrossKeyspaceBinding>>,
}

pub(crate) fn hosted_embedding_enabled(config: &astersql_config::Config, starter: bool) -> bool {
    starter && config.hosted_embedding.enabled
}

/// Go's auto-analyze worker submits `analyze table ...` through a system
/// session obtained from the session pool. The Rust Domain owns no SQL engine,
/// so the session layer registers an executor with
/// [`Domain::register_auto_analyze_executor`].
pub trait AutoAnalyzeExecutor: Send + Sync {
    fn execute_auto_analyze(&self, sql: &str) -> Result<(), String>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 统计子系统使用的会话变量快照。
pub struct StatsSessionVars {
    pub analyze_version: i32,
    pub partition_prune_mode: String,
    pub historical_stats: bool,
    pub analyze_snapshot: bool,
    pub skip_missing_partition_stats: bool,
    /// Go `SessionVars.AnalyzeSkipColumnTypes`, refreshed from
    /// `@@global.tidb_analyze_skip_column_types` by `UpdateSCtxVarsForStats`.
    pub analyze_skip_column_types: BTreeSet<String>,
}

impl Default for StatsSessionVars {
    fn default() -> Self {
        Self {
            analyze_version: 2,
            partition_prune_mode: "dynamic".to_owned(),
            historical_stats: false,
            analyze_snapshot: false,
            skip_missing_partition_stats: true,
            analyze_skip_column_types: parse_analyze_skip_column_types(
                DEF_ANALYZE_SKIP_COLUMN_TYPES,
            ),
        }
    }
}

/// Go `variable.analyzeSkipAllowedTypes`.
const ANALYZE_SKIP_ALLOWED_TYPES: [&str; 7] = [
    "json",
    "text",
    "mediumtext",
    "longtext",
    "blob",
    "mediumblob",
    "longblob",
];

/// Go `sysvar.go` default for `tidb_analyze_skip_column_types`.
const DEF_ANALYZE_SKIP_COLUMN_TYPES: &str = "json,blob,mediumblob,longblob,mediumtext,longtext";

/// Go `variable.ParseAnalyzeSkipColumnTypes`.
fn parse_analyze_skip_column_types(value: &str) -> BTreeSet<String> {
    value
        .to_ascii_lowercase()
        .split(',')
        .map(str::trim)
        .filter(|item| ANALYZE_SKIP_ALLOWED_TYPES.contains(item))
        .map(str::to_owned)
        .collect()
}

/// Go `table.GetColOriginDefaultValue` followed by `ConvertTo(TypeBlob)`.
///
/// `None` stands for Go's `value.IsNull()`, which makes the add-column DDL
/// handler record a null count instead of a single-value bucket.
fn origin_default_value_bytes(column: &astersql_meta_model::ColumnInfo) -> Option<Vec<u8>> {
    if column.DefaultIsExpr {
        return None;
    }
    match column.GetOriginDefaultValue()? {
        astersql_meta_model::DefaultValue::Bool(value) => {
            Some(i64::from(value).to_string().into_bytes())
        }
        astersql_meta_model::DefaultValue::Int(value) => Some(value.to_string().into_bytes()),
        astersql_meta_model::DefaultValue::Uint(value) => Some(value.to_string().into_bytes()),
        astersql_meta_model::DefaultValue::Float(value) => Some(value.to_string().into_bytes()),
        astersql_meta_model::DefaultValue::String(value) => Some(value),
    }
}

#[derive(Clone, Default)]
/// Domain 实现的统计 Handle 后端适配。
pub struct DomainStatsBackend {
    preflush_error: Arc<Mutex<Option<String>>>,
}

#[derive(Clone)]
/// 尚未提交的统计增量刷盘批次。
struct PendingStatsFlush {
    source_deltas: Vec<(i64, (i64, i64))>,
    locked_deltas: Vec<(i64, (i64, i64))>,
    persisted_stats: Vec<TableStats>,
    version: Option<u64>,
}

impl DomainStatsBackend {
    fn inject_preflush_error(&self, message: String) {
        *self
            .preflush_error
            .lock()
            .expect("domain stats preflush error lock poisoned") = Some(message);
    }
}

impl HandleBackend for DomainStatsBackend {
    fn memory_schema_id(&self, _database_id: i64) -> bool {
        false
    }
    fn system_schema(&mut self, _database_id: i64) -> Result<bool, StatsHandleError> {
        Ok(false)
    }
    fn reset_session_stats_list(&mut self) {}
    fn dump_stats_delta(&mut self, _dump_all: bool) -> Result<(), StatsHandleError> {
        match self
            .preflush_error
            .lock()
            .expect("domain stats preflush error lock poisoned")
            .take()
        {
            Some(message) => Err(astersql_statistics_handle::Error(message)),
            None => Ok(()),
        }
    }
    fn start_usage_worker(&mut self) {}
    fn close_pool(&mut self) {}
    fn close_usage(&mut self) {}
    fn close_auto_analyze(&mut self) {}
}

impl stats_storage::StatsCatalog for Domain {
    fn lease(&self) -> Duration {
        self.config.stats_lease
    }

    fn table_exists(&self, physical_id: i64) -> bool {
        self.stats_catalog
            .read()
            .expect("domain stats catalog lock poisoned")
            .values()
            .any(|(key, table)| {
                key.table_id == physical_id
                    || table.GetPartitionInfo().is_some_and(|partition| {
                        partition
                            .Definitions
                            .iter()
                            .any(|definition| definition.ID == physical_id)
                    })
            })
    }

    fn histogram_exists(&self, physical_id: i64, histogram_id: i64, is_index: bool) -> bool {
        self.stats_catalog
            .read()
            .expect("domain stats catalog lock poisoned")
            .values()
            .find(|(key, table)| {
                key.table_id == physical_id
                    || table.GetPartitionInfo().is_some_and(|partition| {
                        partition
                            .Definitions
                            .iter()
                            .any(|definition| definition.ID == physical_id)
                    })
            })
            .is_some_and(|(_, table)| {
                if is_index {
                    table.Indices.iter().any(|index| index.ID == histogram_id)
                } else {
                    table.Columns.iter().any(|column| column.ID == histogram_id)
                }
            })
    }
}

/// 历史统计存取适配器。
struct DomainHistoricalStatsStore {
    stats_handle: Arc<Mutex<Handle<DomainStatsBackend>>>,
}

impl HistoricalStatsStore for DomainHistoricalStatsStore {
    fn table_exists(&self, table_id: i64) -> Result<bool, String> {
        Ok(self
            .stats_handle
            .lock()
            .expect("domain stats handle lock poisoned")
            .stats_meta_rows()
            .iter()
            .any(|stats| stats.physical_id == table_id))
    }

    /// 转储指定表的历史统计。
    fn dump_historical_stats(&self, table_id: i64) -> Result<(), String> {
        let mut handle = self
            .stats_handle
            .lock()
            .expect("domain stats handle lock poisoned");
        if !handle
            .dump_historical_stats(table_id, u64::MAX)
            .map_err(|error| error.to_string())?
        {
            return Err(format!(
                "table {table_id} has no analyzed statistics to dump"
            ));
        }
        let blocks = handle
            .historical_json_blocks(table_id, u64::MAX)
            .ok_or_else(|| format!("table {table_id} historical dump was not persisted"))?;
        if blocks.is_empty() || blocks.iter().any(Vec::is_empty) {
            return Err(format!(
                "table {table_id} produced empty historical JSON blocks"
            ));
        }
        Ok(())
    }
}

#[derive(Clone)]
/// 统计任务执行上下文（会话变量、catalog 视图等）。
pub struct DomainStatsContext {
    handle: Arc<Mutex<Handle<DomainStatsBackend>>>,
    stats_store: Arc<KvStatsStore<DomainStatsBackend>>,
    catalog:
        Arc<RwLock<BTreeMap<(String, String), (StatsTableKey, astersql_meta_model::TableInfo)>>>,
    catalog_history: Arc<
        RwLock<
            Vec<(
                u64,
                BTreeMap<(String, String), (StatsTableKey, astersql_meta_model::TableInfo)>,
            )>,
        >,
    >,
    catalog_version: Arc<AtomicU64>,
    catalog_timeline: Arc<RwLock<Vec<(SystemTime, u64)>>>,
    dropped_stats_ids: Arc<Mutex<BTreeSet<i64>>>,
    preflush_count: Arc<AtomicU64>,
    pending_deltas: Arc<Mutex<BTreeMap<i64, (i64, i64)>>>,
    last_preflush_ids: Arc<Mutex<Vec<i64>>>,
}

impl DomainStatsContext {
    pub fn table(&self, database: &str, table: &str) -> Option<StatsMetaRow> {
        let key = self
            .catalog
            .read()
            .expect("domain stats catalog lock poisoned")
            .get(&(database.to_lowercase(), table.to_lowercase()))
            .map(|(key, _)| key.clone())?;
        let handle = self
            .handle
            .lock()
            .expect("domain stats handle lock poisoned");
        let stats = handle.stats_meta(key.table_id)?;
        let pending = self
            .pending_deltas
            .lock()
            .expect("domain pending stats delta lock poisoned")
            .get(&key.table_id)
            .copied()
            .unwrap_or_default();
        Some(StatsMetaRow {
            database: key.database,
            table: key.table,
            table_id: key.table_id,
            version: stats.version,
            modify_count: stats.modify_count.saturating_add(pending.1),
            row_count: stats.realtime_count.saturating_add(pending.0).max(0),
        })
    }

    pub fn history(&self, table_id: i64) -> Vec<HistoricalStatsMeta> {
        self.handle
            .lock()
            .expect("domain stats handle lock poisoned")
            .historical_stats(table_id)
    }

    pub fn historical_snapshot(
        &self,
        table_id: i64,
        version: u64,
    ) -> Option<RuntimeHistoricalSnapshot> {
        self.handle
            .lock()
            .expect("domain stats handle lock poisoned")
            .historical_snapshot(table_id, version)
    }

    pub fn historical_json_blocks(&self, table_id: i64, version: u64) -> Option<Vec<Vec<u8>>> {
        self.handle
            .lock()
            .expect("domain stats handle lock poisoned")
            .historical_json_blocks(table_id, version)
    }

    pub fn decode_historical_json_blocks(
        &self,
        blocks: &[Vec<u8>],
    ) -> Result<(String, TableStats), String> {
        astersql_statistics_handle::DecodeHistoricalJsonBlocks(blocks)
            .map_err(|error| error.to_string())
    }

    /// 转储指定表的历史统计。
    pub fn dump_historical_stats(&self, table_id: i64) -> Result<(), String> {
        let mut handle = self
            .handle
            .lock()
            .expect("domain stats handle lock poisoned");
        if !handle
            .dump_historical_stats(table_id, u64::MAX)
            .map_err(|error| error.to_string())?
        {
            return Err(format!("table {table_id} has no historical meta"));
        }
        Ok(())
    }

    /// Go `Handle::RecordHistoricalStatsToStorage`: dumps the table statistics
    /// JSON and persists every block as a `mysql.stats_history` row, returning
    /// the recorded version (0 when the table has nothing to dump).
    pub fn record_historical_stats_to_storage(&self, table_id: i64) -> Result<u64, String> {
        let (version, blocks) = {
            let mut handle = self
                .handle
                .lock()
                .expect("domain stats handle lock poisoned");
            if !handle
                .dump_historical_stats(table_id, u64::MAX)
                .map_err(|error| error.to_string())?
            {
                return Ok(0);
            }
            let version = handle
                .historical_stats(table_id)
                .last()
                .map(|meta| meta.version)
                .unwrap_or_default();
            let blocks = handle
                .historical_json_blocks(table_id, u64::MAX)
                .ok_or_else(|| format!("table {table_id} historical dump was not persisted"))?;
            (version, blocks)
        };
        let create_time = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros() as i64;
        for (sequence, block) in blocks.iter().enumerate() {
            let mut payload = String::from("x'");
            for byte in block {
                payload.push_str(&format!("{byte:02x}"));
            }
            payload.push('\'');
            self.stats_store
                .execute(
                    &format!(
                        "insert into mysql.stats_history(table_id, stats_data, seq_no, version, create_time) values ({table_id}, {payload}, {sequence}, {version}, {create_time})"
                    ),
                    &[],
                )
                .map_err(|error| error.to_string())?;
        }
        Ok(version)
    }

    /// 回收已删表残留统计。
    pub fn gc_dropped_stats(&self) -> Result<(), DomainError> {
        let ids = self
            .dropped_stats_ids
            .lock()
            .expect("dropped stats IDs lock poisoned")
            .iter()
            .copied()
            .collect::<Vec<_>>();
        for physical_id in &ids {
            self.stats_store
                .remove_persisted_table(*physical_id)
                .map_err(DomainError::Stats)?;
        }
        self.handle
            .lock()
            .expect("domain stats handle lock poisoned")
            .remove_historical_snapshots(&ids);
        self.dropped_stats_ids
            .lock()
            .expect("dropped stats IDs lock poisoned")
            .clear();
        Ok(())
    }

    /// 回收早于保留期的历史统计。
    pub fn gc_historical_stats_older_than(&self, retention: Duration) -> usize {
        self.handle
            .lock()
            .expect("domain stats handle lock poisoned")
            .gc_historical_stats_older_than(retention)
    }

    /// 当前统计 catalog 版本。
    pub fn catalog_version(&self) -> u64 {
        self.catalog_version.load(Ordering::Acquire)
    }

    /// 某时刻对应的 catalog 版本。
    pub fn catalog_version_at_time(&self, snapshot: SystemTime) -> Option<u64> {
        self.catalog_timeline
            .read()
            .expect("domain stats catalog timeline lock poisoned")
            .iter()
            .rev()
            .find_map(|(recorded_at, version)| (*recorded_at <= snapshot).then_some(*version))
    }

    /// 判断统计 catalog 历史中是否存在精确版本。
    pub fn has_catalog_version(&self, version: u64) -> bool {
        self.catalog_history
            .read()
            .expect("domain stats catalog history lock poisoned")
            .iter()
            .any(|(entry_version, _)| *entry_version == version)
    }

    /// 按版本取 catalog 快照。
    pub fn catalog_at(
        &self,
        version: u64,
    ) -> BTreeMap<(String, String), (StatsTableKey, astersql_meta_model::TableInfo)> {
        self.catalog_history
            .read()
            .expect("domain stats catalog history lock poisoned")
            .iter()
            .rev()
            .find(|(entry_version, _)| *entry_version <= version)
            .map(|(_, catalog)| catalog.clone())
            .unwrap_or_default()
    }

    /// 当前统计 catalog。
    pub fn catalog(
        &self,
    ) -> BTreeMap<(String, String), (StatsTableKey, astersql_meta_model::TableInfo)> {
        self.catalog
            .read()
            .expect("domain stats catalog lock poisoned")
            .clone()
    }

    /// 内存中的物理表统计。
    pub fn physical_stats(&self, physical_id: i64) -> Option<TableStats> {
        let mut stats = self
            .handle
            .lock()
            .expect("domain stats handle lock poisoned")
            .stats_meta(physical_id)
            .cloned()?;
        if let Some((row_delta, modified_rows)) = self
            .pending_deltas
            .lock()
            .expect("domain pending stats delta lock poisoned")
            .get(&physical_id)
            .copied()
        {
            stats.realtime_count = stats.realtime_count.saturating_add(row_delta).max(0);
            stats.modify_count = stats.modify_count.saturating_add(modified_rows);
        }
        Some(stats)
    }

    /// 已持久化的物理表统计。
    pub fn persisted_physical_stats(&self, physical_id: i64) -> Option<TableStats> {
        self.handle
            .lock()
            .expect("domain stats handle lock poisoned")
            .stats_meta(physical_id)
            .cloned()
    }

    /// 已锁定统计的表 ID 集合。
    pub fn locked_table_ids(&self) -> BTreeSet<i64> {
        self.stats_store
            .locked_ids()
            .expect("read persisted statistics locks")
            .into_iter()
            .collect()
    }

    /// 列使用情况列表。
    pub fn column_usage(&self) -> Vec<RuntimeColumnUsage> {
        self.handle
            .lock()
            .expect("domain stats handle lock poisoned")
            .column_usage()
    }

    /// 进行中的 Analyze 任务。
    pub fn analyze_jobs(&self) -> Vec<RuntimeAnalyzeJob> {
        self.handle
            .lock()
            .expect("domain stats handle lock poisoned")
            .analyze_jobs()
    }

    /// 预刷次数。
    pub fn preflush_count(&self) -> u64 {
        self.preflush_count.load(Ordering::Acquire)
    }

    /// 待刷增量的物理表 ID。
    pub fn pending_stats_delta_ids(&self) -> Vec<i64> {
        self.pending_deltas
            .lock()
            .expect("domain pending stats delta lock poisoned")
            .keys()
            .copied()
            .collect()
    }

    /// 上次预刷涉及的 ID。
    pub fn last_preflush_ids(&self) -> Vec<i64> {
        self.last_preflush_ids
            .lock()
            .expect("domain last preflush ID lock poisoned")
            .clone()
    }
}

impl Domain {
    /// 使用存储与 schema loader 构造 Domain。
    pub fn new<S>(store: S, schema_loader: Arc<dyn InfoSchemaLoader>, config: DomainConfig) -> Self
    where
        S: astersql_kv::Storage + Send + Sync + 'static,
    {
        Self::new_with_storage_handle(Arc::new(StorageHandle::new(store)), schema_loader, config)
    }

    /// 使用已有 StorageHandle 构造 Domain 并初始化统计后端。
    pub fn new_with_storage_handle(
        storage: Arc<StorageHandle>,
        schema_loader: Arc<dyn InfoSchemaLoader>,
        config: DomainConfig,
    ) -> Self {
        let cache = Arc::new(astersql_infoschema::NewCache(
            config.info_cache_capacity.max(1),
        ));
        let mut handle = Handle::new(DomainStatsBackend::default(), false, false)
            .expect("construct domain statistics handle");
        handle.cache_mut().set_capacity(config.stats_cache_capacity);
        let stats_handle = Arc::new(Mutex::new(handle));
        let stats_store = Arc::new(KvStatsStore::new(
            Arc::clone(&storage) as Arc<dyn astersql_statistics_handle::StatsKvStorage>,
            Arc::clone(&stats_handle),
        ));
        let auto_id_store: Arc<dyn IdStore> = Arc::new(KvAutoIdStore::new(Arc::clone(&storage)));
        let historical_stats_worker = HistoricalStatsWorker::new(
            Arc::new(DomainHistoricalStatsStore {
                stats_handle: Arc::clone(&stats_handle),
            }),
            32,
        );
        Self {
            slow_queries: Mutex::new(SlowQueryState::new(
                config.slow_query_top_n,
                config.slow_query_recent,
            )),
            config,
            store: storage,
            schema_loader,
            ddl_metadata: Arc::new(DdlMetadataService::new()),
            ddl: RwLock::new(None),
            schema_coordinator: RwLock::new(None),
            external_workload_manager: RwLock::new(None),
            server_info_syncer: RwLock::new(None),
            cross_ks_manager: RwLock::new(None),
            embed_fn: RwLock::new(None),
            ttl_job_manager_started: AtomicBool::new(false),
            mlog_purge_worker_started: AtomicBool::new(false),
            info_cache: cache,
            keyspace_runtimes: Mutex::new(BTreeMap::new()),
            initialized: AtomicBool::new(false),
            started: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            close_pair: (Mutex::new(false), Condvar::new()),
            workers: Mutex::new(Vec::new()),
            global_config_events: Mutex::new(VecDeque::new()),
            privilege_events: Mutex::new(VecDeque::new()),
            sysvar_reload_requested: AtomicBool::new(false),
            expired_plan_cache_ts: RwLock::new(None),
            server_id: Arc::new(Mutex::new(None)),
            connection_id: AtomicU64::new(0),
            stats_updating: AtomicBool::new(false),
            stats_owner: AtomicBool::new(false),
            resource_group_version: AtomicU64::new(0),
            ru_version: AtomicU64::new(1),
            plan_cache: RwLock::new(None),
            ruv2_consumption_reporter: RwLock::new(None),
            runaway_manager: RwLock::new(None),
            sys_processes: Arc::new(SysProcesses::default()),
            on_close: Mutex::new(None),
            schema_reload_count: AtomicU64::new(0),
            ddl_notifier_sequence: AtomicI64::new(0),
            stats_handle,
            stats_store,
            stats_catalog: Arc::new(RwLock::new(BTreeMap::new())),
            stats_catalog_history: Arc::new(RwLock::new(vec![(0, BTreeMap::new())])),
            stats_catalog_version: Arc::new(AtomicU64::new(0)),
            stats_catalog_timeline: Arc::new(RwLock::new(vec![(SystemTime::UNIX_EPOCH, 0)])),
            dropped_stats_ids: Arc::new(Mutex::new(BTreeSet::new())),
            persisted_histograms: Arc::new(Mutex::new(BTreeMap::new())),
            pending_column_usage: Arc::new(Mutex::new(BTreeMap::new())),
            auto_id_store,
            stats_auto_id_allocators: Mutex::new(BTreeMap::new()),
            stats_auto_ids: Mutex::new(BTreeMap::new()),
            stats_secondary_auto_ids: Mutex::new(BTreeMap::new()),
            next_stats_table_id: AtomicI64::new(1),
            stats_preflush_count: Arc::new(AtomicU64::new(0)),
            pending_stats_deltas: Arc::new(Mutex::new(BTreeMap::new())),
            auto_analyze_ratio: AtomicU64::new(0.5_f64.to_bits()),
            stats_session_vars: Arc::new(RwLock::new(StatsSessionVars::default())),
            global_system_variables: Arc::new(RwLock::new(BTreeMap::new())),
            embedding_config_version: AtomicU64::new(0),
            global_scatter_region: RwLock::new(vardef::ScatterOff.to_owned()),
            // Go registers `tidb_txn_mode` with `DefTiDBTxnMode` (pessimistic).
            // Keeping this empty made an unqualified BEGIN silently optimistic.
            global_txn_mode: RwLock::new(astersql_sessionctx_vardef::DefTiDBTxnMode.to_owned()),
            global_tiflash_compute_dispatch_policy: RwLock::new(
                astersql_util_tiflashcompute::DispatchPolicyConsistentHash,
            ),
            fail_next_stats_record: AtomicBool::new(false),
            fail_next_ddl_publication: AtomicBool::new(false),
            pending_add_index_owner_lock: Mutex::new(()),
            stats_flush_lock: Mutex::new(()),
            pending_stats_flush: Mutex::new(None),
            last_stats_preflush_ids: Arc::new(Mutex::new(Vec::new())),
            historical_stats_worker,
            auto_analyze_executor: RwLock::new(None),
            cross_keyspace: RwLock::new(None),
        }
    }

    /// 测试用 Domain，采用默认配置。
    pub fn new_mock<S>(store: S, schema_loader: Arc<dyn InfoSchemaLoader>) -> Self
    where
        S: astersql_kv::Storage + Send + Sync + 'static,
    {
        let mut config = DomainConfig::default();
        config.schema_lease = Duration::ZERO;
        config.stats_lease = Duration::ZERO;
        Self::new(store, schema_loader, config)
    }

    /// Return the canonical storage handle used to construct a replacement
    /// Domain after owner/process restart.
    pub fn storage_handle(&self) -> Arc<StorageHandle> {
        Arc::clone(&self.store)
    }

    /// 返回 InfoSchema 缓存（InfoCache）。
    pub fn info_cache(&self) -> Arc<InfoCache> {
        self.info_cache.clone()
    }
    /// 取该 KS 最新 InfoSchema。
    pub fn info_schema(&self) -> SchemaRef {
        self.info_cache
            .GetLatest()
            .expect("domain must be initialized before reading infoschema")
    }

    /// Bind this Domain to a shared cross-keyspace coordinator.
    pub fn bind_cross_keyspace(
        &self,
        coordinator: Arc<CrossKeyspaceCoordinator>,
        keyspace: impl Into<String>,
        new_collation: bool,
    ) {
        let keyspace = keyspace.into();
        coordinator.bind(&keyspace, new_collation);
        *self
            .cross_keyspace
            .write()
            .expect("domain cross-keyspace binding lock poisoned") = Some(CrossKeyspaceBinding {
            coordinator,
            keyspace,
        });
    }

    /// Record one successfully published DDL in the bound cross-keyspace
    /// cluster. Unbound Domains preserve their normal single-keyspace behavior.
    pub fn record_cross_keyspace_ddl(&self, database: &str, add_index: bool) {
        let binding = self
            .cross_keyspace
            .read()
            .expect("domain cross-keyspace binding lock poisoned")
            .clone();
        if let Some(binding) = binding {
            binding
                .coordinator
                .record_ddl(&binding.keyspace, database, add_index);
        }
    }

    /// Keyspaces currently visible to this runtime.
    pub fn cross_keyspaces_for_test(&self) -> Vec<String> {
        const SYSTEM: &str = "SYSTEM";
        let Some(binding) = self
            .cross_keyspace
            .read()
            .expect("domain cross-keyspace binding lock poisoned")
            .clone()
        else {
            return Vec::new();
        };
        if !binding.keyspace.eq_ignore_ascii_case(SYSTEM) {
            return vec![SYSTEM.to_owned()];
        }
        let state = binding
            .coordinator
            .state
            .lock()
            .expect("cross-keyspace coordinator lock poisoned");
        state
            .registered_users
            .union(&state.leased_users)
            .cloned()
            .collect()
    }

    /// Acquire a user-keyspace runtime through the SYSTEM Domain.
    pub fn acquire_cross_keyspace_runtime(
        &self,
        keyspace: &str,
        idle_timeout: Duration,
    ) -> Result<CrossKeyspaceRuntimeHandle, String> {
        const SYSTEM: &str = "SYSTEM";
        let binding = self
            .cross_keyspace
            .read()
            .expect("domain cross-keyspace binding lock poisoned")
            .clone()
            .ok_or_else(|| "domain is not bound to a cross-keyspace coordinator".to_owned())?;
        if !binding.keyspace.eq_ignore_ascii_case(SYSTEM) {
            return Err("only SYSTEM may acquire a cross-keyspace runtime".to_owned());
        }
        binding.coordinator.acquire_runtime(keyspace, idle_timeout)
    }

    /// Number of distributed ADD INDEX task records visible from this runtime.
    pub fn cross_task_count_for_test(&self) -> usize {
        const SYSTEM: &str = "SYSTEM";
        let Some(binding) = self
            .cross_keyspace
            .read()
            .expect("domain cross-keyspace binding lock poisoned")
            .clone()
        else {
            return 0;
        };
        if !binding.keyspace.eq_ignore_ascii_case(SYSTEM) {
            return 0;
        }
        binding
            .coordinator
            .state
            .lock()
            .expect("cross-keyspace coordinator lock poisoned")
            .system_tasks
            .len()
    }

    /// Whether this runtime can see the named distributed task.
    pub fn cross_task_count_for_key_for_test(&self, task_key: &str) -> usize {
        const SYSTEM: &str = "SYSTEM";
        let Some(binding) = self
            .cross_keyspace
            .read()
            .expect("domain cross-keyspace binding lock poisoned")
            .clone()
        else {
            return 0;
        };
        if !binding.keyspace.eq_ignore_ascii_case(SYSTEM) {
            return 0;
        }
        usize::from(
            binding
                .coordinator
                .state
                .lock()
                .expect("cross-keyspace coordinator lock poisoned")
                .system_tasks
                .contains(task_key),
        )
    }

    /// Last schema synchronization result observed by this runtime.
    pub fn last_cross_sync_summary_for_test(&self) -> Option<CrossKeyspaceSyncSummary> {
        let binding = self
            .cross_keyspace
            .read()
            .expect("domain cross-keyspace binding lock poisoned")
            .clone()?;
        binding
            .coordinator
            .state
            .lock()
            .expect("cross-keyspace coordinator lock poisoned")
            .runtimes
            .get(&binding.keyspace)
            .and_then(|runtime| runtime.last_sync_summary)
    }

    /// Last ADD INDEX collation resolution observed by this runtime.
    pub fn last_backfill_collation_for_test(&self) -> Option<BackfillCollationResolution> {
        let binding = self
            .cross_keyspace
            .read()
            .expect("domain cross-keyspace binding lock poisoned")
            .clone()?;
        binding
            .coordinator
            .state
            .lock()
            .expect("cross-keyspace coordinator lock poisoned")
            .runtimes
            .get(&binding.keyspace)
            .and_then(|runtime| runtime.last_backfill_collation)
    }

    /// Last ADD INDEX DDL job ID observed by this runtime.
    pub fn last_ddl_job_id_for_test(&self) -> Option<i64> {
        let binding = self
            .cross_keyspace
            .read()
            .expect("domain cross-keyspace binding lock poisoned")
            .clone()?;
        binding
            .coordinator
            .state
            .lock()
            .expect("cross-keyspace coordinator lock poisoned")
            .runtimes
            .get(&binding.keyspace)
            .and_then(|runtime| runtime.last_ddl_job_id)
    }

    /// Last ADD INDEX distributed task key observed by this runtime.
    pub fn last_ddl_task_key_for_test(&self) -> Option<String> {
        let binding = self
            .cross_keyspace
            .read()
            .expect("domain cross-keyspace binding lock poisoned")
            .clone()?;
        binding
            .coordinator
            .state
            .lock()
            .expect("cross-keyspace coordinator lock poisoned")
            .runtimes
            .get(&binding.keyspace)
            .and_then(|runtime| runtime.last_ddl_task_key.clone())
    }

    /// 按库表名查找表元信息。
    pub fn table_by_name(
        &self,
        database: &str,
        table: &str,
    ) -> Result<Arc<astersql_meta_model::TableInfo>, DomainError> {
        self.info_schema()
            .ModelTableInfoByName(
                &astersql_infoschema::CiString::new(database),
                &astersql_infoschema::CiString::new(table),
            )
            .map_err(|error| DomainError::Ddl(error.to_string()))
    }

    /// 返回底层存储句柄。
    pub fn storage(&self) -> Arc<StorageHandle> {
        self.store.clone()
    }

    pub fn bind_ruv2_consumption_reporter(
        &self,
        reporter: Option<Arc<dyn crate::ruv2_reporter::RUV2ConsumptionReporter>>,
    ) {
        *self
            .ruv2_consumption_reporter
            .write()
            .expect("RUv2 reporter lock poisoned") = reporter;
    }

    pub fn ruv2_consumption_reporter(
        &self,
    ) -> Option<Arc<dyn crate::ruv2_reporter::RUV2ConsumptionReporter>> {
        self.ruv2_consumption_reporter
            .read()
            .expect("RUv2 reporter lock poisoned")
            .clone()
    }

    /// Active RU accounting version; the default matches a controller-free domain.
    pub fn ru_version(&self) -> u64 {
        self.ru_version.load(Ordering::Acquire)
    }

    /// Update the RU version when the resource-group controller changes policy.
    pub fn set_ru_version(&self, version: u64) {
        self.ru_version.store(version, Ordering::Release);
    }

    pub fn bind_runaway_manager(
        &self,
        manager: Option<Arc<astersql_resourcegroup_runaway::manager::Manager>>,
    ) {
        *self
            .runaway_manager
            .write()
            .expect("runaway manager lock poisoned") = manager;
    }

    pub fn runaway_manager(&self) -> Option<Arc<astersql_resourcegroup_runaway::manager::Manager>> {
        self.runaway_manager
            .read()
            .expect("runaway manager lock poisoned")
            .clone()
    }

    /// 返回统计 Handle。
    pub fn stats_handle(&self) -> Arc<Mutex<Handle<DomainStatsBackend>>> {
        Arc::clone(&self.stats_handle)
    }

    /// 返回 ANALYZE 持久化的完整统计载荷，供同步加载器按项恢复缓存。
    pub fn persisted_table_stats(&self, physical_id: i64) -> Option<TableStats> {
        self.persisted_histograms
            .lock()
            .expect("persisted histogram lock poisoned")
            .get(&physical_id)
            .cloned()
    }

    /// Configures the statistics lease used by asynchronous histogram reloads.
    pub fn set_stats_lease(&self, lease: Duration) -> Result<(), DomainError> {
        self.stats_handle
            .lock()
            .expect("domain stats handle lock poisoned")
            .set_lease(lease);
        Ok(())
    }

    /// Refreshes the cache's metadata and lightweight histogram membership.
    pub fn update_stats(&self) -> Result<(), DomainError> {
        // Go `Handle.Update` reads through util.ExecRows before replacing the
        // cache. Preserve both its timeout error and all-or-nothing cache
        // behavior: fail before `init_stats_lite` mutates cached metadata.
        self.restricted_stats_query("select table_id from mysql.stats_meta limit 0", &[])?;
        self.init_stats_lite(&[])
    }

    /// Reloads full histogram payloads after a lease-enabled planner request.
    ///
    /// Mirrors Go's `storage.LoadNeededHistograms`: it first drains
    /// `AsyncLoadHistogramNeededItems`, loading each queued column/index from
    /// the restricted statistics store, then applies the lease-driven bulk
    // / refresh. The restricted statistics store persists histogram membership
    // / separately from the cached bucket payload. Keep the latter as the
    /// domain's canonical analyzed snapshot so `Clear` followed by `Update` can
    /// retain Go's lazy-histogram behavior without re-analyzing user tables.
    pub fn load_needed_histograms(&self) -> Result<(), DomainError> {
        for item in asyncload::AsyncLoadHistogramNeededItems.AllItems() {
            if item.TableItemID.IsIndex {
                // Index is always full load, as in Go.
                self.load_needed_index_histogram(&item.TableItemID)?;
            } else {
                self.load_needed_column_histogram(&item.TableItemID, item.FullLoad)?;
            }
        }
        if self
            .stats_handle
            .lock()
            .expect("domain stats handle lock poisoned")
            .lease()
            .is_zero()
        {
            return Ok(());
        }
        let profiles = self
            .persisted_histograms
            .lock()
            .expect("persisted histogram lock poisoned")
            .values()
            .cloned()
            .collect::<Vec<_>>();
        self.stats_handle
            .lock()
            .expect("domain stats handle lock poisoned")
            .merge_cache_profiles(profiles);
        Ok(())
    }

    /// Go: `loadNeededColumnHistograms`.
    ///
    /// The queued item is dropped whatever happens, because async load gives
    /// every column exactly one attempt; sync load owns the retry policy.
    fn load_needed_column_histogram(
        &self,
        item: &asyncload::TableItemID,
        full_load: bool,
    ) -> Result<(), DomainError> {
        let result = self.load_needed_column_histogram_inner(item, full_load);
        asyncload::AsyncLoadHistogramNeededItems.Delete(*item);
        result
    }

    /// 列直方图加载内部实现。
    fn load_needed_column_histogram_inner(
        &self,
        item: &asyncload::TableItemID,
        full_load: bool,
    ) -> Result<(), DomainError> {
        // Internal pseudo columns (for example `_tidb_rowid` with ID -1) have
        // no column metadata or statistics; skip them defensively.
        if item.ID <= 0 {
            return Ok(());
        }
        // The table may have been dropped after the async load was triggered.
        if self
            .stats_handle
            .lock()
            .expect("domain stats handle lock poisoned")
            .stats_meta(item.TableID)
            .is_none()
        {
            return Ok(());
        }
        let Some(table) = self.catalog_table_by_physical_id(item.TableID) else {
            return Ok(());
        };
        // The column may have been dropped after the async load was triggered.
        if !table.Columns.iter().any(|column| column.ID == item.ID) {
            return Ok(());
        }
        let Some((analyzed, load_needed)) = self.column_load_state(item.TableID, item.ID) else {
            return Ok(());
        };
        if !load_needed || !analyzed {
            if load_needed && !analyzed {
                // Not analyzed yet: publish an empty column so the pseudo
                // estimation stops re-triggering sync/async load.
                if let Some(column) = self
                    .stats_handle
                    .lock()
                    .expect("domain stats handle lock poisoned")
                    .cache_mut()
                    .get_mut(item.TableID)
                    .and_then(|stats| stats.columns.get_mut(&item.ID))
                {
                    column.loaded_or_evicted = true;
                }
            }
            return Ok(());
        }
        if !self.persisted_histogram_exists(item.TableID, false, item.ID) {
            // Histogram not found, possibly because a DDL event was not
            // handled; Go only warns here and keeps the item dropped.
            return Ok(());
        }
        let payload = self.persisted_histogram_payload(item.TableID, false, item.ID);
        let buckets = if full_load {
            self.stats_store
                .histogram_buckets(item.TableID, false, item.ID)
                .map_err(DomainError::Stats)?
        } else {
            Vec::new()
        };
        let mut handle = self
            .stats_handle
            .lock()
            .expect("domain stats handle lock poisoned");
        // Re-read the cache: unrelated loads may have replaced the entry.
        let Some(stats) = handle.cache_mut().get_mut(item.TableID) else {
            return Ok(());
        };
        let Some(column) = stats.columns.get_mut(&item.ID) else {
            return Ok(());
        };
        if let Some(payload) = payload
            .as_ref()
            .and_then(|stats| stats.columns.get(&item.ID))
        {
            column.analyzed_or_synthesized = payload.analyzed_or_synthesized;
            column.stats_version = payload.stats_version;
            column.ndv = payload.ndv;
            column.null_count = payload.null_count;
            column.total_column_size = payload.total_column_size;
            column.version = payload.version;
            column.field_type = payload.field_type;
            column.correlation = payload.correlation;
            column.average_size = payload.average_size;
            column.fm_sketch = payload.fm_sketch.clone();
            if full_load {
                column.top_n = payload.top_n.clone();
            }
        }
        if full_load {
            column.buckets = buckets;
        }
        // Go marks the column full-loaded only for a full load and all-evicted
        // otherwise; this cache models both with `loaded_or_evicted`.
        column.loaded_or_evicted = full_load;
        if column.stats_version != 0 {
            stats.stats_version = column.stats_version;
            stats.last_analyze_version = stats.last_analyze_version.max(column.version);
        }
        Ok(())
    }

    /// Go: `loadNeededIndexHistograms`.
    fn load_needed_index_histogram(
        &self,
        item: &asyncload::TableItemID,
    ) -> Result<(), DomainError> {
        let result = self.load_needed_index_histogram_inner(item);
        asyncload::AsyncLoadHistogramNeededItems.Delete(*item);
        result
    }

    /// 索引直方图加载内部实现。
    fn load_needed_index_histogram_inner(
        &self,
        item: &asyncload::TableItemID,
    ) -> Result<(), DomainError> {
        let load_needed = {
            let handle = self
                .stats_handle
                .lock()
                .expect("domain stats handle lock poisoned");
            let Some(stats) = handle.stats_meta(item.TableID) else {
                return Ok(());
            };
            match stats.indexes.get(&item.ID) {
                Some(index) => !index.fully_loaded,
                None => return Ok(()),
            }
        };
        if !load_needed {
            return Ok(());
        }
        if !self.persisted_histogram_exists(item.TableID, true, item.ID) {
            return Ok(());
        }
        let Some(table) = self.catalog_table_by_physical_id(item.TableID) else {
            return Ok(());
        };
        // The index may have been dropped after the async load was triggered.
        if !table.Indices.iter().any(|index| index.ID == item.ID) {
            return Ok(());
        }
        let payload = self.persisted_histogram_payload(item.TableID, true, item.ID);
        let buckets = self
            .stats_store
            .histogram_buckets(item.TableID, true, item.ID)
            .map_err(DomainError::Stats)?;
        let mut handle = self
            .stats_handle
            .lock()
            .expect("domain stats handle lock poisoned");
        let Some(stats) = handle.cache_mut().get_mut(item.TableID) else {
            return Ok(());
        };
        let Some(index) = stats.indexes.get_mut(&item.ID) else {
            return Ok(());
        };
        if let Some(payload) = payload
            .as_ref()
            .and_then(|stats| stats.indexes.get(&item.ID))
        {
            index.analyzed = payload.analyzed;
            index.stats_version = payload.stats_version;
            index.version = payload.version;
            index.ndv = payload.ndv;
            index.null_count = payload.null_count;
            index.correlation = payload.correlation;
            index.cms_loaded = payload.cms_loaded;
            index.top_n = payload.top_n.clone();
            index.fm_sketch = payload.fm_sketch.clone();
        }
        index.buckets = buckets;
        index.fully_loaded = true;
        if index.stats_version != 0 {
            stats.stats_version = index.stats_version;
            stats.last_analyze_version = stats.last_analyze_version.max(index.version);
        }
        Ok(())
    }

    /// Returns `(analyzed, load_needed)` for a cached column, mirroring Go's
    /// `Table.ColumnIsLoadNeeded`. `None` means the column is not tracked by
    /// the cache at all, which Go treats as "nothing to load".
    fn column_load_state(&self, physical_id: i64, column_id: i64) -> Option<(bool, bool)> {
        let handle = self
            .stats_handle
            .lock()
            .expect("domain stats handle lock poisoned");
        let column = handle.stats_meta(physical_id)?.columns.get(&column_id)?;
        Some((
            column.analyzed_or_synthesized
                || self.persisted_histogram_exists(physical_id, false, column_id),
            !column.loaded_or_evicted,
        ))
    }

    /// Mirrors Go's `mysql.stats_histograms` existence probe: the restricted
    /// store persists histogram membership independently of the payload.
    fn persisted_histogram_exists(&self, physical_id: i64, is_index: bool, id: i64) -> bool {
        self.restricted_stats_query(
            &format!(
                "SELECT hist_id FROM mysql.stats_histograms WHERE table_id = {physical_id} AND is_index = {} AND hist_id = {id}",
                i64::from(is_index)
            ),
            &[],
        )
        .is_ok_and(|rows| !rows.is_empty())
    }

    /// 读取持久化直方图载荷。
    fn persisted_histogram_payload(
        &self,
        physical_id: i64,
        is_index: bool,
        id: i64,
    ) -> Option<TableStats> {
        let payload = self
            .persisted_histograms
            .lock()
            .expect("persisted histogram lock poisoned")
            .get(&physical_id)
            .cloned()?;
        let present = if is_index {
            payload.indexes.contains_key(&id)
        } else {
            payload.columns.contains_key(&id)
        };
        present.then_some(payload)
    }

    /// 按物理 ID 查 catalog 表。
    fn catalog_table_by_physical_id(
        &self,
        physical_id: i64,
    ) -> Option<astersql_meta_model::TableInfo> {
        self.stats_catalog
            .read()
            .expect("domain stats catalog lock poisoned")
            .values()
            .find(|(key, table)| {
                key.table_id == physical_id
                    || table.GetPartitionInfo().is_some_and(|partition| {
                        partition
                            .Definitions
                            .iter()
                            .any(|definition| definition.ID == physical_id)
                    })
            })
            .map(|(_, table)| table.clone())
    }

    /// Queues the column and index histograms a predicate needs but that the
    /// cache holds only in evicted form.
    ///
    /// Go does this inside `CollectPredicateColumnsPoint` via
    /// `ColumnStatsIsInvalid`/`IndexStatsIsInvalid` when
    /// `tidb_stats_load_sync_wait` is `0`; the caller is responsible for that
    /// gate.
    ///
    /// `kept_indexes` is the caller's result of Go's access-path pruning
    /// (`pruneIndexesForDataSource`): `None` means no index was pruned, so
    /// `collectSyncIndices` considers every index that covers a needed column,
    /// while `Some(ids)` restricts it to the surviving access paths.
    ///
    /// `static_partition_ids` mirrors Go's
    /// `expandStatsNeededColumnsForStaticPruning`: under static partition
    /// pruning every needed item is also queued for each partition.
    pub fn enqueue_async_load_items(
        &self,
        table: &astersql_meta_model::TableInfo,
        predicate_column_ids: &[i64],
        kept_indexes: Option<&BTreeSet<i64>>,
        static_partition_ids: &[i64],
    ) {
        self.enqueue_async_load_items_with_failure_status(
            table,
            predicate_column_ids,
            kept_indexes,
            static_partition_ids,
            false,
        );
    }

    /// 同步加载失败后回退异步队列，并在键上保留失败标记供计划缓存失效判断。
    pub fn enqueue_async_load_items_after_sync_failure(
        &self,
        table: &astersql_meta_model::TableInfo,
        predicate_column_ids: &[i64],
        kept_indexes: Option<&BTreeSet<i64>>,
        static_partition_ids: &[i64],
    ) {
        self.enqueue_async_load_items_with_failure_status(
            table,
            predicate_column_ids,
            kept_indexes,
            static_partition_ids,
            true,
        );
    }

    /// 按物理表扩展统计加载项，并统一携带同步失败状态。
    fn enqueue_async_load_items_with_failure_status(
        &self,
        table: &astersql_meta_model::TableInfo,
        predicate_column_ids: &[i64],
        kept_indexes: Option<&BTreeSet<i64>>,
        static_partition_ids: &[i64],
        sync_load_failed: bool,
    ) {
        for physical_id in std::iter::once(table.ID).chain(static_partition_ids.iter().copied()) {
            self.enqueue_async_load_items_for_physical_id(
                physical_id,
                table,
                predicate_column_ids,
                kept_indexes,
                sync_load_failed,
            );
        }
    }

    /// 为物理表入队异步加载项。
    fn enqueue_async_load_items_for_physical_id(
        &self,
        physical_id: i64,
        table: &astersql_meta_model::TableInfo,
        predicate_column_ids: &[i64],
        kept_indexes: Option<&BTreeSet<i64>>,
        sync_load_failed: bool,
    ) {
        let Some(stats) = self
            .stats_handle
            .lock()
            .expect("domain stats handle lock poisoned")
            .stats_meta(physical_id)
            .cloned()
        else {
            return;
        };
        if stats.pseudo {
            return;
        }
        for column_id in predicate_column_ids {
            let needed = stats
                .columns
                .get(column_id)
                .is_some_and(|column| !column.loaded_or_evicted);
            if needed {
                asyncload::AsyncLoadHistogramNeededItems.Insert(
                    asyncload::TableItemID {
                        TableID: physical_id,
                        ID: *column_id,
                        IsIndex: false,
                        IsSyncLoadFailed: sync_load_failed,
                    },
                    true,
                );
            }
        }
        for index in &table.Indices {
            if !index
                .Columns
                .iter()
                .any(|column| predicate_column_ids.contains(&self.index_column_id(table, column)))
            {
                continue;
            }
            if kept_indexes.is_some_and(|kept| !kept.contains(&index.ID)) {
                continue;
            }
            let needed = stats
                .indexes
                .get(&index.ID)
                .is_some_and(|index| !index.fully_loaded);
            if needed {
                asyncload::AsyncLoadHistogramNeededItems.Insert(
                    asyncload::TableItemID {
                        TableID: physical_id,
                        ID: index.ID,
                        IsIndex: true,
                        IsSyncLoadFailed: sync_load_failed,
                    },
                    true,
                );
            }
        }
    }

    /// 解析索引列 ID。
    fn index_column_id(
        &self,
        table: &astersql_meta_model::TableInfo,
        index_column: &astersql_meta_model::IndexColumn,
    ) -> i64 {
        table
            .Columns
            .iter()
            .find(|column| column.Name.L == index_column.Name.L)
            .map_or(-1, |column| column.ID)
    }

    /// Adds predicate columns collected during user SELECT planning to the
    /// domain buffer. The usage becomes visible only after
    /// [`Self::dump_col_stats_usage_to_kv`], matching the Go worker boundary.
    pub fn record_predicate_column_usage(&self, table_id: i64, column_ids: &[i64]) {
        let timestamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let timestamp = Self::format_stats_usage_timestamp(timestamp);
        let mut pending = self
            .pending_column_usage
            .lock()
            .expect("pending column usage lock poisoned");
        for column_id in column_ids {
            pending.insert(
                (table_id, *column_id),
                RuntimeColumnUsage {
                    table_id,
                    column_id: *column_id,
                    last_used_at: Some(timestamp.clone()),
                    last_analyzed_at: None,
                },
            );
        }
    }

    /// `DELETE FROM mysql.column_stats_usage`: drops both the flushed rows and
    /// the usage still buffered by the session collectors.
    pub fn clear_column_usage(&self) {
        self.pending_column_usage
            .lock()
            .expect("pending column usage lock poisoned")
            .clear();
        self.stats_handle
            .lock()
            .expect("domain stats handle lock poisoned")
            .clear_column_usage();
    }

    /// Flushes collected predicate-column usage into the runtime equivalent of
    /// `mysql.column_stats_usage`.
    pub fn dump_col_stats_usage_to_kv(&self) -> Result<(), DomainError> {
        let usage = {
            let mut pending = self
                .pending_column_usage
                .lock()
                .expect("pending column usage lock poisoned");
            std::mem::take(&mut *pending)
        };
        let mut handle = self
            .stats_handle
            .lock()
            .expect("domain stats handle lock poisoned");
        for entry in usage.values() {
            handle.record_column_usage(entry.clone());
        }
        Ok(())
    }

    /// Loads persisted statistics metadata and histogram membership into the
    // / runtime cache. A non-empty ID list is additive; an empty list performs
    /// the bootstrap-style complete cache refresh.
    pub fn init_stats_lite(&self, table_ids: &[i64]) -> Result<(), DomainError> {
        let requested = table_ids.iter().copied().collect::<BTreeSet<_>>();
        let catalog = self
            .stats_catalog
            .read()
            .expect("domain stats catalog lock poisoned")
            .clone();
        let mut physical_tables = BTreeMap::new();
        for (_, table) in catalog.values() {
            let column_ids = table
                .Columns
                .iter()
                .map(|column| column.ID)
                .collect::<BTreeSet<_>>();
            let index_ids = table
                .Indices
                .iter()
                .map(|index| index.ID)
                .collect::<BTreeSet<_>>();
            for physical_id in Self::physical_ids(table) {
                if requested.is_empty() || requested.contains(&physical_id) {
                    physical_tables.insert(physical_id, (column_ids.clone(), index_ids.clone()));
                }
            }
        }
        let persisted = self
            .stats_store
            .init_tables_lite(&physical_tables, requested.is_empty())
            .map_err(DomainError::Stats)?;
        self.restore_lite_stats_versions(persisted.into_iter());
        Ok(())
    }

    /// Go's lite initialization reads `stats_ver` out of `mysql.stats_histograms`
    /// and keeps it on the table even though no histogram payload is loaded.
    /// The restricted statistics store keeps the analyzed payload separately
    /// from cached histogram membership, so replay the version from there.
    fn restore_lite_stats_versions(&self, physical_ids: impl Iterator<Item = i64>) {
        let persisted = self
            .persisted_histograms
            .lock()
            .expect("persisted histogram lock poisoned")
            .clone();
        let mut handle = self
            .stats_handle
            .lock()
            .expect("domain stats handle lock poisoned");
        for physical_id in physical_ids {
            let Some(payload) = persisted.get(&physical_id) else {
                continue;
            };
            if let Some(stats) = handle.cache_mut().get_mut(physical_id)
                && stats.stats_version == 0
            {
                stats.stats_version = payload.stats_version;
            }
        }
    }

    /// Sets the per-domain equivalent of `tidb_auto_analyze_ratio`.
    pub fn set_auto_analyze_ratio(&self, ratio: f64) -> Result<(), DomainError> {
        if !ratio.is_finite() || !(0.0..=1.0).contains(&ratio) {
            return Err(DomainError::Stats(
                "tidb_auto_analyze_ratio must be finite and between 0 and 1".to_owned(),
            ));
        }
        self.auto_analyze_ratio
            .store(ratio.to_bits(), Ordering::Release);
        Ok(())
    }

    /// 读取自动 Analyze 比例。
    pub fn auto_analyze_ratio(&self) -> f64 {
        f64::from_bits(self.auto_analyze_ratio.load(Ordering::Acquire))
    }

    /// Refreshes the statistics session values from this Domain's global
    /// variable store, the subset read by Go `UpdateSCtxVarsForStats`.
    pub fn stats_session_vars(&self) -> StatsSessionVars {
        self.stats_session_vars
            .read()
            .expect("domain statistics session variables lock poisoned")
            .clone()
    }

    /// Store a global system-variable value for current and future sessions.
    pub fn set_global_system_variable(&self, name: &str, value: &str) {
        let name = name.to_ascii_lowercase();
        self.global_system_variables
            .write()
            .expect("global system variable lock poisoned")
            .insert(name.clone(), value.to_owned());
        if matches!(
            name.as_str(),
            "tidb_exp_embed_openai_api_key"
                | "tidb_exp_embed_openai_api_base"
                | "tidb_exp_embed_jina_ai_api_key"
        ) {
            let version = self.embedding_config_version.fetch_add(1, Ordering::AcqRel) + 1;
            if let Some(embed_fn) = self.get_embed_fn() {
                embed_fn.set_config_version(version);
            }
        }
    }

    /// Read one global system-variable override.
    pub fn global_system_variable(&self, name: &str) -> Option<String> {
        self.global_system_variables
            .read()
            .expect("global system variable lock poisoned")
            .get(&name.to_ascii_lowercase())
            .cloned()
    }

    /// Snapshot global system-variable overrides for session initialization.
    pub fn global_system_variables(&self) -> BTreeMap<String, String> {
        self.global_system_variables
            .read()
            .expect("global system variable lock poisoned")
            .clone()
    }

    /// Go `GetCurrentPruneMode` equivalent for the Domain's statistics worker.
    pub fn get_current_prune_mode(&self) -> String {
        self.stats_session_vars().partition_prune_mode
    }

    /// 按名更新统计相关全局变量。
    pub fn set_stats_global_variable(&self, name: &str, value: &str) -> Result<(), DomainError> {
        let value = value.trim_matches(['\'', '"']);
        let enabled = |name: &str| match value.to_ascii_lowercase().as_str() {
            "1" | "on" | "true" => Ok(true),
            "0" | "off" | "false" => Ok(false),
            _ => Err(DomainError::Stats(format!("{name} must be ON or OFF"))),
        };
        let mut variables = self
            .stats_session_vars
            .write()
            .expect("domain statistics session variables lock poisoned");
        match name.to_ascii_lowercase().as_str() {
            "tidb_analyze_version" => {
                variables.analyze_version = value.parse().map_err(|_| {
                    DomainError::Stats("tidb_analyze_version must be an integer".to_owned())
                })?;
            }
            "tidb_partition_prune_mode" => {
                if !matches!(
                    value.to_ascii_lowercase().as_str(),
                    "dynamic" | "dynamic-only" | "static" | "static-only"
                ) {
                    return Err(DomainError::Stats(
                        "tidb_partition_prune_mode must be dynamic or static".to_owned(),
                    ));
                }
                variables.partition_prune_mode = value.to_ascii_lowercase();
            }
            "tidb_enable_historical_stats" => variables.historical_stats = enabled(name)?,
            "tidb_enable_analyze_snapshot" => variables.analyze_snapshot = enabled(name)?,
            "tidb_skip_missing_partition_stats" => {
                variables.skip_missing_partition_stats = enabled(name)?;
            }
            // Go `sysvar.go` validation for `tidb_enable_auto_analyze_priority_queue`:
            // turning the priority queue off is deprecated and rejected.
            "tidb_enable_auto_analyze_priority_queue" => {
                if !enabled(name)? {
                    return Err(DomainError::Stats(
                        "tidb_enable_auto_analyze_priority_queue has been deprecated and TiDB \
                         will always use priority queue to schedule auto analyze"
                            .to_owned(),
                    ));
                }
                vardef::EnableAutoAnalyzePriorityQueue.Store(true);
            }
            // Go `sysvar.go` for `tidb_analyze_column_options`.
            "tidb_analyze_column_options" => {
                let choice = value.to_ascii_uppercase();
                if !matches!(choice.as_str(), "ALL" | "PREDICATE") {
                    return Err(DomainError::Stats(format!(
                        "Variable 'tidb_analyze_column_options' can't be set to the value of '{value}'"
                    )));
                }
                vardef::AnalyzeColumnOptions.Store(&choice);
            }
            // Go `variable.ValidAnalyzeSkipColumnTypes`.
            "tidb_analyze_skip_column_types" => {
                if !value.is_empty()
                    && value
                        .to_ascii_lowercase()
                        .split(',')
                        .any(|item| !ANALYZE_SKIP_ALLOWED_TYPES.contains(&item.trim()))
                {
                    return Err(DomainError::Stats(format!(
                        "Variable 'tidb_analyze_skip_column_types' can't be set to the value of '{value}'"
                    )));
                }
                variables.analyze_skip_column_types = parse_analyze_skip_column_types(value);
            }
            _ => {}
        }
        Ok(())
    }

    /// Store the global `tidb_scatter_region` value for future sessions.
    pub fn set_global_scatter_region(&self, value: &str) {
        *self
            .global_scatter_region
            .write()
            .expect("global scatter region lock poisoned") = value.to_owned();
    }

    /// Value inherited by a new session, matching MySQL global/session scope.
    pub fn global_scatter_region(&self) -> String {
        self.global_scatter_region
            .read()
            .expect("global scatter region lock poisoned")
            .clone()
    }

    /// Store the global `tidb_txn_mode` value for future sessions.
    pub fn set_global_txn_mode(&self, value: &str) {
        *self
            .global_txn_mode
            .write()
            .expect("global transaction mode lock poisoned") = value.to_owned();
    }

    /// Value inherited by a new session, matching MySQL global/session scope.
    pub fn global_txn_mode(&self) -> String {
        self.global_txn_mode
            .read()
            .expect("global transaction mode lock poisoned")
            .clone()
    }

    /// Store the global TiFlash Compute dispatch policy for future sessions.
    pub fn set_global_tiflash_compute_dispatch_policy(
        &self,
        policy: astersql_util_tiflashcompute::DispatchPolicy,
    ) {
        *self
            .global_tiflash_compute_dispatch_policy
            .write()
            .expect("global TiFlash dispatch policy lock poisoned") = policy;
    }

    /// Value inherited by a new session, matching Go global/session scope.
    pub fn global_tiflash_compute_dispatch_policy(
        &self,
    ) -> astersql_util_tiflashcompute::DispatchPolicy {
        *self
            .global_tiflash_compute_dispatch_policy
            .read()
            .expect("global TiFlash dispatch policy lock poisoned")
    }

    /// Flushes eligible local statistics deltas to persistent storage. A
    /// forced call flushes every pending table, matching Go's
    /// `DumpStatsDeltaToKV(true)`.
    ///
    /// System/memory database tables are skipped (`metadef.IsMemOrSysDB`), and
    /// pending deltas for physical ids that no longer have handle meta are
    /// dropped, matching Go `needDumpStatsDelta`.
    pub fn dump_stats_delta_to_kv(&self, force: bool) -> Result<(), DomainError> {
        let pending = self
            .pending_stats_deltas
            .lock()
            .expect("domain pending stats delta lock poisoned")
            .clone();
        let ratio = dump_stats_delta_ratio();
        let catalog = self
            .stats_catalog
            .read()
            .expect("domain stats catalog lock poisoned")
            .clone();
        let handle = self
            .stats_handle
            .lock()
            .expect("domain stats handle lock poisoned");
        let mut drop_pending = Vec::new();
        let physical_ids = pending
            .iter()
            .filter_map(|(physical_id, (_, modified_rows))| {
                let db_name = catalog.values().find_map(|(key, table)| {
                    Self::physical_ids(table)
                        .contains(physical_id)
                        .then(|| key.database.as_str())
                });
                if db_name.is_some_and(Self::is_mem_or_sys_db) {
                    drop_pending.push(*physical_id);
                    return None;
                }
                if handle.stats_meta(*physical_id).is_none() {
                    drop_pending.push(*physical_id);
                    return None;
                }
                let should_dump = force
                    || handle.stats_meta(*physical_id).is_none_or(|stats| {
                        stats.realtime_count <= 0
                            || (*modified_rows as f64 / stats.realtime_count as f64) > ratio
                    });
                should_dump.then_some(*physical_id)
            })
            .collect::<Vec<_>>();
        drop(handle);
        if !drop_pending.is_empty() {
            let mut pending = self
                .pending_stats_deltas
                .lock()
                .expect("domain pending stats delta lock poisoned");
            for physical_id in drop_pending {
                pending.remove(&physical_id);
            }
        }
        if physical_ids.is_empty() {
            return Ok(());
        }
        self.flush_stats_delta_history(&physical_ids).map(|_| ())
    }

    /// 强制刷盘统计增量。
    pub fn flush_stats(&self) -> Result<(), DomainError> {
        self.dump_stats_delta_to_kv(true)
    }

    /// Runs one eligible auto-analyze operation. It flushes eligible deltas,
    /// skips locked tables, and considers every physical partition as an
    /// independent statistics target.
    pub fn handle_auto_analyze(&self) -> bool {
        self.try_handle_auto_analyze().unwrap_or(false)
    }

    /// Fallible variant for callers that need persistence and lock errors.
    ///
    /// Matches Go `HandleAutoAnalyze` for partitioned tables in dynamic prune
    /// mode: once any physical target of a table is eligible, analyze every
    /// unlocked physical id for that table (global + partitions) so partition
    /// and global `modify_count` are cleared together.
    pub fn try_handle_auto_analyze(&self) -> Result<bool, DomainError> {
        self.dump_stats_delta_to_kv(false)?;
        let locked = self.stats_store.locked_ids().map_err(DomainError::Stats)?;
        let ratio = self.auto_analyze_ratio();
        let catalog = self
            .stats_catalog
            .read()
            .expect("domain stats catalog lock poisoned")
            .clone();
        let handle = self
            .stats_handle
            .lock()
            .expect("domain stats handle lock poisoned");
        let needs_analyze = |physical_id: i64| -> bool {
            if locked.contains(&physical_id) {
                return false;
            }
            let Some(stats) = handle.stats_meta(physical_id) else {
                return false;
            };
            let meets_minimum = stats.realtime_count >= EffectiveAutoAnalyzeMinCnt();
            let is_unanalyzed = stats.last_analyze_version == 0;
            // Go `NeedAnalyzeTable` uses AnalyzeRowCount (hist) when available.
            let tbl_cnt = if stats.analyze_count > 0 {
                stats.analyze_count as f64
            } else {
                stats.realtime_count as f64
            };
            let needs_reanalyze =
                ratio > 0.0 && tbl_cnt > 0.0 && (stats.modify_count as f64 / tbl_cnt) > ratio;
            meets_minimum && (is_unanalyzed || needs_reanalyze)
        };
        // Go `analyzeHasNoStatsIndexes`: a table that no longer needs a full
        // re-analyze is still picked when one of its public indexes carries no
        // statistics yet, for instance right after `ALTER TABLE ... ADD INDEX`.
        let indexes_without_stats =
            |physical_id: i64, table: &astersql_meta_model::TableInfo| -> Vec<String> {
                if locked.contains(&physical_id) {
                    return Vec::new();
                }
                let Some(stats) = handle.stats_meta(physical_id) else {
                    return Vec::new();
                };
                if stats.pseudo || stats.realtime_count < EffectiveAutoAnalyzeMinCnt() {
                    return Vec::new();
                }
                table
                    .Indices
                    .iter()
                    .filter(|index| {
                        index.State == astersql_meta_model::SchemaState::Public
                            && !index.MVIndex
                            && index.VectorInfo.is_none()
                            && index.InvertedInfo.is_none()
                            && index.FullTextInfo.is_none()
                            && !stats.indexes.contains_key(&index.ID)
                    })
                    .map(|index| index.Name.L.clone())
                    .collect()
            };
        let table_targets = |key: &StatsTableKey, table: &astersql_meta_model::TableInfo| {
            Self::physical_ids(table)
                .into_iter()
                .filter(|physical_id| !locked.contains(physical_id))
                .map(|physical_id| StatsTableKey::new(&key.database, &key.table, physical_id))
                .collect::<Vec<_>>()
        };
        let full_analyze = catalog.values().find_map(|(key, table)| {
            if Self::is_mem_or_sys_db(&key.database) {
                return None;
            }
            if !Self::physical_ids(table)
                .iter()
                .any(|physical_id| needs_analyze(*physical_id))
            {
                return None;
            }
            let targets = table_targets(key, table);
            (!targets.is_empty()).then(|| (targets, Vec::new()))
        });
        let candidate = full_analyze.or_else(|| {
            catalog.values().find_map(|(key, table)| {
                if Self::is_mem_or_sys_db(&key.database) {
                    return None;
                }
                let indexes = Self::physical_ids(table)
                    .into_iter()
                    .flat_map(|physical_id| indexes_without_stats(physical_id, table))
                    .collect::<BTreeSet<_>>();
                if indexes.is_empty() {
                    return None;
                }
                let targets = table_targets(key, table);
                (!targets.is_empty()).then(|| (targets, indexes.into_iter().collect::<Vec<_>>()))
            })
        });
        drop(handle);
        let Some((candidates, indexes)) = candidate else {
            return Ok(false);
        };
        // Go's auto-analyze worker runs `analyze table ...` through a system
        // session. When the session layer registered one, use it so the job is
        // recorded in `mysql.analyze_jobs` and real histograms are rebuilt.
        if let Some(executor) = self.auto_analyze_executor() {
            let table = &candidates[0];
            let mut sql = format!("analyze table `{}`.`{}`", table.database, table.table);
            for index in &indexes {
                sql.push_str(&format!(" index `{index}`"));
            }
            return match executor.execute_auto_analyze(&sql) {
                Ok(()) => {
                    astersql_metrics::stats::IncAutoAnalyzeCounter("succ");
                    Ok(true)
                }
                Err(error) => {
                    astersql_metrics::stats::IncAutoAnalyzeCounter("failed");
                    Err(DomainError::Stats(error))
                }
            };
        }
        let result = candidates
            .iter()
            .try_for_each(|candidate| self.analyze_stats_table(candidate).map(|_| ()));
        match result {
            Ok(()) => {
                astersql_metrics::stats::IncAutoAnalyzeCounter("succ");
                Ok(true)
            }
            Err(error) => {
                astersql_metrics::stats::IncAutoAnalyzeCounter("failed");
                Err(error)
            }
        }
    }

    /// Registers the SQL executor Go reaches through its system session pool.
    /// The Domain keeps a weak handle so the session owner stays in charge of
    /// the executor lifetime.
    pub fn register_auto_analyze_executor(
        &self,
        executor: std::sync::Weak<dyn AutoAnalyzeExecutor>,
    ) {
        *self
            .auto_analyze_executor
            .write()
            .expect("domain auto analyze executor lock poisoned") = Some(executor);
    }

    /// 升级 Weak 执行器引用。
    fn auto_analyze_executor(&self) -> Option<Arc<dyn AutoAnalyzeExecutor>> {
        self.auto_analyze_executor
            .read()
            .expect("domain auto analyze executor lock poisoned")
            .as_ref()
            .and_then(std::sync::Weak::upgrade)
    }

    /// 返回统计锁实现。
    pub fn stats_lock(&self) -> lockstats::statsLockImpl {
        lockstats::NewStatsLock(Arc::clone(&self.stats_store) as lockstats::SessionRef)
    }

    /// 列出已锁定统计的表行。
    pub fn stats_locked_rows(&self) -> Result<Vec<(i64, i64, i64)>, DomainError> {
        self.stats_store.locked_rows().map_err(DomainError::Stats)
    }

    /// 读取已持久化的统计元数据行。
    pub fn persisted_stats_meta_rows(&self) -> Result<Vec<(i64, u64, i64, i64)>, DomainError> {
        self.stats_store
            .persisted_meta_rows()
            .map_err(DomainError::Stats)
    }

    /// 受限统计查询入口。
    pub fn restricted_stats_query(
        &self,
        sql: &str,
        arguments: &[lockstats::SqlValue],
    ) -> Result<Vec<Vec<String>>, DomainError> {
        self.stats_store
            .query_strings(sql, arguments)
            .map_err(|error| DomainError::Stats(error.to_string()))
    }

    /// 受限统计执行入口。
    pub fn restricted_stats_execute(
        &self,
        sql: &str,
        arguments: &[lockstats::SqlValue],
    ) -> Result<(), DomainError> {
        self.stats_store
            .execute(sql, arguments)
            .map(|_| ())
            .map_err(|error| DomainError::Stats(error.to_string()))
    }

    pub fn fail_next_stats_lock_delete_for_test(&self) {
        self.stats_store.fail_next_lock_delete_for_test();
    }

    pub fn fail_next_ddl_publication_for_test(&self) {
        self.fail_next_ddl_publication
            .store(true, Ordering::Release);
    }

    pub fn EnablePanicWhenRecordingHistoricalStatsMetaForTest(&self) -> HistoricalMetaPanicGuard {
        self.stats_handle
            .lock()
            .expect("domain stats handle lock poisoned")
            .EnablePanicWhenRecordingHistoricalStatsMetaForTest()
    }

    /// 持久化指定物理表的统计元数据。
    pub fn persist_stats_meta(&self, physical_ids: &[i64]) -> Result<(), DomainError> {
        for physical_id in physical_ids {
            let profile = self
                .stats_handle
                .lock()
                .expect("domain stats handle lock poisoned")
                .stats_meta(*physical_id)
                .cloned()
                .ok_or_else(|| {
                    DomainError::Stats(format!(
                        "cannot persist statistics for unknown table {physical_id}"
                    ))
                })?;
            self.stats_store
                .persist_meta(*physical_id)
                .map_err(DomainError::Stats)?;
            self.persisted_histograms
                .lock()
                .expect("persisted histogram lock poisoned")
                .insert(*physical_id, profile);
        }
        Ok(())
    }

    /// Persist zero-valued histogram identities for a logical table without
    /// making it an analyzed global statistics profile.
    pub fn persist_unanalyzed_histograms(
        &self,
        table_id: i64,
        column_ids: &[i64],
        index_ids: &[i64],
    ) -> Result<(), DomainError> {
        self.stats_store
            .persist_unanalyzed_histograms(table_id, column_ids, index_ids)
            .map_err(DomainError::Stats)
    }

    /// Invalidate persisted histogram versions, matching
    /// `UPDATE mysql.stats_histograms SET stats_ver = 0` used by CBO tests.
    /// The restricted stats store keeps histogram membership as keys and the
    /// analyzed payload separately, so update both the persisted profile and
    /// the live cache before a subsequent `Update` reloads it.
    pub fn invalidate_histogram_stats_versions(&self) {
        {
            let mut persisted = self
                .persisted_histograms
                .lock()
                .expect("persisted histogram lock poisoned");
            for profile in persisted.values_mut() {
                profile.stats_version = 0;
                for column in profile.columns.values_mut() {
                    column.stats_version = 0;
                }
                for index in profile.indexes.values_mut() {
                    index.stats_version = 0;
                }
            }
        }
        let mut handle = self
            .stats_handle
            .lock()
            .expect("domain stats handle lock poisoned");
        let table_ids = handle
            .stats_meta_rows()
            .into_iter()
            .map(|profile| profile.physical_id)
            .collect::<Vec<_>>();
        for table_id in table_ids {
            if let Some(profile) = handle.cache_mut().get_mut(table_id) {
                profile.stats_version = 0;
                for column in profile.columns.values_mut() {
                    column.stats_version = 0;
                }
                for index in profile.indexes.values_mut() {
                    index.stats_version = 0;
                }
            }
        }
    }

    /// 按 DDL 租约回收过期统计。
    pub fn gc_stats(&self, ddl_lease: Duration) -> Result<(), DomainError> {
        // Go `GCStats` removes the statistics of every physical table that no
        // longer exists in the InfoSchema. The scan below is bounded by the
        // persisted GC watermark, so drop the known-dropped tables explicitly
        // first instead of waiting for their `mysql.stats_meta` version to fall
        // inside the scanned window.
        self.stats_context().gc_dropped_stats()?;
        stats_storage::new_stats_gc(self.stats_store.as_ref(), self)
            .gc_stats(ddl_lease)
            .map_err(|error| DomainError::Stats(error.to_string()))?;
        self.dropped_stats_ids
            .lock()
            .expect("dropped stats IDs lock poisoned")
            .clear();
        Ok(())
    }

    /// 清理超出保留期的历史统计。
    pub fn clear_outdated_history_stats(&self, retention: Duration) -> Result<(), DomainError> {
        stats_storage::new_stats_gc(self.stats_store.as_ref(), self)
            .clear_outdated_history_stats(retention)
            .map_err(|error| DomainError::Stats(error.to_string()))
    }

    /// 构造统计上下文。
    pub fn stats_context(&self) -> DomainStatsContext {
        DomainStatsContext {
            handle: Arc::clone(&self.stats_handle),
            stats_store: Arc::clone(&self.stats_store),
            catalog: Arc::clone(&self.stats_catalog),
            catalog_history: Arc::clone(&self.stats_catalog_history),
            catalog_version: Arc::clone(&self.stats_catalog_version),
            catalog_timeline: Arc::clone(&self.stats_catalog_timeline),
            dropped_stats_ids: Arc::clone(&self.dropped_stats_ids),
            preflush_count: Arc::clone(&self.stats_preflush_count),
            pending_deltas: Arc::clone(&self.pending_stats_deltas),
            last_preflush_ids: Arc::clone(&self.last_stats_preflush_ids),
        }
    }

    /// 收集表及其分区物理 ID。
    fn physical_ids(table: &astersql_meta_model::TableInfo) -> BTreeSet<i64> {
        let mut ids = BTreeSet::from([table.ID]);
        if let Some(partition) = table.GetPartitionInfo() {
            ids.extend(partition.Definitions.iter().map(|definition| definition.ID));
        }
        ids
    }

    /// Local stand-in for `metadef.IsMemOrSysDB` (crate is windows-gated in domain deps).
    fn is_mem_or_sys_db(db_lower_name: &str) -> bool {
        matches!(
            db_lower_name,
            "mysql" | "information_schema" | "performance_schema" | "metrics_schema" | "sys"
        ) || db_lower_name.starts_with("__tidb_br_temporary_")
    }

    /// 格式化统计使用时间戳。
    fn format_stats_usage_timestamp(seconds_since_epoch: i64) -> String {
        let days = seconds_since_epoch.div_euclid(86_400);
        let seconds = seconds_since_epoch.rem_euclid(86_400);
        let z = days + 719_468;
        let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
        let mut year = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let month_prime = (5 * doy + 2) / 153;
        let day = doy - (153 * month_prime + 2) / 5 + 1;
        let month = month_prime + if month_prime < 10 { 3 } else { -9 };
        year += i64::from(month <= 2);
        format!(
            "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}",
            seconds / 3_600,
            (seconds % 3_600) / 60,
            seconds % 60
        )
    }

    fn new_stats_auto_id_allocator(
        &self,
        table: &astersql_meta_model::TableInfo,
        allocator_type: AllocatorType,
        is_unsigned: bool,
    ) -> Arc<dyn AutoIdAllocator> {
        Arc::new(DefaultAllocator::with_options(
            Arc::clone(&self.auto_id_store),
            table.DBID,
            table.ID,
            is_unsigned,
            allocator_type,
            &[
                AllocatorOption::CustomStep(table.AutoIDCache),
                AllocatorOption::TableInfoVersion(table.Version),
            ],
        ))
    }

    fn rebase_stats_auto_id_allocator(
        allocator: &Arc<dyn AutoIdAllocator>,
        persisted_base: i64,
    ) -> Result<(), DomainError> {
        if persisted_base <= 0 {
            return Ok(());
        }
        allocator
            .rebase(&AutoIdContext::background(), persisted_base, false)
            .map_err(|error| DomainError::Store(error.to_string()))
    }

    /// Go `handleAutoIncID` stores the requested next value in table metadata,
    /// but rebases the allocator to one less so its first allocation returns
    /// the requested `AUTO_INCREMENT` value.
    fn auto_increment_allocator_base(next_value: i64) -> i64 {
        if next_value > 1 { next_value - 1 } else { 0 }
    }

    /// 重建表到分配器的路由，并复用仍兼容的本地预留区间。
    fn reconcile_stats_auto_id_allocators(
        &self,
        catalog: &BTreeMap<(String, String), (StatsTableKey, astersql_meta_model::TableInfo)>,
    ) -> Result<(), DomainError> {
        let existing = self
            .stats_auto_id_allocators
            .lock()
            .expect("domain AutoID allocator lock poisoned")
            .clone();
        let mut rebuilt = BTreeMap::new();

        for (_, table) in catalog.values() {
            let has_row_id = !table.PKIsHandle && !table.IsCommonHandle;
            let has_auto_increment = table.GetAutoIncrementColInfo().is_some();
            let separate_auto_increment = table.SepAutoInc();

            let row_allocator = if has_row_id || (has_auto_increment && !separate_auto_increment) {
                let allocator = existing
                    .get(&(table.ID, 2))
                    .or_else(|| existing.get(&(table.ID, 0)))
                    .filter(|allocator| allocator.get_type() == AllocatorType::RowId)
                    .cloned()
                    .unwrap_or_else(|| {
                        self.new_stats_auto_id_allocator(
                            table,
                            AllocatorType::RowId,
                            table.IsAutoIncColUnsigned(),
                        )
                    });
                let row_base = if separate_auto_increment {
                    table.AutoIncIDExtra
                } else {
                    table.AutoIncID.max(table.AutoIncIDExtra)
                };
                Self::rebase_stats_auto_id_allocator(
                    &allocator,
                    Self::auto_increment_allocator_base(row_base),
                )?;
                Some(allocator)
            } else {
                None
            };

            if has_auto_increment {
                let allocator = if separate_auto_increment {
                    let allocator = existing
                        .get(&(table.ID, 0))
                        .filter(|allocator| allocator.get_type() == AllocatorType::AutoIncrement)
                        .cloned()
                        .unwrap_or_else(|| {
                            self.new_stats_auto_id_allocator(
                                table,
                                AllocatorType::AutoIncrement,
                                table.IsAutoIncColUnsigned(),
                            )
                        });
                    Self::rebase_stats_auto_id_allocator(
                        &allocator,
                        Self::auto_increment_allocator_base(table.AutoIncID),
                    )?;
                    allocator
                } else {
                    row_allocator
                        .as_ref()
                        .expect("shared AUTO_INCREMENT requires RowID allocator")
                        .clone()
                };
                rebuilt.insert((table.ID, 0), allocator);
            }

            if table.ContainsAutoRandomBits() {
                let allocator = existing
                    .get(&(table.ID, 1))
                    .filter(|allocator| allocator.get_type() == AllocatorType::AutoRandom)
                    .cloned()
                    .unwrap_or_else(|| {
                        self.new_stats_auto_id_allocator(
                            table,
                            AllocatorType::AutoRandom,
                            table.IsAutoRandomBitColUnsigned(),
                        )
                    });
                Self::rebase_stats_auto_id_allocator(
                    &allocator,
                    Self::auto_increment_allocator_base(table.AutoRandID),
                )?;
                rebuilt.insert((table.ID, 1), allocator);
            }

            if has_row_id {
                rebuilt.insert(
                    (table.ID, 2),
                    row_allocator.expect("hidden row handle requires RowID allocator"),
                );
            }
        }

        *self
            .stats_auto_id_allocators
            .lock()
            .expect("domain AutoID allocator lock poisoned") = rebuilt;
        Ok(())
    }

    /// 根据已提交的 DDL 元数据变更调和本地统计/catalog 状态。
    fn reconcile_from_committed_metadata_with_mode(
        &self,
        reconcile_persisted_stats: bool,
    ) -> Result<(), DomainError> {
        let loaded = self.load_info_schema(&self.config.keyspace)?;
        let mut catalog = BTreeMap::new();
        let mut physical_tables = BTreeMap::new();
        let mut temporary_ids = BTreeSet::new();
        let mut auto_ids = BTreeMap::new();
        let tables = loaded
            .schema
            .AllSchemas()
            .into_iter()
            .flat_map(|database| {
                let database_name = database.name.original.clone();
                database
                    .tables
                    .iter()
                    .filter_map(move |table| {
                        table
                            .model_meta
                            .as_ref()
                            .cloned()
                            .map(|table| (database_name.clone(), table.as_ref().clone()))
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        for (database, table) in tables {
            let key = (database.to_ascii_lowercase(), table.Name.L.clone());
            let column_ids = table
                .Columns
                .iter()
                .map(|column| column.ID)
                .collect::<BTreeSet<_>>();
            let index_ids = table
                .Indices
                .iter()
                .map(|index| index.ID)
                .collect::<BTreeSet<_>>();
            // Go never caches system/memory table statistics. Temporary
            // tables also stay out of the cache until explicitly analyzed:
            // sessions borrowed from the pool cannot read their metadata.
            let temporary = table.TempTableType != astersql_meta_model::TempTableNone;
            let uncached = temporary || Self::is_mem_or_sys_db(&key.0);
            for physical_id in Self::physical_ids(&table) {
                if uncached {
                    temporary_ids.insert(physical_id);
                    continue;
                }
                physical_tables.insert(physical_id, (column_ids.clone(), index_ids.clone()));
            }
            auto_ids.insert(
                table.ID,
                u64::try_from(table.AutoIncID.max(table.AutoIncIDExtra).max(1)).unwrap_or(1),
            );
            catalog.insert(
                key.clone(),
                (StatsTableKey::new(&key.0, &key.1, table.ID), table),
            );
        }

        self.reconcile_stats_auto_id_allocators(&catalog)?;

        let canonical_ids = physical_tables
            .keys()
            .copied()
            .chain(temporary_ids.iter().copied())
            .collect::<BTreeSet<_>>();
        let previous_ids = self
            .stats_catalog
            .read()
            .expect("domain stats catalog lock poisoned")
            .values()
            .flat_map(|(_, table)| Self::physical_ids(table))
            .collect::<BTreeSet<_>>();
        let (persisted_ids, locked) = if physical_tables.is_empty() {
            let stale = previous_ids.iter().copied().collect::<Vec<_>>();
            self.stats_handle
                .lock()
                .expect("domain stats handle lock poisoned")
                .remove_tables(&stale);
            (previous_ids.clone(), BTreeSet::new())
        } else if !reconcile_persisted_stats {
            // Domain creation only publishes schema/catalog. Go starts
            // `initStats` after bootstrap sessions and system tables are ready.
            (BTreeSet::new(), BTreeSet::new())
        } else {
            let persisted_ids = self
                .stats_store
                .persisted_meta_rows()
                .map_err(DomainError::Stats)?
                .into_iter()
                .map(|row| row.0)
                .collect::<BTreeSet<_>>();
            self.stats_store
                .reconcile_tables(&physical_tables)
                .map_err(DomainError::Stats)?;
            let locked = self.stats_store.locked_ids().map_err(DomainError::Stats)?;
            (persisted_ids, locked)
        };

        for (_, table) in catalog.values() {
            if !locked.contains(&table.ID) {
                continue;
            }
            for physical_id in Self::physical_ids(table) {
                if locked.contains(&physical_id) {
                    continue;
                }
                let mut insert_lock = |executor: &mut dyn lockstats::RestrictedSQLExecutor| {
                    lockstats::InsertLockAndUpdateVersion(executor, physical_id)
                };
                lockstats::StatsSession::WithSession(
                    self.stats_store.as_ref(),
                    true,
                    &mut insert_lock,
                )
                .map_err(|error| DomainError::Stats(error.to_string()))?;
            }
        }

        self.dropped_stats_ids
            .lock()
            .expect("dropped stats IDs lock poisoned")
            .extend(
                previous_ids
                    .difference(&canonical_ids)
                    .chain(persisted_ids.difference(&canonical_ids))
                    .copied(),
            );
        {
            // Row-ID allocation survives DDL in Go because it is persisted in
            // the table meta; keep the high-water mark so a schema change does
            // not hand out handles that existing records already own.
            let mut allocators = self
                .stats_auto_ids
                .lock()
                .expect("domain stats auto ID lock poisoned");
            for (table_id, next_id) in &mut auto_ids {
                if let Some(allocated) = allocators.get(table_id) {
                    *next_id = (*next_id).max(*allocated);
                }
            }
            *allocators = auto_ids;
        }
        *self
            .stats_catalog
            .write()
            .expect("domain stats catalog lock poisoned") = catalog;
        self.info_cache.Insert(loaded.schema, loaded.timestamp);
        self.record_stats_catalog_version();
        Ok(())
    }

    fn reconcile_from_committed_metadata(&self) -> Result<(), DomainError> {
        self.reconcile_from_committed_metadata_with_mode(true)
    }

    /// 发布 DDL 元数据变更通知。
    fn publish_ddl_metadata_change(
        &self,
        change: DdlMetadataChange,
    ) -> Result<DdlMetadataChange, DomainError> {
        if !change.changed {
            return Ok(change);
        }
        let publication = if self.fail_next_ddl_publication.swap(false, Ordering::AcqRel) {
            Err(DomainError::Stats(
                "injected DDL runtime publication failure".to_owned(),
            ))
        } else {
            self.reconcile_from_committed_metadata()
        };
        if publication.is_err() {
            self.reconcile_from_committed_metadata()?;
        }
        Ok(change)
    }

    /// DDL 建库后刷新 canonical InfoSchema。
    pub fn ddl_create_database(
        &self,
        database: &str,
        if_not_exists: bool,
    ) -> Result<(), DomainError> {
        self.ddl_create_database_with_id(database, if_not_exists, None)
    }

    /// DDL 建库，并允许 next-gen bootstrap 注入 metadef 保留 ID。
    pub fn ddl_create_database_with_id(
        &self,
        database: &str,
        if_not_exists: bool,
        preferred_id: Option<i64>,
    ) -> Result<(), DomainError> {
        let change = self
            .store
            .with_storage(|store| {
                self.ddl_metadata.create_database_with_id(
                    store,
                    database,
                    if_not_exists,
                    preferred_id,
                )
            })
            .map_err(|error| DomainError::Ddl(error.to_string()))?;
        self.publish_ddl_metadata_change(change).map(|_| ())
    }

    /// DDL 删库及库内表后刷新 canonical InfoSchema。
    pub fn ddl_drop_database(&self, database: &str, if_exists: bool) -> Result<(), DomainError> {
        let change = self
            .store
            .with_storage(|store| self.ddl_metadata.drop_database(store, database, if_exists))
            .map_err(|error| DomainError::Ddl(error.to_string()))?;
        let change = self.publish_ddl_metadata_change(change)?;
        self.remove_ttl_tables_from_external_workload(&change.old_tables)?;
        self.remove_tiflash_rules_for_dropped_tables(&change.old_tables)
    }

    /// Read canonical database names directly from KV so empty schemas remain
    /// observable even while an InfoSchema cache is being refreshed.
    pub fn ddl_database_names(&self) -> Result<Vec<String>, DomainError> {
        self.store
            .with_storage(|store| self.ddl_metadata.database_names(store))
            .map_err(|error| DomainError::Ddl(error.to_string()))
    }

    /// DDL 建表后同步统计 catalog。
    pub fn ddl_create_table(
        &self,
        database: &str,
        table: astersql_meta_model::TableInfo,
        if_not_exists: bool,
    ) -> Result<astersql_meta_model::TableInfo, DomainError> {
        let change = self
            .store
            .with_storage(|store| {
                self.ddl_metadata
                    .create_table(store, database, table, if_not_exists)
            })
            .map_err(|error| DomainError::Ddl(error.to_string()))?;
        let created_now = change.changed;
        let change = self.publish_ddl_metadata_change(change)?;
        let created = change
            .new_tables
            .into_iter()
            .next()
            .map(|(_, table)| table)
            .ok_or_else(|| DomainError::Ddl("CREATE TABLE produced no metadata".to_owned()))?;
        if created_now && created.TTLInfo.as_ref().is_some_and(|ttl| ttl.Enable) {
            self.sync_ttl_table_to_external_workload(&created)?;
        }
        Ok(created)
    }

    fn sync_ttl_table_to_external_workload(
        &self,
        table: &astersql_meta_model::TableInfo,
    ) -> Result<(), DomainError> {
        let Some(manager) = self.external_workload_manager() else {
            return Ok(());
        };
        let mut manager = manager
            .lock()
            .expect("external workload manager lock poisoned");
        let context = astersql_extworkload::context::Background();
        let result = if table.TTLInfo.as_ref().is_some_and(|ttl| ttl.Enable) {
            manager.RegisterTTLTask(
                &context,
                table.ID,
                astersql_sessionctx_vardef::EnableTTLJob.Load(),
            )
        } else {
            manager.DeleteTTLTableInfo(&context, table.ID)
        };
        result.map_err(|error| {
            DomainError::Ddl(format!("sync TTL table with external workload: {error}"))
        })
    }

    /// Persist a materialized-view log and link its base table atomically.
    pub fn ddl_create_materialized_view_log(
        &self,
        database: &str,
        base_name: &str,
        log: astersql_meta_model::TableInfo,
        next_purge_unix_seconds: Option<i64>,
    ) -> Result<(), DomainError> {
        let change = self
            .store
            .with_storage(|store| {
                self.ddl_metadata.create_materialized_view_log(
                    store,
                    database,
                    base_name,
                    log,
                    next_purge_unix_seconds,
                )
            })
            .map_err(|error| DomainError::Ddl(error.to_string()))?;
        self.publish_ddl_metadata_change(change).map(|_| ())
    }

    /// Install an available virtual TiFlash replica for planner casetests.
    /// This is the Rust equivalent of Go `testkit.SetTiFlashReplica`.
    pub fn set_tiflash_replica_for_test(
        &self,
        database: &str,
        table: &str,
        count: u64,
        available: bool,
    ) -> Result<(), DomainError> {
        let change = self
            .store
            .with_storage(|store| {
                self.ddl_metadata.set_tiflash_replica(
                    store,
                    database,
                    table,
                    count,
                    available,
                    Vec::new(),
                )
            })
            .map_err(|error| DomainError::Ddl(error.to_string()))?;
        self.publish_ddl_metadata_change(change).map(|_| ())
    }

    /// Publish TiFlash replica metadata changed by ALTER TABLE.
    pub fn ddl_set_tiflash_replica(
        &self,
        database: &str,
        table: &str,
        count: u64,
        location_labels: Vec<String>,
    ) -> Result<(), DomainError> {
        let change = self
            .store
            .with_storage(|store| {
                self.ddl_metadata.set_tiflash_replica(
                    store,
                    database,
                    table,
                    count,
                    false,
                    location_labels.clone(),
                )
            })
            .map_err(|error| DomainError::Ddl(error.to_string()))?;
        let change = self.publish_ddl_metadata_change(change)?;
        if let Some((_, table)) = change.new_tables.first() {
            self.store
                .with_storage(|store| {
                    if let Some(partition) = &table.Partition {
                        for definition in &partition.Definitions {
                            store.PublishTiFlashPlacementRule(
                                definition.ID,
                                count,
                                &location_labels,
                            )?;
                        }
                        store.PublishTiFlashPlacementRule(table.ID, 0, &[])?;
                    } else {
                        store.PublishTiFlashPlacementRule(table.ID, count, &location_labels)?;
                    }
                    Ok::<_, astersql_kv::errors::SharedError>(())
                })
                .map_err(|error| DomainError::Ddl(error.to_string()))?;
        }
        Ok(())
    }

    /// Publish replica progress observed by a TiFlash status collector.
    /// Only full progress can make the replica available to the planner.
    pub fn publish_tiflash_replica_progress(
        &self,
        database: &str,
        table: &str,
        table_id: i64,
        progress: f64,
    ) -> Result<(), DomainError> {
        if !progress.is_finite() || !(0.0..=1.0).contains(&progress) {
            return Err(DomainError::Ddl(
                "invalid TiFlash replica progress".to_owned(),
            ));
        }
        let change = self
            .store
            .with_storage(|store| {
                self.ddl_metadata.update_tiflash_replica_availability(
                    store,
                    database,
                    table,
                    table_id,
                    progress == 1.0,
                )
            })
            .map_err(|error| DomainError::Ddl(error.to_string()))?;
        self.publish_ddl_metadata_change(change).map(|_| ())
    }

    /// DDL 删表后清理统计。
    pub fn ddl_drop_tables(
        &self,
        tables: Vec<(String, String)>,
        if_exists: bool,
    ) -> Result<(), DomainError> {
        let change = self
            .store
            .with_storage(|store| self.ddl_metadata.drop_tables(store, tables, if_exists))
            .map_err(|error| DomainError::Ddl(error.to_string()))?;
        let change = self.publish_ddl_metadata_change(change)?;
        self.remove_ttl_tables_from_external_workload(&change.old_tables)?;
        self.remove_tiflash_rules_for_dropped_tables(&change.old_tables)
    }

    fn remove_ttl_tables_from_external_workload(
        &self,
        tables: &[(String, astersql_meta_model::TableInfo)],
    ) -> Result<(), DomainError> {
        if let Some(manager) = self.external_workload_manager() {
            let mut manager = manager
                .lock()
                .expect("external workload manager lock poisoned");
            let context = astersql_extworkload::context::Background();
            for (_, table) in tables {
                manager
                    .DeleteTTLTableInfo(&context, table.ID)
                    .map_err(|error| {
                        DomainError::Ddl(format!(
                            "remove TTL table from external workload: {error}"
                        ))
                    })?;
            }
        }
        Ok(())
    }

    fn remove_tiflash_rules_for_dropped_tables(
        &self,
        tables: &[(String, astersql_meta_model::TableInfo)],
    ) -> Result<(), DomainError> {
        self.store
            .with_storage(|store| {
                for (_, table) in tables {
                    if table.TiFlashReplica.is_none() {
                        continue;
                    }
                    if let Some(partition) = &table.Partition {
                        for definition in &partition.Definitions {
                            store.PublishTiFlashPlacementRule(definition.ID, 0, &[])?;
                        }
                    }
                    store.PublishTiFlashPlacementRule(table.ID, 0, &[])?;
                }
                Ok::<_, astersql_kv::errors::SharedError>(())
            })
            .map_err(|error| DomainError::Ddl(error.to_string()))
    }

    /// Persist ALTER TABLE SHARD_ROW_ID_BITS through canonical metadata.
    pub fn ddl_set_shard_row_id_bits(
        &self,
        database: &str,
        table: &str,
        bits: u64,
    ) -> Result<(), DomainError> {
        let change = self
            .store
            .with_storage(|store| {
                self.ddl_metadata
                    .set_shard_row_id_bits(store, database, table, bits)
            })
            .map_err(|error| DomainError::Ddl(error.to_string()))?;
        self.publish_ddl_metadata_change(change).map(|_| ())
    }

    /// 原子重命名单表或多表，保留物理表 ID 与现有数据。
    pub fn ddl_rename_tables(
        &self,
        renames: Vec<(String, String, String, String)>,
    ) -> Result<(), DomainError> {
        let change = self
            .store
            .with_storage(|store| self.ddl_metadata.rename_tables(store, renames))
            .map_err(|error| DomainError::Ddl(error.to_string()))?;
        self.publish_ddl_metadata_change(change).map(|_| ())
    }

    /// DDL 截断表后重置统计。
    pub fn ddl_truncate_table(
        &self,
        database: &str,
        table: &str,
    ) -> Result<astersql_meta_model::TableInfo, DomainError> {
        let change = self
            .store
            .with_storage(|store| self.ddl_metadata.truncate_table(store, database, table))
            .map_err(|error| DomainError::Ddl(error.to_string()))?;
        let change = self.publish_ddl_metadata_change(change)?;
        change
            .new_tables
            .into_iter()
            .next()
            .map(|(_, table)| table)
            .ok_or_else(|| DomainError::Ddl("TRUNCATE TABLE produced no metadata".to_owned()))
    }

    /// DDL 替换分区后更新统计。
    pub fn ddl_replace_partitions(
        &self,
        database: &str,
        table: &str,
        removed_names: &BTreeSet<String>,
        added: Vec<astersql_meta_model::PartitionDefinition>,
    ) -> Result<astersql_meta_model::TableInfo, DomainError> {
        let change = self
            .store
            .with_storage(|store| {
                self.ddl_metadata
                    .replace_partitions(store, database, table, removed_names, added)
            })
            .map_err(|error| DomainError::Ddl(error.to_string()))?;
        let change = self.publish_ddl_metadata_change(change)?;
        change
            .new_tables
            .into_iter()
            .next()
            .map(|(_, table)| table)
            .ok_or_else(|| DomainError::Ddl("partition DDL produced no metadata".to_owned()))
    }

    pub fn ddl_set_table_partitioning(
        &self,
        database: &str,
        table: &str,
        partition: Option<astersql_meta_model::PartitionInfo>,
    ) -> Result<(), DomainError> {
        let change = self
            .store
            .with_storage(|store| {
                self.ddl_metadata
                    .set_table_partitioning(store, database, table, partition)
            })
            .map_err(|error| DomainError::Ddl(error.to_string()))?;
        self.publish_ddl_metadata_change(change).map(|_| ())
    }

    pub fn ddl_set_table_placement(
        &self,
        database: &str,
        table: &str,
        placement: Option<astersql_meta_model::PolicyRefInfo>,
    ) -> Result<(), DomainError> {
        let change = self
            .store
            .with_storage(|store| {
                self.ddl_metadata
                    .set_table_placement(store, database, table, placement)
            })
            .map_err(|error| DomainError::Ddl(error.to_string()))?;
        self.publish_ddl_metadata_change(change).map(|_| ())
    }

    /// Classic IMPORT INTO 的同步表模式 DDL；按库表 ID 定位目标。
    pub fn ddl_set_table_mode_by_ids(
        &self,
        schema_id: i64,
        table_id: i64,
        mode: astersql_meta_model::TableMode,
    ) -> Result<(), DomainError> {
        let schema = self
            .info_schema()
            .SchemaByID(schema_id)
            .ok_or_else(|| DomainError::Ddl(format!("schema {schema_id} not found")))?;
        let table = self
            .info_schema()
            .TableByID(table_id)
            .ok_or_else(|| DomainError::Ddl(format!("table {table_id} not found")))?;
        let in_schema = self.table_by_name(&schema.name.lower, &table.Meta().name.lower)?;
        if in_schema.ID != table_id {
            return Err(DomainError::Ddl("schema or table ID mismatch".into()));
        }
        let change = self
            .store
            .with_storage(|store| {
                self.ddl_metadata.set_table_mode(
                    store,
                    &schema.name.lower,
                    &table.Meta().name.lower,
                    mode,
                )
            })
            .map_err(|error| DomainError::Ddl(error.to_string()))?;
        self.publish_ddl_metadata_change(change).map(|_| ())
    }

    /// DDL 交换分区后更新统计。
    pub fn ddl_exchange_partition(
        &self,
        database: &str,
        table: &str,
        partition_name: &str,
        exchange_database: &str,
        exchange_table: &str,
    ) -> Result<(), DomainError> {
        let change = self
            .store
            .with_storage(|store| {
                self.ddl_metadata.exchange_partition(
                    store,
                    database,
                    table,
                    partition_name,
                    exchange_database,
                    exchange_table,
                )
            })
            .map_err(|error| DomainError::Ddl(error.to_string()))?;
        self.publish_ddl_metadata_change(change).map(|_| ())
    }

    /// DDL 删除表内对象后更新统计。
    pub fn ddl_drop_table_items(
        &self,
        database: &str,
        table: &str,
        columns: &BTreeSet<String>,
        indexes: &BTreeSet<String>,
    ) -> Result<(), DomainError> {
        let change = self
            .store
            .with_storage(|store| {
                self.ddl_metadata
                    .drop_table_items(store, database, table, columns, indexes)
            })
            .map_err(|error| DomainError::Ddl(error.to_string()))?;
        self.publish_ddl_metadata_change(change).map(|_| ())
    }

    /// DDL replaces the public foreign-key metadata for one table.
    pub fn ddl_replace_foreign_keys(
        &self,
        database: &str,
        table: &str,
        foreign_keys: Vec<astersql_meta_model::FKInfo>,
    ) -> Result<(), DomainError> {
        let change = self
            .store
            .with_storage(|store| {
                self.ddl_metadata
                    .replace_foreign_keys(store, database, table, foreign_keys)
            })
            .map_err(|error| DomainError::Ddl(error.to_string()))?;
        self.publish_ddl_metadata_change(change).map(|_| ())
    }

    /// `ALTER TABLE ... ADD COLUMN`: publishes the metadata and then runs Go's
    /// `statistics/handle/ddl` add-column handler, which synthesizes the
    /// statistics of the new columns out of their origin default value.
    pub fn ddl_add_columns(
        &self,
        database: &str,
        table: &str,
        columns: Vec<astersql_meta_model::ColumnInfo>,
    ) -> Result<astersql_meta_model::TableInfo, DomainError> {
        let change = self
            .store
            .with_storage(|store| {
                self.ddl_metadata
                    .add_columns(store, database, table, columns.clone())
            })
            .map_err(|error| DomainError::Ddl(error.to_string()))?;
        let change = self.publish_ddl_metadata_change(change)?;
        let new_table = change
            .new_tables
            .into_iter()
            .next()
            .map(|(_, table)| table)
            .ok_or_else(|| {
                DomainError::Ddl("ALTER TABLE ADD COLUMN produced no metadata".to_owned())
            })?;
        let added_names = columns
            .iter()
            .map(|column| column.Name.L.clone())
            .collect::<BTreeSet<_>>();
        let added = new_table
            .Columns
            .iter()
            .filter(|column| added_names.contains(&column.Name.L))
            .cloned()
            .collect::<Vec<_>>();
        self.insert_col_stats_2_kv(&new_table, &added)?;
        Ok(new_table)
    }

    /// `ALTER TABLE ... MODIFY COLUMN`: replace the public column metadata
    /// while preserving its stable ID/offset and publish the schema version.
    pub fn ddl_modify_column(
        &self,
        database: &str,
        table: &str,
        column: astersql_meta_model::ColumnInfo,
        auto_random: Option<(u64, u64)>,
    ) -> Result<(), DomainError> {
        let change = self
            .store
            .with_storage(|store| {
                self.ddl_metadata
                    .modify_column(store, database, table, column, auto_random)
            })
            .map_err(|error| DomainError::Ddl(error.to_string()))?;
        self.publish_ddl_metadata_change(change).map(|_| ())
    }

    /// `ALTER TABLE ... RENAME INDEX`: publish the renamed index metadata.
    pub fn ddl_rename_index(
        &self,
        database: &str,
        table: &str,
        from: &str,
        to: &str,
    ) -> Result<(), DomainError> {
        let change = self
            .store
            .with_storage(|store| {
                self.ddl_metadata
                    .rename_index(store, database, table, from, to)
            })
            .map_err(|error| DomainError::Ddl(error.to_string()))?;
        self.publish_ddl_metadata_change(change).map(|_| ())
    }

    /// `ALTER TABLE ... ALTER INDEX ... VISIBLE/INVISIBLE`: publish metadata.
    pub fn ddl_set_index_visibility(
        &self,
        database: &str,
        table: &str,
        index_name: &str,
        invisible: bool,
    ) -> Result<(), DomainError> {
        let change = self
            .store
            .with_storage(|store| {
                self.ddl_metadata
                    .set_index_visibility(store, database, table, index_name, invisible)
            })
            .map_err(|error| DomainError::Ddl(error.to_string()))?;
        self.publish_ddl_metadata_change(change).map(|_| ())
    }

    /// `ALTER TABLE ... CHANGE COLUMN`: publish the replacement and repair
    /// index-column references in the same metadata transaction.
    pub fn ddl_change_column(
        &self,
        database: &str,
        table: &str,
        old_name: &str,
        column: astersql_meta_model::ColumnInfo,
    ) -> Result<(), DomainError> {
        let change = self
            .store
            .with_storage(|store| {
                self.ddl_metadata
                    .change_column(store, database, table, old_name, column)
            })
            .map_err(|error| DomainError::Ddl(error.to_string()))?;
        self.publish_ddl_metadata_change(change).map(|_| ())
    }

    /// Go `storage::InsertColStats2KV`.
    ///
    /// The added columns have no analyzed payload, but every existing row now
    /// carries the origin default value, so the DDL handler writes a single
    /// bucket holding that value (or the NULL count when the default is NULL).
    fn insert_col_stats_2_kv(
        &self,
        table: &astersql_meta_model::TableInfo,
        columns: &[astersql_meta_model::ColumnInfo],
    ) -> Result<(), DomainError> {
        if columns.is_empty() {
            return Ok(());
        }
        let counts = self
            .stats_store
            .persisted_meta_rows()
            .map_err(DomainError::Stats)?
            .into_iter()
            .map(|row| (row.0, row.2))
            .collect::<BTreeMap<_, _>>();
        for physical_id in Self::physical_ids(table) {
            // Go returns early when `mysql.stats_meta` has no row for the
            // physical table, which means the table has no statistics at all.
            let Some(count) = counts.get(&physical_id).copied() else {
                continue;
            };
            let version = stats_storage::gc::current_ts();
            let mut updated = false;
            {
                let mut handle = self
                    .stats_handle
                    .lock()
                    .expect("domain stats handle lock poisoned");
                let Some(stats) = handle.cache_mut().get_mut(physical_id) else {
                    continue;
                };
                for column in columns {
                    let entry = stats.columns.entry(column.ID).or_default();
                    // `insert ignore`: ANALYZE may already own this histogram.
                    if entry.analyzed_or_synthesized {
                        continue;
                    }
                    match origin_default_value_bytes(column) {
                        None => {
                            entry.ndv = 0;
                            entry.null_count = count;
                        }
                        Some(value) => {
                            entry.ndv = 1;
                            entry.total_column_size = value.len() as i64 * count;
                            entry.buckets = Vec::from([StatsBucket {
                                count,
                                repeats: count,
                                lower: value.clone(),
                                upper: value,
                                ndv: 1,
                            }]);
                        }
                    }
                    entry.analyzed_or_synthesized = true;
                    // Go leaves `stats_ver` at 0 for a synthesized histogram
                    // but the payload is complete, so it counts as full load.
                    entry.loaded_or_evicted = true;
                    entry.field_type = column.GetType();
                    entry.version = version;
                    updated = true;
                }
                if updated {
                    stats.version = version;
                    stats.last_stats_hist_version = version;
                }
            }
            if updated {
                self.persist_stats_meta(&[physical_id])?;
            }
        }
        Ok(())
    }

    /// DDL 加索引后更新统计。
    pub fn ddl_add_index(
        &self,
        database: &str,
        table: &str,
        index: astersql_meta_model::IndexInfo,
    ) -> Result<astersql_meta_model::TableInfo, DomainError> {
        let change = self
            .store
            .with_storage(|store| self.ddl_metadata.add_index(store, database, table, index))
            .map_err(|error| DomainError::Ddl(error.to_string()))?;
        let change = self.publish_ddl_metadata_change(change)?;
        change
            .new_tables
            .into_iter()
            .next()
            .map(|(_, table)| table)
            .ok_or_else(|| {
                DomainError::Ddl("ALTER TABLE ADD INDEX produced no metadata".to_owned())
            })
    }

    /// Persist an ADD INDEX job before the owner starts physical backfill.
    ///
    /// Re-staging the same job is idempotent, which mirrors a replacement
    /// owner discovering an existing DDL job in the system table.
    pub fn stage_pending_add_index(
        &self,
        database: &str,
        table: &str,
        index: astersql_meta_model::IndexInfo,
        partition_count: usize,
    ) -> Result<(), DomainError> {
        let key = pending_add_index_key(database, table, &index.Name.L);
        let job = PendingAddIndexJob {
            database: database.to_ascii_lowercase(),
            table: table.to_ascii_lowercase(),
            index,
            partition_count: partition_count.max(1),
            next_partition: 0,
        };
        self.store
            .with_storage(|store| -> Result<(), astersql_kv::errors::SharedError> {
                let mut transaction = store.Begin(&[])?;
                match transaction.Get(&astersql_kv::Context::default(), key.clone(), &[]) {
                    Ok(_) => transaction.Rollback(),
                    Err(error) if astersql_kv::IsErrNotFound(&error) => {
                        transaction.Set(key, encode_pending_add_index(&job)?)?;
                        transaction.Commit(&astersql_kv::Context::default())
                    }
                    Err(error) => Err(error),
                }
            })
            .map_err(|error| DomainError::Ddl(error.to_string()))
    }

    /// Acquire this Domain's single DDL-owner lease for pending ADD INDEX work.
    pub fn pending_add_index_owner_guard(&self) -> std::sync::MutexGuard<'_, ()> {
        self.pending_add_index_owner_lock
            .lock()
            // A failover test deliberately unwinds the former owner while it
            // holds this process-local lease. The durable job cursor lives in
            // KV, so a replacement owner must recover the lease and resume
            // from that cursor instead of treating Rust mutex poisoning as
            // persistent DDL corruption.
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Snapshot pending ADD INDEX jobs for a SQL owner to resume.
    pub fn pending_add_index_jobs(&self) -> Result<Vec<PendingAddIndexJob>, DomainError> {
        self.store
            .with_storage(
                |store| -> Result<Vec<PendingAddIndexJob>, astersql_kv::errors::SharedError> {
                    // MaxVersion is the storage contract for a latest snapshot
                    // and does not allocate a fresh oracle timestamp. This
                    // owner-resume check runs before every SQL statement.
                    let snapshot = store.GetSnapshot(astersql_kv::MaxVersion);
                    let start = astersql_kv::Key(PENDING_ADD_INDEX_PREFIX.to_vec());
                    let mut iterator = snapshot.Iter(start.clone(), Some(start.PrefixNext()))?;
                    let mut jobs = Vec::new();
                    while iterator.Valid() {
                        jobs.push(decode_pending_add_index(&iterator.Value())?);
                        iterator.Next()?;
                    }
                    iterator.Close();
                    Ok(jobs)
                },
            )
            .map_err(|error| DomainError::Ddl(error.to_string()))
    }

    /// Durably advance one physical-partition checkpoint before exposing the
    /// corresponding failpoint callback.
    pub fn advance_pending_add_index(
        &self,
        database: &str,
        table: &str,
        index: &str,
    ) -> Result<usize, DomainError> {
        let key = pending_add_index_key(database, table, index);
        self.store
            .with_storage(|store| -> Result<usize, astersql_kv::errors::SharedError> {
                let mut transaction = store.Begin(&[])?;
                let value = transaction.Get(&astersql_kv::Context::default(), key.clone(), &[])?;
                let mut job = decode_pending_add_index(&value.Value)?;
                if job.next_partition < job.partition_count {
                    job.next_partition += 1;
                }
                transaction.Set(key, encode_pending_add_index(&job)?)?;
                transaction.Commit(&astersql_kv::Context::default())?;
                Ok(job.next_partition)
            })
            .map_err(|error| DomainError::Ddl(error.to_string()))
    }

    /// Remove a published ADD INDEX job from the owner-handoff queue.
    pub fn finish_pending_add_index(
        &self,
        database: &str,
        table: &str,
        index: &str,
    ) -> Result<(), DomainError> {
        let key = pending_add_index_key(database, table, index);
        self.store
            .with_storage(|store| -> Result<(), astersql_kv::errors::SharedError> {
                let mut transaction = store.Begin(&[])?;
                transaction.Delete(key)?;
                transaction.Commit(&astersql_kv::Context::default())
            })
            .map_err(|error| DomainError::Ddl(error.to_string()))
    }

    /// 递增并记录 catalog 版本。
    fn record_stats_catalog_version(&self) -> u64 {
        let version = self.stats_catalog_version.fetch_add(1, Ordering::AcqRel) + 1;
        let catalog = self
            .stats_catalog
            .read()
            .expect("domain stats catalog lock poisoned")
            .clone();
        self.stats_catalog_history
            .write()
            .expect("domain stats catalog history lock poisoned")
            .push((version, catalog));
        self.stats_catalog_timeline
            .write()
            .expect("domain stats catalog timeline lock poisoned")
            .push((SystemTime::now(), version));
        version
    }

    /// 注册表到统计 catalog。
    pub fn register_stats_table(
        &self,
        database: &str,
        mut table: astersql_meta_model::TableInfo,
    ) -> Result<StatsTableKey, DomainError> {
        let database = database.to_lowercase();
        let table_name = table.Name.L.clone();
        let catalog_key = (database.clone(), table_name.clone());
        let mut catalog = self
            .stats_catalog
            .write()
            .expect("domain stats catalog lock poisoned");
        if catalog.contains_key(&catalog_key) {
            return Err(DomainError::Stats(format!(
                "table {database}.{table_name} already exists"
            )));
        }
        if table.ID <= 0 {
            table.ID = self.next_stats_table_id.fetch_add(1, Ordering::AcqRel);
        } else {
            self.next_stats_table_id
                .fetch_max(table.ID.saturating_add(1), Ordering::AcqRel);
        }
        let mut physical_ids = vec![table.ID];
        if let Some(partition) = table.Partition.as_mut() {
            if partition.Definitions.is_empty() && partition.Num > 0 {
                partition.Definitions = (0..partition.Num)
                    .map(|number| astersql_meta_model::PartitionDefinition {
                        Name: astersql_parser_ast::NewCIStr(&format!("p{number}")),
                        ..Default::default()
                    })
                    .collect();
            }
            for definition in &mut partition.Definitions {
                if definition.ID <= 0 || physical_ids.contains(&definition.ID) {
                    definition.ID = self.next_stats_table_id.fetch_add(1, Ordering::AcqRel);
                } else {
                    self.next_stats_table_id
                        .fetch_max(definition.ID.saturating_add(1), Ordering::AcqRel);
                }
                physical_ids.push(definition.ID);
            }
        }
        let key = StatsTableKey::new(&database, &table_name, table.ID);
        self.stats_auto_ids
            .lock()
            .expect("domain stats auto ID lock poisoned")
            .insert(
                table.ID,
                u64::try_from(table.AutoIncID.max(table.AutoIncIDExtra).max(1)).unwrap_or(1),
            );
        {
            let mut handle = self
                .stats_handle
                .lock()
                .expect("domain stats handle lock poisoned");
            for physical_id in &physical_ids {
                handle
                    .register_table_stats(*physical_id)
                    .map_err(|error| DomainError::Stats(error.to_string()))?;
                let stats = handle
                    .cache_mut()
                    .get_mut(*physical_id)
                    .expect("newly registered statistics table");
                for column in table.Columns.iter().filter(|column| !column.Hidden) {
                    stats.columns.entry(column.ID).or_default();
                }
            }
        }
        for physical_id in physical_ids {
            self.stats_store
                .persist_meta(physical_id)
                .map_err(DomainError::Stats)?;
        }
        catalog.insert(catalog_key, (key.clone(), table));
        drop(catalog);
        self.record_stats_catalog_version();
        Ok(key)
    }

    /// Registers the statistics of a temporary table on demand.
    ///
    /// Go keeps temporary tables out of the statistics cache until ANALYZE
    /// writes their `mysql.stats_meta` row; this is that write.
    pub fn register_temporary_stats_table(
        &self,
        table: &astersql_meta_model::TableInfo,
    ) -> Result<(), DomainError> {
        if table.TempTableType == astersql_meta_model::TempTableNone {
            return Ok(());
        }
        let missing = {
            let handle = self
                .stats_handle
                .lock()
                .expect("domain stats handle lock poisoned");
            Self::physical_ids(table)
                .into_iter()
                .filter(|physical_id| handle.stats_meta(*physical_id).is_none())
                .collect::<Vec<_>>()
        };
        {
            let mut handle = self
                .stats_handle
                .lock()
                .expect("domain stats handle lock poisoned");
            for physical_id in &missing {
                handle
                    .register_table_stats(*physical_id)
                    .map_err(|error| DomainError::Stats(error.to_string()))?;
            }
        }
        for physical_id in &missing {
            self.stats_store
                .persist_meta(*physical_id)
                .map_err(DomainError::Stats)?;
        }
        Ok(())
    }

    /// 从统计 catalog 删除表。
    pub fn drop_stats_tables(&self, database: &str, tables: &[String]) -> Result<(), DomainError> {
        let database = database.to_lowercase();
        let mut catalog = self
            .stats_catalog
            .write()
            .expect("domain stats catalog lock poisoned");
        let mut dropped = self
            .dropped_stats_ids
            .lock()
            .expect("dropped stats IDs lock poisoned");
        for table in tables {
            if let Some((key, info)) = catalog.remove(&(database.clone(), table.to_lowercase())) {
                dropped.insert(key.table_id);
                self.stats_auto_ids
                    .lock()
                    .expect("domain stats auto ID lock poisoned")
                    .remove(&key.table_id);
                self.stats_secondary_auto_ids
                    .lock()
                    .expect("domain secondary auto ID lock poisoned")
                    .retain(|(table_id, _), _| *table_id != key.table_id);
                if let Some(partition) = info.GetPartitionInfo() {
                    dropped.extend(partition.Definitions.iter().map(|definition| definition.ID));
                }
            }
        }
        drop(dropped);
        drop(catalog);
        self.record_stats_catalog_version();
        Ok(())
    }

    /// 替换分区统计映射。
    pub fn replace_stats_partitions(
        &self,
        database: &str,
        table: &str,
        removed_names: &BTreeSet<String>,
        mut added: Vec<astersql_meta_model::PartitionDefinition>,
    ) -> Result<(), DomainError> {
        let catalog_key = (database.to_lowercase(), table.to_lowercase());
        let mut catalog = self
            .stats_catalog
            .write()
            .expect("domain stats catalog lock poisoned");
        let (key, info) = catalog
            .get_mut(&catalog_key)
            .ok_or_else(|| DomainError::Stats(format!("unknown table {database}.{table}")))?;
        let partition = info.Partition.as_mut().ok_or_else(|| {
            DomainError::Stats(format!("table {database}.{table} is not partitioned"))
        })?;
        let removed = partition
            .Definitions
            .iter()
            .filter(|definition| removed_names.contains(&definition.Name.L))
            .map(|definition| definition.ID)
            .collect::<Vec<_>>();
        partition
            .Definitions
            .retain(|definition| !removed_names.contains(&definition.Name.L));
        let mut added_ids = Vec::new();
        for definition in &mut added {
            definition.ID = self.next_stats_table_id.fetch_add(1, Ordering::AcqRel);
            added_ids.push(definition.ID);
        }
        partition.Definitions.extend(added);
        let table_locked = self
            .stats_store
            .locked_ids()
            .map_err(DomainError::Stats)?
            .contains(&key.table_id);
        let mut handle = self
            .stats_handle
            .lock()
            .expect("domain stats handle lock poisoned");
        for physical_id in &added_ids {
            handle
                .register_table_stats(*physical_id)
                .map_err(|error| DomainError::Stats(error.to_string()))?;
        }
        drop(handle);
        for physical_id in &added_ids {
            self.stats_store
                .persist_meta(*physical_id)
                .map_err(DomainError::Stats)?;
            if table_locked {
                let mut insert_lock = |executor: &mut dyn lockstats::RestrictedSQLExecutor| {
                    lockstats::InsertLockAndUpdateVersion(executor, *physical_id)
                };
                lockstats::StatsSession::WithSession(
                    self.stats_store.as_ref(),
                    true,
                    &mut insert_lock,
                )
                .map_err(|error| DomainError::Stats(error.to_string()))?;
            }
        }
        self.dropped_stats_ids
            .lock()
            .expect("dropped stats IDs lock poisoned")
            .extend(removed);
        drop(catalog);
        self.record_stats_catalog_version();
        Ok(())
    }

    /// 交换分区统计。
    pub fn exchange_stats_partition(
        &self,
        database: &str,
        table: &str,
        partition_name: &str,
        exchange_database: &str,
        exchange_table: &str,
    ) -> Result<(), DomainError> {
        let partitioned_key = (database.to_lowercase(), table.to_lowercase());
        let exchange_key = (
            exchange_database.to_lowercase(),
            exchange_table.to_lowercase(),
        );
        if partitioned_key == exchange_key {
            return Err(DomainError::Stats(
                "cannot exchange a partition with its own table".to_owned(),
            ));
        }
        let mut catalog = self
            .stats_catalog
            .write()
            .expect("domain stats catalog lock poisoned");
        let (partitioned_stats_key, mut partitioned_info) = catalog
            .get(&partitioned_key)
            .cloned()
            .ok_or_else(|| DomainError::Stats(format!("unknown table {database}.{table}")))?;
        let (mut exchange_stats_key, mut exchange_info) =
            catalog.get(&exchange_key).cloned().ok_or_else(|| {
                DomainError::Stats(format!(
                    "unknown table {exchange_database}.{exchange_table}"
                ))
            })?;
        if exchange_info.GetPartitionInfo().is_some() {
            return Err(DomainError::Stats(format!(
                "exchange table {exchange_database}.{exchange_table} must not be partitioned"
            )));
        }
        let partition = partitioned_info.Partition.as_mut().ok_or_else(|| {
            DomainError::Stats(format!("table {database}.{table} is not partitioned"))
        })?;
        let definition = partition
            .Definitions
            .iter_mut()
            .find(|definition| definition.Name.L.eq_ignore_ascii_case(partition_name))
            .ok_or_else(|| {
                DomainError::Stats(format!(
                    "unknown partition {partition_name} of {database}.{table}"
                ))
            })?;
        let old_partition_id = definition.ID;
        let old_exchange_id = exchange_stats_key.table_id;
        definition.ID = old_exchange_id;
        exchange_stats_key.table_id = old_partition_id;
        exchange_info.ID = old_partition_id;

        catalog.insert(partitioned_key, (partitioned_stats_key, partitioned_info));
        catalog.insert(exchange_key, (exchange_stats_key, exchange_info));
        drop(catalog);

        let mut auto_ids = self
            .stats_auto_ids
            .lock()
            .expect("domain stats auto ID lock poisoned");
        if let Some(next_id) = auto_ids.remove(&old_exchange_id) {
            auto_ids.insert(old_partition_id, next_id);
        }
        drop(auto_ids);
        self.record_stats_catalog_version();
        Ok(())
    }

    /// 开关历史统计。
    pub fn set_historical_stats_enabled(&self, enabled: bool) {
        self.stats_handle
            .lock()
            .expect("domain stats handle lock poisoned")
            .set_historical_enabled(enabled);
    }

    /// 按物理 ID 取表统计。
    pub fn stats_table(
        &self,
        database: &str,
        table: &str,
    ) -> Option<(StatsTableKey, astersql_meta_model::TableInfo)> {
        let table = self.table_by_name(database, table).ok()?;
        Some((
            StatsTableKey::new(database, &table.Name.L, table.ID),
            table.as_ref().clone(),
        ))
    }

    /// 分区对应的全局统计 ID。
    fn global_stats_id_for_partition(&self, physical_id: i64) -> Option<i64> {
        self.stats_catalog
            .read()
            .expect("domain stats catalog lock poisoned")
            .values()
            .find_map(|(key, table)| {
                table
                    .GetPartitionInfo()
                    .is_some_and(|partition| {
                        partition
                            .Definitions
                            .iter()
                            .any(|definition| definition.ID == physical_id)
                    })
                    .then_some(key.table_id)
            })
    }

    /// 分配统计自增 ID。
    pub fn allocate_stats_auto_id(
        &self,
        table_id: i64,
        explicit: Option<u64>,
    ) -> Result<(u64, bool), DomainError> {
        self.allocate_stats_auto_id_kind(table_id, explicit, 0)
    }

    /// Allocate an independent AUTO_RANDOM or hidden-row-id stream.
    ///
    /// `kind=0` is the table AUTO_INCREMENT stream, `kind=1` AUTO_RANDOM, and
    /// `kind=2` the hidden row handle. TiDB keeps these allocators independent.
    pub fn allocate_stats_auto_id_kind(
        &self,
        table_id: i64,
        explicit: Option<u64>,
        kind: u8,
    ) -> Result<(u64, bool), DomainError> {
        let allocator = self
            .stats_auto_id_allocators
            .lock()
            .expect("domain AutoID allocator lock poisoned")
            .get(&(table_id, kind))
            .cloned()
            .ok_or_else(|| {
                DomainError::Stats(format!(
                    "AutoID allocator kind {kind} not found for table ID {table_id}"
                ))
            })?;

        if let Some(value) = explicit {
            let rebased = if kind == 0 {
                let next = self
                    .stats_auto_ids
                    .lock()
                    .expect("domain stats auto ID lock poisoned")
                    .get(&table_id)
                    .copied()
                    .unwrap_or(1);
                value > next
            } else {
                let next = self
                    .stats_secondary_auto_ids
                    .lock()
                    .expect("domain secondary auto ID lock poisoned")
                    .get(&(table_id, kind))
                    .copied()
                    .unwrap_or(1);
                value > next
            };
            allocator
                .rebase(&AutoIdContext::background(), value as i64, false)
                .map_err(|error| DomainError::Store(error.to_string()))?;
            self.record_stats_auto_id_next(table_id, kind, value.saturating_add(1));
            return Ok((value, rebased));
        }

        let (_, maximum) = allocator
            .alloc(&AutoIdContext::background(), 1, 1, 1)
            .map_err(|error| DomainError::Store(error.to_string()))?;
        let value = maximum as u64;
        let next = value
            .checked_add(1)
            .ok_or_else(|| DomainError::Stats("auto-increment allocator overflow".to_owned()))?;
        self.record_stats_auto_id_next(table_id, kind, next);
        Ok((value, false))
    }

    /// Allocate from one stream using MySQL AUTO_INCREMENT increment/offset.
    pub fn allocate_stats_auto_id_with_increment(
        &self,
        table_id: i64,
        explicit: Option<u64>,
        kind: u8,
        increment: u64,
        offset: u64,
    ) -> Result<(u64, bool), DomainError> {
        if explicit.is_some() {
            return self.allocate_stats_auto_id_kind(table_id, explicit, kind);
        }
        let increment = increment.max(1);
        let offset = if offset == 0 || offset > increment {
            1
        } else {
            offset
        };
        let increment = i64::try_from(increment).map_err(|_| {
            DomainError::Stats("auto-increment increment is out of range".to_owned())
        })?;
        let offset = i64::try_from(offset)
            .map_err(|_| DomainError::Stats("auto-increment offset is out of range".to_owned()))?;
        let allocator = self
            .stats_auto_id_allocators
            .lock()
            .expect("domain AutoID allocator lock poisoned")
            .get(&(table_id, kind))
            .cloned()
            .ok_or_else(|| {
                DomainError::Stats(format!(
                    "AutoID allocator kind {kind} not found for table ID {table_id}"
                ))
            })?;
        let (_, maximum) = allocator
            .alloc(&AutoIdContext::background(), 1, increment, offset)
            .map_err(|error| DomainError::Store(error.to_string()))?;
        let value = maximum as u64;
        let next = value
            .checked_add(1)
            .ok_or_else(|| DomainError::Stats("auto-increment allocator overflow".to_owned()))?;
        self.record_stats_auto_id_next(table_id, kind, next);
        Ok((value, false))
    }

    fn record_stats_auto_id_next(&self, table_id: i64, kind: u8, next: u64) {
        if kind == 0 {
            let mut auto_ids = self
                .stats_auto_ids
                .lock()
                .expect("domain stats auto ID lock poisoned");
            auto_ids
                .entry(table_id)
                .and_modify(|current| *current = (*current).max(next))
                .or_insert(next);
        } else {
            let mut auto_ids = self
                .stats_secondary_auto_ids
                .lock()
                .expect("domain secondary auto ID lock poisoned");
            auto_ids
                .entry((table_id, kind))
                .and_modify(|current| *current = (*current).max(next))
                .or_insert(next);
        }
    }

    /// Return the persisted high-water base for SHOW CREATE TABLE.
    pub fn stats_auto_id_base(&self, table_id: i64, kind: u8) -> Option<u64> {
        let local = if kind == 0 {
            self.stats_auto_ids
                .lock()
                .expect("domain stats auto ID lock poisoned")
                .get(&table_id)
                .copied()
        } else {
            self.stats_secondary_auto_ids
                .lock()
                .expect("domain secondary auto ID lock poisoned")
                .get(&(table_id, kind))
                .copied()
        };
        let global = self
            .stats_auto_id_allocators
            .lock()
            .expect("domain AutoID allocator lock poisoned")
            .get(&(table_id, kind))
            .cloned()
            .and_then(|allocator| allocator.next_global_auto_id().ok())
            .map(|value| value as u64);
        match (local, global) {
            (Some(local), Some(global)) => Some(local.max(global)),
            (Some(local), None) => Some(local),
            (None, Some(global)) => Some(global),
            (None, None) => None,
        }
    }

    /// Restore an allocator high-water mark for a test that writes a row
    /// directly, bypassing the allocator just like Go `Table.AddRecord`.
    pub fn restore_stats_auto_id_base_for_test(&self, table_id: i64, kind: u8, next_id: u64) {
        if kind == 0 {
            self.stats_auto_ids
                .lock()
                .expect("domain stats auto ID lock poisoned")
                .insert(table_id, next_id);
        } else {
            self.stats_secondary_auto_ids
                .lock()
                .expect("domain secondary auto ID lock poisoned")
                .insert((table_id, kind), next_id);
        }
    }

    /// 记录行变更引起的统计增量。
    pub fn record_stats_mutation(
        &self,
        table: &StatsTableKey,
        row_delta: i64,
        modified_rows: i64,
    ) -> Result<(), DomainError> {
        if modified_rows < 0 {
            return Err(DomainError::Stats(
                "modified row count must not be negative".to_owned(),
            ));
        }
        if self.fail_next_stats_record.swap(false, Ordering::AcqRel) {
            return Err(DomainError::Stats(
                "injected statistics recording failure".to_owned(),
            ));
        }
        // Go does not cache or update statistics for mysql/sys/memory schemas.
        // The metadata reconciler deliberately excludes these tables, so their
        // ordinary DML must not be rejected for lacking a stats_meta entry.
        if Self::is_mem_or_sys_db(&table.database) {
            return Ok(());
        }
        if self
            .stats_handle
            .lock()
            .expect("domain stats handle lock poisoned")
            .stats_meta(table.table_id)
            .is_none()
        {
            // Temporary tables have no statistics entry until they are
            // explicitly analyzed, so their DML never produces a delta.
            if self.is_temporary_stats_table(table.table_id) {
                return Ok(());
            }
            return Err(DomainError::Stats(format!(
                "unknown statistics table {}",
                table.table_id
            )));
        }
        self.enqueue_stats_mutations(std::iter::once((table.table_id, row_delta, modified_rows)));
        Ok(())
    }

    /// True when the physical table belongs to a local or global temporary
    /// table, which Go keeps out of the statistics cache.
    fn is_temporary_stats_table(&self, physical_id: i64) -> bool {
        self.stats_catalog
            .read()
            .expect("domain stats catalog lock poisoned")
            .values()
            .any(|(_, table)| {
                table.TempTableType != astersql_meta_model::TempTableNone
                    && Self::physical_ids(table).contains(&physical_id)
            })
    }

    /// 批量入队统计增量。
    pub fn enqueue_stats_mutations(&self, deltas: impl IntoIterator<Item = (i64, i64, i64)>) {
        let mut pending = self
            .pending_stats_deltas
            .lock()
            .expect("domain pending stats delta lock poisoned");
        for (table_id, row_delta, modified_rows) in deltas {
            let delta = pending.entry(table_id).or_default();
            delta.0 = delta.0.saturating_add(row_delta);
            delta.1 = delta.1.saturating_add(modified_rows.max(0));
        }
    }

    /// 从待刷增量中扣除已刷部分。
    fn subtract_pending_stats_deltas(&self, flushed: &[(i64, (i64, i64))]) {
        let mut pending = self
            .pending_stats_deltas
            .lock()
            .expect("domain pending stats delta lock poisoned");
        for (physical_id, delta) in flushed {
            let Some(current) = pending.get_mut(physical_id) else {
                continue;
            };
            current.0 = current.0.saturating_sub(delta.0);
            current.1 = current.1.saturating_sub(delta.1);
            if *current == (0, 0) {
                pending.remove(physical_id);
            }
        }
    }

    /// 提交挂起的统计刷盘批次。
    fn commit_pending_stats_flush(&self) -> Result<Option<u64>, DomainError> {
        let batch = self
            .pending_stats_flush
            .lock()
            .expect("domain pending stats flush lock poisoned")
            .clone();
        let Some(batch) = batch else {
            return Ok(None);
        };
        self.stats_store
            .apply_stats_flush_batch(&batch.locked_deltas, &batch.persisted_stats)
            .map_err(DomainError::Stats)?;
        self.subtract_pending_stats_deltas(&batch.source_deltas);
        *self
            .pending_stats_flush
            .lock()
            .expect("domain pending stats flush lock poisoned") = None;
        Ok(batch.version)
    }

    /// Flushes only this Domain's local stats collector. Transaction-local
    /// deltas are withheld until commit by the Session, then enter this pending
    /// map and are drained into the one canonical Handle at this boundary.
    pub fn preflush_stats_delta(&self, physical_ids: &[i64]) -> Result<(), DomainError> {
        let _flush = self
            .stats_flush_lock
            .lock()
            .expect("domain statistics flush lock poisoned");
        self.commit_pending_stats_flush()?;
        let locked = self.stats_store.locked_ids().map_err(DomainError::Stats)?;
        let mut handle = self
            .stats_handle
            .lock()
            .expect("domain stats handle lock poisoned");
        for physical_id in physical_ids {
            if handle.stats_meta(*physical_id).is_none() {
                return Err(DomainError::Stats(format!(
                    "cannot preflush unknown statistics table {physical_id}"
                )));
            }
        }
        handle
            .preflush_stats_delta()
            .map_err(|error| DomainError::Stats(error.to_string()))?;
        drop(handle);
        let target_ids = physical_ids.iter().copied().collect::<BTreeSet<_>>();
        let deltas = {
            let pending = self
                .pending_stats_deltas
                .lock()
                .expect("domain pending stats delta lock poisoned");
            target_ids
                .iter()
                .filter_map(|physical_id| {
                    pending
                        .get(physical_id)
                        .copied()
                        .map(|delta| (*physical_id, delta))
                })
                .collect::<Vec<_>>()
        };
        let source_deltas = deltas.clone();
        let (locked_deltas, unlocked_deltas): (Vec<_>, Vec<_>) = deltas
            .into_iter()
            .partition(|(physical_id, _)| locked.contains(physical_id));
        self.stats_store
            .apply_stats_flush_batch(&locked_deltas, &[])
            .map_err(DomainError::Stats)?;
        let mut handle = self
            .stats_handle
            .lock()
            .expect("domain stats handle lock poisoned");
        for (physical_id, (row_delta, modified_rows)) in unlocked_deltas {
            handle
                .record_table_mutation(physical_id, row_delta, modified_rows)
                .map_err(|error| DomainError::Stats(error.to_string()))?;
        }
        drop(handle);
        self.subtract_pending_stats_deltas(&source_deltas);
        *self
            .last_stats_preflush_ids
            .lock()
            .expect("domain last preflush ID lock poisoned") = target_ids.into_iter().collect();
        self.stats_preflush_count.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }

    /// 提交统计增量历史（两阶段刷盘后半）。
    pub fn flush_stats_delta_history(
        &self,
        physical_ids: &[i64],
    ) -> Result<Option<u64>, DomainError> {
        let _flush = self
            .stats_flush_lock
            .lock()
            .expect("domain statistics flush lock poisoned");
        let retried_version = self.commit_pending_stats_flush()?;
        let locked = self.stats_store.locked_ids().map_err(DomainError::Stats)?;
        let mut handle = self
            .stats_handle
            .lock()
            .expect("domain stats handle lock poisoned");
        // Skip physical ids that no longer have handle meta (Go needDumpStatsDelta
        // returns false when the table item is missing).
        let physical_ids = physical_ids
            .iter()
            .copied()
            .filter(|physical_id| handle.stats_meta(*physical_id).is_some())
            .collect::<Vec<_>>();
        if physical_ids.is_empty() {
            drop(handle);
            return Ok(retried_version);
        }
        handle
            .preflush_stats_delta()
            .map_err(|error| DomainError::Stats(error.to_string()))?;
        let target_ids = physical_ids.iter().copied().collect::<BTreeSet<_>>();
        let deltas = {
            let pending = self
                .pending_stats_deltas
                .lock()
                .expect("domain pending stats delta lock poisoned");
            target_ids
                .iter()
                .filter_map(|physical_id| {
                    pending
                        .get(physical_id)
                        .copied()
                        .map(|delta| (*physical_id, delta))
                })
                .collect::<Vec<_>>()
        };
        let original_delta_ids = deltas.clone();
        let (locked_deltas, mut unlocked_deltas): (Vec<_>, Vec<_>) = deltas
            .into_iter()
            .partition(|(physical_id, _)| locked.contains(physical_id));
        let mut global_deltas = BTreeMap::<i64, (i64, i64)>::new();
        for (physical_id, (row_delta, modified_rows)) in &unlocked_deltas {
            if let Some(global_id) = self.global_stats_id_for_partition(*physical_id) {
                let delta = global_deltas.entry(global_id).or_default();
                delta.0 = delta.0.saturating_add(*row_delta);
                delta.1 = delta.1.saturating_add(*modified_rows);
            }
        }
        unlocked_deltas.extend(global_deltas);
        let version = if unlocked_deltas.is_empty() {
            None
        } else {
            Some(
                handle
                    .flush_runtime_stats_deltas(&unlocked_deltas, "flush stats")
                    .map_err(|error| DomainError::Stats(error.to_string()))?,
            )
        };
        let persisted_stats = unlocked_deltas
            .iter()
            .filter_map(|(physical_id, _)| handle.stats_meta(*physical_id).cloned())
            .collect::<Vec<_>>();
        drop(handle);
        if !original_delta_ids.is_empty() {
            *self
                .pending_stats_flush
                .lock()
                .expect("domain pending stats flush lock poisoned") = Some(PendingStatsFlush {
                source_deltas: original_delta_ids,
                locked_deltas,
                persisted_stats,
                version,
            });
            self.commit_pending_stats_flush()?;
        }
        *self
            .last_stats_preflush_ids
            .lock()
            .expect("domain last preflush ID lock poisoned") = target_ids.into_iter().collect();
        self.stats_preflush_count.fetch_add(1, Ordering::AcqRel);
        Ok(version.or(retried_version))
    }

    pub fn inject_stats_preflush_error_for_test(&self, message: impl Into<String>) {
        self.stats_handle
            .lock()
            .expect("domain stats handle lock poisoned")
            .backend()
            .inject_preflush_error(message.into());
    }

    pub fn fail_next_stats_record_for_test(&self) {
        self.fail_next_stats_record.store(true, Ordering::Release);
    }

    pub fn fail_stats_batch_after_for_test(&self, operations: usize) {
        self.stats_store.fail_stats_batch_after_for_test(operations);
    }

    /// 对指定表执行 Analyze 并返回版本。
    pub fn analyze_stats_table(&self, table: &StatsTableKey) -> Result<u64, DomainError> {
        let (version, historical_enabled, profile) = {
            let mut handle = self
                .stats_handle
                .lock()
                .expect("domain stats handle lock poisoned");
            let version = handle
                .analyze_table_stats(table.table_id)
                .map_err(|error| DomainError::Stats(error.to_string()))?;
            let profile = handle.stats_meta(table.table_id).cloned().ok_or_else(|| {
                DomainError::Stats(format!(
                    "analyze did not produce statistics for table {}",
                    table.table_id
                ))
            })?;
            (version, handle.historical_enabled(), profile)
        };
        self.stats_store
            .persist_meta(table.table_id)
            .map_err(DomainError::Stats)?;
        self.persisted_histograms
            .lock()
            .expect("persisted histogram lock poisoned")
            .insert(table.table_id, profile);
        if !historical_enabled {
            return Ok(version);
        }
        if !self
            .historical_stats_worker
            .send_table_to_dump_historical_stats(table.table_id)
        {
            return Err(DomainError::Stats(format!(
                "historical stats queue is full for table {}",
                table.table_id
            )));
        }
        let queued = self
            .historical_stats_worker
            .get_one_historical_stats_table()
            .ok_or_else(|| DomainError::Stats("historical stats task disappeared".to_owned()))?;
        self.historical_stats_worker
            .dump_historical_stats(queued)
            .map_err(DomainError::Stats)?;
        self.stats_context()
            .record_historical_stats_to_storage(queued)
            .map_err(DomainError::Stats)?;
        Ok(version)
    }

    /// 列出统计元数据行。
    pub fn stats_meta_rows(&self) -> Vec<StatsMetaRow> {
        let catalog = self
            .stats_catalog
            .read()
            .expect("domain stats catalog lock poisoned");
        let handle = self
            .stats_handle
            .lock()
            .expect("domain stats handle lock poisoned");
        let pending = self
            .pending_stats_deltas
            .lock()
            .expect("domain pending stats delta lock poisoned");
        catalog
            .values()
            .filter_map(|(key, _)| {
                handle.stats_meta(key.table_id).map(|stats| {
                    let delta = pending.get(&key.table_id).copied().unwrap_or_default();
                    StatsMetaRow {
                        database: key.database.clone(),
                        table: key.table.clone(),
                        table_id: key.table_id,
                        version: stats.version,
                        modify_count: stats.modify_count.saturating_add(delta.1),
                        row_count: stats.realtime_count.saturating_add(delta.0).max(0),
                    }
                })
            })
            .collect()
    }

    /// 按时间戳取 InfoSchema 快照（MVCC 读）。
    pub fn snapshot_info_schema(&self, timestamp: u64) -> Result<SchemaRef, DomainError> {
        if let Some(schema) = self.info_cache.GetBySnapshotTS(timestamp) {
            return Ok(schema);
        }
        self.load_snapshot_info_schema(&self.config.keyspace, timestamp)
            .map(|loaded| loaded.schema)
    }

    /// 读取计划缓存过期时间戳。
    pub fn expired_plan_cache_timestamp(&self) -> Option<SystemTime> {
        *self
            .expired_plan_cache_ts
            .read()
            .expect("expired timestamp lock poisoned")
    }
    /// 设置计划缓存过期时间戳。
    pub fn set_expired_plan_cache_timestamp(&self, timestamp: SystemTime) {
        *self
            .expired_plan_cache_ts
            .write()
            .expect("expired timestamp lock poisoned") = Some(timestamp);
    }
    /// The serving connection manager is weakly held so Domain and Server
    /// cannot keep each other alive after shutdown.
    pub fn set_schema_coordinator(
        &self,
        coordinator: std::sync::Weak<dyn astersql_session_sessmgr::InfoSchemaCoordinator>,
    ) {
        *self
            .schema_coordinator
            .write()
            .expect("schema coordinator lock poisoned") = Some(coordinator);
    }
    pub fn schema_coordinator(
        &self,
    ) -> Option<Arc<dyn astersql_session_sessmgr::InfoSchemaCoordinator>> {
        self.schema_coordinator
            .read()
            .expect("schema coordinator lock poisoned")
            .as_ref()
            .and_then(std::sync::Weak::upgrade)
    }

    /// 注入 DDL 服务实现。
    pub fn set_ddl(&self, ddl: Arc<dyn DdlService>) {
        *self.ddl.write().expect("ddl lock poisoned") = Some(ddl);
    }
    /// 获取 DDL 服务。
    pub fn ddl(&self) -> Option<Arc<dyn DdlService>> {
        self.ddl.read().expect("ddl lock poisoned").clone()
    }

    /// Register this serving Domain with the cluster's server-info and
    /// topology keys. The caller supplies its connected, namespaced etcd
    /// client and the construction options used by Go's InfoSyncer.
    pub fn install_server_info_syncer(
        &self,
        id: String,
        client: Arc<dyn astersql_domain_serverinfo::EtcdClient>,
        options: &[astersql_domain_serverinfo::SyncerOption],
    ) -> Result<(), DomainError> {
        let server_id = Arc::clone(&self.server_id);
        let getter = Arc::new(move || {
            server_id
                .lock()
                .expect("server id lock poisoned")
                .as_ref()
                .filter(|lease| lease.expires_at > Instant::now())
                .map(|lease| lease.id)
                .unwrap_or(0)
        });
        let mut syncer = astersql_domain_serverinfo::NewSyncerWithOptions(
            id,
            getter,
            Some(client),
            Arc::new(astersql_domain_serverinfo::NoopMinStartTSReporter),
            options,
        );
        let context = astersql_domain_serverinfo::Context::Background();
        syncer
            .NewSessionAndStoreServerInfo(context.clone())
            .map_err(|error| DomainError::Worker(format!("register server info: {error}")))?;
        if let Err(error) = syncer.NewTopologySessionAndStoreServerInfo(context) {
            syncer.RemoveServerInfo();
            syncer.RevokeSession();
            syncer.RevokeTopologySession();
            return Err(DomainError::Worker(format!(
                "register topology info: {error}"
            )));
        }
        let mut slot = self
            .server_info_syncer
            .write()
            .expect("server info syncer lock poisoned");
        if let Some(previous) = slot.replace(Arc::new(Mutex::new(syncer))) {
            let previous = previous.lock().expect("server info syncer lock poisoned");
            previous.RemoveServerInfo();
            previous.RemoveTopologyInfo();
            previous.RevokeSession();
            previous.RevokeTopologySession();
        }
        Ok(())
    }

    /// Return the live server-info syncer when this Domain registered in etcd.
    pub fn server_info_syncer(
        &self,
    ) -> Option<Arc<Mutex<Box<astersql_domain_serverinfo::Syncer>>>> {
        self.server_info_syncer
            .read()
            .expect("server info syncer lock poisoned")
            .clone()
    }

    /// Install the cross-keyspace session manager used by this serving Domain.
    /// Closing the Domain drains every target runtime before its own etcd lease.
    pub fn install_cross_ks_manager(&self, manager: Arc<astersql_domain_crossks::Manager>) {
        let previous = self
            .cross_ks_manager
            .write()
            .expect("cross-keyspace manager lock poisoned")
            .replace(manager);
        if let Some(previous) = previous {
            previous.close();
        }
    }

    /// Return the installed cross-keyspace manager.
    pub fn cross_ks_manager(&self) -> Option<Arc<astersql_domain_crossks::Manager>> {
        self.cross_ks_manager
            .read()
            .expect("cross-keyspace manager lock poisoned")
            .clone()
    }

    /// Install the controller used by background workloads.
    pub fn set_external_workload_manager(
        &self,
        manager: Option<Box<dyn astersql_extworkload::Manager>>,
    ) {
        let previous = std::mem::replace(
            &mut *self
                .external_workload_manager
                .write()
                .expect("external workload manager lock poisoned"),
            manager.map(|manager| Arc::new(Mutex::new(manager))),
        );
        if let Some(previous) = previous {
            let _ = previous
                .lock()
                .expect("external workload manager lock poisoned")
                .Close();
        }
    }

    /// Returns the controller if it is installed on this Domain.
    pub fn external_workload_manager(
        &self,
    ) -> Option<Arc<Mutex<Box<dyn astersql_extworkload::Manager>>>> {
        self.external_workload_manager
            .read()
            .expect("external workload manager lock poisoned")
            .clone()
    }

    /// Returns the embedding function shared by sessions on this Domain.
    pub fn get_embed_fn(&self) -> Option<Arc<astersql_inference::EmbedFn>> {
        self.embed_fn
            .read()
            .expect("embedding function lock poisoned")
            .clone()
    }

    fn init_inference_providers(&self) {
        let embed_fn = astersql_inference::EmbedFn::new();
        let key_variables = Arc::clone(&self.global_system_variables);
        let base_variables = Arc::clone(&self.global_system_variables);
        embed_fn
            .register(
                "openai",
                Arc::new(astersql_inference::openai::OpenAIEmbedder::new(
                    move || {
                        key_variables
                            .read()
                            .expect("global system variable lock poisoned")
                            .get("tidb_exp_embed_openai_api_key")
                            .cloned()
                            .unwrap_or_default()
                    },
                    move || {
                        base_variables
                            .read()
                            .expect("global system variable lock poisoned")
                            .get("tidb_exp_embed_openai_api_base")
                            .cloned()
                            .unwrap_or_default()
                    },
                )),
            )
            .expect("register OpenAI embedding provider");
        let jina_key_variables = Arc::clone(&self.global_system_variables);
        embed_fn
            .register(
                "jina_ai",
                Arc::new(astersql_inference::jina::JinaEmbedder::new(
                    move || {
                        jina_key_variables
                            .read()
                            .expect("global system variable lock poisoned")
                            .get("tidb_exp_embed_jina_ai_api_key")
                            .cloned()
                            .unwrap_or_default()
                    },
                    String::new,
                )),
            )
            .expect("register Jina embedding provider");
        let cohere_key_variables = Arc::clone(&self.global_system_variables);
        embed_fn
            .register(
                "cohere",
                Arc::new(astersql_inference::cohere::CohereEmbedder::new(
                    move || {
                        cohere_key_variables
                            .read()
                            .expect("global system variable lock poisoned")
                            .get("tidb_exp_embed_cohere_api_key")
                            .cloned()
                            .unwrap_or_default()
                    },
                    String::new,
                )),
            )
            .expect("register Cohere embedding provider");
        let huggingface_key_variables = Arc::clone(&self.global_system_variables);
        embed_fn
            .register(
                "huggingface",
                Arc::new(astersql_inference::huggingface::HuggingFaceEmbedder::new(
                    move || {
                        huggingface_key_variables
                            .read()
                            .expect("global system variable lock poisoned")
                            .get("tidb_exp_embed_huggingface_api_key")
                            .cloned()
                            .unwrap_or_default()
                    },
                    String::new,
                )),
            )
            .expect("register HuggingFace embedding provider");
        let nvidia_key_variables = Arc::clone(&self.global_system_variables);
        embed_fn
            .register(
                "nvidia_nim",
                Arc::new(astersql_inference::nvidia::NvidiaEmbedder::new(
                    move || {
                        nvidia_key_variables
                            .read()
                            .expect("global system variable lock poisoned")
                            .get("tidb_exp_embed_nvidia_nim_api_key")
                            .cloned()
                            .unwrap_or_default()
                    },
                    String::new,
                )),
            )
            .expect("register NVIDIA NIM embedding provider");
        let gemini_key_variables = Arc::clone(&self.global_system_variables);
        embed_fn
            .register(
                "gemini",
                Arc::new(astersql_inference::gemini::GeminiEmbedder::new(
                    move || {
                        gemini_key_variables
                            .read()
                            .expect("global system variable lock poisoned")
                            .get("tidb_exp_embed_gemini_api_key")
                            .cloned()
                            .unwrap_or_default()
                    },
                    String::new,
                )),
            )
            .expect("register Gemini embedding provider");
        if hosted_embedding_enabled(
            astersql_config::get_global_config().as_ref(),
            astersql_config_deploymode::IsStarter(),
        ) {
            embed_fn
                .register(
                    "tidbcloud_free",
                    Arc::new(astersql_inference::tidbcloud::TiDBCloudFreeEmbedder::new(
                        || {
                            let cluster_id = astersql_config::get_global_config()
                                .auto_scaler_cluster_id
                                .clone();
                            if cluster_id.is_empty() {
                                String::new()
                            } else {
                                format!("cluster_{cluster_id}")
                            }
                        },
                        || {
                            let path = astersql_config::get_global_config()
                                .hosted_embedding
                                .api_key_path
                                .clone();
                            if path.is_empty() {
                                return String::new();
                            }
                            std::fs::read_to_string(path)
                                .map(|key| key.trim().to_owned())
                                .unwrap_or_default()
                        },
                        || {
                            astersql_config::get_global_config()
                                .hosted_embedding
                                .api_endpoint
                                .clone()
                        },
                    )),
                )
                .expect("register TiDB Cloud embedding provider");
        }
        embed_fn.set_config_version(self.embedding_config_version.load(Ordering::Acquire));
        #[cfg(test)]
        embed_fn
            .register("mock", Arc::new(astersql_inference::MockEmbedder))
            .expect("register test embedding provider");
        *self
            .embed_fn
            .write()
            .expect("embedding function lock poisoned") = Some(Arc::new(embed_fn));
    }

    fn close_inference_providers(&self) {
        if let Some(embed_fn) = self
            .embed_fn
            .write()
            .expect("embedding function lock poisoned")
            .take()
        {
            embed_fn.close();
        }
    }

    /// Go `ttlExternalWorkloadRole`: a live manager takes priority over config.
    pub fn ttl_external_workload_role(&self) -> (String, bool) {
        if let Some(manager) = self.external_workload_manager() {
            return (
                manager
                    .lock()
                    .expect("external workload manager poisoned")
                    .Role(),
                true,
            );
        }
        let config = astersql_config::get_global_config();
        if !config.external_workload.Enable {
            return (String::new(), false);
        }
        let role = &config.external_workload.Role;
        (
            if role.is_empty() {
                astersql_config::RoleMaster.to_owned()
            } else {
                role.clone()
            },
            true,
        )
    }

    /// Go `shouldStartTTLJobManager`: never schedule locally without a live
    /// controller once external workloads are configured.
    pub fn should_start_ttl_job_manager(&self) -> bool {
        let (role, configured) = self.ttl_external_workload_role();
        !configured
            || (self.external_workload_manager().is_some()
                && role == astersql_config::RoleTTLTaskWorker)
    }

    /// Own and stop the concrete SQL-backed TTL manager loop with this Domain.
    /// The session layer supplies its executable tick after bootstrap, keeping
    /// the Domain crate independent of `pkg/session`.
    pub fn start_ttl_job_manager<F>(
        &self,
        interval: Duration,
        mut tick: F,
    ) -> Result<bool, DomainError>
    where
        F: FnMut(&AtomicBool) + Send + 'static,
    {
        if self.closed.load(Ordering::Acquire) {
            return Err(DomainError::Closed);
        }
        if !self.should_start_ttl_job_manager() {
            return Ok(false);
        }
        if self.ttl_job_manager_started.swap(true, Ordering::AcqRel) {
            return Ok(false);
        }
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let join = thread::Builder::new()
            .name("ttl-job-manager".into())
            .spawn(move || {
                while !worker_stop.load(Ordering::Acquire) {
                    tick(&worker_stop);
                    thread::park_timeout(interval);
                }
            })
            .map_err(|error| {
                self.ttl_job_manager_started.store(false, Ordering::Release);
                DomainError::Worker(format!("start TTL job manager: {error}"))
            })?;
        self.workers
            .lock()
            .expect("worker lock poisoned")
            .push(WorkerHandle {
                name: "ttl-job-manager".into(),
                stop,
                join: Some(join),
            });
        Ok(true)
    }

    /// Own the SQL-backed MLog purge scheduler and stop it with the Domain.
    pub fn start_mlog_purge_worker<F>(
        &self,
        interval: Duration,
        mut tick: F,
    ) -> Result<bool, DomainError>
    where
        F: FnMut(&AtomicBool) + Send + 'static,
    {
        if self.closed.load(Ordering::Acquire) {
            return Err(DomainError::Closed);
        }
        if self.mlog_purge_worker_started.swap(true, Ordering::AcqRel) {
            return Ok(false);
        }
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let join = thread::Builder::new()
            .name("mlog-purge-worker".into())
            .spawn(move || {
                while !worker_stop.load(Ordering::Acquire) {
                    tick(&worker_stop);
                    thread::park_timeout(interval);
                }
            })
            .map_err(|error| {
                self.mlog_purge_worker_started
                    .store(false, Ordering::Release);
                DomainError::Worker(format!("start MLog purge worker: {error}"))
            })?;
        self.workers
            .lock()
            .expect("worker lock poisoned")
            .push(WorkerHandle {
                name: "mlog-purge-worker".into(),
                stop,
                join: Some(join),
            });
        Ok(true)
    }

    /// Only the master forwards `tidb_ttl_job_enable` changes to the controller.
    pub fn update_external_workload_ttl_job_enable(
        &self,
        context: &astersql_extworkload::context::Context,
        enable: bool,
    ) -> Result<(), String> {
        if let Some(manager) = self.external_workload_manager() {
            let mut manager = manager.lock().expect("external workload manager poisoned");
            if manager.Role() == astersql_config::RoleMaster {
                manager
                    .UpdateTTLJobEnable(context, enable)
                    .map_err(|error| error.to_string())?;
            }
        }
        Ok(())
    }

    /// 加载指定 KS 的 InfoSchema。
    fn load_info_schema(&self, keyspace: &str) -> Result<LoadedInfoSchema, DomainError> {
        self.store
            .with_storage(|store| self.schema_loader.load_info_schema(store, keyspace))
            .map_err(|error| DomainError::Store(error.to_string()))
    }

    /// 按时间戳加载 InfoSchema 快照。
    fn load_snapshot_info_schema(
        &self,
        keyspace: &str,
        timestamp: u64,
    ) -> Result<LoadedInfoSchema, DomainError> {
        self.store
            .with_storage(|store| {
                self.schema_loader
                    .load_snapshot_info_schema(store, keyspace, timestamp)
            })
            .map_err(|error| DomainError::Store(error.to_string()))
    }

    /// 初始化 Domain（加载 schema 等）。
    pub fn init(&self) -> Result<(), DomainError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(DomainError::Closed);
        }
        self.reconcile_from_committed_metadata_with_mode(false)?;
        self.initialized.store(true, Ordering::Release);
        Ok(())
    }

    /// 在 bootstrap 完成后执行 Go `initStats` 等价的尽力而为初始化。
    ///
    /// 调用方负责记录返回错误但不得因此终止 Domain；无论成功、错误或 panic，
    /// `init_stats_done` 都会被置位，以免服务启动等待永久阻塞。
    pub fn initialize_stats(&self) -> Result<(), DomainError> {
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.init_stats_lite(&[])))
                .unwrap_or_else(|panic| {
                    let message = panic
                        .downcast_ref::<&str>()
                        .copied()
                        .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
                        .unwrap_or("unknown panic");
                    Err(DomainError::Stats(format!(
                        "panic when initiating stats: {message}"
                    )))
                });
        self.stats_handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .init_stats_done = true;
        result
    }

    /// 按 StartMode 启动 Domain 与后台 worker。
    pub fn start(&self, mode: StartMode) -> Result<(), DomainError> {
        if !self.initialized.load(Ordering::Acquire) {
            return Err(DomainError::NotInitialized);
        }
        if self.closed.load(Ordering::Acquire) {
            return Err(DomainError::Closed);
        }
        if self.started.swap(true, Ordering::AcqRel) {
            return Err(DomainError::AlreadyStarted);
        }
        if let Some(ddl) = self.ddl() {
            if let Err(error) = ddl.start(mode) {
                self.started.store(false, Ordering::Release);
                return Err(DomainError::Ddl(error));
            }
        }
        if let Some(syncer) = self.server_info_syncer() {
            self.start_periodic_worker("server-info-sync", Duration::from_secs(15), move || {
                let syncer = syncer.lock().expect("server info syncer lock poisoned");
                let context = astersql_domain_serverinfo::Context::Background();
                let _ = syncer.StoreServerInfo(context.clone());
                let _ = syncer.updateTopologyAliveness(context);
            });
        }
        if !self.ddl().is_some_and(|ddl| ddl.manages_schema_sync()) {
            self.start_periodic_worker(
                "schema-reload",
                self.config.schema_lease.max(Duration::from_millis(10)),
                {
                    let store = self.store.clone();
                    let schema_loader = self.schema_loader.clone();
                    let cache = self.info_cache.clone();
                    let keyspace = self.config.keyspace.clone();
                    move || {
                        if let Ok(loaded) = store
                            .with_storage(|store| schema_loader.load_info_schema(store, &keyspace))
                        {
                            let current = cache
                                .GetLatest()
                                .map(|schema| schema.SchemaMetaVersion())
                                .unwrap_or(i64::MIN);
                            if loaded.schema.SchemaMetaVersion() > current {
                                cache.Insert(loaded.schema, loaded.timestamp);
                            }
                        }
                    }
                },
            );
        }
        self.start_periodic_worker("tiflash-replica-progress", Duration::from_secs(2), {
            let store = self.store.clone();
            let metadata = self.ddl_metadata.clone();
            let loader = self.schema_loader.clone();
            let cache = self.info_cache.clone();
            let keyspace = self.config.keyspace.clone();
            move || {
                let Ok(tables) = store.with_storage(|storage| metadata.replica_tables(storage))
                else {
                    return;
                };
                let mut changed = false;
                for (database, table, id, count, available, physical_ids) in tables {
                    let mut ready = true;
                    let mut supported = true;
                    for physical_id in physical_ids {
                        match store.with_storage(|storage| {
                            storage.ObserveTiFlashReplicaProgress(physical_id, count)
                        }) {
                            Ok(Some(progress)) => ready &= progress == 1.0,
                            Ok(None) => {
                                supported = false;
                                break;
                            }
                            Err(_) => ready = false,
                        }
                    }
                    if !supported {
                        continue;
                    }
                    if ready != available {
                        if store
                            .with_storage(|storage| {
                                metadata.update_tiflash_replica_availability(
                                    storage, &database, &table, id, ready,
                                )
                            })
                            .is_ok()
                        {
                            changed = true;
                        }
                    }
                }
                if changed {
                    if let Ok(loaded) =
                        store.with_storage(|storage| loader.load_info_schema(storage, &keyspace))
                    {
                        cache.Insert(loaded.schema, loaded.timestamp);
                    }
                }
            }
        });
        self.init_inference_providers();
        Ok(())
    }

    /// 启动周期性后台 worker。
    /// Names of currently registered Domain background workers for diagnostics.
    pub fn background_worker_names(&self) -> Vec<String> {
        self.workers
            .lock()
            .expect("worker lock poisoned")
            .iter()
            .map(|w| w.name.clone())
            .collect()
    }

    fn start_periodic_worker<F>(&self, name: &str, interval: Duration, mut task: F)
    where
        F: FnMut() + Send + 'static,
    {
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker_name = name.to_string();
        let join = thread::Builder::new()
            .name(worker_name.clone())
            .spawn(move || {
                while !worker_stop.load(Ordering::Acquire) {
                    task();
                    thread::park_timeout(interval);
                }
            })
            .ok();
        self.workers
            .lock()
            .expect("worker lock poisoned")
            .push(WorkerHandle {
                name: worker_name,
                stop,
                join,
            });
    }

    /// 重载 InfoSchema，返回版本。
    pub fn reload(&self) -> Result<i64, DomainError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(DomainError::Closed);
        }
        let loaded = self.load_info_schema(&self.config.keyspace)?;
        let version = loaded.schema.SchemaMetaVersion();
        let current = self
            .info_cache
            .GetLatest()
            .map(|schema| schema.SchemaMetaVersion())
            .unwrap_or(i64::MIN);
        if version >= current {
            // Persisted DDL can introduce an MLog (or other row-ID table)
            // without passing through the SQL stats catalog registration path.
            // Construct/reuse its canonical allocator before publishing schema.
            let catalog = loaded
                .schema
                .AllSchemas()
                .into_iter()
                .flat_map(|db| {
                    let name = db.name.lower.clone();
                    db.tables
                        .iter()
                        .filter_map(move |table| {
                            table.model_meta.as_ref().map(|meta| {
                                (
                                    (name.clone(), meta.Name.L.clone()),
                                    (
                                        StatsTableKey::new(&name, &meta.Name.L, meta.ID),
                                        meta.as_ref().clone(),
                                    ),
                                )
                            })
                        })
                        .collect::<Vec<_>>()
                })
                .collect::<BTreeMap<_, _>>();
            self.reconcile_stats_auto_id_allocators(&catalog)?;
            self.info_cache.Insert(loaded.schema, loaded.timestamp);
        }
        self.schema_reload_count.fetch_add(1, Ordering::Relaxed);
        Ok(version)
    }

    /// Force the full-schema reload path used by DDL compatibility tests.
    ///
    /// A full reload replaces table objects, so their local AutoID reservation
    /// caches must be rebuilt from the persisted allocator high-water marks.
    pub fn force_full_reload_for_test(&self) -> Result<i64, DomainError> {
        self.stats_auto_id_allocators
            .lock()
            .expect("domain AutoID allocator lock poisoned")
            .clear();
        self.reconcile_from_committed_metadata_with_mode(false)?;
        self.reload()
    }

    /// schema 重载次数。
    pub fn schema_reload_count(&self) -> u64 {
        self.schema_reload_count.load(Ordering::Relaxed)
    }
    /// 返回 schema 租约。
    pub fn schema_lease(&self) -> Duration {
        self.config.schema_lease
    }
    /// 判断 schema 租约是否过期。
    pub fn is_lease_expired(&self) -> bool {
        self.config.schema_lease.is_zero() || self.closed.load(Ordering::Acquire)
    }

    /// 获取指定 keyspace 运行时句柄。
    pub fn acquire_keyspace_runtime(
        &self,
        keyspace: &str,
        holder: &str,
    ) -> Result<KeyspaceRuntimeHandle, DomainError> {
        let exists = self
            .store
            .with_storage(|store| self.schema_loader.keyspace_exists(store, keyspace))
            .map_err(|error| DomainError::Store(error.to_string()))?;
        if !exists {
            return Err(DomainError::KeyspaceNotFound(keyspace.to_string()));
        }
        let mut runtimes = self
            .keyspace_runtimes
            .lock()
            .expect("keyspace runtime lock poisoned");
        let runtime = if let Some(runtime) = runtimes.get(keyspace) {
            runtime.clone()
        } else {
            let loaded = self.load_info_schema(keyspace)?;
            let cache = Arc::new(astersql_infoschema::NewCache(
                self.config.info_cache_capacity.max(1),
            ));
            cache.Insert(loaded.schema, loaded.timestamp);
            let runtime = KeyspaceRuntime {
                keyspace: keyspace.to_string(),
                info_cache: cache,
                holders: Arc::new(Mutex::new(BTreeSet::new())),
            };
            runtimes.insert(keyspace.to_string(), runtime.clone());
            runtime
        };
        drop(runtimes);
        runtime.acquire(holder)
    }

    /// 记录慢查询。
    pub fn log_slow_query(&self, query: SlowQueryInfo) {
        self.slow_queries
            .lock()
            .expect("slow query lock poisoned")
            .add(query);
    }
    /// 按种类取出慢查询列表。
    pub fn show_slow_queries(&self, kind: SlowQueryKind) -> Vec<SlowQueryInfo> {
        let state = self.slow_queries.lock().expect("slow query lock poisoned");
        match kind {
            SlowQueryKind::Top => state.top.clone(),
            SlowQueryKind::Recent => state.recent.iter().cloned().collect(),
            SlowQueryKind::All => {
                let mut all = state.top.clone();
                all.extend(state.recent.iter().cloned());
                all
            }
        }
    }

    /// 通知全局配置变更。
    pub fn notify_global_config_change(&self, name: impl Into<String>, value: impl Into<String>) {
        self.global_config_events
            .lock()
            .expect("config event lock poisoned")
            .push_back((name.into(), value.into()));
    }
    /// 取出并清空全局配置变更队列。
    pub fn take_global_config_changes(&self) -> Vec<(String, String)> {
        self.global_config_events
            .lock()
            .expect("config event lock poisoned")
            .drain(..)
            .collect()
    }

    /// 通知全量刷新权限缓存。
    pub fn notify_update_all_privileges(&self) {
        self.privilege_events
            .lock()
            .expect("privilege event lock poisoned")
            .push_back(PrivilegeEvent {
                event_type: PrivilegeEventType::UpdateAll,
                users: Vec::new(),
            });
    }
    /// 通知按用户刷新权限缓存。
    pub fn notify_update_privilege(&self, users: Vec<String>) {
        self.privilege_events
            .lock()
            .expect("privilege event lock poisoned")
            .push_back(PrivilegeEvent {
                event_type: PrivilegeEventType::UpdateUsers,
                users,
            });
    }
    /// 取出合并后的权限事件。
    pub fn drain_privilege_events(&self) -> Option<PrivilegeEvent> {
        let mut events = self
            .privilege_events
            .lock()
            .expect("privilege event lock poisoned");
        let mut merged = events.pop_front()?;
        for next in events.drain(..) {
            merged.merge(next);
        }
        Some(merged)
    }

    /// 请求刷新系统变量缓存。
    pub fn notify_update_sysvar_cache(&self, update_local: bool) {
        self.sysvar_reload_requested
            .store(update_local, Ordering::Release);
    }
    /// 取出并清除 sysvar 重载请求标志。
    pub fn take_sysvar_reload_request(&self) -> bool {
        self.sysvar_reload_requested.swap(false, Ordering::AcqRel)
    }

    /// 检查当前是否落在自动 Analyze 时间窗。
    pub fn check_auto_analyze_window(
        now_minutes: u16,
        start_minutes: u16,
        end_minutes: u16,
    ) -> bool {
        if start_minutes <= end_minutes {
            (start_minutes..end_minutes).contains(&now_minutes)
        } else {
            now_minutes >= start_minutes || now_minutes < end_minutes
        }
    }

    /// 标记统计更新循环状态。
    pub fn set_stats_updating(&self, value: bool) {
        self.stats_updating.store(value, Ordering::Release);
    }
    /// 统计更新循环是否在跑。
    pub fn stats_updating(&self) -> bool {
        self.stats_updating.load(Ordering::Acquire)
    }
    /// 成为统计 Owner。
    pub fn enable_stats_owner(&self) -> bool {
        !self.stats_owner.swap(true, Ordering::AcqRel)
    }
    /// 卸任统计 Owner。
    pub fn disable_stats_owner(&self) -> bool {
        self.stats_owner.swap(false, Ordering::AcqRel)
    }

    /// 设置资源组版本号。
    pub fn set_resource_group_version(&self, version: u64) {
        self.resource_group_version
            .store(version, Ordering::Release);
    }
    /// 读取资源组版本号。
    pub fn resource_group_version(&self) -> u64 {
        self.resource_group_version.load(Ordering::Acquire)
    }

    /// 初始化实例级计划缓存。
    pub fn init_instance_plan_cache(&self, capacity: usize) {
        *self.plan_cache.write().expect("plan cache lock poisoned") =
            Some(Arc::new(Mutex::new(PlanCache::new(capacity))));
    }
    /// 获取实例级计划缓存。
    pub fn instance_plan_cache(&self) -> Option<Arc<Mutex<PlanCache>>> {
        self.plan_cache
            .read()
            .expect("plan cache lock poisoned")
            .clone()
    }

    /// 申请唯一 server_id 租约。
    pub fn acquire_server_id(
        &self,
        proposed: impl Fn(u32) -> u64,
        in_use: impl Fn(u64) -> bool,
    ) -> Result<u64, DomainError> {
        let mut lease = self.server_id.lock().expect("server id lock poisoned");
        if let Some(existing) = lease.as_mut() {
            if existing.expires_at > Instant::now() {
                existing.expires_at = Instant::now() + self.config.server_id_ttl;
                return Ok(existing.id);
            }
        }
        for conflict in 0..1_000 {
            let id = proposed(conflict);
            if id == 0 {
                continue;
            }
            if !in_use(id) {
                *lease = Some(ServerIdLease {
                    id,
                    expires_at: Instant::now() + self.config.server_id_ttl,
                });
                return Ok(id);
            }
        }
        Err(DomainError::ServerIdExhausted)
    }

    /// 续期 server_id 租约。
    pub fn refresh_server_id_ttl(&self) -> Result<(), DomainError> {
        let mut lease = self.server_id.lock().expect("server id lock poisoned");
        let Some(lease) = lease.as_mut() else {
            return Err(DomainError::ServerIdConflict);
        };
        lease.expires_at = Instant::now() + self.config.server_id_ttl;
        Ok(())
    }
    /// 释放 server_id。
    pub fn release_server_id(&self) -> Option<u64> {
        self.server_id
            .lock()
            .expect("server id lock poisoned")
            .take()
            .map(|lease| lease.id)
    }
    /// 当前 server_id（无则 0）。
    pub fn server_id(&self) -> u64 {
        self.server_id
            .lock()
            .expect("server id lock poisoned")
            .as_ref()
            .filter(|lease| lease.expires_at > Instant::now())
            .map(|lease| lease.id)
            .unwrap_or(0)
    }
    /// 是否与 PD 失联（租约过期近似）。
    pub fn lost_connection_to_pd(&self) -> bool {
        self.server_id
            .lock()
            .expect("server id lock poisoned")
            .as_ref()
            .is_some_and(|lease| lease.expires_at <= Instant::now())
    }

    /// 分配下一个连接 ID。
    pub fn next_connection_id(&self) -> u64 {
        let local = self
            .connection_id
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1)
            & 0x0000_FFFF_FFFF_FFFF;
        (self.server_id() << 48) | local
    }
    /// 释放连接 ID（当前为 no-op 占位）。
    pub fn release_connection_id(&self, connection_id: u64) {
        self.sys_processes.untrack(connection_id);
    }
    /// 返回系统进程表。
    pub fn sys_processes(&self) -> Arc<SysProcesses> {
        self.sys_processes.clone()
    }
    /// 分配 DDL notifier 序号。
    pub fn next_ddl_notifier_sequence(&self) -> i64 {
        self.ddl_notifier_sequence.fetch_add(1, Ordering::Relaxed) + 1
    }
    /// 注册关闭回调。
    pub fn set_on_close(&self, callback: impl FnOnce() + Send + 'static) {
        *self.on_close.lock().expect("on-close lock poisoned") = Some(Box::new(callback));
    }
    /// Domain 是否已关闭。
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// 关闭 Domain：停 worker、关 DDL、触发回调。
    pub fn close(&self) {
        // 用 swap 保证 close 幂等。
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        let workers = std::mem::take(&mut *self.workers.lock().expect("worker lock poisoned"));
        for worker in workers {
            worker.stop();
        }
        // Drop the service reference after stopping: its pool owns sessions
        // referring to this Domain, so retaining it would form an Arc cycle.
        let ddl = self.ddl.write().expect("DDL service lock poisoned").take();
        if let Some(ddl) = ddl {
            let _ = ddl.stop();
        }
        if let Some(manager) = self
            .cross_ks_manager
            .write()
            .expect("cross-keyspace manager lock poisoned")
            .take()
        {
            manager.close();
        }
        if let Some(syncer) = self
            .server_info_syncer
            .write()
            .expect("server info syncer lock poisoned")
            .take()
        {
            let syncer = syncer.lock().expect("server info syncer lock poisoned");
            syncer.RemoveServerInfo();
            syncer.RemoveTopologyInfo();
            syncer.RevokeSession();
            syncer.RevokeTopologySession();
        }
        if let Some(manager) = self
            .external_workload_manager
            .write()
            .expect("external workload manager lock poisoned")
            .take()
        {
            let _ = manager
                .lock()
                .expect("external workload manager lock poisoned")
                .Close();
        }
        self.release_server_id();
        self.close_inference_providers();
        self.started.store(false, Ordering::Release);
        if let Some(callback) = self.on_close.lock().expect("on-close lock poisoned").take() {
            callback();
        }
        let (lock, condvar) = &self.close_pair;
        *lock.lock().expect("close lock poisoned") = true;
        condvar.notify_all();
    }

    /// 等待关闭完成或超时。
    pub fn wait_closed(&self, timeout: Duration) -> bool {
        if self.is_closed() {
            return true;
        }
        let (lock, condvar) = &self.close_pair;
        let closed = lock.lock().expect("close lock poisoned");
        condvar
            .wait_timeout_while(closed, timeout, |closed| !*closed)
            .expect("close lock poisoned")
            .0
            .to_owned()
    }
}

impl Drop for Domain {
    fn drop(&mut self) {
        self.close();
        let _ = self.store.close();
    }
}

/// 系统进程（连接）抽象：查询信息与 kill。
pub trait SystemProcess: Send + Sync {
    fn connection_id(&self) -> u64;
    fn process_info(&self) -> Option<ProcessInfo>;
    /// 杀死指定连接。
    fn kill(&self);
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 进程/连接展示信息。
pub struct ProcessInfo {
    pub id: u64,
    pub user: String,
    pub database: String,
    pub command: String,
    pub sql: String,
}

#[derive(Default)]
/// 系统进程表：跟踪连接并支持 kill。
pub struct SysProcesses {
    processes: RwLock<BTreeMap<u64, Arc<dyn SystemProcess>>>,
}

impl SysProcesses {
    /// 登记系统进程。
    pub fn track(&self, id: u64, process: Arc<dyn SystemProcess>) -> Result<(), DomainError> {
        if id != process.connection_id() {
            return Err(DomainError::ProcessNotFound(id));
        }
        let mut processes = self.processes.write().expect("process lock poisoned");
        if processes.contains_key(&id) {
            return Err(DomainError::ProcessExists(id));
        }
        processes.insert(id, process);
        Ok(())
    }
    /// 注销系统进程。
    pub fn untrack(&self, id: u64) {
        self.processes
            .write()
            .expect("process lock poisoned")
            .remove(&id);
    }
    /// 列出进程信息。
    pub fn process_list(&self) -> BTreeMap<u64, ProcessInfo> {
        self.processes
            .read()
            .expect("process lock poisoned")
            .iter()
            .filter_map(|(id, process)| {
                process
                    .process_info()
                    .filter(|info| info.id == *id)
                    .map(|info| (*id, info))
            })
            .collect()
    }
    /// 杀死指定连接。
    pub fn kill(&self, id: u64) -> Result<(), DomainError> {
        let process = self
            .processes
            .read()
            .expect("process lock poisoned")
            .get(&id)
            .cloned()
            .ok_or(DomainError::ProcessNotFound(id))?;
        process.kill();
        Ok(())
    }
    /// 进程数量。
    pub fn len(&self) -> usize {
        self.processes.read().expect("process lock poisoned").len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// 判断 SQL 是否为 `ANALYZE TABLE` 语句。
pub fn is_analyze_table_sql(sql: &str) -> bool {
    let mut normalized = sql.trim_start();
    loop {
        if let Some(comment) = normalized.strip_prefix("/*") {
            let Some(end) = comment.find("*/") else {
                return false;
            };
            normalized = comment[end + 2..].trim_start();
            continue;
        }
        if let Some(comment) = normalized.strip_prefix("--") {
            let Some(end) = comment.find('\n') else {
                return false;
            };
            normalized = comment[end + 1..].trim_start();
            continue;
        }
        if let Some(comment) = normalized.strip_prefix('#') {
            let Some(end) = comment.find('\n') else {
                return false;
            };
            normalized = comment[end + 1..].trim_start();
            continue;
        }
        break;
    }
    normalized.to_ascii_lowercase().starts_with("analyze table")
}

/// 在 [min, max) 秒间生成伪随机 Duration（用 entropy 取模）。
pub fn random_duration(min_seconds: u64, max_seconds: u64, entropy: u64) -> Duration {
    if min_seconds >= max_seconds {
        return Duration::from_secs(min_seconds);
    }
    Duration::from_secs(min_seconds + entropy % (max_seconds - min_seconds))
}

pub use astersql_infoschema_issyncer::KvSchemaStore;

impl astersql_infoschema_issyncer::KvSchemaSource for StorageHandle {
    fn current_version(&self) -> Result<u64, astersql_infoschema_issyncer::SyncError> {
        self.with_storage(|store| {
            store
                .CurrentVersion("global")
                .map(|v| v.Ver)
                .map_err(|e| astersql_infoschema_issyncer::SyncError(e.to_string()))
        })
    }
    fn snapshot(&self, ts: u64) -> Box<dyn astersql_kv::Snapshot> {
        self.with_storage(|store| store.GetSnapshot(astersql_kv::NewVersion(ts)))
    }
    fn keyspace(&self) -> String {
        self.with_storage(|store| store.GetKeyspace())
    }
    fn delete_cached_table(&self, id: i64) {
        self.with_storage(|store| store.GetMemCache().Delete(id));
    }
}

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

// A narrow executable session runtime over the canonical parser, Domain and KV
// interfaces. The full planner/executor-backed `sessionapi::Session` remains a
// separate ABI: this module supplies the concrete lifecycle needed by the Go
// testutil flow without pretending that a fixed-response SQL stub is a server.
//
// 基于规范解析器、Domain 与 KV 接口的可执行会话运行时。
//
// 完整 planner/executor 版 `sessionapi::Session` 仍是独立 ABI；
// 本模块为 Go testutil 流程提供具体生命周期，而非固定响应的 SQL stub。
// 覆盖 ANALYZE 暂停/错误注入、统计同步加载、计划缓存、关系型 DML、
// SHOW STATS、会话 KV 表以及 ConcreteSession 执行路径。

#![allow(non_snake_case)]

mod admin;
mod control;
pub mod crossks_job_submit;
#[cfg(test)]
mod crossks_job_submit_test;
pub mod crossks_owner;
#[cfg(test)]
mod crossks_owner_test;
pub mod crossks_runtime;
#[cfg(test)]
mod crossks_runtime_test;
pub mod crossks_schema;
#[cfg(test)]
mod crossks_schema_test;
pub mod crossks_session_pool;
#[cfg(test)]
mod crossks_session_pool_test;
pub mod crossks_store;
#[cfg(test)]
mod crossks_store_test;
mod ddl;
mod ddl_index_validation;
mod dispatch;
mod dml;
mod dxf_session;
mod explain_analyze;
mod explain_query;
mod explain_read;
mod explain_select;
mod import_compression;
mod import_file;
mod import_sst;
pub use import_sst::NewImportLocalBackend;
mod load_data;
mod mlog;
mod mlog_purge;
pub(crate) use mlog_purge::purge_mlog_snapshot_batch;
pub(crate) use mlog_purge::run_mlog_purge_tick;
mod mview_ddl;
pub use import_file::{ImportFileSubtask, ImportFileTask};
mod planning;
mod query;
#[cfg(test)]
mod query_binary_test;
mod recovery;
mod relational_scan;
pub(crate) use relational_scan::scan_mlog_record_commit_ts;
mod relational_value;
pub(crate) use relational_value::{relational_compare, relational_window_value};
#[cfg(test)]
mod durable_scheduler_test;
#[cfg(test)]
mod lifecycle_test;
pub mod normal_ddl_service;
#[cfg(test)]
mod normal_ddl_test;
mod row_codec;
#[cfg(test)]
mod row_codec_test;
mod scan_adapter_runtime;
#[cfg(test)]
mod scan_adapter_runtime_test;
mod select_into;
mod session;
pub mod session_factory;
pub mod system_session;
use session::RuntimeForeignKeyDeleteCascade;
#[cfg(test)]
mod inference_test;
mod source;
mod statistics;
pub(crate) mod system_query;
mod transaction;
pub mod ttl_metadata;
#[cfg(test)]
mod ttl_metadata_test;
pub mod ttl_runtime;
#[cfg(test)]
mod ttl_runtime_test;
#[cfg(test)]
mod ttl_sysvar_test;
mod ttl_timer;
mod ttl_timer_etcd;
#[cfg(test)]
mod ttl_timer_etcd_test;
pub mod ttl_timer_store;
#[cfg(test)]
mod ttl_timer_store_test;
#[cfg(test)]
mod ttl_timer_test;
pub mod ttl_worker_session;
#[cfg(test)]
mod ttl_worker_session_test;
mod typed_adapter_bridge;
mod typed_analyze_executor;
mod typed_dml_executor;
mod typed_fk_cascade_executor;
mod typed_runaway_checker;
pub use typed_adapter_bridge::{OwnedKVSnapshotSource, SessionBoundAdapterOwner, TypedScanSpec};

pub(crate) use dispatch::{quote_argument, split_statement_sql};
pub(super) use dml::*;
use explain_analyze::*;
use explain_query::*;
use explain_select::*;
use mlog::RuntimeMLog;
pub use planning::PlannedKVResult;
pub(crate) use planning::{
    SessionDomainDataSourceProvider, SessionKVDataSourceProvider, SessionStatsSyncLoadAdapter,
    estimated_table_records, estimated_table_stats, plan_context_with_params_and_explain,
    should_use_pseudo_for_outdated_stats,
};
use planning::{
    bind_parameter_markers, parameter_markers, parameter_shape_class, plan_context_with_params,
};
use query::*;
pub use relational_scan::transaction_has_table_prefix;
use relational_scan::*;
pub(crate) use relational_scan::{
    RelationalRowScanRange, RelationalSecondaryIndexAccess, count_relational_rows,
    decode_relational_checksum_count_response, decode_relational_count_response,
    integer_handle_offset_candidate, relational_count_checksum, relational_count_dag,
    relational_primary_key_scan_ranges, scan_relational_rows_window_key_only,
    scan_relational_rows_window_key_only_from,
};
use relational_value::*;
use row_codec::*;
pub(crate) use source::is_scalar_count_non_null_constant;
use source::*;
use statistics::*;
use system_query::*;

pub use session::{
    AddRecordWithoutAutoIDRebaseForTest, BootstrapCanonicalDomain, CanonicalSessionFactory,
    ConcretePreparedArgument, ConcreteProtocolState, ConcreteRecordSet, ConcreteResultField,
    ConcreteSession, ConcreteTestRuntime, CreateAnalyzeSession, CreateDanglingIndexForTest,
    RuntimeDomain, SplitSQLStatements,
};
use session::{NamedPreparedStatement, SessionState, SessionWarning, build_bootstrap_view_table};
pub use statistics::{
    AnalyzePauseGuard, AnalyzeSaveErrorGuard, EnableAnalyzeBuildPauseForTest,
    EnableAnalyzePauseForTest, EnableAnalyzeSaveErrorForTest,
};
use transaction::*;

use std::any::Any;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Condvar, LazyLock, Mutex, OnceLock, RwLock, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use astersql_domain::domain::StartMode;
use astersql_domain::{Domain, DomainConfig, InfoSchemaLoader, KvInfoSchemaLoader, SQLKiller};
use astersql_infoschema::{InfoSchema as _, SchemaRef};
use astersql_kv as kv;
use astersql_parser::Parser;
use astersql_parser_ast as ast;
use astersql_planner_core_base::{PhysicalPlan, Plan};
use astersql_planner_core_metrics::planner_core_metrics::GetPlanCacheHitCounter;
use astersql_planner_core_operator_logicalop::LogicalPlan as _;
use astersql_planner_core_resolve::ResultField;
use astersql_planner_core_rule::rule_prune_indexes as prune_indexes;
use astersql_util_dbterror_plannererrors as plannererrors;
use astersql_util_logutil::log::{BgLogger, LogField, LogLevel};
use astersql_util_memory::action::{DefLogPriority, DefRateLimitPriority, PanicOnExceed};
use astersql_util_memory::global_arbitrator::{
    GetGlobalMemArbitratorSoftLimitText, GetGlobalMemArbitratorWorkModeText, GlobalMemArbitrator,
    SetGlobalMemArbitratorLimit, SetGlobalMemArbitratorSoftLimit, SetGlobalMemArbitratorWorkMode,
};
use astersql_util_memory::sqlkiller::KilledByMemArbitrator;
use astersql_util_memory::tracker::{
    ActionOnExceed, LabelForSQLText, LabelForSession, NewTracker, Tracker,
};
use astersql_util_memory::{
    ArbitrateHelper, ArbitrateOk, ArbitrationPriority, ArbitrationPriorityHigh,
    ArbitrationPriorityLow, ArbitrationPriorityMedium, ArbitratorStopReason, CancelReceiver,
    MemArbitrator, NewArbitrationContext,
};
use astersql_util_traceevent::traceevent::{
    Context as TraceContext, Field as TraceField, STMT_LIFECYCLE, generate_trace_id, trace_event,
};
use chrono::{DateTime, Datelike, FixedOffset, Timelike, Utc};
use chrono_tz::Tz;

use crate::plan_cache_runtime::{
    PreparedKVPhysicalPlan, PreparedPlannedKVResult, PreparedPlannedKVSelect, ProcessPlanSnapshot,
};

static RUNTIME_INSTANCE_PLAN_CACHES: LazyLock<
    Mutex<HashMap<u64, Weak<astersql_planner_core::InstancePlanCache>>>,
> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn runtime_instance_plan_cache(
    domain: &Arc<Domain>,
) -> Arc<astersql_planner_core::InstancePlanCache> {
    let domain_id = runtime_domain_id(domain);
    let mut caches = RUNTIME_INSTANCE_PLAN_CACHES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(cache) = caches.get(&domain_id).and_then(Weak::upgrade) {
        return cache;
    }
    let cache = Arc::new(astersql_planner_core::NewInstancePlanCache(
        i64::MAX,
        i64::MAX,
    ));
    caches.insert(domain_id, Arc::downgrade(&cache));
    cache
}
use crate::testutil::{TestDomain, TestRecordSet, TestRuntime, TestSession, TestStore};
use crate::{SessionError, SessionResult};

/// Whether a cluster-wide read-only switch currently rejects ordinary writes.
///
/// The setting is process-global just as it is in TiDB, so commit paths must
/// sample it at their final write boundary rather than caching it per session.
pub(super) fn runtime_read_only_mode_enabled() -> bool {
    astersql_sessionctx_variable::vardef::RestrictedReadOnly.Load()
        || astersql_sessionctx_variable::vardef::VarTiDBSuperReadOnly.Load()
}

/// Preserve TiDB's optimizer-class read-only error at runtime commit checks.
pub(super) fn runtime_read_only_mode_error() -> SessionError {
    SessionError::new(
        plannererrors::ErrSQLInReadOnlyMode
            .GenWithStackByArgs(&[])
            .to_string(),
    )
}

/// 会话内置 KV 表名（文本 SQL 路径）。
const SESSION_KV_TABLE: &str = "aster_session_kv";
/// Rendering Go uses for a NULL cell in `SHOW STATS_*` result sets.
/// `SHOW STATS_*` 结果集中 NULL 单元格的 Go 风格展示文本。
const SHOW_NULL_CELL: &str = "<nil>";
/// ConcreteRecordSet 的 SQL NULL 内部标记，由 MySQL 协议适配器还原为 NULL。
pub const CONCRETE_NULL_VALUE: &str = "__astersql_internal_null__";

fn trace_id_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
/// 会话 KV 在底层存储中的 key 前缀字节。
const SESSION_KV_PREFIX: &[u8] = b"__aster_session_kv/";
/// 大 OFFSET 顺序扫描的远端分页大小。只在根 LIMIT/OFFSET 已可安全下推时使用，
/// 避免 client-rust 默认 256 行一页产生数万次串行 Scan RPC。
const RELATIONAL_OFFSET_SCAN_BATCH_SIZE: usize = 65_536;
/// 超过一批时尝试整数聚簇 handle seek；更小 OFFSET 直接顺序扫更便宜。
const RELATIONAL_OFFSET_HANDLE_SEEK_THRESHOLD: usize = RELATIONAL_OFFSET_SCAN_BATCH_SIZE;
/// Go add-index reorg writes bounded transactional batches instead of one table-sized txn.
const RELATIONAL_INDEX_BACKFILL_BATCH_SIZE: usize = 4_096;
/// 与 TiDB `DefDistSQLScanConcurrency` 一致的 TiKV coprocessor 并发度。
const RELATIONAL_COP_SCAN_CONCURRENCY: i32 = 15;
/// Local temporary tables never enter the Domain catalog, but their physical
/// keys must still be unique across sessions sharing one storage instance.
static NEXT_RUNTIME_LOCAL_TEMPORARY_TABLE_ID: AtomicI64 = AtomicI64::new(i64::MAX / 2);
/// Go `expensivequery.Handle.LogOnQueryExceedMemQuota` 在 bootstrap 阶段的日志。
const EXPENSIVE_QUERY_DURING_BOOTSTRAP: &str = "expensive_query during bootstrap phase";
/// Cop rate-limit 动作无法继续限速时委托给 fallback 的日志。
const RATE_LIMIT_DELEGATE_TO_FALLBACK: &str =
    "memory exceeds quota, rateLimitAction delegate to fallback action";
/// Prepared-statement counters and limits are isolated per canonical Domain.
/// A Domain is the runtime instance boundary used by TestKit and by embedded
/// servers, so one test/store cannot consume another instance's quota.
static RUNTIME_PREPARED_STMT_COUNTS: LazyLock<Mutex<HashMap<usize, i64>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static RUNTIME_MAX_PREPARED_STMT_COUNTS: LazyLock<Mutex<HashMap<usize, i64>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
/// Instance-scope plan-cache generation. Incrementing it invalidates named
/// prepared plans in every session attached to the Domain.
static RUNTIME_PLAN_CACHE_GENERATIONS: LazyLock<Mutex<HashMap<usize, u64>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
/// Global SQL bindings are shared by every session attached to one Domain.
/// Session bindings remain inside `SessionBindingCatalog` and disappear with
/// their owning connection, matching Go's scope boundary.
static RUNTIME_GLOBAL_BINDINGS: LazyLock<
    Mutex<HashMap<u64, Vec<Arc<astersql_bindinfo::Binding>>>>,
> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn runtime_global_bindings(domain: &Arc<Domain>) -> Vec<Arc<astersql_bindinfo::Binding>> {
    let domain_id = runtime_domain_id(domain);
    RUNTIME_GLOBAL_BINDINGS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&domain_id)
        .cloned()
        .unwrap_or_default()
}

fn runtime_upsert_global_binding(domain: &Arc<Domain>, binding: astersql_bindinfo::Binding) {
    let domain_id = runtime_domain_id(domain);
    let mut all_bindings = RUNTIME_GLOBAL_BINDINGS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let bindings = all_bindings.entry(domain_id).or_default();
    bindings.retain(|existing| existing.SQLDigest != binding.SQLDigest);
    bindings.push(Arc::new(binding));
}

fn runtime_drop_global_bindings(domain: &Arc<Domain>, digests: &[String]) -> usize {
    let domain_id = runtime_domain_id(domain);
    let mut all_bindings = RUNTIME_GLOBAL_BINDINGS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let bindings = all_bindings.entry(domain_id).or_default();
    let before = bindings.len();
    bindings.retain(|binding| !digests.contains(&binding.SQLDigest));
    before - bindings.len()
}

fn runtime_prepared_stmt_reserve(domain: &Arc<Domain>) -> Result<(), i64> {
    let key = Arc::as_ptr(domain) as usize;
    let limit = RUNTIME_MAX_PREPARED_STMT_COUNTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&key)
        .copied()
        .unwrap_or(astersql_sessionctx_vardef::DefMaxPreparedStmtCount);
    let mut counts = RUNTIME_PREPARED_STMT_COUNTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let count = counts.entry(key).or_default();
    *count += 1;
    if limit >= 0 && *count > limit {
        *count -= 1;
        return Err(limit);
    }
    Ok(())
}

fn runtime_prepared_stmt_release(domain: &Arc<Domain>, count: usize) {
    if count == 0 {
        return;
    }
    let key = Arc::as_ptr(domain) as usize;
    let mut counts = RUNTIME_PREPARED_STMT_COUNTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let current = counts.entry(key).or_default();
    *current = current.saturating_sub(count as i64);
}

fn runtime_max_prepared_stmt_count(domain: &Arc<Domain>) -> i64 {
    RUNTIME_MAX_PREPARED_STMT_COUNTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&(Arc::as_ptr(domain) as usize))
        .copied()
        .unwrap_or(astersql_sessionctx_vardef::DefMaxPreparedStmtCount)
}

fn runtime_set_max_prepared_stmt_count(domain: &Arc<Domain>, value: i64) {
    RUNTIME_MAX_PREPARED_STMT_COUNTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(Arc::as_ptr(domain) as usize, value);
}

fn runtime_plan_cache_generation(domain: &Arc<Domain>) -> u64 {
    RUNTIME_PLAN_CACHE_GENERATIONS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&(Arc::as_ptr(domain) as usize))
        .copied()
        .unwrap_or_default()
}

fn runtime_bump_plan_cache_generation(domain: &Arc<Domain>) {
    let mut generations = RUNTIME_PLAN_CACHE_GENERATIONS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let generation = generations.entry(Arc::as_ptr(domain) as usize).or_default();
    *generation = generation.saturating_add(1);
}

struct RuntimeLogOnExceed {
    fallback: Option<Box<dyn ActionOnExceed>>,
    connection_id: u64,
    acted: bool,
    finished: bool,
}

// The statement tracker still uses the mutable legacy action interface while
// PanicOnExceed is implemented against the shared action interface. Keep the
// cancellation behavior intact at this boundary until both interfaces merge.
struct RuntimePanicOnExceed(PanicOnExceed);

impl ActionOnExceed for RuntimePanicOnExceed {
    fn Action(&mut self, tracker: &mut Tracker) {
        astersql_util_memory::action::ActionOnExceed::Action(&self.0, tracker);
    }

    fn SetFallback(&mut self, _action: Option<Box<dyn ActionOnExceed>>) {}

    fn GetFallback(&mut self) -> Option<Box<dyn ActionOnExceed>> {
        None
    }

    fn GetPriority(&self) -> i64 {
        astersql_util_memory::action::ActionOnExceed::GetPriority(&self.0)
    }

    fn SetFinished(&mut self) {
        astersql_util_memory::action::ActionOnExceed::SetFinished(&self.0);
    }

    fn IsFinished(&self) -> bool {
        astersql_util_memory::action::ActionOnExceed::IsFinished(&self.0)
    }
}

struct RuntimeCancelOnExceed {
    killer: Arc<SQLKiller>,
    fallback: Option<Box<dyn ActionOnExceed>>,
    finished: bool,
}

impl ActionOnExceed for RuntimeCancelOnExceed {
    fn Action(&mut self, _tracker: &mut Tracker) {
        self.killer
            .SendKillSignal(astersql_util_memory::sqlkiller::QueryMemoryExceeded);
    }

    fn SetFallback(&mut self, action: Option<Box<dyn ActionOnExceed>>) {
        self.fallback = action;
    }

    fn GetFallback(&mut self) -> Option<Box<dyn ActionOnExceed>> {
        self.fallback.take()
    }

    fn GetPriority(&self) -> i64 {
        0
    }

    fn SetFinished(&mut self) {
        self.finished = true;
    }

    fn IsFinished(&self) -> bool {
        self.finished
    }
}

impl RuntimeLogOnExceed {
    fn new(connection_id: u64) -> Self {
        Self {
            fallback: None,
            connection_id,
            acted: false,
            finished: false,
        }
    }
}

impl ActionOnExceed for RuntimeLogOnExceed {
    fn Action(&mut self, tracker: &mut Tracker) {
        if self.acted {
            if let Some(fallback) = self.fallback.as_mut() {
                fallback.Action(tracker);
            }
            return;
        }
        self.acted = true;
        BgLogger().log(
            LogLevel::Info,
            EXPENSIVE_QUERY_DURING_BOOTSTRAP,
            [LogField::U64("conn".to_owned(), self.connection_id)],
        );
    }

    fn SetFallback(&mut self, action: Option<Box<dyn ActionOnExceed>>) {
        self.fallback = action;
    }

    fn GetFallback(&mut self) -> Option<Box<dyn ActionOnExceed>> {
        self.fallback.take()
    }

    fn GetPriority(&self) -> i64 {
        DefLogPriority
    }

    fn SetFinished(&mut self) {
        self.finished = true;
    }

    fn IsFinished(&self) -> bool {
        self.finished
    }
}

#[derive(Default)]
struct RuntimeRateLimitAction {
    fallback: Option<Box<dyn ActionOnExceed>>,
    finished: bool,
}

impl ActionOnExceed for RuntimeRateLimitAction {
    fn Action(&mut self, tracker: &mut Tracker) {
        BgLogger().log(
            LogLevel::Info,
            RATE_LIMIT_DELEGATE_TO_FALLBACK,
            [
                LogField::I64("consumed".to_owned(), tracker.BytesConsumed()),
                LogField::I64("quota".to_owned(), tracker.GetBytesLimit()),
            ],
        );
        self.finished = true;
    }

    fn SetFallback(&mut self, action: Option<Box<dyn ActionOnExceed>>) {
        self.fallback = action;
    }

    fn GetFallback(&mut self) -> Option<Box<dyn ActionOnExceed>> {
        self.fallback.take()
    }

    fn GetPriority(&self) -> i64 {
        DefRateLimitPriority
    }

    fn SetFinished(&mut self) {
        self.finished = true;
    }

    fn IsFinished(&self) -> bool {
        self.finished
    }
}

/// Read-only observation of the latest logical KV request emitted by a SQL
/// statement. Tests use this instead of mocking SQL results or client state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeReplicaReadRequest {
    pub statement_kind: String,
    pub request_kind: String,
    pub replica_read: String,
    pub txn_scope: String,
    pub store_labels: HashMap<String, String>,
    pub stale_read: bool,
    pub is_retry_request: bool,
}

/// The latest concrete `kv::Request` shape emitted by a relational SELECT.
///
/// Index lookup and index merge own independent requests for every scan
/// branch. `request` is the first branch and `auxiliary_requests` contains the
/// remaining index/table-lookup branches, so tests can verify that no mutable
/// request is shared across concurrent workers.
#[derive(Clone)]
pub struct RuntimeSelectRequest {
    pub access_path: String,
    pub request: Arc<kv::Request>,
    pub auxiliary_requests: Vec<Arc<kv::Request>>,
    pub dispatches: Vec<RuntimeSelectRequestDispatch>,
    pub max_parallel_workers: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeSelectRequestDispatch {
    pub branch: usize,
    pub store_id: u64,
    pub request_address: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RuntimeStaleReadState {
    pub transaction_active: bool,
    pub start_ts: u64,
    pub is_staleness: bool,
    pub txn_read_ts: u64,
    pub pending_read_ts: Option<u64>,
    pub session_read_ts: Option<u64>,
    pub statement_is_stale: bool,
    pub last_statement_was_stale: bool,
    pub snapshot_ts: u64,
    pub snapshot_info_schema_version: Option<i64>,
    pub session_info_schema_version: i64,
    pub txn_info_schema_version: i64,
}
static RUNTIME_DATABASES: LazyLock<Mutex<HashMap<usize, BTreeSet<String>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
#[derive(Clone, Debug, Eq, PartialEq)]
struct RuntimeDatabaseOptions {
    charset: String,
    explicit_collation: Option<String>,
    placement_policy: Option<String>,
}
static RUNTIME_DATABASE_OPTIONS: LazyLock<
    Mutex<HashMap<usize, BTreeMap<String, RuntimeDatabaseOptions>>>,
> = LazyLock::new(|| Mutex::new(HashMap::new()));
#[derive(Clone, Debug, Eq, PartialEq)]
struct RuntimePlacementPolicy {
    name: String,
    settings: String,
}
static RUNTIME_PLACEMENT_POLICIES: LazyLock<
    Mutex<HashMap<u64, BTreeMap<String, RuntimePlacementPolicy>>>,
> = LazyLock::new(|| Mutex::new(HashMap::new()));
static RUNTIME_CREATE_USER_SQL: LazyLock<Mutex<HashMap<u64, BTreeMap<(String, String), String>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static RUNTIME_CLUSTER_CONFIGS: LazyLock<Mutex<HashMap<u64, Result<Vec<Vec<String>>, String>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Install the deterministic SHOW CONFIG provider used by executor parity tests.
pub fn SetShowClusterConfigForTest(domain: &Arc<Domain>, rows: Result<Vec<Vec<String>>, String>) {
    RUNTIME_CLUSTER_CONFIGS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(runtime_domain_id(domain), rows);
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RuntimeResourceGroup {
    ru_per_sec: u64,
    priority: ArbitrationPriority,
}
static RUNTIME_RESOURCE_GROUPS: LazyLock<
    Mutex<HashMap<u64, BTreeMap<String, RuntimeResourceGroup>>>,
> = LazyLock::new(|| Mutex::new(HashMap::new()));
static NEXT_MEMORY_ROOT_UID: AtomicU64 = AtomicU64::new(1);
static NEXT_RUNTIME_DOMAIN_ID: AtomicU64 = AtomicU64::new(1);
static RUNTIME_DOMAIN_IDS: LazyLock<Mutex<HashMap<usize, (Weak<Domain>, u64)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static RUNTIME_PRIVILEGE_HANDLES: LazyLock<
    Mutex<HashMap<u64, astersql_privilege_privileges::Handle>>,
> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// Domain/database/table/index scoped region counts produced by SPLIT REGION.
/// `None` denotes record regions and `Some(index)` denotes index regions.
static RUNTIME_REGION_COUNTS: LazyLock<
    Mutex<HashMap<(u64, String, String, Option<String>), usize>>,
> = LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Clone, Debug, Default)]
struct RuntimeIndexUsageSample {
    last_access_time: Option<SystemTime>,
    query_total: u64,
    kv_req_total: u64,
    row_access_total: u64,
    percentage_access: [u64; 7],
}

static RUNTIME_INDEX_USAGE: LazyLock<Mutex<HashMap<(u64, i64, i64), RuntimeIndexUsageSample>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Return a process-unique identity for one live Domain. Raw Arc addresses can
/// be reused after a test drops its Domain, which must not revive stale split
/// region counts in a later Domain allocated at the same address.
fn runtime_domain_id(domain: &Arc<Domain>) -> u64 {
    let pointer = Arc::as_ptr(domain) as usize;
    let mut identities = RUNTIME_DOMAIN_IDS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    identities.retain(|_, (candidate, _)| candidate.strong_count() != 0);
    if let Some((candidate, id)) = identities.get(&pointer)
        && candidate
            .upgrade()
            .is_some_and(|candidate| Arc::ptr_eq(&candidate, domain))
    {
        return *id;
    }
    let id = NEXT_RUNTIME_DOMAIN_ID.fetch_add(1, Ordering::Relaxed);
    identities.insert(pointer, (Arc::downgrade(domain), id));
    id
}

fn runtime_privilege_handle(domain: &Arc<Domain>) -> astersql_privilege_privileges::Handle {
    let domain_id = runtime_domain_id(domain);
    RUNTIME_PRIVILEGE_HANDLES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .entry(domain_id)
        .or_insert_with(astersql_privilege_privileges::NewHandle)
        .clone()
}

fn account_host(host: &str) -> &str {
    if host.is_empty() { "%" } else { host }
}

fn decode_query_component(component: &str) -> SessionResult<String> {
    let bytes = component.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut offset = 0;
    while offset < bytes.len() {
        match bytes[offset] {
            b'+' => decoded.push(b' '),
            b'%' if offset + 2 < bytes.len() => {
                let hex = |byte: u8| match byte {
                    b'0'..=b'9' => Some(byte - b'0'),
                    b'a'..=b'f' => Some(byte - b'a' + 10),
                    b'A'..=b'F' => Some(byte - b'A' + 10),
                    _ => None,
                };
                let high = hex(bytes[offset + 1])
                    .ok_or_else(|| SessionError::new("invalid IMPORT INTO URI query escape"))?;
                let low = hex(bytes[offset + 2])
                    .ok_or_else(|| SessionError::new("invalid IMPORT INTO URI query escape"))?;
                decoded.push((high << 4) | low);
                offset += 2;
            }
            b'%' => return Err(SessionError::new("invalid IMPORT INTO URI query escape")),
            byte => decoded.push(byte),
        }
        offset += 1;
    }
    String::from_utf8(decoded)
        .map_err(|error| session_error("IMPORT INTO URI query is not UTF-8", error))
}

fn encode_query_component(component: &str) -> String {
    let mut encoded = String::new();
    for byte in component.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(char::from(byte))
            }
            b' ' => encoded.push('+'),
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

fn prepare_import_path_for_kernel(path: &str, is_nextgen: bool) -> SessionResult<String> {
    if !is_nextgen || !astersql_util_sem_compat::IsEnabled() {
        return Ok(path.to_owned());
    }
    let Some((scheme, remainder)) = path.split_once("://") else {
        return Err(SessionError::new("invalid IMPORT INTO URI"));
    };
    if !matches!(scheme.to_ascii_lowercase().as_str(), "s3" | "oss") {
        return Ok(path.to_owned());
    }

    let (base, raw_query) = remainder
        .split_once('?')
        .map_or((remainder, ""), |(base, query)| (base, query));
    let expected_external_id = astersql_config::get_global_keyspace_name();
    let mut has_access_key = false;
    let mut has_secret_access_key = false;
    let mut has_role_arn = false;
    let mut retained_pairs = Vec::new();
    for pair in raw_query.split('&').filter(|pair| !pair.is_empty()) {
        let (raw_key, raw_value) = pair.split_once('=').unwrap_or((pair, ""));
        let normalized_key = decode_query_component(raw_key)?
            .replace('_', "-")
            .to_ascii_lowercase();
        let value = decode_query_component(raw_value)?;
        match normalized_key.as_str() {
            "external-id" => {
                if value != expected_external_id {
                    return Err(SessionError::new(
                        "Feature 'IMPORT INTO with explicit external ID' is not supported when \
                         security enhanced mode is enabled",
                    ));
                }
            }
            "access-key" => {
                has_access_key |= !value.is_empty();
                retained_pairs.push(pair.to_owned());
            }
            "secret-access-key" => {
                has_secret_access_key |= !value.is_empty();
                retained_pairs.push(pair.to_owned());
            }
            "role-arn" => {
                has_role_arn |= !value.is_empty();
                retained_pairs.push(pair.to_owned());
            }
            _ => retained_pairs.push(pair.to_owned()),
        }
    }
    if !has_role_arn && !(has_access_key && has_secret_access_key) {
        return Err(SessionError::new(
            "Feature 'IMPORT INTO from S3-like storage without access key/secret access key or \
             role ARN' is not supported when security enhanced mode is enabled",
        ));
    }

    retained_pairs.push(format!(
        "external-id={}",
        encode_query_component(&expected_external_id)
    ));
    Ok(format!("{scheme}://{base}?{}", retained_pairs.join("&")))
}

#[cfg(test)]
pub(crate) fn ValidateImportPathForKernelForTest(
    path: &str,
    is_nextgen: bool,
) -> SessionResult<()> {
    prepare_import_path_for_kernel(path, is_nextgen).map(|_| ())
}

#[cfg(test)]
pub(crate) fn PrepareImportPathForKernelForTest(
    path: &str,
    is_nextgen: bool,
) -> SessionResult<String> {
    prepare_import_path_for_kernel(path, is_nextgen)
}

struct RuntimeMemArbitrationHelper {
    killer: Arc<SQLKiller>,
    stop_reason: Mutex<Option<ArbitratorStopReason>>,
}

impl ArbitrateHelper for RuntimeMemArbitrationHelper {
    fn Stop(&self, reason: ArbitratorStopReason) -> bool {
        *self
            .stop_reason
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(reason);
        self.killer
            .SendKillSignalWithKillEventReason(KilledByMemArbitrator, reason.String());
        true
    }

    fn HeapInuse(&self) -> i64 {
        0
    }

    fn Finish(&self) {}
}

struct RuntimeMemoryArbitrationGuard {
    arbitrator: Arc<MemArbitrator>,
    uid: u64,
    reserved: i64,
}

impl Drop for RuntimeMemoryArbitrationGuard {
    fn drop(&mut self) {
        let _ = self
            .arbitrator
            .ResetRootPoolByID(self.uid, self.reserved, true);
        let _ = self.arbitrator.RemoveRootPoolByID(self.uid);
    }
}

#[derive(Clone, Debug)]
struct RuntimeStoreNode {
    store_id: u64,
    address: String,
}

static RUNTIME_TOPOLOGIES: LazyLock<Mutex<HashMap<usize, Vec<RuntimeStoreNode>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Register the physical store topology supplied by a TestKit/runtime factory.
/// Query execution and cluster virtual tables consume this shared description;
/// the SQL layer never invents replica addresses or a replica count.
pub fn RegisterRuntimeTopology(domain: &Arc<Domain>, nodes: Vec<(u64, String)>) {
    let nodes = nodes
        .into_iter()
        .filter(|(_, address)| !address.is_empty())
        .map(|(store_id, address)| RuntimeStoreNode { store_id, address })
        .collect::<Vec<_>>();
    RUNTIME_TOPOLOGIES
        .lock()
        .expect("runtime topology map poisoned")
        .insert(Arc::as_ptr(domain) as usize, nodes);
}

#[derive(Clone, Debug)]
struct RuntimeDdlJob {
    id: i64,
    domain: Weak<Domain>,
    domain_id: usize,
    database: String,
    table: String,
    kind: String,
    state: String,
    detail: String,
    concurrency: i64,
    batch_size: i64,
    max_write_speed: i64,
    cancelled: bool,
    row_count: usize,
    scan_attempts: usize,
    import_attempts: usize,
    checkpoint_rows: usize,
    start_ts: u64,
    real_start_ts: u64,
    old_tables: Vec<(String, astersql_meta_model::TableInfo)>,
    table_info: Option<astersql_meta_model::TableInfo>,
}

#[derive(Default)]
struct RuntimeDdlJobs {
    active: BTreeMap<i64, RuntimeDdlJob>,
    history: Vec<RuntimeDdlJob>,
    global_task_history: BTreeMap<usize, Vec<String>>,
}

static NEXT_DDL_JOB_ID: AtomicU64 = AtomicU64::new(1);
static LAST_RUNTIME_DDL_TS: AtomicU64 = AtomicU64::new(0);
static RUNTIME_DDL_JOBS: LazyLock<Mutex<RuntimeDdlJobs>> =
    LazyLock::new(|| Mutex::new(RuntimeDdlJobs::default()));
static RUNTIME_DDL_CHANGED: Condvar = Condvar::new();

fn runtime_ddl_tso() -> u64 {
    let physical = (SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min((u64::MAX >> 18) as u128) as u64)
        << 18;
    let mut previous = LAST_RUNTIME_DDL_TS.load(Ordering::Acquire);
    loop {
        let next = physical.max(previous.saturating_add(1));
        match LAST_RUNTIME_DDL_TS.compare_exchange_weak(
            previous,
            next,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => return next,
            Err(actual) => previous = actual,
        }
    }
}

fn begin_runtime_ddl_job(domain: &Arc<Domain>, database: &str, table: &str, kind: &str) -> i64 {
    let id = NEXT_DDL_JOB_ID.fetch_add(1, Ordering::Relaxed) as i64;
    let domain_id = Arc::as_ptr(domain) as usize;
    let start_ts = runtime_ddl_tso();
    RUNTIME_DDL_JOBS
        .lock()
        .expect("runtime DDL jobs lock poisoned")
        .active
        .insert(
            id,
            RuntimeDdlJob {
                id,
                domain: Arc::downgrade(domain),
                domain_id,
                database: database.to_owned(),
                table: table.to_owned(),
                kind: kind.to_owned(),
                state: "running".to_owned(),
                detail: "running".to_owned(),
                concurrency: 0,
                batch_size: 0,
                max_write_speed: 0,
                cancelled: false,
                row_count: 0,
                scan_attempts: 0,
                import_attempts: 0,
                checkpoint_rows: 0,
                start_ts,
                real_start_ts: 0,
                old_tables: Vec::new(),
                table_info: None,
            },
        );
    // Submission is observable before delivery. Callback failpoints may block
    // here so another session can enqueue an independent job behind this one.
    astersql_testkit_testfailpoint::inject("github.com/pingcap/tidb/pkg/ddl/waitJobSubmitted");
    astersql_testkit_testfailpoint::inject(
        "github.com/pingcap/tidb/pkg/ddl/beforeLoadAndDeliverJobs",
    );

    // Preserve submission order for conflicting jobs. A schema-level job
    // conflicts with every table job in that schema; otherwise independent
    // tables may execute concurrently, matching TiDB's DDL owner queue.
    let jobs = RUNTIME_DDL_JOBS
        .lock()
        .expect("runtime DDL jobs lock poisoned");
    let mut jobs = RUNTIME_DDL_CHANGED
        .wait_while(jobs, |jobs| {
            jobs.active.values().any(|job| {
                job.domain_id == domain_id
                    && job.database.eq_ignore_ascii_case(database)
                    && (job.table.is_empty()
                        || table.is_empty()
                        || job.table.eq_ignore_ascii_case(table))
                    && job.id < id
            })
        })
        .expect("runtime DDL jobs lock poisoned");
    if let Some(job) = jobs.active.get_mut(&id) {
        job.real_start_ts = runtime_ddl_tso();
    }
    id
}

fn attach_runtime_ddl_snapshot(id: i64, old_tables: Vec<(String, astersql_meta_model::TableInfo)>) {
    if let Some(job) = RUNTIME_DDL_JOBS
        .lock()
        .expect("runtime DDL jobs lock poisoned")
        .active
        .get_mut(&id)
    {
        job.old_tables = old_tables;
    }
}

fn attach_runtime_ddl_table_info(id: i64, table: astersql_meta_model::TableInfo) {
    if let Some(job) = RUNTIME_DDL_JOBS
        .lock()
        .expect("runtime DDL jobs lock poisoned")
        .active
        .get_mut(&id)
    {
        job.table_info = Some(table);
    }
}

fn update_runtime_ddl_checkpoint(
    id: i64,
    row_count: usize,
    scan_attempts: usize,
    import_attempts: usize,
    checkpoint_rows: usize,
    detail: &str,
) {
    if let Some(job) = RUNTIME_DDL_JOBS
        .lock()
        .expect("runtime DDL jobs lock poisoned")
        .active
        .get_mut(&id)
    {
        job.row_count = row_count;
        job.scan_attempts = scan_attempts;
        job.import_attempts = import_attempts;
        job.checkpoint_rows = checkpoint_rows;
        job.detail = detail.to_owned();
    }
}

fn update_runtime_ddl_detail(id: i64, detail: &str) {
    if let Some(job) = RUNTIME_DDL_JOBS
        .lock()
        .expect("runtime DDL jobs lock poisoned")
        .active
        .get_mut(&id)
    {
        job.detail = detail.to_owned();
    }
}

fn runtime_ddl_cancelled(id: i64) -> bool {
    RUNTIME_DDL_JOBS
        .lock()
        .expect("runtime DDL jobs lock poisoned")
        .active
        .get(&id)
        .is_some_and(|job| job.cancelled)
}

fn finish_runtime_ddl_job(id: i64, result: &SessionResult<()>) {
    let mut jobs = RUNTIME_DDL_JOBS
        .lock()
        .expect("runtime DDL jobs lock poisoned");
    let Some(mut job) = jobs.active.remove(&id) else {
        return;
    };
    if result.is_ok() {
        job.state = "synced".to_owned();
        if job.detail == "running" {
            job.detail = "done".to_owned();
        }
    } else {
        job.state = if job.cancelled {
            "cancelled".to_owned()
        } else {
            "rollback done".to_owned()
        };
        if job.cancelled {
            job.detail = "cancelled".to_owned();
            let history = jobs.global_task_history.entry(job.domain_id).or_default();
            history.clear();
            history.push("reverted".to_owned());
        }
    }
    jobs.history.push(job);
    drop(jobs);
    RUNTIME_DDL_CHANGED.notify_all();
}

/// Stable test view of the runtime DDL history entry used by serial parity
/// tests that inspect Go `model.Job.StartTS`, `RealStartTS`, and BinlogInfo.
#[derive(Clone, Debug)]
pub struct RuntimeDdlHistoryJob {
    pub id: i64,
    pub database: String,
    pub table: String,
    pub kind: String,
    pub start_ts: u64,
    pub real_start_ts: u64,
    pub table_info: Option<astersql_meta_model::TableInfo>,
}

/// Read one history job belonging to the supplied Domain.
pub fn RuntimeDdlHistoryJobForTest(domain: &Arc<Domain>, id: i64) -> Option<RuntimeDdlHistoryJob> {
    let domain_id = Arc::as_ptr(domain) as usize;
    RUNTIME_DDL_JOBS
        .lock()
        .expect("runtime DDL jobs lock poisoned")
        .history
        .iter()
        .rev()
        .find(|job| job.id == id && job.domain_id == domain_id)
        .map(|job| RuntimeDdlHistoryJob {
            id: job.id,
            database: job.database.clone(),
            table: job.table.clone(),
            kind: job.kind.clone(),
            start_ts: job.start_ts,
            real_start_ts: job.real_start_ts,
            table_info: job.table_info.clone(),
        })
}

/// Ensures an owner panic cannot leave its process-local DDL job active.
///
/// Canonical recovery state lives in Domain storage and is intentionally not
/// touched here: a replacement owner can still resume the persisted job.
struct RuntimeDdlJobGuard {
    id: i64,
    finished: bool,
}

impl RuntimeDdlJobGuard {
    fn new(id: i64) -> Self {
        Self {
            id,
            finished: false,
        }
    }

    fn finish(mut self, result: &SessionResult<()>) {
        finish_runtime_ddl_job(self.id, result);
        self.finished = true;
    }
}

impl Drop for RuntimeDdlJobGuard {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        finish_runtime_ddl_job(
            self.id,
            &Err(SessionError::new("DDL job owner exited before completion")),
        );
    }
}

/// 会话侧计划上下文：绑定会话变量、表达式与统计等待器。
/// 将底层错误包装为 SessionError。
fn session_error(context: &str, error: impl std::fmt::Display) -> SessionError {
    SessionError::new(format!("{context}: {error}"))
}

fn session_kv_error(context: &str, error: astersql_errors::SharedError) -> SessionError {
    SessionError::with_source(format!("{context}: {error}"), error)
}

/// Substitutes `EXECUTE ... USING` arguments into the `?` markers of a
/// prepared statement text, mirroring Go's `ParamMarkerExpr` binding.
/// 将 `?` 参数标记绑定为字面量。
fn variable_is_on(value: &str) -> bool {
    matches!(
        value
            .trim_matches(['\'', '"'])
            .to_ascii_lowercase()
            .as_str(),
        "1" | "on" | "true"
    )
}

/// 解析 SQL 文本为 AST 节点列表。
pub(crate) fn parse(sql: &str) -> SessionResult<Vec<Box<dyn ast::Node>>> {
    let mode =
        astersql_parser_mysql::r#const::GetSQLMode(astersql_parser_mysql::r#const::DefaultSQLMode)
            .map_err(|error| session_error("parse default sql_mode", error))?;
    parse_with_sql_mode(sql, mode)
}

fn parse_with_sql_mode(
    sql: &str,
    mode: astersql_parser_mysql::r#const::SQLMode,
) -> SessionResult<Vec<Box<dyn ast::Node>>> {
    let normalized = sql.trim_start().to_ascii_lowercase();
    if normalized.starts_with("declare ") && normalized.contains(" cursor ") {
        return Err(SessionError::new("unsupported SQL cursor statement"));
    }
    let mut parser = Parser::default();
    parser.SetSQLMode(mode);
    parser
        .ParseSQL(sql, &[])
        .map(|(statements, _warnings)| statements)
        .map_err(|error| {
            SessionError::new(format!(
                "[parser:1064]You have an error in your SQL syntax; {error}"
            ))
        })
}

/// 从 TableRefsClause 取出表名。
fn table_name(table: &ast::TableRefsClause) -> SessionResult<&str> {
    if table.TableRefs.Right.is_some() {
        return Err(SessionError::new(
            "joins are not supported by the session KV executor",
        ));
    }
    match table.TableRefs.Left.as_deref() {
        Some(ast::ResultSetNode::TableSource(source)) if source.QuerySource.is_none() => {
            Ok(source.Source.Name.L.as_str())
        }
        _ => Err(SessionError::new("expected one physical table")),
    }
}

fn split_integer(expr: &ast::ExprNode) -> SessionResult<i64> {
    match &expr.Kind {
        ast::ExprKind::Value(value) => match &value.Datum {
            ast::ValueDatum::Int64(value) => Ok(*value),
            ast::ValueDatum::Uint64(value) => {
                i64::try_from(*value).map_err(|_| SessionError::new("split bound exceeds BIGINT"))
            }
            _ => literal(expr)?
                .parse()
                .map_err(|error| session_error("parse split bound", error)),
        },
        ast::ExprKind::Unary { Op, V } if Op == "-" => split_integer(V)?
            .checked_neg()
            .ok_or_else(|| SessionError::new("split bound overflow")),
        ast::ExprKind::Unary { Op, V } if Op == "+" => split_integer(V),
        ast::ExprKind::Binary { Op, L, R } => {
            let left = split_integer(L)?;
            let right = split_integer(R)?;
            match Op.as_str() {
                "+" => left.checked_add(right),
                "-" => left.checked_sub(right),
                "*" => left.checked_mul(right),
                "/" if right != 0 => left.checked_div(right),
                _ => None,
            }
            .ok_or_else(|| SessionError::new("invalid or overflowing split bound expression"))
        }
        _ => Err(SessionError::new("split bound must be a constant integer")),
    }
}

fn split_datums(
    expressions: &[ast::ExprNode],
) -> SessionResult<Vec<astersql_ddl::index_cop::Datum>> {
    expressions
        .iter()
        .map(|expression| split_integer(expression).map(astersql_ddl::index_cop::Datum::Int))
        .collect()
}

/// Evaluates a table's partition expression against one decoded row, matching
/// Go's `PartitionExpr` evaluation for the shapes this runtime understands: a
/// bare (already lowercased and unquoted) column reference or one of the
/// temporal helpers TiDB uses for `PARTITION BY HASH(...)`.
/// 求值分区表达式。
fn partition_expression_value(
    expression: &str,
    row: &HashMap<String, Option<String>>,
) -> Option<i64> {
    let column = |name: &str| {
        row.get(name)
            .and_then(Option::as_ref)
            .map(|value| value.trim().to_owned())
    };
    if let Some(value) = column(expression) {
        return value.parse::<i64>().ok();
    }
    if let Ok(parsed) = crate::dml_runtime::ParseGeneratedExpr(expression)
        && let Ok(Some(value)) = crate::dml_runtime::EvalExpr(&parsed, row, None)
        && let Ok(value) = value.parse::<i64>()
    {
        return Some(value);
    }
    let (function, argument) = expression.split_once('(')?;
    let argument = argument
        .trim_end()
        .strip_suffix(')')?
        .trim()
        .trim_matches('`');
    let value = column(argument)?;
    let date = value.split_whitespace().next().unwrap_or_default();
    let mut parts = date
        .split('-')
        .map(|part| part.parse::<i64>().unwrap_or_default());
    let (year, month, day) = (
        parts.next().unwrap_or_default(),
        parts.next().unwrap_or_default(),
        parts.next().unwrap_or_default(),
    );
    match function.trim() {
        "year" => Some(year),
        "month" => Some(month),
        "day" | "dayofmonth" => Some(day),
        "to_days" => Some(days_from_civil(year, month, day) + 719_528),
        "abs" => value.parse::<i64>().ok().map(i64::abs),
        _ => None,
    }
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's
/// `days_from_civil`).
/// 公历日期转天数（分区/日期函数辅助）。
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// 将字符串编码为 KV Key。
fn storage_key(value: &str) -> kv::Key {
    let mut key = Vec::with_capacity(SESSION_KV_PREFIX.len() + value.len());
    key.extend_from_slice(SESSION_KV_PREFIX);
    key.extend_from_slice(value.as_bytes());
    kv::Key(key)
}

fn primary_point_get_value(
    table: &astersql_meta_model::TableInfo,
    predicate: Option<&ast::ExprNode>,
) -> Option<String> {
    let primary = table.GetPkColInfo()?.Name.L.as_str();
    let ast::ExprKind::Binary { Op, L, R } = &predicate?.Kind else {
        return None;
    };
    if Op != "=" && Op != "==" {
        return None;
    }
    match (&L.Kind, &R.Kind) {
        (ast::ExprKind::Column(column), _) if column.Name.L == primary => literal(R).ok(),
        (_, ast::ExprKind::Column(column)) if column.Name.L == primary => literal(L).ok(),
        _ => None,
    }
}

/// 简单 WHERE 谓词过滤关系行。
fn row_matches_simple_where(
    row: &HashMap<String, Option<String>>,
    predicate: &ast::ExprNode,
) -> bool {
    if let ast::ExprKind::Parentheses(inner) = &predicate.Kind {
        return row_matches_simple_where(row, inner);
    }
    if let ast::ExprKind::IsNull { Expr, Not } = &predicate.Kind
        && let ast::ExprKind::Column(column) = &Expr.Kind
    {
        let is_null = row.get(&column.Name.L).is_none_or(Option::is_none);
        return if *Not { !is_null } else { is_null };
    }
    if let ast::ExprKind::InList {
        Expr, List, Not, ..
    } = &predicate.Kind
        && let ast::ExprKind::Column(column) = &Expr.Kind
    {
        let actual = row.get(&column.Name.L).and_then(Option::as_ref);
        let matched = actual.is_some_and(|actual| {
            List.iter()
                .any(|candidate| literal(candidate).is_ok_and(|candidate| candidate == *actual))
        });
        return if *Not { !matched } else { matched };
    }
    if let ast::ExprKind::Between {
        Expr,
        Left,
        Right,
        Not,
    } = &predicate.Kind
        && let ast::ExprKind::Column(column) = &Expr.Kind
    {
        let matched = row
            .get(&column.Name.L)
            .and_then(Option::as_ref)
            .is_some_and(|actual| {
                let left = literal(Left).unwrap_or_default();
                let right = literal(Right).unwrap_or_default();
                match (
                    actual.parse::<i128>(),
                    left.parse::<i128>(),
                    right.parse::<i128>(),
                ) {
                    (Ok(actual), Ok(left), Ok(right)) => (left..=right).contains(&actual),
                    _ => actual >= &left && actual <= &right,
                }
            });
        return if *Not { !matched } else { matched };
    }
    if let ast::ExprKind::Like {
        Expr, Pattern, Not, ..
    } = &predicate.Kind
        && let ast::ExprKind::Column(column) = &Expr.Kind
    {
        let pattern = literal(Pattern).unwrap_or_default().to_lowercase();
        let value = row
            .get(&column.Name.L)
            .and_then(Option::as_ref)
            .map(|value| value.to_lowercase())
            .unwrap_or_default();
        let pattern = pattern.as_bytes();
        let value = value.as_bytes();
        let mut current = vec![false; value.len() + 1];
        current[0] = true;
        for token in pattern.iter().copied() {
            let mut next = vec![false; value.len() + 1];
            match token {
                b'%' => {
                    next[0] = current[0];
                    for index in 1..=value.len() {
                        next[index] = current[index] || next[index - 1];
                    }
                }
                b'_' => {
                    for index in 1..=value.len() {
                        next[index] = current[index - 1];
                    }
                }
                literal => {
                    for index in 1..=value.len() {
                        next[index] = current[index - 1] && value[index - 1] == literal;
                    }
                }
            }
            current = next;
        }
        let matched = current[value.len()];
        return if *Not { !matched } else { matched };
    }
    let ast::ExprKind::Binary { Op, L, R } = &predicate.Kind else {
        return true;
    };
    if Op.eq_ignore_ascii_case("and") {
        return row_matches_simple_where(row, L) && row_matches_simple_where(row, R);
    }
    if Op.eq_ignore_ascii_case("or") {
        return row_matches_simple_where(row, L) || row_matches_simple_where(row, R);
    }
    let (column, expected, reverse) = match (&L.Kind, &R.Kind) {
        (ast::ExprKind::Column(column), ast::ExprKind::Value(value)) => {
            (column.Name.L.as_str(), value.text(), false)
        }
        (ast::ExprKind::Value(value), ast::ExprKind::Column(column)) => {
            (column.Name.L.as_str(), value.text(), true)
        }
        (ast::ExprKind::Column(left), ast::ExprKind::Column(right)) => {
            let left = row.get(&left.Name.L).and_then(Option::as_ref);
            let right = row.get(&right.Name.L).and_then(Option::as_ref);
            let equal = left == right;
            return match Op.as_str() {
                "=" | "==" => equal,
                "!=" | "<>" => !equal,
                _ => true,
            };
        }
        _ => return true,
    };
    row.get(column)
        .and_then(Option::as_ref)
        .is_some_and(|actual| {
            if matches!(Op.as_str(), "=" | "==" | "!=" | "<>")
                && actual.parse::<i128>().is_err()
                && expected.parse::<i128>().is_err()
            {
                let equal = actual.eq_ignore_ascii_case(&expected);
                return match Op.as_str() {
                    "=" | "==" => equal,
                    "!=" | "<>" => !equal,
                    _ => true,
                };
            }
            // MySQL integer literals may be unsigned BIGINT values above
            // `i64::MAX`; compare through i128 so YEAR/integer columns keep
            // numeric ordering instead of falling back to lexicographic order.
            let ordering = match (actual.parse::<i128>(), expected.parse::<i128>()) {
                (Ok(actual), Ok(expected)) => actual.cmp(&expected),
                _ => actual.cmp(&expected),
            };
            let ordering = if reverse {
                ordering.reverse()
            } else {
                ordering
            };
            match Op.as_str() {
                "=" | "==" => ordering.is_eq(),
                "!=" | "<>" => !ordering.is_eq(),
                ">" => ordering.is_gt(),
                ">=" => ordering.is_ge(),
                "<" => ordering.is_lt(),
                "<=" => ordering.is_le(),
                _ => true,
            }
        })
}

/// 将 SystemTime 格式化为可读字符串。
fn format_system_time(value: SystemTime) -> String {
    let seconds = value
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let days = seconds.div_euclid(86_400);
    let second_of_day = seconds.rem_euclid(86_400);
    let shifted_days = days + 719_468;
    let era = if shifted_days >= 0 {
        shifted_days
    } else {
        shifted_days - 146_096
    } / 146_097;
    let day_of_era = shifted_days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}",
        second_of_day / 3_600,
        second_of_day / 60 % 60,
        second_of_day % 60
    )
}

/// Increment the process metric for TTL rows that survived the transaction's
/// savepoint semantics.
fn increment_ttl_insert_rows_metric(rows: usize) {
    if rows == 0 {
        return;
    }
    // The application initializes package metrics during server startup.  The
    // guard keeps lightweight unit runtimes safe when that startup is omitted.
    unsafe {
        // Read the Option through a raw pointer to avoid creating a Rust 2024
        // shared reference to the legacy `static mut` metric handle.  The
        // temporary is deliberately forgotten because the static retains the
        // owning handle for the process lifetime.
        let counter = std::ptr::read(std::ptr::addr_of!(
            astersql_metrics::ttl::TTLInsertRowsCount
        ));
        if let Some(counter) = counter {
            counter.inc_by(rows as f64);
            std::mem::forget(counter);
        }
    }
}

/// 解析 SQL 文本为 AST 节点列表。
fn parse_datetime_millis(
    value: &str,
) -> Result<i64, astersql_executor::show_stats::ShowStatsError> {
    parse_datetime_micros(value).map(|micros| micros / 1_000)
}

/// 解析 SQL 文本为 AST 节点列表。
fn parse_datetime_micros(
    value: &str,
) -> Result<i64, astersql_executor::show_stats::ShowStatsError> {
    let invalid = || {
        astersql_executor::show_stats::ShowStatsError::new(
            "column statistics usage timestamp",
            format!("invalid datetime {value:?}"),
        )
    };
    if value.len() != 19 && value.len() != 26 {
        return Err(invalid());
    }
    let component = |range: std::ops::Range<usize>| {
        value
            .get(range)
            .ok_or_else(invalid)?
            .parse::<i64>()
            .map_err(|_| invalid())
    };
    let mut year = component(0..4)?;
    let month = component(5..7)?;
    let day = component(8..10)?;
    let hour = component(11..13)?;
    let minute = component(14..16)?;
    let second = component(17..19)?;
    let leap_year =
        year.rem_euclid(4) == 0 && (year.rem_euclid(100) != 0 || year.rem_euclid(400) == 0);
    let days_in_month = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap_year => 29,
        2 => 28,
        _ => 0,
    };
    if !(1..=12).contains(&month)
        || !(1..=days_in_month).contains(&day)
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || !(0..=59).contains(&second)
    {
        return Err(invalid());
    }
    year -= i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_prime = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * month_prime + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    let fractional_micros = if value.len() == 26 && value.as_bytes()[19] == b'.' {
        component(20..26)?
    } else if value.len() == 19 {
        0
    } else {
        return Err(invalid());
    };
    Ok((days * 86_400 + hour * 3_600 + minute * 60 + second) * 1_000_000 + fractional_micros)
}

#[cfg(test)]
pub(crate) fn ParseDateTimeMicrosForTest(
    value: &str,
) -> Result<i64, astersql_executor::show_stats::ShowStatsError> {
    parse_datetime_micros(value)
}

/// Parse an unqualified SQL datetime in the process/session system timezone.
/// `parse_datetime_micros` intentionally yields the naive UTC-shaped value
/// used by statistics code; stale reads must interpret that wall clock in the
/// session timezone before constructing a TSO.
fn parse_stale_datetime_micros(
    value: &str,
) -> Result<i64, astersql_executor::show_stats::ShowStatsError> {
    let micros = parse_datetime_micros(value)?;
    let offset_seconds = astersql_util_timeutil::time_zone::Zone(
        &astersql_util_timeutil::time_zone::SystemLocation(),
    )
    .1;
    Ok(micros.saturating_sub(offset_seconds.saturating_mul(1_000_000)))
}

/// 带列类型的字面量转为运行时字符串值。
/// Build the MySQL duplicate-primary-key error exposed by relational DML.
fn duplicate_primary_key_error(
    table: &astersql_meta_model::TableInfo,
    row: &HashMap<String, Option<String>>,
) -> SessionError {
    let value = table
        .Columns
        .iter()
        .filter(|column| astersql_parser_mysql::r#type::HasPriKeyFlag(column.GetFlag()))
        .filter_map(|column| row.get(&column.Name.L).cloned().flatten())
        .collect::<Vec<_>>()
        .join("-");
    SessionError::new(format!(
        "[kv:1062]Duplicate entry '{value}' for key '{}.PRIMARY'",
        table.Name.O
    ))
}

fn ensure_session_table(actual: &str) -> SessionResult {
    if actual == SESSION_KV_TABLE {
        Ok(())
    } else {
        Err(SessionError::new(format!(
            "minimal session executor only owns table {SESSION_KV_TABLE}, got {actual}"
        )))
    }
}

/// 从 SELECT 语句提取会话 KV 的 key。
fn select_key(statement: &ast::SelectStmt) -> SessionResult<String> {
    let table = statement
        .From
        .as_ref()
        .ok_or_else(|| SessionError::new("SELECT has no table"))?;
    ensure_session_table(table_name(table)?)?;
    if statement.Fields.Fields.len() != 1 {
        return Err(SessionError::new("expected one projected value column"));
    }
    let field = &statement.Fields.Fields[0];
    let projects_value = field.WildCard.is_some()
        || matches!(field.Expr.as_ref().map(|expr| &expr.Kind),
            Some(ast::ExprKind::Column(column)) if column.Name.L == "v");
    if !projects_value {
        return Err(SessionError::new("expected projection v"));
    }
    let condition = statement
        .Where
        .as_ref()
        .ok_or_else(|| SessionError::new("SELECT requires WHERE k = literal"))?;
    let ast::ExprKind::Binary { Op, L, R } = &condition.Kind else {
        return Err(SessionError::new("SELECT requires WHERE k = literal"));
    };
    if Op != "=" {
        return Err(SessionError::new("SELECT requires equality on k"));
    }
    match (&L.Kind, &R.Kind) {
        (ast::ExprKind::Column(column), _) if column.Name.L == "k" => literal(R),
        (_, ast::ExprKind::Column(column)) if column.Name.L == "k" => literal(L),
        _ => Err(SessionError::new("SELECT requires WHERE k = literal")),
    }
}

mod schema_validation;

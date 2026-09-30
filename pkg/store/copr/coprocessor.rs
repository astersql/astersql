// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// Coprocessor（协处理器）客户端：把 DAG/Analyze/Checksum 等计算下推到 TiKV/TiFlash。
//
// 负责任务切分（按 Region 定位 key range）、大小任务并发控制、分页（paging）、
// 副本读、响应缓存、速率限制（runaway）以及 `CopIterator`/`CopClient` 驱动执行。
// MVCC 快照隔离通过 start_ts 在存储侧过滤可见版本。

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt::{self, Display, Formatter};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::batch_coprocessor::{BatchCopIterator, CopRuntimeStats};
use crate::batch_request_sender::{
    Backoffer, BatchError, BatchResult, CommandType, KeyRange, KeyRanges, Peer, RegionVerId,
};
use crate::coprocessor_cache::{
    CoprocessorCache, CoprocessorCacheRequest, CoprocessorCacheValue, coprocessor_cache_build_key,
};
use crate::ema::RuEma;
use crate::range_diagnostics::{RangeIssueStats, range_issues_for_key_ranges};

/// 构建 Cop 任务时的最大退避。
pub const COP_BUILD_TASK_MAX_BACKOFF: usize = 5_000;
/// 迭代取下一批结果时的最大退避。
pub const COP_NEXT_MAX_BACKOFF: usize = 20_000;
/// 小任务行数提示阈值：hint ≤ 该值视为小任务。
pub const COP_SMALL_TASK_ROW: usize = 32;
/// 小任务并发估算用的波动系数 σ。
pub const SMALL_TASK_SIGMA: f64 = 0.5;
/// 每 CPU 核允许的小任务并发上限因子。
pub const SMALL_CONCURRENCY_PER_CORE: usize = 20;
/// 单个任务合并的 key range 数量上限。
pub const RANGES_PER_TASK: usize = 25_000;
/// 超出边界类错误的最大重试次数。
pub const MAX_EXCEEDS_BOUND_RETRIES: usize = 3;
/// 测试用模拟响应大小（100MiB）。
pub const MOCK_RESPONSE_SIZE_FOR_TEST: usize = 100 * 1024 * 1024;

/// 目标存储引擎类型：TiKV / TiFlash / TiDB。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum StoreType {
    #[default]
    TiKv,
    TiFlash,
    TiDb,
}

/// 协处理器请求类型：DAG 执行、ANALYZE 统计、Checksum。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RequestType {
    #[default]
    Dag,
    Analyze,
    Checksum,
}

/// 副本读类型：Leader / Follower / Mixed。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ReplicaReadType {
    #[default]
    Leader,
    Follower,
    Mixed,
}

/// 请求优先级。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Priority {
    Low,
    #[default]
    Normal,
    High,
}

/// 隔离级别：RC、快照隔离（SI）、带 CheckTs 的 RC。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum IsolationLevel {
    ReadCommitted,
    #[default]
    SnapshotIsolation,
    ReadCommittedCheckTs,
}

/// 分页读选项：是否启用及最小/最大/当前页大小。
#[derive(Clone, Debug, Default)]
pub struct PagingOptions {
    pub enabled: bool,
    pub minimum_size: u64,
    pub maximum_size: u64,
    pub size_bytes: u64,
}

/// 请求来源标记（内部请求与标签）。
#[derive(Clone, Debug, Default)]
pub struct RequestSource {
    pub internal: bool,
    pub label: String,
}

/// 并发请求令牌桶；令牌在一次 store 发送返回（含错误）后立即归还。
#[derive(Debug)]
pub struct RateLimit {
    capacity: usize,
    in_flight: Mutex<usize>,
    available: Condvar,
}

impl RateLimit {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            in_flight: Mutex::new(0),
            available: Condvar::new(),
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    fn acquire(&self, finish: Option<&AtomicBool>) -> Option<RateLimitPermit<'_>> {
        let mut in_flight = self
            .in_flight
            .lock()
            .expect("cop request rate-limit lock poisoned");
        while *in_flight >= self.capacity {
            if finish.is_some_and(|finish| finish.load(Ordering::Acquire)) {
                return None;
            }
            let (guard, _) = self
                .available
                .wait_timeout(in_flight, Duration::from_millis(10))
                .expect("cop request rate-limit lock poisoned");
            in_flight = guard;
        }
        *in_flight += 1;
        Some(RateLimitPermit { limiter: self })
    }

    fn release(&self) {
        let mut in_flight = self
            .in_flight
            .lock()
            .expect("cop request rate-limit lock poisoned");
        *in_flight = in_flight.saturating_sub(1);
        self.available.notify_one();
    }
}

struct RateLimitPermit<'a> {
    limiter: &'a RateLimit,
}

impl Drop for RateLimitPermit<'_> {
    fn drop(&mut self) {
        self.limiter.release();
    }
}

/// Runaway 查询处置动作；CoolDown 会把 iterator 并发压到一个普通 worker。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RunawayAction {
    #[default]
    None,
    DryRun,
    CoolDown,
    Kill,
}

/// Coprocessor 路径使用的 runaway 检查器边界，与 Go 调用顺序保持一致。
pub trait RunawayChecker: fmt::Debug + Send + Sync + 'static {
    fn before_executor(&self) -> BatchResult<()> {
        Ok(())
    }

    fn before_cop_request(&self, _request: &mut CopWireRequest) -> BatchResult<()> {
        Ok(())
    }

    fn check_thresholds(
        &self,
        _ru: Option<&CopRUDetails>,
        _processed_keys: u64,
        _error: Option<&BatchError>,
    ) -> BatchResult<()> {
        Ok(())
    }

    fn reset_total_processed_keys(&self) {}

    fn check_action(&self) -> RunawayAction;
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CopRUDetails {
    pub read_ru: f64,
    pub write_ru: f64,
}

/// TiKV RU v1 calculator used by the production client request path.
#[derive(Debug, Default)]
pub struct ProductionCopRUInterceptor;

impl CopRUInterceptor for ProductionCopRUInterceptor {
    fn on_request_wait(&self, _task: &CopTask, wire: &CopWireRequest) -> BatchResult<CopRUDetails> {
        const READ_BASE_COST: f64 = 1.0 / 8.0;
        const READ_PER_BATCH_BASE_COST: f64 = 1.0 / 2.0;
        const AVERAGE_BATCH_PROPORTION: f64 = 0.7;
        const READ_COST_PER_BYTE: f64 = 1.0 / (64.0 * 1024.0);
        Ok(CopRUDetails {
            read_ru: READ_BASE_COST
                + READ_PER_BATCH_BASE_COST * AVERAGE_BATCH_PROPORTION
                + wire.predicted_read_bytes as f64 * READ_COST_PER_BYTE,
            write_ru: 0.0,
        })
    }

    fn on_response_wait(
        &self,
        _task: &CopTask,
        wire: &CopWireRequest,
        response: &CopProtocolResponse,
    ) -> BatchResult<CopRUDetails> {
        const READ_COST_PER_BYTE: f64 = 1.0 / (64.0 * 1024.0);
        const CPU_MS_COST: f64 = 1.0 / 3.0;
        let child_read_bytes = response
            .batch_responses
            .values()
            .map(|child| child.read_bytes)
            .sum::<u64>();
        let child_cpu_ms = response
            .batch_responses
            .values()
            .map(|child| child.kv_cpu_ms)
            .sum::<f64>();
        Ok(CopRUDetails {
            read_ru: ((response.read_bytes + child_read_bytes) as f64
                - wire.predicted_read_bytes as f64)
                * READ_COST_PER_BYTE
                + (response.kv_cpu_ms + child_cpu_ms) * CPU_MS_COST,
            write_ru: 0.0,
        })
    }
}

/// Client-side resource-control interceptor matching Go's request/response wait hooks.
pub trait CopRUInterceptor: fmt::Debug + Send + Sync + 'static {
    fn on_request_wait(&self, task: &CopTask, wire: &CopWireRequest) -> BatchResult<CopRUDetails>;
    fn on_response_wait(
        &self,
        task: &CopTask,
        wire: &CopWireRequest,
        response: &CopProtocolResponse,
    ) -> BatchResult<CopRUDetails>;
}

/// 分区内的 key ranges 与对应行数提示。
#[derive(Clone, Debug, Default)]
pub struct PartitionKeyRanges {
    pub ranges: Vec<KeyRange>,
    pub row_hints: Vec<usize>,
}

/// 一次协处理器请求的完整描述（类型、引擎、TS、ranges、并发与超时等）。
#[derive(Clone)]
pub struct CopRequest {
    pub read_stats: Option<Arc<tikv_client::ReadStats>>,
    pub request_type: RequestType,
    pub store_type: StoreType,
    pub batch_cop: bool,
    pub start_ts: u64,
    pub data: Vec<u8>,
    pub schema_version: i64,
    pub key_ranges: Vec<PartitionKeyRanges>,
    pub keep_order: bool,
    pub descending: bool,
    pub concurrency: usize,
    pub store_batch_size: usize,
    pub allow_batch_task_data_merge: bool,
    pub execute_batch_tasks_serially: bool,
    pub replica_read: ReplicaReadType,
    pub paging: PagingOptions,
    pub limit_size: u64,
    pub maximum_execution_time: Duration,
    pub tikv_client_read_timeout: Duration,
    pub store_busy_threshold: Duration,
    pub request_source: RequestSource,
    pub priority: Priority,
    pub isolation_level: IsolationLevel,
    pub not_fill_cache: bool,
    pub task_id: u64,
    pub connection_id: u64,
    pub connection_alias: String,
    pub resource_group_name: String,
    pub copr_request_rate_limit: Option<Arc<RateLimit>>,
    pub copr_request_limiter: Option<Arc<astersql_kv::CoprRequestLimiter>>,
    pub query_cop_store_limiter: Option<Arc<astersql_kv::QueryCopStoreLimiter>>,
    pub resolved_locks: Vec<u64>,
    pub committed_locks: Vec<u64>,
    pub runaway_checker: Option<Arc<dyn RunawayChecker>>,
    pub resource_control_interceptor: Option<Arc<dyn CopRUInterceptor>>,
    pub resource_control_ru: Arc<Mutex<CopRUDetails>>,
    pub is_staleness: bool,
    pub maximum_keys_read: u64,
}

impl fmt::Debug for CopRequest {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CopRequest")
            .field("request_type", &self.request_type)
            .field("store_type", &self.store_type)
            .field("store_batch_size", &self.store_batch_size)
            .finish_non_exhaustive()
    }
}

impl Default for CopRequest {
    fn default() -> Self {
        Self {
            read_stats: None,
            request_type: RequestType::Dag,
            store_type: StoreType::TiKv,
            batch_cop: false,
            start_ts: 0,
            data: Vec::new(),
            schema_version: 0,
            key_ranges: Vec::new(),
            keep_order: false,
            descending: false,
            concurrency: 1,
            store_batch_size: 0,
            allow_batch_task_data_merge: false,
            execute_batch_tasks_serially: false,
            replica_read: ReplicaReadType::Leader,
            paging: PagingOptions::default(),
            limit_size: 0,
            maximum_execution_time: Duration::ZERO,
            tikv_client_read_timeout: Duration::ZERO,
            store_busy_threshold: Duration::ZERO,
            request_source: RequestSource::default(),
            priority: Priority::Normal,
            isolation_level: IsolationLevel::SnapshotIsolation,
            not_fill_cache: false,
            task_id: 0,
            connection_id: 0,
            connection_alias: String::new(),
            resource_group_name: String::new(),
            copr_request_rate_limit: None,
            copr_request_limiter: None,
            query_cop_store_limiter: None,
            resolved_locks: Vec::new(),
            committed_locks: Vec::new(),
            runaway_checker: None,
            resource_control_interceptor: None,
            resource_control_ru: Arc::new(Mutex::new(CopRUDetails::default())),
            is_staleness: false,
            maximum_keys_read: 0,
        }
    }
}

/// 已定位到具体 Region 的 key ranges 及 store/peer 信息。
#[derive(Clone, Debug, Default)]
pub struct LocatedKeyRanges {
    pub region: RegionVerId,
    pub ranges: KeyRanges,
    pub location_start: Vec<u8>,
    pub location_end: Vec<u8>,
    pub bucket_version: u64,
    pub store_address: String,
    pub store_id: u64,
    pub peer: Option<Peer>,
}

/// 按 store 批处理包装的 CopTask（可含基于负载的副本重试标记）。
#[derive(Clone, Debug, Default)]
pub struct BatchedCopTask {
    pub task: Box<CopTask>,
    pub store_id: u64,
    pub peer: Option<Peer>,
    pub load_based_replica_retry: bool,
}

/// 单个下推任务：Region、ranges、分页、store 类型与命令等。
#[derive(Clone, Debug)]
pub struct CopTask {
    pub read_stats: Option<Arc<tikv_client::ReadStats>>,
    pub task_id: u64,
    pub region: RegionVerId,
    pub bucket_version: u64,
    pub ranges: KeyRanges,
    pub build_location_start: Vec<u8>,
    pub build_location_end: Vec<u8>,
    pub store_address: String,
    pub command_type: CommandType,
    pub store_type: StoreType,
    pub paging: bool,
    pub paging_size: u64,
    pub paging_task_index: u32,
    pub partition_index: i64,
    pub request_source: RequestSource,
    pub row_count_hint: isize,
    pub batch_task_list: HashMap<u64, BatchedCopTask>,
    pub redirect_to_replica: Option<u64>,
    pub busy_threshold: Duration,
    pub meet_lock_fallback: bool,
    pub client_read_timeout: Duration,
    pub first_read_type: String,
    pub skip_buckets: bool,
    pub exceeds_bound_retry: usize,
}

impl Default for CopTask {
    fn default() -> Self {
        Self {
            read_stats: None,
            task_id: 0,
            region: RegionVerId::default(),
            bucket_version: 0,
            ranges: KeyRanges::default(),
            build_location_start: Vec::new(),
            build_location_end: Vec::new(),
            store_address: String::new(),
            command_type: CommandType::BatchCop,
            store_type: StoreType::TiKv,
            paging: false,
            paging_size: 0,
            paging_task_index: 0,
            partition_index: 0,
            request_source: RequestSource::default(),
            row_count_hint: -1,
            batch_task_list: HashMap::new(),
            redirect_to_replica: None,
            busy_threshold: Duration::ZERO,
            meet_lock_fallback: false,
            client_read_timeout: Duration::ZERO,
            first_read_type: String::new(),
            skip_buckets: false,
            exceeds_bound_retry: 0,
        }
    }
}

impl Display for CopTask {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "region({} {} {}) ranges({}) store({})",
            self.region.id,
            self.region.conf_ver,
            self.region.version,
            self.ranges.len(),
            self.store_address
        )
    }
}

/// 发往 store 的批处理 wire 任务片段。
#[derive(Clone, Debug, Default)]
pub struct StoreBatchWireTask {
    pub region: RegionVerId,
    pub peer: Option<Peer>,
    pub ranges: Vec<KeyRange>,
    pub task_id: u64,
    pub bucket_version: u64,
}

impl CopTask {
    /// 展开为 store 批处理 wire 任务列表。
    pub fn to_pb_batch_tasks(&self) -> Vec<StoreBatchWireTask> {
        self.batch_task_list
            .values()
            .map(|batch| StoreBatchWireTask {
                region: batch.task.region,
                peer: batch.peer.clone(),
                ranges: batch.task.ranges.to_ranges(),
                task_id: batch.task.task_id,
                bucket_version: batch.task.bucket_version,
            })
            .collect()
    }
}

/// 构建 Cop 任务时的附加选项。
#[derive(Clone, Debug, Default)]
pub struct BuildCopTaskOptions {
    pub row_hints: Vec<usize>,
    pub keep_order_response_channel: bool,
    pub ignore_client_read_timeout: bool,
    pub skip_buckets: bool,
    pub exceeds_bound_retry: usize,
}

/// 编码后发往存储节点的协处理器 wire 请求。
#[derive(Clone, Debug, Default)]
pub struct CopWireRequest {
    pub is_staleness: bool,
    pub request_type: RequestType,
    pub start_ts: u64,
    pub data: Vec<u8>,
    pub ranges: Vec<KeyRange>,
    pub schema_version: i64,
    pub paging_size: u64,
    pub maximum_keys_read: u64,
    pub paging_size_bytes: u64,
    pub predicted_read_bytes: u64,
    pub tasks: Vec<StoreBatchWireTask>,
    pub allow_batch_task_data_merge: bool,
    pub execute_batch_tasks_serially: bool,
    pub connection_id: u64,
    pub connection_alias: String,
    pub resource_group_name: String,
    pub max_execution_duration_ms: u64,
    pub priority: Priority,
    pub isolation_level: IsolationLevel,
    pub not_fill_cache: bool,
    pub busy_threshold: Duration,
    pub bucket_version: u64,
    pub replica_read: ReplicaReadType,
    pub read_type: String,
    pub retry_request: bool,
    pub attempt_limiter: Option<Arc<CopRequestAttemptLimiter>>,
    pub resolved_locks: Vec<u64>,
    pub committed_locks: Vec<u64>,
}

/// Admission is evaluated for the actual destination of every RPC attempt.
pub struct CopRequestAttemptLimiter {
    request: Option<Arc<astersql_kv::CoprRequestLimiter>>,
    query: Option<Arc<astersql_kv::QueryCopStoreLimiter>>,
    finish: Option<Arc<AtomicBool>>,
    wait_stats: Option<Arc<Mutex<LimiterWaitStats>>>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LimiterWaitStats {
    pub total_time: Duration,
    pub max_time: Duration,
}

impl LimiterWaitStats {
    pub fn record(&mut self, wait: Duration) {
        self.total_time += wait;
        self.max_time = self.max_time.max(wait);
    }

    pub fn merge(&mut self, other: Self) {
        self.total_time += other.total_time;
        self.max_time = self.max_time.max(other.max_time);
    }

    pub fn is_zero(&self) -> bool {
        self.total_time.is_zero()
    }
}

impl fmt::Debug for CopRequestAttemptLimiter {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CopRequestAttemptLimiter")
            .finish_non_exhaustive()
    }
}

pub struct CopRequestAttemptPermit(Arc<astersql_kv::CoprRequestLimiter>);

impl Drop for CopRequestAttemptPermit {
    fn drop(&mut self) {
        self.0.Release();
    }
}

impl CopRequestAttemptLimiter {
    pub fn new(
        request: Option<Arc<astersql_kv::CoprRequestLimiter>>,
        query: Option<Arc<astersql_kv::QueryCopStoreLimiter>>,
        finish: Option<Arc<AtomicBool>>,
    ) -> Self {
        Self {
            request,
            query,
            finish,
            wait_stats: None,
        }
    }

    fn with_wait_stats(mut self, stats: Arc<Mutex<LimiterWaitStats>>) -> Self {
        self.wait_stats = Some(stats);
        self
    }

    pub fn wait_stats(&self) -> LimiterWaitStats {
        self.wait_stats
            .as_ref()
            .map_or(LimiterWaitStats::default(), |stats| {
                *stats.lock().expect("limiter wait statistics lock poisoned")
            })
    }

    pub fn acquire(&self, store_id: u64) -> BatchResult<Option<CopRequestAttemptPermit>> {
        let limiter = match &self.query {
            Some(query) => query.GetStoreLimiter(store_id),
            None => self.request.clone(),
        };
        let Some(limiter) = limiter else {
            return Ok(None);
        };
        let mut wait_started: Option<Instant> = None;
        loop {
            if self
                .finish
                .as_ref()
                .is_some_and(|finish| finish.load(Ordering::Acquire))
            {
                return Err(BatchError::Cancelled);
            }
            if limiter.TryAcquire() {
                if let (Some(started), Some(stats)) = (wait_started, &self.wait_stats) {
                    stats
                        .lock()
                        .expect("limiter wait statistics lock poisoned")
                        .record(started.elapsed());
                }
                return Ok(Some(CopRequestAttemptPermit(limiter)));
            }
            wait_started.get_or_insert_with(Instant::now);
            thread::sleep(Duration::from_millis(10));
        }
    }
}

/// 存储节点返回的协议层响应。
#[derive(Clone, Debug, Default)]
pub struct CopProtocolResponse {
    pub data: Vec<u8>,
    pub region_error: Option<String>,
    pub locked: Option<Vec<u8>>,
    pub other_error: String,
    pub range: Option<KeyRange>,
    pub latest_bucket_version: u64,
    pub cache_last_version: u64,
    pub can_be_cached: bool,
    pub execution_summary: Vec<u8>,
    pub scanned_keys: u64,
    pub read_bytes: u64,
    pub kv_cpu_ms: f64,
    pub read_pool_task_details: Option<crate::pool_task_details::PoolTaskDetails>,
    pub ru_details: Option<CopRUDetails>,
    pub batch_responses: HashMap<u64, CopProtocolResponse>,
    pub batch_region_errors: HashSet<u64>,
    pub batch_locked: HashSet<u64>,
    pub data_merged_into_response: bool,
}

/// 对上层暴露的协处理器响应（数据、详情、错误）。
#[derive(Clone, Debug, Default)]
pub struct CopResponse {
    pub response: Option<CopProtocolResponse>,
    pub detail: Option<CopRuntimeStats>,
    pub start_key: Vec<u8>,
    pub error: Option<BatchError>,
    pub response_size: usize,
    pub response_time: Duration,
}

impl CopResponse {
    pub fn get_data(&self) -> &[u8] {
        self.response
            .as_ref()
            .map(|response| response.data.as_slice())
            .unwrap_or_default()
    }

    /// 惰性估算响应内存占用。
    pub fn mem_size(&mut self) -> usize {
        if self.response_size == 0 {
            self.response_size = self.start_key.capacity();
            if self.detail.is_some() {
                self.response_size += std::mem::size_of::<CopRuntimeStats>();
            }
            if let Some(response) = &self.response {
                self.response_size += response.data.len() + response.other_error.len();
            }
        }
        self.response_size
    }
}

/// 单个任务执行结果包装。
#[derive(Clone, Debug, Default)]
pub struct CopTaskResult {
    pub response: Option<CopResponse>,
    pub batch_responses: Vec<CopResponse>,
    pub remains: Vec<CopTask>,
}

/// Cop 后端抽象：切分 range、发 RPC、失效 Region、解析锁与可见性检查等。
pub trait CopBackend: Send + Sync + 'static {
    fn split_key_ranges(
        &self,
        ranges: &KeyRanges,
        skip_buckets: bool,
    ) -> BatchResult<Vec<LocatedKeyRanges>>;
    fn build_batch_task(
        &self,
        task: &CopTask,
        replica_read: ReplicaReadType,
    ) -> BatchResult<Option<BatchedCopTask>>;
    fn tidb_server_addresses(&self) -> BatchResult<Vec<(u64, String)>>;
    fn send(&self, task: &CopTask, request: &CopWireRequest) -> BatchResult<CopProtocolResponse>;
    fn invalidate_region(&self, region: RegionVerId);
    fn update_buckets(&self, region: RegionVerId, old_version: u64, new_version: u64);
    fn resolve_lock(&self, lock: &[u8], start_ts: u64) -> BatchResult<()>;
    fn check_visibility(&self, start_ts: u64) -> BatchResult<()>;
    fn build_batch_iterator(&self, _request: &CopRequest) -> BatchResult<BatchCopIterator> {
        Err(BatchError::OtherResponse(
            "batch iterator is not configured".to_owned(),
        ))
    }
    fn send_tiflash_batch(
        &self,
        _request: &CopRequest,
    ) -> BatchResult<Vec<crate::batch_request_sender::BatchResponse>> {
        Err(BatchError::OtherResponse(
            "TiFlash batch transport is unavailable".to_owned(),
        ))
    }
}

/// 确保 key ranges 单调有序；若需修正则原地排序并返回是否改动。
pub fn ensure_monotonic_key_ranges(ranges: &mut KeyRanges) -> bool {
    let stats = range_issues_for_key_ranges(ranges);
    if stats.is_empty() {
        return false;
    }
    let mut sorted = ranges.to_ranges();
    sorted.sort_by(|left, right| {
        left.start
            .cmp(&right.start)
            .then_with(|| left.end.cmp(&right.end))
    });
    ranges.reset(sorted);
    true
}

/// 统计 key ranges 中的异常（重叠、空洞等）诊断信息。
pub fn range_issue_stats(ranges: &KeyRanges) -> RangeIssueStats {
    range_issues_for_key_ranges(ranges)
}

// 按策略增大分页大小，不超过 maximum。
fn grow_paging_size(current: u64, maximum: u64) -> u64 {
    if current == 0 {
        return 0;
    }
    current.saturating_mul(2).min(maximum.max(current))
}

// 按定位后的 ranges 估算/映射行数 hint。
fn row_hint_for_location(original: &KeyRanges, hints: &[usize], location: &KeyRanges) -> isize {
    if hints.len() != original.len() || location.is_empty() {
        return -1;
    }
    let start = &location.ref_at(0).expect("non-empty location").start;
    let end = &location
        .ref_at(location.len() - 1)
        .expect("non-empty location")
        .end;
    original
        .iter()
        .zip(hints)
        .filter(|(range, _)| {
            (end.is_empty() || range.start <= *end) && (range.end.is_empty() || range.end > *start)
        })
        .map(|(_, hint)| *hint as isize)
        .sum()
}

/// 根据 row_count_hint 判断是否为小任务。
pub fn is_small_task(task: &CopTask) -> bool {
    task.row_count_hint > 0
        && if task.batch_task_list.is_empty() {
            task.row_count_hint <= COP_SMALL_TASK_ROW as isize
        } else {
            task.row_count_hint <= (2 * COP_SMALL_TASK_ROW) as isize
        }
}

/// 计算普通并发与小任务并发（第二个返回值）。
pub fn small_task_concurrency(tasks: &[CopTask], cpu_count: usize) -> (usize, usize) {
    let count = tasks.iter().filter(|task| is_small_task(task)).count();
    if count == 0 {
        return (0, 0);
    }
    let extra =
        (count as f64 / (1.0 + SMALL_TASK_SIGMA * (2.0 * (count as f64).ln()).sqrt())) as usize;
    (
        count,
        extra.min(SMALL_CONCURRENCY_PER_CORE * cpu_count.max(1)),
    )
}

// 将任务追加到按 store 分组的批处理列表。
fn append_batched_task(
    tasks: &mut Vec<CopTask>,
    indexes: &mut HashMap<(u64, bool), usize>,
    mut batch: BatchedCopTask,
    limit: usize,
) {
    let key = (batch.store_id, batch.load_based_replica_retry);
    let target = indexes
        .get(&key)
        .copied()
        .filter(|index| tasks[*index].batch_task_list.len() < limit);
    if let Some(index) = target {
        if tasks[index].batch_task_list.is_empty() {
            tasks[index].paging = false;
            tasks[index].paging_size = 0;
            tasks[index].busy_threshold = Duration::ZERO;
        }
        if batch.task.row_count_hint > 0 {
            tasks[index].row_count_hint += batch.task.row_count_hint;
        }
        batch.task.paging = false;
        batch.task.paging_size = 0;
        batch.task.busy_threshold = Duration::ZERO;
        tasks[index]
            .batch_task_list
            .insert(batch.task.task_id, batch);
    } else {
        if batch.load_based_replica_retry {
            batch.task.redirect_to_replica = Some(batch.store_id);
        }
        let index = tasks.len();
        tasks.push(*batch.task);
        indexes.insert(key, index);
    }
}

/// 按 Region 定位切分请求，构建 CopTask 列表（可含 store 批处理）。
pub fn build_cop_tasks(
    backend: &dyn CopBackend,
    request: &CopRequest,
    mut ranges: KeyRanges,
    mut options: BuildCopTaskOptions,
) -> BatchResult<Vec<CopTask>> {
    let reordered = ensure_monotonic_key_ranges(&mut ranges);
    if reordered || options.row_hints.len() != ranges.len() {
        options.row_hints.clear();
    }
    if request.store_type == StoreType::TiDb {
        let mut tasks = Vec::new();
        for (_, address) in backend.tidb_server_addresses()? {
            tasks.push(CopTask {
                ranges: ranges.clone(),
                store_address: address,
                store_type: StoreType::TiDb,
                row_count_hint: -1,
                ..CopTask::default()
            });
        }
        return Ok(tasks);
    }
    let locations = backend.split_key_ranges(&ranges, options.skip_buckets)?;
    let mut tasks = Vec::new();
    let mut batch_indexes = HashMap::new();
    let mut next_task_id = 0u64;
    for location in locations {
        if options.skip_buckets
            && crate::range_diagnostics::first_out_of_bound_key_range_in_location(
                &location.ranges,
                &location.location_start,
                &location.location_end,
            )
            .is_some()
        {
            // Diagnostics are intentionally non-fatal; the bounded response retry
            // path below is the self-healing mechanism.
        }
        let mut paging_size = if request.paging.enabled {
            request.paging.minimum_size
        } else {
            0
        };
        for from in (0..location.ranges.len()).step_by(RANGES_PER_TASK) {
            let to = (from + RANGES_PER_TASK).min(location.ranges.len());
            let task_ranges = location.ranges.slice(from, to);
            let hint = row_hint_for_location(&ranges, &options.row_hints, &task_ranges);
            next_task_id += 1;
            let mut task = CopTask {
                read_stats: request.read_stats.clone(),
                task_id: next_task_id,
                region: location.region,
                bucket_version: location.bucket_version,
                ranges: task_ranges,
                store_address: location.store_address.clone(),
                store_type: request.store_type,
                paging: request.paging.enabled,
                paging_size,
                request_source: request.request_source.clone(),
                row_count_hint: hint,
                busy_threshold: request.store_busy_threshold,
                client_read_timeout: if options.ignore_client_read_timeout {
                    Duration::ZERO
                } else {
                    request.tikv_client_read_timeout
                },
                skip_buckets: options.skip_buckets,
                exceeds_bound_retry: options.exceeds_bound_retry,
                build_location_start: if options.skip_buckets {
                    location.location_start.clone()
                } else {
                    Vec::new()
                },
                build_location_end: if options.skip_buckets {
                    location.location_end.clone()
                } else {
                    Vec::new()
                },
                ..CopTask::default()
            };
            if request.limit_size != 0 && request.limit_size < paging_size {
                task.paging = false;
                task.paging_size = 0;
            } else if request.paging.enabled {
                paging_size = grow_paging_size(paging_size, request.paging.maximum_size);
            }

            let may_batch = request.store_batch_size > 0
                && (!options.row_hints.is_empty() || request.allow_batch_task_data_merge)
                && (request.allow_batch_task_data_merge || is_small_task(&task));
            if may_batch {
                if let Some(batch) = backend.build_batch_task(&task, request.replica_read)? {
                    append_batched_task(
                        &mut tasks,
                        &mut batch_indexes,
                        batch,
                        request.store_batch_size,
                    );
                    continue;
                }
            }
            tasks.push(task);
        }
    }
    if request.descending {
        tasks.reverse();
    }
    Ok(tasks)
}

/// 根据错误与剩余 ranges 计算重试任务。
pub fn calculate_retry(
    ranges: &KeyRanges,
    split: Option<&KeyRange>,
    descending: bool,
) -> KeyRanges {
    let Some(split) = split else {
        return ranges.clone();
    };
    if descending {
        ranges.split(&split.end).0
    } else {
        ranges.split(&split.start).1
    }
}

/// 计算任务尚未完成的剩余 key ranges。
pub fn calculate_remain(
    ranges: &KeyRanges,
    split: Option<&KeyRange>,
    descending: bool,
) -> KeyRanges {
    let Some(split) = split else {
        return ranges.clone();
    };
    if descending {
        ranges.split(&split.start).0
    } else {
        ranges.split(&split.end).1
    }
}

// 由 CopRequest + CopTask 组装 CopWireRequest。
fn build_wire_request(
    request: &CopRequest,
    task: &CopTask,
    remaining_key_budget: u64,
) -> CopWireRequest {
    CopWireRequest {
        is_staleness: request.is_staleness,
        request_type: request.request_type,
        start_ts: request.start_ts,
        data: request.data.clone(),
        ranges: task.ranges.to_pb_ranges(),
        schema_version: request.schema_version,
        paging_size: task.paging_size,
        maximum_keys_read: remaining_key_budget,
        paging_size_bytes: request.paging.size_bytes,
        predicted_read_bytes: 0,
        tasks: task.to_pb_batch_tasks(),
        allow_batch_task_data_merge: request.allow_batch_task_data_merge,
        execute_batch_tasks_serially: request.execute_batch_tasks_serially,
        connection_id: request.connection_id,
        connection_alias: request.connection_alias.clone(),
        resource_group_name: request.resource_group_name.clone(),
        max_execution_duration_ms: request
            .maximum_execution_time
            .as_millis()
            .min(u64::MAX as u128) as u64,
        priority: request.priority,
        isolation_level: request.isolation_level,
        not_fill_cache: request.not_fill_cache,
        busy_threshold: task.busy_threshold,
        bucket_version: task.bucket_version,
        replica_read: if task.redirect_to_replica.is_some() {
            ReplicaReadType::Follower
        } else {
            request.replica_read
        },
        read_type: task.first_read_type.clone(),
        retry_request: !task.first_read_type.is_empty(),
        attempt_limiter: None,
        resolved_locks: Vec::new(),
        committed_locks: Vec::new(),
    }
}

// 批处理路径出错时计算仍需重试的剩余部分。
fn batch_remains_on_error(
    mut task: CopTask,
    mut remains: Vec<CopTask>,
    response: &CopProtocolResponse,
) -> CopTaskResult {
    if task.batch_task_list.is_empty() {
        return CopTaskResult {
            remains,
            ..CopTaskResult::default()
        };
    }
    let batches = std::mem::take(&mut task.batch_task_list);
    let (batch_responses, batch_remains) = handle_batch_responses(response, &batches);
    remains.extend(batch_remains);
    CopTaskResult {
        batch_responses,
        remains,
        ..CopTaskResult::default()
    }
}

// 拆解批处理响应为逐任务 CopResponse。
fn handle_batch_responses(
    response: &CopProtocolResponse,
    batches: &HashMap<u64, BatchedCopTask>,
) -> (Vec<CopResponse>, Vec<CopTask>) {
    let mut responses = Vec::new();
    let mut remains = Vec::new();
    for (id, batch) in batches {
        if response.batch_region_errors.contains(id) || response.batch_locked.contains(id) {
            let mut task = (*batch.task).clone();
            task.meet_lock_fallback = response.batch_locked.contains(id);
            remains.push(task);
        } else if let Some(value) = response.batch_responses.get(id) {
            if value.data_merged_into_response {
                continue;
            }
            responses.push(CopResponse {
                start_key: batch
                    .task
                    .ranges
                    .ref_at(0)
                    .map(|range| range.start.clone())
                    .unwrap_or_default(),
                response: Some(value.clone()),
                detail: Some(CopRuntimeStats {
                    read_pool_task_details: value.read_pool_task_details.clone(),
                    ..CopRuntimeStats::default()
                }),
                ..CopResponse::default()
            });
        } else {
            remains.push((*batch.task).clone());
        }
    }
    (responses, remains)
}

// 构造协处理器响应缓存键。
fn response_cache_key(request: &CopRequest, task: &CopTask) -> BatchResult<Vec<u8>> {
    coprocessor_cache_build_key(&CoprocessorCacheRequest {
        request_type: request.request_type as u64,
        data: request.data.clone(),
        ranges: task.ranges.to_ranges(),
        paging_size: task.paging_size,
        paging_size_bytes: request.paging.size_bytes,
    })
}

fn lock_txn_ids(bytes: &[u8]) -> Vec<u64> {
    let Ok(lock) = protobuf::parse_from_bytes::<kvproto::kvrpcpb::LockInfo>(bytes) else {
        return Vec::new();
    };
    let locks = lock.get_shared_lock_infos();
    if locks.is_empty() {
        vec![lock.get_lock_version()]
    } else {
        locks.iter().map(|lock| lock.get_lock_version()).collect()
    }
}

/// 执行单个 CopTask 的 worker：发送、处理错误、写回迭代器。
pub struct CopTaskWorker {
    backend: Arc<dyn CopBackend>,
    request: Arc<CopRequest>,
    cache: Option<Arc<CoprocessorCache>>,
    keys_read: Arc<AtomicU64>,
    paging_task_index: Arc<AtomicU32>,
    finish: Option<Arc<AtomicBool>>,
    ema: Arc<RuEma>,
    limiter_wait: Arc<Mutex<LimiterWaitStats>>,
    resolved_locks: Arc<Mutex<HashSet<u64>>>,
    committed_locks: Arc<Mutex<HashSet<u64>>>,
    runtime_stats: Arc<Mutex<Vec<CopRuntimeStats>>>,
    store_batched_num: Arc<AtomicU64>,
    store_batched_fallback_num: Arc<AtomicU64>,
}

impl CopTaskWorker {
    pub fn new(
        backend: Arc<dyn CopBackend>,
        request: Arc<CopRequest>,
        cache: Option<Arc<CoprocessorCache>>,
        keys_read: Arc<AtomicU64>,
        paging_task_index: Arc<AtomicU32>,
    ) -> Self {
        let ema = Arc::new(RuEma::new(request.paging.size_bytes));
        let resolved_locks = Arc::new(Mutex::new(request.resolved_locks.iter().copied().collect()));
        let committed_locks = Arc::new(Mutex::new(
            request.committed_locks.iter().copied().collect(),
        ));
        Self {
            backend,
            request,
            cache,
            keys_read,
            paging_task_index,
            finish: None,
            ema,
            limiter_wait: Arc::new(Mutex::new(LimiterWaitStats::default())),
            resolved_locks,
            committed_locks,
            runtime_stats: Arc::new(Mutex::new(Vec::new())),
            store_batched_num: Arc::new(AtomicU64::new(0)),
            store_batched_fallback_num: Arc::new(AtomicU64::new(0)),
        }
    }

    fn with_finish(mut self, finish: Arc<AtomicBool>) -> Self {
        self.finish = Some(finish);
        self
    }

    fn with_ema(mut self, ema: Arc<RuEma>) -> Self {
        self.ema = ema;
        self
    }

    fn with_limiter_wait(mut self, stats: Arc<Mutex<LimiterWaitStats>>) -> Self {
        self.limiter_wait = stats;
        self
    }

    fn with_lock_sets(
        mut self,
        resolved: Arc<Mutex<HashSet<u64>>>,
        committed: Arc<Mutex<HashSet<u64>>>,
    ) -> Self {
        self.resolved_locks = resolved;
        self.committed_locks = committed;
        self
    }

    fn with_runtime_stats(mut self, stats: Arc<Mutex<Vec<CopRuntimeStats>>>) -> Self {
        self.runtime_stats = stats;
        self
    }

    fn with_store_batch_stats(mut self, batched: Arc<AtomicU64>, fallback: Arc<AtomicU64>) -> Self {
        self.store_batched_num = batched;
        self.store_batched_fallback_num = fallback;
        self
    }

    pub fn store_batch_stats(&self) -> (u64, u64) {
        (
            self.store_batched_num.load(Ordering::Acquire),
            self.store_batched_fallback_num.load(Ordering::Acquire),
        )
    }

    pub fn runtime_stats(&self) -> Vec<CopRuntimeStats> {
        self.runtime_stats
            .lock()
            .expect("cop runtime stats lock poisoned")
            .clone()
    }

    fn rebuild(
        &self,
        task: &CopTask,
        skip_buckets: bool,
        retry: usize,
    ) -> BatchResult<Vec<CopTask>> {
        build_cop_tasks(
            self.backend.as_ref(),
            &self.request,
            task.ranges.clone(),
            BuildCopTaskOptions {
                ignore_client_read_timeout: true,
                skip_buckets,
                exceeds_bound_retry: retry,
                ..BuildCopTaskOptions::default()
            },
        )
    }

    fn rebuild_whole_store_batch(&self, task: &CopTask) -> BatchResult<Vec<CopTask>> {
        let mut ranges = task.ranges.to_ranges();
        for child in task.batch_task_list.values() {
            ranges.extend(child.task.ranges.to_ranges());
        }
        ranges.sort_by(|a, b| a.start.cmp(&b.start).then_with(|| a.end.cmp(&b.end)));
        let mut request = (*self.request).clone();
        request.store_batch_size = task.batch_task_list.len();
        build_cop_tasks(
            self.backend.as_ref(),
            &request,
            KeyRanges::new(ranges),
            BuildCopTaskOptions {
                ignore_client_read_timeout: true,
                ..BuildCopTaskOptions::default()
            },
        )
    }

    fn rebuild_failed_batch_children(
        &self,
        backoffer: &mut Backoffer,
        response: &CopProtocolResponse,
        remains: Vec<CopTask>,
    ) -> BatchResult<Vec<CopTask>> {
        let mut rebuilt = Vec::new();
        for child in remains {
            if response.batch_region_errors.contains(&child.task_id) {
                self.backend.invalidate_region(child.region);
                backoffer.backoff(&BatchError::Transport(
                    response
                        .batch_responses
                        .get(&child.task_id)
                        .and_then(|response| response.region_error.clone())
                        .unwrap_or_else(|| "batch child region error".to_owned()),
                ))?;
                rebuilt.extend(self.rebuild(&child, false, child.exceeds_bound_retry)?);
            } else {
                rebuilt.push(child);
            }
        }
        Ok(rebuilt)
    }

    fn resolve_response_lock(
        &self,
        backoffer: &mut Backoffer,
        wire: &CopWireRequest,
        lock: &[u8],
        backed_off_for_hint: &mut bool,
    ) -> BatchResult<()> {
        let ids = lock_txn_ids(lock);
        if !*backed_off_for_hint
            && ids
                .iter()
                .any(|id| wire.resolved_locks.contains(id) || wire.committed_locks.contains(id))
        {
            backoffer.backoff(&BatchError::OtherResponse(
                "lock was reported despite being included in request hints".to_owned(),
            ))?;
            *backed_off_for_hint = true;
        }
        self.backend.resolve_lock(lock, self.request.start_ts)?;
        self.resolved_locks
            .lock()
            .expect("resolved locks lock poisoned")
            .extend(ids.into_iter().filter(|id| *id != 0));
        Ok(())
    }

    fn resolve_batch_locks(
        &self,
        backoffer: &mut Backoffer,
        wire: &CopWireRequest,
        response: &mut CopProtocolResponse,
        backed_off_for_hint: &mut bool,
    ) -> BatchResult<()> {
        for (task_id, child) in &response.batch_responses {
            if let Some(lock) = &child.locked {
                self.resolve_response_lock(backoffer, wire, lock, backed_off_for_hint)?;
                response.batch_locked.insert(*task_id);
            }
        }
        Ok(())
    }

    /// 执行任务至多一次完整发送-处理循环（含缓存与错误重试分支）。
    pub fn handle_task_once(
        &self,
        backoffer: &mut Backoffer,
        mut task: CopTask,
    ) -> BatchResult<CopTaskResult> {
        if task.paging || self.request.paging.size_bytes > 0 {
            task.paging_task_index = self.paging_task_index.fetch_add(1, Ordering::AcqRel) + 1;
        }
        let remaining_budget = if self.request.maximum_keys_read == 0 {
            0
        } else {
            let read = self.keys_read.load(Ordering::Acquire);
            if read >= self.request.maximum_keys_read {
                return Err(BatchError::OtherResponse(
                    "maximum keys read exceeded".to_owned(),
                ));
            }
            self.request.maximum_keys_read - read
        };
        let mut wire = build_wire_request(&self.request, &task, remaining_budget);
        wire.resolved_locks = self
            .resolved_locks
            .lock()
            .expect("resolved locks lock poisoned")
            .iter()
            .copied()
            .collect();
        wire.committed_locks = self
            .committed_locks
            .lock()
            .expect("committed locks lock poisoned")
            .iter()
            .copied()
            .collect();
        wire.resolved_locks.sort_unstable();
        wire.committed_locks.sort_unstable();
        if task.paging || self.request.paging.size_bytes > 0 {
            wire.predicted_read_bytes = self.ema.predict();
        }
        if self.request.copr_request_limiter.is_some()
            || self.request.query_cop_store_limiter.is_some()
        {
            wire.attempt_limiter = Some(Arc::new(
                CopRequestAttemptLimiter::new(
                    self.request.copr_request_limiter.clone(),
                    self.request.query_cop_store_limiter.clone(),
                    self.finish.clone(),
                )
                .with_wait_stats(Arc::clone(&self.limiter_wait)),
            ));
        }
        let cache_key = response_cache_key(&self.request, &task)?;
        if let Some(cache) = &self.cache
            && cache.check_request_admission(task.ranges.len())
            && let Some(value) = cache.get(&cache_key)
        {
            return Ok(CopTaskResult {
                response: Some(CopResponse {
                    response: Some(CopProtocolResponse {
                        data: value.data,
                        ..CopProtocolResponse::default()
                    }),
                    detail: Some(CopRuntimeStats::default()),
                    start_key: value.page_start,
                    ..CopResponse::default()
                }),
                ..CopTaskResult::default()
            });
        }
        if let Some(checker) = &self.request.runaway_checker {
            checker.before_cop_request(&mut wire)?;
        }
        let resource_control = self
            .request
            .resource_control_interceptor
            .as_ref()
            .filter(|_| !wire.resource_group_name.is_empty());
        if let Some(interceptor) = resource_control {
            let delta = interceptor.on_request_wait(&task, &wire)?;
            let mut details = self
                .request
                .resource_control_ru
                .lock()
                .expect("resource control RU lock poisoned");
            details.read_ru += delta.read_ru;
            details.write_ru += delta.write_ru;
        }
        let started = Instant::now();
        let request_permit = match self.request.copr_request_rate_limit.as_ref() {
            Some(limiter) => Some(
                limiter
                    .acquire(self.finish.as_deref())
                    .ok_or(BatchError::Cancelled)?,
            ),
            None => None,
        };
        let send_result = self.backend.send(&task, &wire);
        drop(request_permit);
        let mut response = match send_result {
            Ok(response) => response,
            Err(error) => {
                if matches!(error, BatchError::MissingRegion(_))
                    && self.request.allow_batch_task_data_merge
                    && !task.batch_task_list.is_empty()
                {
                    backoffer.backoff(&error)?;
                    return Ok(CopTaskResult {
                        remains: self.rebuild_whole_store_batch(&task)?,
                        ..CopTaskResult::default()
                    });
                }
                if let Some(checker) = &self.request.runaway_checker {
                    let accumulated_ru = resource_control.map(|_| {
                        *self
                            .request
                            .resource_control_ru
                            .lock()
                            .expect("resource control RU lock poisoned")
                    });
                    checker.check_thresholds(accumulated_ru.as_ref(), 0, Some(&error))?;
                }
                return Err(error);
            }
        };
        if let Some(interceptor) = resource_control {
            let delta = interceptor.on_response_wait(&task, &wire, &response)?;
            let mut details = self
                .request
                .resource_control_ru
                .lock()
                .expect("resource control RU lock poisoned");
            details.read_ru += delta.read_ru;
            details.write_ru += delta.write_ru;
        }
        let accumulated_ru = resource_control.map(|_| {
            *self
                .request
                .resource_control_ru
                .lock()
                .expect("resource control RU lock poisoned")
        });
        let response_error = response
            .region_error
            .as_ref()
            .map(|error| BatchError::Transport(error.clone()))
            .or_else(|| {
                (!response.other_error.is_empty())
                    .then(|| BatchError::OtherResponse(response.other_error.clone()))
            });
        if let Some(checker) = &self.request.runaway_checker {
            checker.check_thresholds(
                accumulated_ru.as_ref().or(response.ru_details.as_ref()),
                response.scanned_keys,
                response_error.as_ref(),
            )?;
            for task_id in task.batch_task_list.keys() {
                if let Some(child) = response.batch_responses.get(task_id) {
                    let child_error = child
                        .region_error
                        .as_ref()
                        .map(|error| BatchError::Transport(error.clone()))
                        .or_else(|| {
                            (!child.other_error.is_empty())
                                .then(|| BatchError::OtherResponse(child.other_error.clone()))
                        });
                    checker.check_thresholds(
                        accumulated_ru.as_ref().or(child.ru_details.as_ref()),
                        child.scanned_keys,
                        child_error.as_ref(),
                    )?;
                }
            }
        }
        let elapsed = started.elapsed();
        if response.latest_bucket_version > task.bucket_version {
            self.backend.update_buckets(
                task.region,
                task.bucket_version,
                response.latest_bucket_version,
            );
        }
        // Store-batch responses carry an independent bucket version for every
        // child task. The Go path updates those child regions before splitting
        // the batch response; otherwise stale bucket metadata survives even
        // though TiKV returned a newer version for the child.
        for (task_id, batch) in &task.batch_task_list {
            if let Some(child_response) = response.batch_responses.get(task_id)
                && child_response.latest_bucket_version > batch.task.bucket_version
            {
                self.backend.update_buckets(
                    batch.task.region,
                    batch.task.bucket_version,
                    child_response.latest_bucket_version,
                );
            }
        }
        for child in response.batch_responses.values() {
            if child.data_merged_into_response {
                self.runtime_stats
                    .lock()
                    .expect("cop runtime stats lock poisoned")
                    .push(CopRuntimeStats {
                        read_pool_task_details: child.read_pool_task_details.clone(),
                        ..CopRuntimeStats::default()
                    });
            }
        }
        for (task_id, child) in &response.batch_responses {
            if !task.batch_task_list.contains_key(task_id) {
                return Err(BatchError::OtherResponse(format!(
                    "batch task id {task_id} not found"
                )));
            }
            if !child.other_error.is_empty() {
                return Err(BatchError::OtherResponse(
                    if child.other_error.contains("write conflict") {
                        format!("write conflict: {}", child.other_error)
                    } else {
                        format!("other error: {}", child.other_error)
                    },
                ));
            }
        }
        let mut backed_off_for_hint = false;
        if response.region_error.is_none() {
            if let Some(lock) = &response.locked {
                self.resolve_response_lock(backoffer, &wire, lock, &mut backed_off_for_hint)?;
            }
        }
        self.resolve_batch_locks(backoffer, &wire, &mut response, &mut backed_off_for_hint)?;
        if !task.batch_task_list.is_empty()
            && !response.region_error.as_deref().is_some_and(|error| {
                !task.busy_threshold.is_zero()
                    && error.to_ascii_lowercase().contains("server is busy")
            })
        {
            let fallback = task
                .batch_task_list
                .keys()
                .filter(|id| {
                    response.batch_region_errors.contains(id)
                        || response.batch_locked.contains(id)
                        || !response.batch_responses.contains_key(id)
                })
                .count() as u64;
            self.store_batched_num.fetch_add(
                task.batch_task_list.len() as u64 - fallback,
                Ordering::AcqRel,
            );
            self.store_batched_fallback_num
                .fetch_add(fallback, Ordering::AcqRel);
        }
        if let Some(region_error) = &response.region_error {
            backoffer.backoff(&BatchError::Transport(region_error.clone()))?;
            let remains = self.rebuild(&task, false, task.exceeds_bound_retry)?;
            let mut result = batch_remains_on_error(task, remains, &response);
            result.remains =
                self.rebuild_failed_batch_children(backoffer, &response, result.remains)?;
            return Ok(result);
        }
        if response.locked.is_some() {
            task.meet_lock_fallback = true;
            let mut result = batch_remains_on_error(task.clone(), vec![task], &response);
            result.remains =
                self.rebuild_failed_batch_children(backoffer, &response, result.remains)?;
            return Ok(result);
        }
        if !response.other_error.is_empty() {
            if response.other_error.contains("Request range exceeds bound") {
                if task.skip_buckets && task.exceeds_bound_retry >= MAX_EXCEEDS_BOUND_RETRIES {
                    return Err(BatchError::OtherResponse(format!(
                        "request range exceeds bound persists after bucket-less retry and exceeded retry budget ({}/{}): {}",
                        task.exceeds_bound_retry, MAX_EXCEEDS_BOUND_RETRIES, response.other_error
                    )));
                }
                self.backend.invalidate_region(task.region);
                backoffer.backoff(&BatchError::Transport(response.other_error.clone()))?;
                let retry = task.exceeds_bound_retry + 1;
                let remains = self.rebuild(&task, true, retry)?;
                let mut result = batch_remains_on_error(task, remains, &response);
                result.remains =
                    self.rebuild_failed_batch_children(backoffer, &response, result.remains)?;
                return Ok(result);
            }
            if response.other_error.contains("write conflict") {
                return Err(BatchError::OtherResponse(format!(
                    "write conflict: {}",
                    response.other_error
                )));
            }
            return Err(BatchError::OtherResponse(response.other_error));
        }

        self.keys_read
            .fetch_add(response.scanned_keys, Ordering::AcqRel);
        let start_key = response
            .range
            .as_ref()
            .map(|range| range.start.clone())
            .or_else(|| task.ranges.ref_at(0).map(|range| range.start.clone()))
            .unwrap_or_default();
        if let Some(cache) = &self.cache
            && response.can_be_cached
            && cache.check_response_admission(response.data.len(), elapsed, task.paging_task_index)
        {
            cache.set(
                cache_key.clone(),
                CoprocessorCacheValue {
                    key: cache_key,
                    data: response.data.clone(),
                    timestamp: self.request.start_ts,
                    region_id: task.region.id,
                    region_data_version: response.cache_last_version,
                    page_start: start_key.clone(),
                    page_end: response
                        .range
                        .as_ref()
                        .map(|range| range.end.clone())
                        .unwrap_or_default(),
                },
            );
        }
        let (batch_responses, batch_remains) =
            handle_batch_responses(&response, &task.batch_task_list);
        let mut batch_remains =
            self.rebuild_failed_batch_children(backoffer, &response, batch_remains)?;
        if response.range.is_some() && response.read_bytes > 0 {
            self.ema.observe(response.read_bytes, Instant::now());
        }
        if task.paging || response.range.is_some() {
            let remaining = calculate_remain(
                &task.ranges,
                response.range.as_ref(),
                self.request.descending,
            );
            if !remaining.is_empty() {
                task.ranges = remaining;
                task.paging_size =
                    grow_paging_size(task.paging_size, self.request.paging.maximum_size);
                batch_remains.push(task.clone());
            }
        }
        Ok(CopTaskResult {
            response: Some(CopResponse {
                detail: Some(CopRuntimeStats {
                    read_pool_task_details: response.read_pool_task_details.clone(),
                    ..CopRuntimeStats::default()
                }),
                response: Some(response),
                start_key,
                response_time: elapsed,
                ..CopResponse::default()
            }),
            batch_responses,
            remains: batch_remains,
        })
    }
}

enum IteratorMessage {
    Response(usize, CopResponse),
    Done(usize),
}

fn send_iterator_message(
    sender: &mpsc::SyncSender<IteratorMessage>,
    finish: &AtomicBool,
    mut message: IteratorMessage,
) -> bool {
    loop {
        if finish.load(Ordering::Acquire) {
            return false;
        }
        match sender.try_send(message) {
            Ok(()) => return true,
            Err(mpsc::TrySendError::Full(value)) => {
                message = value;
                thread::yield_now();
            }
            Err(mpsc::TrySendError::Disconnected(_)) => return false,
        }
    }
}

struct LiteCopIteratorWorker {
    worker: CopTaskWorker,
    batch_responses: VecDeque<CopResponse>,
    try_state: Arc<AtomicU32>,
    backoffer: Backoffer,
}

struct RunawayWorkerCompletion {
    checker: Option<Arc<dyn RunawayChecker>>,
    remaining: Arc<AtomicUsize>,
}

impl Drop for RunawayWorkerCompletion {
    fn drop(&mut self) {
        if self.remaining.fetch_sub(1, Ordering::AcqRel) == 1
            && let Some(checker) = &self.checker
        {
            checker.reset_total_processed_keys();
        }
    }
}

/// 协处理器结果迭代器：管理大小任务 worker 池与速率限制。
pub struct CopIterator {
    backend: Arc<dyn CopBackend>,
    request: Arc<CopRequest>,
    cache: Option<Arc<CoprocessorCache>>,
    tasks: Vec<CopTask>,
    concurrency: usize,
    small_task_concurrency: usize,
    finish: Arc<AtomicBool>,
    killed: Arc<AtomicU32>,
    keys_read: Arc<AtomicU64>,
    paging_task_index: Arc<AtomicU32>,
    ema: Arc<RuEma>,
    limiter_wait: Arc<Mutex<LimiterWaitStats>>,
    resolved_locks: Arc<Mutex<HashSet<u64>>>,
    committed_locks: Arc<Mutex<HashSet<u64>>>,
    runtime_stats: Arc<Mutex<Vec<CopRuntimeStats>>>,
    store_batched_num: Arc<AtomicU64>,
    store_batched_fallback_num: Arc<AtomicU64>,
    receiver: Option<mpsc::Receiver<IteratorMessage>>,
    workers: Vec<JoinHandle<()>>,
    ordered_buffer: HashMap<usize, VecDeque<CopResponse>>,
    ordered_done: HashSet<usize>,
    next_ordered_task: usize,
    started: bool,
    lite_worker: Option<LiteCopIteratorWorker>,
    pub build_task_elapsed: Duration,
    send_rate: Arc<RateLimit>,
    rate_limit_action: Arc<RateLimitAction>,
}

impl CopIterator {
    fn new(
        backend: Arc<dyn CopBackend>,
        request: Arc<CopRequest>,
        cache: Option<Arc<CoprocessorCache>>,
        tasks: Vec<CopTask>,
        concurrency: usize,
        small_task_concurrency: usize,
        build_task_elapsed: Duration,
    ) -> Self {
        let worker_capacity = (concurrency + small_task_concurrency).max(1);
        let send_capacity = if request.keep_order {
            2 * worker_capacity
        } else {
            worker_capacity
        };
        let ema = Arc::new(RuEma::new(request.paging.size_bytes));
        let resolved_locks = Arc::new(Mutex::new(request.resolved_locks.iter().copied().collect()));
        let committed_locks = Arc::new(Mutex::new(
            request.committed_locks.iter().copied().collect(),
        ));
        Self {
            backend,
            request,
            cache,
            tasks,
            concurrency,
            small_task_concurrency,
            finish: Arc::new(AtomicBool::new(false)),
            killed: Arc::new(AtomicU32::new(0)),
            keys_read: Arc::new(AtomicU64::new(0)),
            paging_task_index: Arc::new(AtomicU32::new(0)),
            ema,
            limiter_wait: Arc::new(Mutex::new(LimiterWaitStats::default())),
            resolved_locks,
            committed_locks,
            runtime_stats: Arc::new(Mutex::new(Vec::new())),
            store_batched_num: Arc::new(AtomicU64::new(0)),
            store_batched_fallback_num: Arc::new(AtomicU64::new(0)),
            receiver: None,
            workers: Vec::new(),
            ordered_buffer: HashMap::new(),
            ordered_done: HashSet::new(),
            next_ordered_task: 0,
            started: false,
            lite_worker: None,
            build_task_elapsed,
            send_rate: Arc::new(RateLimit::new(send_capacity)),
            rate_limit_action: Arc::new(RateLimitAction::new(send_capacity)),
        }
    }

    pub fn concurrency(&self) -> (usize, usize) {
        (self.concurrency, self.small_task_concurrency)
    }

    pub fn killed_signal(&self) -> Arc<AtomicU32> {
        Arc::clone(&self.killed)
    }

    pub fn tasks(&self) -> &[CopTask] {
        &self.tasks
    }

    pub fn send_rate(&self) -> &Arc<RateLimit> {
        &self.send_rate
    }

    pub fn request_rate_limit(&self) -> Option<&Arc<RateLimit>> {
        self.request.copr_request_rate_limit.as_ref()
    }

    pub fn request_limiter(&self) -> Option<&Arc<astersql_kv::CoprRequestLimiter>> {
        self.request.copr_request_limiter.as_ref()
    }

    pub fn limiter_wait_stats(&self) -> LimiterWaitStats {
        *self
            .limiter_wait
            .lock()
            .expect("limiter wait statistics lock poisoned")
    }

    pub fn runtime_stats(&self) -> Vec<CopRuntimeStats> {
        self.runtime_stats
            .lock()
            .expect("cop runtime stats lock poisoned")
            .clone()
    }

    pub fn store_batch_stats(&self) -> (u64, u64) {
        (
            self.store_batched_num.load(Ordering::Acquire),
            self.store_batched_fallback_num.load(Ordering::Acquire),
        )
    }

    /// 启动 worker，开始并行执行任务。
    pub fn open(&mut self) {
        if self.started {
            return;
        }
        self.started = true;
        self.open_concurrent();
    }

    /// Go `TryCopLiteWorker` 对应入口：仅首个单任务 iterator 可同步执行。
    pub fn open_with_lite_worker(&mut self, try_state: Arc<AtomicU32>) {
        if self.started {
            return;
        }
        self.started = true;
        if self.tasks.len() == 1
            && try_state
                .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        {
            self.lite_worker = Some(LiteCopIteratorWorker {
                worker: CopTaskWorker::new(
                    Arc::clone(&self.backend),
                    Arc::clone(&self.request),
                    self.cache.clone(),
                    Arc::clone(&self.keys_read),
                    Arc::clone(&self.paging_task_index),
                )
                .with_ema(Arc::clone(&self.ema))
                .with_limiter_wait(Arc::clone(&self.limiter_wait))
                .with_lock_sets(
                    Arc::clone(&self.resolved_locks),
                    Arc::clone(&self.committed_locks),
                )
                .with_runtime_stats(Arc::clone(&self.runtime_stats))
                .with_store_batch_stats(
                    Arc::clone(&self.store_batched_num),
                    Arc::clone(&self.store_batched_fallback_num),
                )
                .with_finish(Arc::clone(&self.finish)),
                batch_responses: VecDeque::new(),
                try_state,
                backoffer: Backoffer::new(COP_NEXT_MAX_BACKOFF),
            });
            return;
        }
        self.open_concurrent();
    }

    fn open_concurrent(&mut self) {
        let task_count = self.tasks.len();
        let queue = Arc::new((
            Mutex::new(
                std::mem::take(&mut self.tasks)
                    .into_iter()
                    .enumerate()
                    .collect::<VecDeque<_>>(),
            ),
            Condvar::new(),
        ));
        let channel_size = if self.request.keep_order {
            2 * (self.concurrency + self.small_task_concurrency).max(1)
        } else {
            (self.concurrency + self.small_task_concurrency).max(1)
        };
        let (sender, receiver) = mpsc::sync_channel(channel_size);
        self.receiver = Some(receiver);
        let workers = (self.concurrency + self.small_task_concurrency)
            .max(1)
            .min(task_count.max(1));
        let remaining_workers = Arc::new(AtomicUsize::new(workers));
        for _ in 0..workers {
            let backend = Arc::clone(&self.backend);
            let request = Arc::clone(&self.request);
            let cache = self.cache.clone();
            let keys_read = Arc::clone(&self.keys_read);
            let paging_index = Arc::clone(&self.paging_task_index);
            let ema = Arc::clone(&self.ema);
            let limiter_wait = Arc::clone(&self.limiter_wait);
            let resolved_locks = Arc::clone(&self.resolved_locks);
            let committed_locks = Arc::clone(&self.committed_locks);
            let runtime_stats = Arc::clone(&self.runtime_stats);
            let store_batched_num = Arc::clone(&self.store_batched_num);
            let store_batched_fallback_num = Arc::clone(&self.store_batched_fallback_num);
            let finish = Arc::clone(&self.finish);
            let queue = Arc::clone(&queue);
            let sender = sender.clone();
            let completion = RunawayWorkerCompletion {
                checker: request.runaway_checker.clone(),
                remaining: Arc::clone(&remaining_workers),
            };
            self.workers.push(thread::spawn(move || {
                let _completion = completion;
                let worker = CopTaskWorker::new(backend, request, cache, keys_read, paging_index)
                    .with_ema(ema)
                    .with_limiter_wait(limiter_wait)
                    .with_lock_sets(resolved_locks, committed_locks)
                    .with_runtime_stats(runtime_stats)
                    .with_store_batch_stats(store_batched_num, store_batched_fallback_num)
                    .with_finish(Arc::clone(&finish));
                loop {
                    let item = queue.0.lock().expect("cop task queue poisoned").pop_front();
                    let Some((sequence, task)) = item else {
                        break;
                    };
                    let mut pending = VecDeque::from([task]);
                    let mut backoffer = Backoffer::new(COP_NEXT_MAX_BACKOFF);
                    while let Some(task) = pending.pop_front() {
                        if finish.load(Ordering::Acquire) {
                            return;
                        }
                        match worker.handle_task_once(&mut backoffer, task) {
                            Ok(result) => {
                                pending.extend(result.remains);
                                if let Some(response) = result.response
                                    && !send_iterator_message(
                                        &sender,
                                        &finish,
                                        IteratorMessage::Response(sequence, response),
                                    )
                                {
                                    return;
                                }
                                for response in result.batch_responses {
                                    if !send_iterator_message(
                                        &sender,
                                        &finish,
                                        IteratorMessage::Response(sequence, response),
                                    ) {
                                        return;
                                    }
                                }
                            }
                            Err(error) => {
                                let _ = send_iterator_message(
                                    &sender,
                                    &finish,
                                    IteratorMessage::Response(
                                        sequence,
                                        CopResponse {
                                            error: Some(error),
                                            ..CopResponse::default()
                                        },
                                    ),
                                );
                                break;
                            }
                        }
                    }
                    if !send_iterator_message(&sender, &finish, IteratorMessage::Done(sequence)) {
                        return;
                    }
                }
            }));
        }
        drop(sender);
    }

    fn next_lite(&mut self) -> BatchResult<Option<CopResponse>> {
        let mut lite = self.lite_worker.take().expect("lite worker is configured");
        let result = loop {
            if let Some(response) = lite.batch_responses.pop_front() {
                break Ok(Some(response));
            }
            let Some(task) = self.tasks.first().cloned() else {
                break Ok(None);
            };
            self.tasks.remove(0);
            match lite.worker.handle_task_once(&mut lite.backoffer, task) {
                Ok(result) => {
                    if !result.remains.is_empty() {
                        let mut tasks = result.remains;
                        tasks.append(&mut self.tasks);
                        self.tasks = tasks;
                    }
                    lite.batch_responses.extend(result.batch_responses);
                    if let Some(response) = result.response {
                        break Ok(Some(response));
                    }
                }
                Err(error) => break Err(error),
            }
        };
        let _ = lite
            .try_state
            .compare_exchange(1, 0, Ordering::AcqRel, Ordering::Acquire);
        match result {
            Ok(Some(response)) => {
                if !self.tasks.is_empty()
                    && lite.batch_responses.is_empty()
                    && response.error.is_none()
                {
                    trigger_lite_worker_fallback_hook();
                    self.open_concurrent();
                } else {
                    self.lite_worker = Some(lite);
                }
                self.backend.check_visibility(self.request.start_ts)?;
                Ok(Some(response))
            }
            Ok(None) => {
                if let Some(checker) = &self.request.runaway_checker {
                    checker.reset_total_processed_keys();
                }
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    fn receive_message(&self) -> BatchResult<Option<IteratorMessage>> {
        loop {
            if self.finish.load(Ordering::Acquire) {
                return Ok(None);
            }
            if self.killed.load(Ordering::Acquire) != 0 {
                return Err(BatchError::QueryInterrupted);
            }
            match self
                .receiver
                .as_ref()
                .expect("iterator opened")
                .recv_timeout(Duration::from_secs(3))
            {
                Ok(message) => return Ok(Some(message)),
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(None),
            }
        }
    }

    /// 取下一条响应；结束时返回 None。
    pub fn next(&mut self) -> BatchResult<Option<CopResponse>> {
        if !self.started {
            self.open();
        }
        if self.lite_worker.is_some() {
            return self.next_lite();
        }
        if !self.request.keep_order {
            loop {
                match self.receive_message()? {
                    Some(IteratorMessage::Response(_, response)) => {
                        if let Some(error) = response.error.clone() {
                            return Err(error);
                        }
                        self.backend.check_visibility(self.request.start_ts)?;
                        return Ok(Some(response));
                    }
                    Some(IteratorMessage::Done(_)) => continue,
                    None => return Ok(None),
                }
            }
        }
        loop {
            if let Some(response) = self
                .ordered_buffer
                .get_mut(&self.next_ordered_task)
                .and_then(VecDeque::pop_front)
            {
                if let Some(error) = response.error.clone() {
                    return Err(error);
                }
                self.backend.check_visibility(self.request.start_ts)?;
                return Ok(Some(response));
            }
            if self.ordered_done.remove(&self.next_ordered_task) {
                self.next_ordered_task += 1;
                continue;
            }
            match self.receive_message()? {
                Some(IteratorMessage::Response(sequence, response)) => self
                    .ordered_buffer
                    .entry(sequence)
                    .or_default()
                    .push_back(response),
                Some(IteratorMessage::Done(sequence)) => {
                    self.ordered_done.insert(sequence);
                }
                None => return Ok(None),
            }
        }
    }

    pub fn collect_unconsumed_runtime_stats(&mut self) -> Vec<CopRuntimeStats> {
        let mut result = Vec::new();
        for responses in self.ordered_buffer.values_mut() {
            for response in responses {
                if let Some(stats) = response.detail.take() {
                    result.push(stats);
                }
            }
        }
        result
    }

    /// 关闭迭代器并回收 worker。
    pub fn close(&mut self) {
        if !self.finish.swap(true, Ordering::AcqRel) {
            self.rate_limit_action.close();
        }
        if let Some(lite) = self.lite_worker.take() {
            let _ = lite
                .try_state
                .compare_exchange(1, 0, Ordering::AcqRel, Ordering::Acquire);
        }
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

impl Drop for CopIterator {
    fn drop(&mut self) {
        self.close();
    }
}

/// `CopClient::send` 返回的流：普通迭代器或 Batch 迭代器。
pub enum CopResponseStream {
    Standard(CopIterator),
    Batch(BatchCopIterator),
    BatchDirect(VecDeque<crate::batch_request_sender::BatchResponse>),
}

/// 协处理器客户端：构建迭代器或直接发送请求。
pub struct CopClient {
    backend: Arc<dyn CopBackend>,
    cache: Option<Arc<CoprocessorCache>>,
    cpu_count: usize,
    replica_read_seed: AtomicU32,
}

impl CopClient {
    pub fn new(
        backend: Arc<dyn CopBackend>,
        cache: Option<Arc<CoprocessorCache>>,
        cpu_count: usize,
    ) -> Self {
        Self {
            backend,
            cache,
            cpu_count,
            replica_read_seed: AtomicU32::new(0),
        }
    }

    /// 发送请求并返回响应流（普通或 Batch）。
    pub fn send(&self, mut request: CopRequest) -> BatchResult<CopResponseStream> {
        if request.store_type == StoreType::TiFlash && request.batch_cop {
            return self
                .backend
                .send_tiflash_batch(&request)
                .map(|responses| CopResponseStream::BatchDirect(responses.into()));
        }
        let mut iterator = self.build_cop_iterator(&mut request)?;
        iterator.open();
        Ok(CopResponseStream::Standard(iterator))
    }

    /// 构建并配置 CopIterator（并发、小任务、速率限制等）。
    pub fn build_cop_iterator(&self, request: &mut CopRequest) -> BatchResult<CopIterator> {
        if request.store_type == StoreType::TiDb || request.request_type != RequestType::Dag {
            request.paging.enabled = false;
        }
        if request.store_type != StoreType::TiKv || request.request_type != RequestType::Dag {
            request.paging.size_bytes = 0;
        }
        if !check_store_batch_coprocessor(request) {
            request.store_batch_size = 0;
        }
        let started = Instant::now();
        let use_hints = optimize_row_hint(request);
        let mut tasks = Vec::new();
        for partition in &request.key_ranges {
            let built = build_cop_tasks(
                self.backend.as_ref(),
                request,
                KeyRanges::new(partition.ranges.clone()),
                BuildCopTaskOptions {
                    row_hints: if use_hints {
                        partition.row_hints.clone()
                    } else {
                        Vec::new()
                    },
                    keep_order_response_channel: request.keep_order,
                    ..BuildCopTaskOptions::default()
                },
            )?;
            tasks.extend(built);
        }
        request.store_batch_size = 0;
        let mut concurrency = request.concurrency.min(tasks.len()).max(1);
        let (_, mut small_concurrency) = if use_hints {
            small_task_concurrency(&tasks, self.cpu_count)
        } else {
            (0, 0)
        };
        let non_small = tasks.iter().filter(|task| !is_small_task(task)).count();
        concurrency = concurrency.min(non_small.max(1));
        if request
            .runaway_checker
            .as_ref()
            .is_some_and(|checker| checker.check_action() == RunawayAction::CoolDown)
        {
            concurrency = 1;
            small_concurrency = 0;
        }
        if request.keep_order {
            small_concurrency = small_concurrency.min(20);
        }
        self.replica_read_seed.fetch_add(1, Ordering::Relaxed);
        Ok(CopIterator::new(
            Arc::clone(&self.backend),
            Arc::new(request.clone()),
            self.cache.clone(),
            tasks,
            concurrency,
            small_concurrency,
            started.elapsed(),
        ))
    }
}

// 速率限制内部状态（令牌与启用标志）。
#[derive(Debug)]
struct RateLimitState {
    exceeded: bool,
    remaining_tokens: usize,
    triggered_in_cycle: bool,
    trigger_count: usize,
}

/// Runaway 速率限制动作：令牌桶式控制发送速率。
pub struct RateLimitAction {
    enabled: AtomicBool,
    total_tokens: usize,
    state: Mutex<RateLimitState>,
}

impl RateLimitAction {
    pub fn new(total_tokens: usize) -> Self {
        Self {
            enabled: AtomicBool::new(false),
            total_tokens,
            state: Mutex::new(RateLimitState {
                exceeded: false,
                remaining_tokens: total_tokens,
                triggered_in_cycle: false,
                trigger_count: 0,
            }),
        }
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Release);
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }

    /// 尝试获取发送令牌；启用限速时可能阻塞/拒绝。
    pub fn action(&self) -> bool {
        if !self.is_enabled() {
            return false;
        }
        let mut state = self.state.lock().expect("rate-limit action lock poisoned");
        if state.triggered_in_cycle {
            return true;
        }
        state.triggered_in_cycle = true;
        if state.remaining_tokens < 2 {
            self.enabled.store(false, Ordering::Release);
            return false;
        }
        state.exceeded = true;
        state.trigger_count += 1;
        true
    }

    /// 在超限周期后归还/销毁令牌。
    pub fn destroy_token_if_needed(&self, return_token: impl FnOnce()) {
        if !self.is_enabled() {
            return_token();
            return;
        }
        let mut state = self.state.lock().expect("rate-limit action lock poisoned");
        if !state.exceeded {
            drop(state);
            return_token();
            return;
        }
        state.remaining_tokens = state.remaining_tokens.saturating_sub(1);
        state.exceeded = false;
        state.triggered_in_cycle = false;
    }

    pub fn close(&self) {
        self.enabled.store(false, Ordering::Release);
        let mut state = self.state.lock().expect("rate-limit action lock poisoned");
        state.exceeded = false;
    }

    pub fn total_tokens(&self) -> usize {
        self.total_tokens
    }
}

/// 将整型优先级映射为 Priority 枚举。
pub fn priority_to_wire(priority: i32) -> Priority {
    match priority {
        value if value < 0 => Priority::Low,
        value if value > 0 => Priority::High,
        _ => Priority::Normal,
    }
}

/// 将整型隔离级别映射为 IsolationLevel 枚举。
pub fn isolation_level_to_wire(level: i32) -> IsolationLevel {
    match level {
        1 => IsolationLevel::ReadCommitted,
        2 => IsolationLevel::ReadCommittedCheckTs,
        _ => IsolationLevel::SnapshotIsolation,
    }
}

/// 将成对字符串键组装为 KeyRange 列表（奇数个键报错）。
pub fn build_key_ranges(keys: &[&str]) -> BatchResult<Vec<KeyRange>> {
    if !keys.len().is_multiple_of(2) {
        return Err(BatchError::OtherResponse(
            "BuildKeyRanges requires paired keys".to_owned(),
        ));
    }
    Ok(keys
        .chunks_exact(2)
        .map(|pair| KeyRange {
            start: pair[0].as_bytes().to_vec(),
            end: pair[1].as_bytes().to_vec(),
        })
        .collect())
}

/// 是否对请求启用行数 hint 优化。
pub fn optimize_row_hint(request: &CopRequest) -> bool {
    request.store_type != StoreType::TiDb
        && !request.request_source.internal
        && request.request_type == RequestType::Dag
}

/// 是否应对该请求启用 store 侧批处理 Coprocessor。
pub fn check_store_batch_coprocessor(request: &CopRequest) -> bool {
    (request.request_type == RequestType::Dag || request.allow_batch_task_data_merge)
        && request.store_type == StoreType::TiKv
        && request.replica_read == ReplicaReadType::Leader
        && !request.keep_order
        && !request.paging.enabled
        && request.paging.size_bytes == 0
        && (!request.request_source.internal || request.allow_batch_task_data_merge)
}

type LiteWorkerHook = Arc<dyn Fn() + Send + Sync>;
static LITE_WORKER_FALLBACK_HOOK: OnceLock<Mutex<Option<LiteWorkerHook>>> = OnceLock::new();

/// 测试钩子：设置 lite worker 回退回调。
pub fn set_lite_worker_fallback_hook_for_test(hook: Option<LiteWorkerHook>) {
    *LITE_WORKER_FALLBACK_HOOK
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("lite-worker hook lock poisoned") = hook;
}

/// 测试钩子：触发 lite worker 回退回调。
pub fn trigger_lite_worker_fallback_hook() {
    if let Some(hook) = LITE_WORKER_FALLBACK_HOOK
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("lite-worker hook lock poisoned")
        .clone()
    {
        hook();
    }
}

/// Go 风格类型别名。
#[allow(non_camel_case_types)]
pub type copTask = CopTask;
/// Go 风格类型别名。
#[allow(non_camel_case_types)]
pub type batchedCopTask = BatchedCopTask;
/// Go 风格类型别名。
#[allow(non_camel_case_types)]
pub type copResponse = CopResponse;
/// Go 风格类型别名。
#[allow(non_camel_case_types)]
pub type copTaskResult = CopTaskResult;
/// Go 风格类型别名。
#[allow(non_camel_case_types)]
pub type copIterator = CopIterator;
/// Go 风格类型别名。
#[allow(non_camel_case_types)]
pub type rateLimitAction = RateLimitAction;

/// Go 风格常量别名。
#[allow(non_upper_case_globals)]
pub const copBuildTaskMaxBackoff: usize = COP_BUILD_TASK_MAX_BACKOFF;
/// Go 风格常量别名。
#[allow(non_upper_case_globals)]
pub const CopNextMaxBackoff: usize = COP_NEXT_MAX_BACKOFF;
/// Go 风格常量别名。
#[allow(non_upper_case_globals)]
pub const CopSmallTaskRow: usize = COP_SMALL_TASK_ROW;

/// Go 风格函数别名：成对键 → KeyRange。
#[allow(non_snake_case)]
pub fn BuildKeyRanges(keys: &[&str]) -> BatchResult<Vec<KeyRange>> {
    build_key_ranges(keys)
}

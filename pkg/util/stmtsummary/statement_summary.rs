// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// 语句摘要（statement summary）v1：按 SQL digest / plan digest 聚合执行统计。
//
// 对应 Go `pkg/util/stmtsummary/statement_summary.go`。维护全局 LRU 映射，
// 按刷新区间（refresh interval）切分历史窗口，汇总时延、Coprocessor 任务、
// 两阶段提交（2PC：prewrite/commit）与资源用量 RU（Request Unit）等指标，
// 供 `INFORMATION_SCHEMA` 语句摘要表与相关诊断查询读取。容量满时淘汰条目
// 归入 `other` 桶。digest 是规范化 SQL 的指纹。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

use base64::Engine as _;
use lru::LruCache;
use ppcpuusage::CPUUsages;
use std::collections::{HashMap, HashSet, VecDeque};
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, AtomicU32, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use task_execdetails::execdetails::util::RUDetails;
use task_execdetails::execdetails::{CopTasksSummary, ExecDetails};
use task_execdetails::util::LoadTiKVExecDetails;
use task_execdetails::util::util::ExecDetails as TiKVExecDetails;
use task_stmtctx::{NewStmtCtx, StatementContext};

use crate::stmtSummaryByDigestEvicted;

/// 与 Go `error` 字符串对齐的错误载体。
type GoError = String;
/// 本模块统一的可失败返回类型。
type GoResult<T> = Result<T, GoError>;

/// 默认 LRU 容量：同时跟踪的 digest 上限。
const DEFAULT_MAX_STMT_COUNT: u32 = 3000;
/// 默认刷新区间（秒），用于切分历史时间窗。
const DEFAULT_REFRESH_INTERVAL: i64 = 1800;
/// 默认保留的历史区间个数。
const DEFAULT_HISTORY_SIZE: i32 = 24;
/// 默认截断后的 SQL 文本最大长度。
const DEFAULT_MAX_SQL_LENGTH: i32 = 32768;

/// 当前窗口内已跟踪摘要条数的 Prometheus 指标。
static WINDOW_RECORD_COUNT: LazyLock<prometheus::Gauge> = LazyLock::new(|| {
    prometheus::Gauge::new(
        "tidb_stmt_summary_window_record_count_v1",
        "The number of statement summary records currently tracked by v1",
    )
    .expect("static statement-summary record metric must be valid")
});
/// 当前窗口内 LRU 淘汰次数的 Prometheus 指标。
static WINDOW_EVICTED_COUNT: LazyLock<prometheus::Gauge> = LazyLock::new(|| {
    prometheus::Gauge::new(
        "tidb_stmt_summary_window_evicted_count_v1",
        "The number of LRU evictions in the current v1 statement-summary window",
    )
    .expect("static statement-summary eviction metric must be valid")
});

/// A small reusable-key pool matching the allocation behavior of Go's sync.Pool.
/// 可复用的 digest 键对象池，行为对齐 Go `sync.Pool`。
pub struct StmtDigestKeyPoolType(Mutex<Vec<StmtDigestKey>>);

impl StmtDigestKeyPoolType {
    /// 构造空对象池。
    pub const fn new() -> Self {
        Self(Mutex::new(Vec::new()))
    }

    /// 取出一个键；池空时返回默认空键。
    pub fn Get(&self) -> StmtDigestKey {
        self.0
            .lock()
            .expect("statement digest key pool poisoned")
            .pop()
            .unwrap_or_default()
    }

    /// 归还键；先清空 hash 缓冲区以便复用。
    pub fn Put(&self, mut key: StmtDigestKey) {
        key.hash.clear();
        self.0
            .lock()
            .expect("statement digest key pool poisoned")
            .push(key);
    }
}

/// 全局 digest 键对象池。
pub static StmtDigestKeyPool: StmtDigestKeyPoolType = StmtDigestKeyPoolType::new();

/// Key for the statement-summary LRU. The empty-user encoding remains byte-identical
/// to the legacy five-field layout; a non-empty user has an explicit length boundary.
/// 语句摘要 LRU 的复合键：schema/digest/prev/plan/resource_group[/user]。
/// 空 user 时编码与旧五字段布局字节一致；非空 user 前写入长度前缀。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StmtDigestKey {
    hash: Vec<u8>,
}

impl StmtDigestKey {
    /// 按 schema/digest/前序 digest/plan digest/资源组/用户拼装哈希键。
    pub fn Init(
        &mut self,
        schemaName: &str,
        digest: &str,
        prevDigest: &str,
        planDigest: &str,
        resourceGroupName: &str,
        user: &str,
    ) {
        // 非空 user 额外预留 4 字节长度前缀，避免资源组与用户名拼接歧义。
        let extra = if user.is_empty() { 0 } else { 4 };
        let length = schemaName.len()
            + digest.len()
            + prevDigest.len()
            + planDigest.len()
            + resourceGroupName.len()
            + user.len()
            + extra;
        self.hash.clear();
        self.hash
            .reserve(length.saturating_sub(self.hash.capacity()));
        self.hash.extend_from_slice(digest.as_bytes());
        self.hash.extend_from_slice(schemaName.as_bytes());
        self.hash.extend_from_slice(prevDigest.as_bytes());
        self.hash.extend_from_slice(planDigest.as_bytes());
        self.hash.extend_from_slice(resourceGroupName.as_bytes());
        if !user.is_empty() {
            self.hash
                .extend_from_slice(&(user.len() as u32).to_be_bytes());
            self.hash.extend_from_slice(user.as_bytes());
        }
    }

    /// 返回内部哈希字节切片，用作 LRU 键。
    pub fn Hash(&self) -> &[u8] {
        &self.hash
    }
}

/// The LRU-backed statement-summary map. Mutating methods require `&mut self`; callers
/// that share a map across threads use `Mutex`, as the global instance below does.
/// 基于 LRU 的语句摘要总表；可变方法需 `&mut self`，跨线程通过下方全局 `Mutex` 共享。
pub struct stmtSummaryByDigestMap {
    /// digest 键 → 摘要条目的 LRU 缓存。
    pub(crate) summaryMap: LruCache<Vec<u8>, stmtSummaryByDigest>,
    /// 当前刷新窗口的起始 Unix 秒。
    pub beginTimeForCurInterval: i64,
    /// 是否启用语句摘要采集。
    optEnabled: AtomicBool,
    /// 是否采集内部 SQL（系统会话等）。
    optEnableInternalQuery: AtomicBool,
    /// 是否保留历史窗口切片。
    optHistoryEnabled: AtomicBool,
    /// LRU 最大条目数。
    optMaxStmtCount: AtomicU32,
    /// 刷新窗口长度（秒）。
    optRefreshInterval: AtomicI64,
    /// 每个 digest 保留的历史窗口数。
    optHistorySize: AtomicI32,
    /// 样例 SQL 最大长度。
    optMaxSQLLength: AtomicI32,
    /// 是否把 user 纳入 digest key（按用户分组）。
    optGroupByUser: AtomicBool,
    /// 被 LRU 淘汰条目的聚合桶。
    pub other: stmtSummaryByDigestEvicted,
    /// 当前窗口内已发生的淘汰次数。
    currentWindowEvictedCount: i64,
    /// 测试注入的“当前时间”（Unix 秒）；`None` 用真实时钟。
    nowForTest: Option<i64>,
}

/// 全局语句摘要映射实例（惰性初始化，外层加锁）。
pub static StmtSummaryByDigestMap: LazyLock<Mutex<stmtSummaryByDigestMap>> =
    LazyLock::new(|| Mutex::new(newStmtSummaryByDigestMap()));

/// 单个 digest（及可选 plan/user 维度）对应的累计与历史统计。
#[derive(Clone, Default)]
pub struct stmtSummaryByDigest {
    /// 是否已用首次执行信息完成元数据初始化。
    pub initialized: bool,
    /// 跨所有窗口的累计统计。
    pub cumulative: stmtSummaryStats,
    /// 历史窗口队列（队头较旧，队尾较新）。
    pub history: VecDeque<stmtSummaryByDigestElement>,
    /// 库名（schema）。
    pub schemaName: String,
    /// SQL digest 指纹。
    pub digest: String,
    /// 执行计划 digest。
    pub planDigest: String,
    /// 语句类型（SELECT/INSERT 等）。
    pub stmtType: String,
    /// 规范化后的 SQL 文本（可能截断）。
    pub normalizedSQL: String,
    /// 涉及表名列表（db.table，逗号分隔）。
    pub tableNames: String,
    /// 是否全部来自内部查询。
    pub isInternal: bool,
    /// 绑定 SQL 文本。
    pub bindingSQL: String,
    /// 绑定 SQL 的 digest。
    pub bindingDigest: String,
}

/// 某一个刷新时间窗口内的摘要元素。
#[derive(Clone, Default)]
pub struct stmtSummaryByDigestElement {
    /// 窗口起始 Unix 秒。
    pub beginTime: i64,
    /// 窗口结束 Unix 秒。
    pub endTime: i64,
    /// 该窗口内的执行指标。
    pub stmtSummaryStats: stmtSummaryStats,
}

/// 单条摘要的聚合指标：时延、Coprocessor、提交、内存/磁盘、RU 与网络流量等。
#[derive(Clone)]
pub struct stmtSummaryStats {
    pub sampleSQL: String,
    pub charset: String,
    pub collation: String,
    pub prevSQL: String,
    pub samplePlan: String,
    pub sampleBinaryPlan: String,
    pub planHint: String,
    pub indexNames: Vec<String>,
    pub execCount: i64,
    pub sumErrors: i32,
    pub sumWarnings: i32,
    pub sumLatency: Duration,
    pub maxLatency: Duration,
    pub minLatency: Duration,
    pub sumParseLatency: Duration,
    pub maxParseLatency: Duration,
    pub sumCompileLatency: Duration,
    pub maxCompileLatency: Duration,
    pub sumNumCopTasks: i64,
    pub sumCopProcessTime: Duration,
    pub maxCopProcessTime: Duration,
    pub maxCopProcessAddress: String,
    pub sumCopWaitTime: Duration,
    pub maxCopWaitTime: Duration,
    pub maxCopWaitAddress: String,
    pub sumProcessTime: Duration,
    pub maxProcessTime: Duration,
    pub sumWaitTime: Duration,
    pub maxWaitTime: Duration,
    pub sumBackoffTime: Duration,
    pub maxBackoffTime: Duration,
    pub sumTotalKeys: i64,
    pub maxTotalKeys: i64,
    pub sumProcessedKeys: i64,
    pub maxProcessedKeys: i64,
    pub sumRocksdbDeleteSkippedCount: u64,
    pub maxRocksdbDeleteSkippedCount: u64,
    pub sumRocksdbKeySkippedCount: u64,
    pub maxRocksdbKeySkippedCount: u64,
    pub sumRocksdbBlockCacheHitCount: u64,
    pub maxRocksdbBlockCacheHitCount: u64,
    pub sumRocksdbBlockReadCount: u64,
    pub maxRocksdbBlockReadCount: u64,
    pub sumRocksdbBlockReadByte: u64,
    pub maxRocksdbBlockReadByte: u64,
    pub commitCount: i64,
    pub sumGetCommitTsTime: Duration,
    pub maxGetCommitTsTime: Duration,
    pub sumPrewriteTime: Duration,
    pub maxPrewriteTime: Duration,
    pub sumCommitTime: Duration,
    pub maxCommitTime: Duration,
    pub sumLocalLatchTime: Duration,
    pub maxLocalLatchTime: Duration,
    pub sumCommitBackoffTime: i64,
    pub maxCommitBackoffTime: i64,
    pub sumResolveLockTime: i64,
    pub maxResolveLockTime: i64,
    pub sumWriteKeys: i64,
    pub maxWriteKeys: i32,
    pub sumWriteSize: i64,
    pub maxWriteSize: i32,
    pub sumPrewriteRegionNum: i64,
    pub maxPrewriteRegionNum: i32,
    pub sumTxnRetry: i64,
    pub maxTxnRetry: i32,
    pub sumBackoffTimes: i64,
    pub backoffTypes: HashMap<String, i32>,
    pub authUsers: HashSet<String>,
    pub sumMem: i64,
    pub maxMem: i64,
    pub sumDisk: i64,
    pub maxDisk: i64,
    pub sumAffectedRows: u64,
    pub sumKVTotal: Duration,
    pub sumPDTotal: Duration,
    pub sumBackoffTotal: Duration,
    pub sumWriteSQLRespTotal: Duration,
    pub sumTidbCPU: Duration,
    pub sumTikvCPU: Duration,
    pub sumResultRows: i64,
    pub maxResultRows: i64,
    pub minResultRows: i64,
    pub prepared: bool,
    pub firstSeen: SystemTime,
    pub lastSeen: SystemTime,
    pub planInCache: bool,
    pub planCacheHits: i64,
    pub planInBinding: bool,
    pub execRetryCount: u32,
    pub execRetryTime: Duration,
    pub resourceGroupName: String,
    pub StmtRUSummary: StmtRUSummary,
    pub StmtNetworkTrafficSummary: StmtNetworkTrafficSummary,
    pub planCacheUnqualifiedCount: i64,
    pub lastPlanCacheUnqualified: String,
    pub storageKV: bool,
    pub storageMPP: bool,
    pub sumMemArbitration: f64,
    pub maxMemArbitration: f64,
}

/// 全零默认值；`minResultRows` 等在首次 `add`/`newStmtSummaryStats` 时再校正。
impl Default for stmtSummaryStats {
    fn default() -> Self {
        Self {
            sampleSQL: String::new(),
            charset: String::new(),
            collation: String::new(),
            prevSQL: String::new(),
            samplePlan: String::new(),
            sampleBinaryPlan: String::new(),
            planHint: String::new(),
            indexNames: Vec::new(),
            execCount: 0,
            sumErrors: 0,
            sumWarnings: 0,
            sumLatency: Duration::ZERO,
            maxLatency: Duration::ZERO,
            minLatency: Duration::ZERO,
            sumParseLatency: Duration::ZERO,
            maxParseLatency: Duration::ZERO,
            sumCompileLatency: Duration::ZERO,
            maxCompileLatency: Duration::ZERO,
            sumNumCopTasks: 0,
            sumCopProcessTime: Duration::ZERO,
            maxCopProcessTime: Duration::ZERO,
            maxCopProcessAddress: String::new(),
            sumCopWaitTime: Duration::ZERO,
            maxCopWaitTime: Duration::ZERO,
            maxCopWaitAddress: String::new(),
            sumProcessTime: Duration::ZERO,
            maxProcessTime: Duration::ZERO,
            sumWaitTime: Duration::ZERO,
            maxWaitTime: Duration::ZERO,
            sumBackoffTime: Duration::ZERO,
            maxBackoffTime: Duration::ZERO,
            sumTotalKeys: 0,
            maxTotalKeys: 0,
            sumProcessedKeys: 0,
            maxProcessedKeys: 0,
            sumRocksdbDeleteSkippedCount: 0,
            maxRocksdbDeleteSkippedCount: 0,
            sumRocksdbKeySkippedCount: 0,
            maxRocksdbKeySkippedCount: 0,
            sumRocksdbBlockCacheHitCount: 0,
            maxRocksdbBlockCacheHitCount: 0,
            sumRocksdbBlockReadCount: 0,
            maxRocksdbBlockReadCount: 0,
            sumRocksdbBlockReadByte: 0,
            maxRocksdbBlockReadByte: 0,
            commitCount: 0,
            sumGetCommitTsTime: Duration::ZERO,
            maxGetCommitTsTime: Duration::ZERO,
            sumPrewriteTime: Duration::ZERO,
            maxPrewriteTime: Duration::ZERO,
            sumCommitTime: Duration::ZERO,
            maxCommitTime: Duration::ZERO,
            sumLocalLatchTime: Duration::ZERO,
            maxLocalLatchTime: Duration::ZERO,
            sumCommitBackoffTime: 0,
            maxCommitBackoffTime: 0,
            sumResolveLockTime: 0,
            maxResolveLockTime: 0,
            sumWriteKeys: 0,
            maxWriteKeys: 0,
            sumWriteSize: 0,
            maxWriteSize: 0,
            sumPrewriteRegionNum: 0,
            maxPrewriteRegionNum: 0,
            sumTxnRetry: 0,
            maxTxnRetry: 0,
            sumBackoffTimes: 0,
            backoffTypes: HashMap::new(),
            authUsers: HashSet::new(),
            sumMem: 0,
            maxMem: 0,
            sumDisk: 0,
            maxDisk: 0,
            sumAffectedRows: 0,
            sumKVTotal: Duration::ZERO,
            sumPDTotal: Duration::ZERO,
            sumBackoffTotal: Duration::ZERO,
            sumWriteSQLRespTotal: Duration::ZERO,
            sumTidbCPU: Duration::ZERO,
            sumTikvCPU: Duration::ZERO,
            sumResultRows: 0,
            maxResultRows: 0,
            minResultRows: 0,
            prepared: false,
            firstSeen: UNIX_EPOCH,
            lastSeen: UNIX_EPOCH,
            planInCache: false,
            planCacheHits: 0,
            planInBinding: false,
            execRetryCount: 0,
            execRetryTime: Duration::ZERO,
            resourceGroupName: String::new(),
            StmtRUSummary: StmtRUSummary::default(),
            StmtNetworkTrafficSummary: StmtNetworkTrafficSummary::default(),
            planCacheUnqualifiedCount: 0,
            lastPlanCacheUnqualified: String::new(),
            storageKV: false,
            storageMPP: false,
            sumMemArbitration: 0.0,
            maxMemArbitration: 0.0,
        }
    }
}

/// 一次语句执行结束后的采集信息，作为写入摘要的输入。
pub struct StmtExecInfo {
    pub SchemaName: String,
    pub Charset: String,
    pub Collation: String,
    pub NormalizedSQL: String,
    pub Digest: String,
    pub PrevSQL: String,
    pub PrevSQLDigest: String,
    pub PlanDigest: String,
    pub User: String,
    pub TotalLatency: Duration,
    pub ParseLatency: Duration,
    pub CompileLatency: Duration,
    pub StmtCtx: StatementContext,
    pub CopTasks: Option<CopTasksSummary>,
    pub ExecDetail: ExecDetails,
    pub MemMax: i64,
    pub MemArbitration: f64,
    pub DiskMax: i64,
    pub StartTime: SystemTime,
    pub IsInternal: bool,
    pub Succeed: bool,
    pub PlanInCache: bool,
    pub PlanInBinding: bool,
    pub ExecRetryCount: u32,
    pub ExecRetryTime: Duration,
    pub WriteSQLRespDuration: Duration,
    pub ResultRows: i64,
    pub TiKVExecDetails: TiKVExecDetails,
    pub Prepared: bool,
    pub KeyspaceName: String,
    pub KeyspaceID: u32,
    pub ResourceGroupName: String,
    pub RUDetail: Option<RUDetails>,
    pub TotalRUV2: f64,
    pub CPUUsages: CPUUsages,
    pub PlanCacheUnqualified: String,
    pub LazyInfo: Box<dyn StmtExecLazyInfo>,
}

/// 懒加载信息的空实现，用于默认/测试路径。
#[derive(Default)]
struct EmptyLazyInfo;

impl StmtExecLazyInfo for EmptyLazyInfo {
    fn GetOriginalSQL(&self) -> String {
        String::new()
    }
    fn GetEncodedPlan(&self) -> (String, String, Option<GoError>) {
        (String::new(), String::new(), None)
    }
    fn GetBinaryPlan(&self) -> String {
        String::new()
    }
    fn GetPlanDigest(&self) -> String {
        String::new()
    }
    fn GetBindingSQLAndDigest(&self) -> (String, String) {
        (String::new(), String::new())
    }
}

impl Default for StmtExecInfo {
    fn default() -> Self {
        Self {
            SchemaName: String::new(),
            Charset: String::new(),
            Collation: String::new(),
            NormalizedSQL: String::new(),
            Digest: String::new(),
            PrevSQL: String::new(),
            PrevSQLDigest: String::new(),
            PlanDigest: String::new(),
            User: String::new(),
            TotalLatency: Duration::ZERO,
            ParseLatency: Duration::ZERO,
            CompileLatency: Duration::ZERO,
            StmtCtx: *NewStmtCtx(),
            CopTasks: None,
            ExecDetail: ExecDetails::default(),
            MemMax: 0,
            MemArbitration: 0.0,
            DiskMax: 0,
            StartTime: UNIX_EPOCH,
            IsInternal: false,
            Succeed: false,
            PlanInCache: false,
            PlanInBinding: false,
            ExecRetryCount: 0,
            ExecRetryTime: Duration::ZERO,
            WriteSQLRespDuration: Duration::ZERO,
            ResultRows: 0,
            TiKVExecDetails: TiKVExecDetails::default(),
            Prepared: false,
            KeyspaceName: String::new(),
            KeyspaceID: 0,
            ResourceGroupName: String::new(),
            RUDetail: None,
            TotalRUV2: 0.0,
            CPUUsages: CPUUsages::default(),
            PlanCacheUnqualified: String::new(),
            LazyInfo: Box::new(EmptyLazyInfo),
        }
    }
}

/// 延迟计算原始 SQL、编码计划、二进制计划与绑定信息，避免热路径过早物化。
pub trait StmtExecLazyInfo {
    fn GetOriginalSQL(&self) -> String;
    fn GetEncodedPlan(&self) -> (String, String, Option<GoError>);
    fn GetBinaryPlan(&self) -> String;
    fn GetPlanDigest(&self) -> String;
    fn GetBindingSQLAndDigest(&self) -> (String, String);
}

/// 使用默认配置构造语句摘要映射。
pub fn newStmtSummaryByDigestMap() -> stmtSummaryByDigestMap {
    stmtSummaryByDigestMap {
        summaryMap: LruCache::new(NonZeroUsize::new(DEFAULT_MAX_STMT_COUNT as usize).unwrap()),
        beginTimeForCurInterval: 0,
        optEnabled: AtomicBool::new(true),
        optEnableInternalQuery: AtomicBool::new(false),
        optHistoryEnabled: AtomicBool::new(true),
        optMaxStmtCount: AtomicU32::new(DEFAULT_MAX_STMT_COUNT),
        optRefreshInterval: AtomicI64::new(DEFAULT_REFRESH_INTERVAL),
        optHistorySize: AtomicI32::new(DEFAULT_HISTORY_SIZE),
        optMaxSQLLength: AtomicI32::new(DEFAULT_MAX_SQL_LENGTH),
        optGroupByUser: AtomicBool::new(false),
        other: stmtSummaryByDigestEvicted::default(),
        currentWindowEvictedCount: 0,
        nowForTest: None,
    }
}

impl stmtSummaryByDigestMap {
    /// 将一次执行统计并入对应 digest 条目；必要时推进时间窗并处理 LRU 淘汰。
    pub fn AddStatement(&mut self, sei: &StmtExecInfo) {
        // 总开关关闭，或内部查询未开启时直接跳过。
        if !self.Enabled() || (sei.IsInternal && !self.EnabledInternal()) {
            return;
        }
        let intervalSeconds = self.refreshInterval();
        let now = self.nowForTest.unwrap_or_else(unix_now);
        let historySize = if self.historyEnabled() {
            self.historySize().max(0) as usize
        } else {
            0
        };
        // 越过当前刷新区间则对齐到新窗口起点，并清零本窗淘汰计数。
        if self.beginTimeForCurInterval + intervalSeconds <= now {
            self.beginTimeForCurInterval = now / intervalSeconds * intervalSeconds;
            self.currentWindowEvictedCount = 0;
        }
        let beginTime = self.beginTimeForCurInterval;
        let max_sql_length = usize::try_from(self.maxSQLLength())
            .expect("negative max SQL length bypassed system-variable validation");
        // group_by_user 关闭时键中不编码用户，保持与旧布局兼容。
        let user = if self.GroupByUser() {
            sei.User.as_str()
        } else {
            ""
        };
        let mut pooled = StmtDigestKeyPool.Get();
        pooled.Init(
            &sei.SchemaName,
            &sei.Digest,
            &sei.PrevSQLDigest,
            &sei.PlanDigest,
            &sei.ResourceGroupName,
            user,
        );
        let key = pooled.Hash().to_vec();

        // 已有条目则就地累加；否则新建，LRU 满时并入 other 淘汰桶。
        if let Some(summary) = self.summaryMap.get_mut(&key) {
            summary.isInternal = summary.isInternal && sei.IsInternal;
            summary.add(sei, beginTime, intervalSeconds, historySize, max_sql_length);
            StmtDigestKeyPool.Put(pooled);
        } else {
            let mut summary = stmtSummaryByDigest {
                isInternal: sei.IsInternal,
                ..Default::default()
            };
            if !summary.add(sei, beginTime, intervalSeconds, historySize, max_sql_length) {
                StmtDigestKeyPool.Put(pooled);
                return;
            }
            if let Some((evicted_key, evicted_summary)) = self.summaryMap.push(key, summary) {
                self.currentWindowEvictedCount += 1;
                let evicted_key = StmtDigestKey { hash: evicted_key };
                self.other
                    .AddEvicted(Some(&evicted_key), Some(&evicted_summary), historySize);
            }
            StmtDigestKeyPool.Put(pooled);
        }
        self.updateMetricsLocked();
    }

    /// 清空全部摘要与淘汰聚合。
    pub fn Clear(&mut self) {
        self.clearLocked();
    }

    /// 已持锁路径下的清空实现。
    fn clearLocked(&mut self) {
        self.summaryMap.clear();
        self.other.Clear();
        self.beginTimeForCurInterval = 0;
        self.currentWindowEvictedCount = 0;
        self.updateMetricsLocked();
    }

    /// 仅移除标记为内部查询的摘要条目。
    fn clearInternal(&mut self) {
        let keys = self
            .summaryMap
            .iter()
            .filter(|(_, summary)| summary.isInternal)
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        for key in keys {
            self.summaryMap.pop(&key);
        }
        self.updateMetricsLocked();
    }

    /// 关闭历史时清空各 digest 的历史队列，仅保留当前区间（队头）。
    fn clearHistory(&mut self) {
        for (_, summary) in self.summaryMap.iter_mut() {
            let first = summary.history.front().cloned();
            summary.history.clear();
            if let Some(first) = first {
                summary.history.push_front(first);
            }
        }
    }

    /// 设置是否启用语句摘要；关闭时清空数据。
    pub fn SetEnabled(&mut self, value: bool) -> GoResult<()> {
        self.optEnabled.store(value, Ordering::SeqCst);
        if !value {
            self.Clear();
        }
        Ok(())
    }
    /// 是否启用语句摘要。
    pub fn Enabled(&self) -> bool {
        self.optEnabled.load(Ordering::SeqCst)
    }

    /// 设置是否汇总内部 SQL；关闭时清除内部条目。
    pub fn SetEnabledInternalQuery(&mut self, value: bool) -> GoResult<()> {
        self.optEnableInternalQuery.store(value, Ordering::SeqCst);
        if !value {
            self.clearInternal();
        }
        Ok(())
    }
    /// 是否汇总内部查询。
    pub fn EnabledInternal(&self) -> bool {
        self.optEnableInternalQuery.load(Ordering::SeqCst)
    }

    /// 设置是否保留历史区间；关闭时裁剪历史。
    pub fn SetHistoryEnabled(&mut self, value: bool) -> GoResult<()> {
        self.optHistoryEnabled.store(value, Ordering::SeqCst);
        if !value {
            self.clearHistory();
        }
        Ok(())
    }
    /// 是否启用历史窗口。
    pub fn historyEnabled(&self) -> bool {
        self.optHistoryEnabled.load(Ordering::SeqCst)
    }

    /// 设置刷新区间（秒）；与 Go 一致，参数校验由系统变量层负责。
    pub fn SetRefreshInterval(&self, value: i64) -> GoResult<()> {
        self.optRefreshInterval.store(value, Ordering::SeqCst);
        Ok(())
    }
    /// 当前刷新区间（秒）。
    pub fn refreshInterval(&self) -> i64 {
        self.optRefreshInterval.load(Ordering::SeqCst)
    }

    /// 设置保留的历史区间个数；与 Go 一致，参数校验由调用方负责。
    pub fn SetHistorySize(&self, value: i32) -> GoResult<()> {
        self.optHistorySize.store(value, Ordering::SeqCst);
        Ok(())
    }
    /// 历史区间保留个数。
    pub fn historySize(&self) -> i32 {
        self.optHistorySize.load(Ordering::SeqCst)
    }

    /// 设置是否按用户维度分组；变更时清空映射以免键语义混用。
    pub fn SetGroupByUser(&mut self, value: bool) -> GoResult<()> {
        if self.GroupByUser() != value {
            self.optGroupByUser.store(value, Ordering::SeqCst);
            self.clearLocked();
        }
        Ok(())
    }
    /// 是否按用户分组。
    pub fn GroupByUser(&self) -> bool {
        self.optGroupByUser.load(Ordering::SeqCst)
    }

    /// 调整 LRU 容量（至少为 1）并同步指标。
    pub fn SetMaxStmtCount(&mut self, value: u32) -> GoResult<()> {
        // Go 在调用 SetCapacity 前先更新 optMaxStmtCount；即使容量为 0
        // 导致 SetCapacity 报错，读取配置仍会看到 0。
        self.optMaxStmtCount.store(value, Ordering::SeqCst);
        let Some(capacity) = NonZeroUsize::new(value as usize) else {
            return Err("capacity of lru cache should be at least 1".to_owned());
        };
        self.summaryMap.resize(capacity);
        self.updateMetricsLocked();
        Ok(())
    }
    /// 当前 LRU 容量。
    pub fn maxStmtCount(&self) -> i32 {
        self.optMaxStmtCount.load(Ordering::SeqCst) as i32
    }

    /// 设置 SQL 截断长度；校验由系统变量层负责。
    pub fn SetMaxSQLLength(&self, value: i32) -> GoResult<()> {
        self.optMaxSQLLength.store(value, Ordering::SeqCst);
        Ok(())
    }
    /// 当前 SQL 截断长度。
    pub fn maxSQLLength(&self) -> i32 {
        self.optMaxSQLLength.load(Ordering::SeqCst)
    }

    /// 当前映射中的摘要条数。
    pub fn Len(&self) -> usize {
        self.summaryMap.len()
    }
    /// 当前刷新窗口内已淘汰次数。
    pub fn CurrentWindowEvictedCount(&self) -> i64 {
        self.currentWindowEvictedCount
    }
    /// 克隆返回全部 digest 摘要（快照）。
    pub fn Summaries(&self) -> Vec<stmtSummaryByDigest> {
        self.summaryMap
            .iter()
            .map(|(_, summary)| summary.clone())
            .collect()
    }
    /// 测试钩子：注入“当前时间”，避免依赖真实时钟。
    pub fn set_now_for_test(&mut self, now: Option<i64>) {
        self.nowForTest = now;
    }

    /// 更新窗口记录数与淘汰次数指标。
    fn updateMetricsLocked(&self) {
        WINDOW_RECORD_COUNT.set(self.summaryMap.len() as f64);
        WINDOW_EVICTED_COUNT.set(self.currentWindowEvictedCount as f64);
    }
}

impl stmtSummaryByDigest {
    /// 首次见到该 digest 时初始化元数据与累计统计。
    fn init(&mut self, sei: &StmtExecInfo, max_sql_length: usize) -> bool {
        let Some(stats) = newStmtSummaryStats(sei, max_sql_length) else {
            return false;
        };
        // 从逻辑计划表列表拼 db.table 串，忽略空表名。
        let tables = sei
            .StmtCtx
            .LogicalPlanTables()
            .iter()
            .filter(|table| !table.Table.is_empty())
            .map(|table| format!("{}.{}", table.DB.to_lowercase(), table.Table.to_lowercase()))
            .collect::<Vec<_>>()
            .join(",");
        self.cumulative = *stats;
        self.schemaName = sei.SchemaName.clone();
        self.digest = sei.Digest.clone();
        self.planDigest = if sei.PlanDigest.is_empty() {
            sei.LazyInfo.GetPlanDigest()
        } else {
            sei.PlanDigest.clone()
        };
        self.stmtType = sei.StmtCtx.StmtType.clone();
        self.normalizedSQL = format_sql_with_limit(&sei.NormalizedSQL, max_sql_length);
        self.tableNames = tables;
        self.history.clear();
        self.initialized = true;
        (self.bindingSQL, self.bindingDigest) = sei.LazyInfo.GetBindingSQLAndDigest();
        true
    }

    /// 累加一次执行到累计统计与当前/新建历史区间。
    fn add(
        &mut self,
        sei: &StmtExecInfo,
        beginTime: i64,
        intervalSeconds: i64,
        historySize: usize,
        max_sql_length: usize,
    ) -> bool {
        if !self.initialized && !self.init(sei, max_sql_length) {
            return false;
        }
        let warningCount = sei.StmtCtx.WarningCount() as i32;
        let affectedRows = sei.StmtCtx.AffectedRows();
        self.cumulative.add(sei, warningCount, affectedRows);
        // 落在同一 beginTime 则并入队尾；否则先让旧区间 onExpire，再开新区间。
        if let Some(last) = self.history.back_mut() {
            if last.beginTime >= beginTime {
                last.add(sei, intervalSeconds, warningCount, affectedRows);
                return true;
            }
            last.onExpire(intervalSeconds);
        }
        if let Some(element) = newStmtSummaryByDigestElement(
            sei,
            beginTime,
            intervalSeconds,
            warningCount,
            affectedRows,
            max_sql_length,
        ) {
            self.history.push_back(element);
        } else {
            return false;
        }
        // 超出历史容量时丢弃最旧区间，至少保留 1 个。
        while self.history.len() > historySize && self.history.len() > 1 {
            self.history.pop_front();
        }
        true
    }

    /// 按请求的历史深度取出区间元素副本。
    pub fn collectHistorySummaries(&self, historySize: usize) -> Vec<stmtSummaryByDigestElement> {
        self.history.iter().take(historySize).cloned().collect()
    }
}

/// 编码计划/二进制计划写入摘要前的大小上限（超限则丢弃占位）。
pub const MaxEncodedPlanSizeInBytes: usize = 1024 * 1024;

/// 从一次执行信息构造初始 `stmtSummaryStats`；计划编码失败时返回 `None`。
pub fn newStmtSummaryStats(
    sei: &StmtExecInfo,
    max_sql_length: usize,
) -> Option<Box<stmtSummaryStats>> {
    let (mut samplePlan, planHint, error) = sei.LazyInfo.GetEncodedPlan();
    if error.is_some() {
        return None;
    }
    // 超大编码计划用占位串，避免撑爆摘要存储。
    if samplePlan.len() > MaxEncodedPlanSizeInBytes {
        samplePlan = "[discard]".to_owned();
    }
    let mut binaryPlan = sei.LazyInfo.GetBinaryPlan();
    if binaryPlan.len() > MaxEncodedPlanSizeInBytes {
        binaryPlan = binary_plan_discarded_encoded();
    }
    Some(Box::new(stmtSummaryStats {
        sampleSQL: format_sql_with_limit(&sei.LazyInfo.GetOriginalSQL(), max_sql_length),
        charset: sei.Charset.clone(),
        collation: sei.Collation.clone(),
        prevSQL: sei.PrevSQL.clone(),
        samplePlan,
        sampleBinaryPlan: binaryPlan,
        planHint,
        indexNames: sei
            .StmtCtx
            .IndexNames
            .lock()
            .expect("statement index names lock poisoned")
            .clone(),
        minLatency: sei.TotalLatency,
        firstSeen: sei.StartTime,
        lastSeen: sei.StartTime,
        backoffTypes: HashMap::new(),
        authUsers: HashSet::new(),
        prepared: sei.Prepared,
        minResultRows: i64::MAX,
        resourceGroupName: sei.ResourceGroupName.clone(),
        ..Default::default()
    }))
}

/// 构造某个时间窗口的摘要元素并计入首次执行。
pub fn newStmtSummaryByDigestElement(
    sei: &StmtExecInfo,
    beginTime: i64,
    intervalSeconds: i64,
    warningCount: i32,
    affectedRows: u64,
    max_sql_length: usize,
) -> Option<stmtSummaryByDigestElement> {
    let stats = newStmtSummaryStats(sei, max_sql_length)?;
    let mut element = stmtSummaryByDigestElement {
        beginTime,
        stmtSummaryStats: *stats,
        ..Default::default()
    };
    element.add(sei, intervalSeconds, warningCount, affectedRows);
    Some(element)
}

impl stmtSummaryByDigestElement {
    /// 区间滚动时校正 `endTime`，对齐刷新边界或当前时间。
    fn onExpire(&mut self, intervalSeconds: i64) {
        let target = self.beginTime + intervalSeconds;
        if target > self.endTime {
            self.endTime = target;
        } else if target < self.endTime {
            let now = unix_now();
            if now > target {
                self.endTime = now;
            }
        }
    }

    /// 更新本区间结束时间并累加统计。
    fn add(
        &mut self,
        sei: &StmtExecInfo,
        intervalSeconds: i64,
        warningCount: i32,
        affectedRows: u64,
    ) {
        self.endTime = self.beginTime + intervalSeconds;
        self.stmtSummaryStats.add(sei, warningCount, affectedRows);
    }
}

impl stmtSummaryStats {
    /// 将一次执行的时延、Coprocessor、提交、缓存命中、资源与网络指标并入聚合。
    fn add(&mut self, sei: &StmtExecInfo, warningCount: i32, affectedRows: u64) {
        if !sei.User.is_empty() {
            self.authUsers.insert(sei.User.clone());
        }
        self.execCount += 1;
        if !sei.Succeed {
            self.sumErrors += 1;
        }
        self.sumWarnings += warningCount;

        self.sumLatency += sei.TotalLatency;
        self.maxLatency = self.maxLatency.max(sei.TotalLatency);
        self.minLatency = self.minLatency.min(sei.TotalLatency);
        self.sumParseLatency += sei.ParseLatency;
        self.maxParseLatency = self.maxParseLatency.max(sei.ParseLatency);
        self.sumCompileLatency += sei.CompileLatency;
        self.maxCompileLatency = self.maxCompileLatency.max(sei.CompileLatency);

        // Coprocessor（协处理器）任务汇总：处理/等待时间与最慢地址。
        if let Some(cop) = &sei.CopTasks {
            self.sumNumCopTasks += cop.NumCopTasks as i64;
            self.sumCopProcessTime += cop.TotProcessTime;
            if cop.MaxProcessTime > self.maxCopProcessTime {
                self.maxCopProcessTime = cop.MaxProcessTime;
                self.maxCopProcessAddress = cop.MaxProcessAddress.clone();
            }
            self.sumCopWaitTime += cop.TotWaitTime;
            if cop.MaxWaitTime > self.maxCopWaitTime {
                self.maxCopWaitTime = cop.MaxWaitTime;
                self.maxCopWaitAddress = cop.MaxWaitAddress.clone();
            }
        }

        let detail = &sei.ExecDetail.CopExecDetails;
        self.sumProcessTime += detail.TimeDetail.ProcessTime;
        self.maxProcessTime = self.maxProcessTime.max(detail.TimeDetail.ProcessTime);
        self.sumWaitTime += detail.TimeDetail.WaitTime;
        self.maxWaitTime = self.maxWaitTime.max(detail.TimeDetail.WaitTime);
        self.sumBackoffTime += detail.BackoffTime;
        self.maxBackoffTime = self.maxBackoffTime.max(detail.BackoffTime);
        if let Some(scan) = &detail.ScanDetail {
            self.sumTotalKeys += scan.TotalKeys;
            self.maxTotalKeys = self.maxTotalKeys.max(scan.TotalKeys);
            self.sumProcessedKeys += scan.ProcessedKeys;
            self.maxProcessedKeys = self.maxProcessedKeys.max(scan.ProcessedKeys);
            self.sumRocksdbDeleteSkippedCount += scan.RocksdbDeleteSkippedCount;
            self.maxRocksdbDeleteSkippedCount = self
                .maxRocksdbDeleteSkippedCount
                .max(scan.RocksdbDeleteSkippedCount);
            self.sumRocksdbKeySkippedCount += scan.RocksdbKeySkippedCount;
            self.maxRocksdbKeySkippedCount = self
                .maxRocksdbKeySkippedCount
                .max(scan.RocksdbKeySkippedCount);
            self.sumRocksdbBlockCacheHitCount += scan.RocksdbBlockCacheHitCount;
            self.maxRocksdbBlockCacheHitCount = self
                .maxRocksdbBlockCacheHitCount
                .max(scan.RocksdbBlockCacheHitCount);
            self.sumRocksdbBlockReadCount += scan.RocksdbBlockReadCount;
            self.maxRocksdbBlockReadCount = self
                .maxRocksdbBlockReadCount
                .max(scan.RocksdbBlockReadCount);
            self.sumRocksdbBlockReadByte += scan.RocksdbBlockReadByte;
            self.maxRocksdbBlockReadByte =
                self.maxRocksdbBlockReadByte.max(scan.RocksdbBlockReadByte);
        }

        // 两阶段提交细节：prewrite/commit、拿 commitTS、resolve lock、写键等。
        if let Some(commit) = &sei.ExecDetail.CommitDetail {
            self.commitCount += 1;
            self.sumPrewriteTime += commit.PrewriteTime;
            self.maxPrewriteTime = self.maxPrewriteTime.max(commit.PrewriteTime);
            self.sumCommitTime += commit.CommitTime;
            self.maxCommitTime = self.maxCommitTime.max(commit.CommitTime);
            self.sumGetCommitTsTime += commit.GetCommitTsTime;
            self.maxGetCommitTsTime = self.maxGetCommitTsTime.max(commit.GetCommitTsTime);
            let resolve = commit.ResolveLock.ResolveLockTime.load(Ordering::Relaxed);
            self.sumResolveLockTime += resolve;
            self.maxResolveLockTime = self.maxResolveLockTime.max(resolve);
            self.sumLocalLatchTime += commit.LocalLatchTime;
            self.maxLocalLatchTime = self.maxLocalLatchTime.max(commit.LocalLatchTime);
            self.sumWriteKeys += commit.WriteKeys as i64;
            self.maxWriteKeys = self.maxWriteKeys.max(commit.WriteKeys);
            self.sumWriteSize += commit.WriteSize as i64;
            self.maxWriteSize = self.maxWriteSize.max(commit.WriteSize);
            let regions = commit.PrewriteRegionNum.load(Ordering::Relaxed);
            self.sumPrewriteRegionNum += regions as i64;
            self.maxPrewriteRegionNum = self.maxPrewriteRegionNum.max(regions);
            self.sumTxnRetry += commit.TxnRetry as i64;
            self.maxTxnRetry = self.maxTxnRetry.max(commit.TxnRetry);
            let mu = commit.Mu.Lock();
            self.sumCommitBackoffTime += mu.CommitBackoffTime;
            self.maxCommitBackoffTime = self.maxCommitBackoffTime.max(mu.CommitBackoffTime);
            self.sumBackoffTimes +=
                (mu.PrewriteBackoffTypes.len() + mu.CommitBackoffTypes.len()) as i64;
            for backoff in mu.PrewriteBackoffTypes.iter().chain(&mu.CommitBackoffTypes) {
                *self.backoffTypes.entry(backoff.clone()).or_default() += 1;
            }
        }

        self.planInCache = sei.PlanInCache;
        if sei.PlanInCache {
            self.planCacheHits += 1;
        }
        if !sei.PlanCacheUnqualified.is_empty() {
            self.planCacheUnqualifiedCount += 1;
            self.lastPlanCacheUnqualified = sei.PlanCacheUnqualified.clone();
        }
        self.planInBinding = sei.PlanInBinding;
        self.sumAffectedRows += affectedRows;
        self.sumMem += sei.MemMax;
        self.maxMem = self.maxMem.max(sei.MemMax);
        self.sumMemArbitration += sei.MemArbitration;
        self.maxMemArbitration = self.maxMemArbitration.max(sei.MemArbitration);
        self.sumDisk += sei.DiskMax;
        self.maxDisk = self.maxDisk.max(sei.DiskMax);
        self.firstSeen = self.firstSeen.min(sei.StartTime);
        self.lastSeen = self.lastSeen.max(sei.StartTime);
        if sei.ExecRetryCount > 0 {
            self.execRetryCount += sei.ExecRetryCount;
            self.execRetryTime += sei.ExecRetryTime;
        }
        if sei.ResultRows > 0 {
            self.sumResultRows += sei.ResultRows;
            self.maxResultRows = self.maxResultRows.max(sei.ResultRows);
            self.minResultRows = self.minResultRows.min(sei.ResultRows);
        } else {
            self.minResultRows = 0;
        }
        self.sumKVTotal += atomic_duration(
            sei.TiKVExecDetails
                .WaitKVRespDuration
                .load(Ordering::Relaxed),
        );
        self.sumPDTotal += atomic_duration(
            sei.TiKVExecDetails
                .WaitPDRespDuration
                .load(Ordering::Relaxed),
        );
        self.sumBackoffTotal +=
            atomic_duration(sei.TiKVExecDetails.BackoffDuration.load(Ordering::Relaxed));
        self.sumWriteSQLRespTotal += sei.WriteSQLRespDuration;
        self.sumTidbCPU += sei.CPUUsages.TidbCPUTime;
        self.sumTikvCPU += sei.CPUUsages.TikvCPUTime;
        self.StmtNetworkTrafficSummary
            .Add(Some(&sei.TiKVExecDetails));
        self.StmtRUSummary.Add(sei.RUDetail.as_ref(), sei.TotalRUV2);
        self.storageKV = sei.StmtCtx.IsTiKV.load(Ordering::Relaxed);
        self.storageMPP = sei.StmtCtx.IsTiFlash.load(Ordering::Relaxed);
    }

    /// 合并另一份聚合统计（用于淘汰桶并入等场景）。
    fn merge(&mut self, other: &stmtSummaryStats) {
        // 自身尚无执行记录时直接覆盖，避免 min/max 被零值污染。
        if self.execCount == 0 {
            *self = other.clone();
            return;
        }
        self.execCount += other.execCount;
        self.sumErrors += other.sumErrors;
        self.sumWarnings += other.sumWarnings;
        self.sumLatency += other.sumLatency;
        self.maxLatency = self.maxLatency.max(other.maxLatency);
        self.minLatency = self.minLatency.min(other.minLatency);
        self.sumParseLatency += other.sumParseLatency;
        self.maxParseLatency = self.maxParseLatency.max(other.maxParseLatency);
        self.sumCompileLatency += other.sumCompileLatency;
        self.maxCompileLatency = self.maxCompileLatency.max(other.maxCompileLatency);
        self.sumNumCopTasks += other.sumNumCopTasks;
        self.sumCopProcessTime += other.sumCopProcessTime;
        if other.maxCopProcessTime > self.maxCopProcessTime {
            self.maxCopProcessTime = other.maxCopProcessTime;
            self.maxCopProcessAddress = other.maxCopProcessAddress.clone();
        }
        self.sumCopWaitTime += other.sumCopWaitTime;
        if other.maxCopWaitTime > self.maxCopWaitTime {
            self.maxCopWaitTime = other.maxCopWaitTime;
            self.maxCopWaitAddress = other.maxCopWaitAddress.clone();
        }
        self.sumProcessTime += other.sumProcessTime;
        self.maxProcessTime = self.maxProcessTime.max(other.maxProcessTime);
        self.sumWaitTime += other.sumWaitTime;
        self.maxWaitTime = self.maxWaitTime.max(other.maxWaitTime);
        self.sumBackoffTime += other.sumBackoffTime;
        self.maxBackoffTime = self.maxBackoffTime.max(other.maxBackoffTime);
        self.sumTotalKeys += other.sumTotalKeys;
        self.maxTotalKeys = self.maxTotalKeys.max(other.maxTotalKeys);
        self.sumProcessedKeys += other.sumProcessedKeys;
        self.maxProcessedKeys = self.maxProcessedKeys.max(other.maxProcessedKeys);
        self.sumRocksdbDeleteSkippedCount += other.sumRocksdbDeleteSkippedCount;
        self.maxRocksdbDeleteSkippedCount = self
            .maxRocksdbDeleteSkippedCount
            .max(other.maxRocksdbDeleteSkippedCount);
        self.sumRocksdbKeySkippedCount += other.sumRocksdbKeySkippedCount;
        self.maxRocksdbKeySkippedCount = self
            .maxRocksdbKeySkippedCount
            .max(other.maxRocksdbKeySkippedCount);
        self.sumRocksdbBlockCacheHitCount += other.sumRocksdbBlockCacheHitCount;
        self.maxRocksdbBlockCacheHitCount = self
            .maxRocksdbBlockCacheHitCount
            .max(other.maxRocksdbBlockCacheHitCount);
        self.sumRocksdbBlockReadCount += other.sumRocksdbBlockReadCount;
        self.maxRocksdbBlockReadCount = self
            .maxRocksdbBlockReadCount
            .max(other.maxRocksdbBlockReadCount);
        self.sumRocksdbBlockReadByte += other.sumRocksdbBlockReadByte;
        self.maxRocksdbBlockReadByte = self
            .maxRocksdbBlockReadByte
            .max(other.maxRocksdbBlockReadByte);
        self.commitCount += other.commitCount;
        self.sumGetCommitTsTime += other.sumGetCommitTsTime;
        self.maxGetCommitTsTime = self.maxGetCommitTsTime.max(other.maxGetCommitTsTime);
        self.sumPrewriteTime += other.sumPrewriteTime;
        self.maxPrewriteTime = self.maxPrewriteTime.max(other.maxPrewriteTime);
        self.sumCommitTime += other.sumCommitTime;
        self.maxCommitTime = self.maxCommitTime.max(other.maxCommitTime);
        self.sumLocalLatchTime += other.sumLocalLatchTime;
        self.maxLocalLatchTime = self.maxLocalLatchTime.max(other.maxLocalLatchTime);
        self.sumCommitBackoffTime += other.sumCommitBackoffTime;
        self.maxCommitBackoffTime = self.maxCommitBackoffTime.max(other.maxCommitBackoffTime);
        self.sumResolveLockTime += other.sumResolveLockTime;
        self.maxResolveLockTime = self.maxResolveLockTime.max(other.maxResolveLockTime);
        self.sumWriteKeys += other.sumWriteKeys;
        self.maxWriteKeys = self.maxWriteKeys.max(other.maxWriteKeys);
        self.sumWriteSize += other.sumWriteSize;
        self.maxWriteSize = self.maxWriteSize.max(other.maxWriteSize);
        self.sumPrewriteRegionNum += other.sumPrewriteRegionNum;
        self.maxPrewriteRegionNum = self.maxPrewriteRegionNum.max(other.maxPrewriteRegionNum);
        self.sumTxnRetry += other.sumTxnRetry;
        self.maxTxnRetry = self.maxTxnRetry.max(other.maxTxnRetry);
        self.sumBackoffTimes += other.sumBackoffTimes;
        for (kind, count) in &other.backoffTypes {
            *self.backoffTypes.entry(kind.clone()).or_default() += count;
        }
        self.authUsers.extend(other.authUsers.iter().cloned());
        self.sumMem += other.sumMem;
        self.maxMem = self.maxMem.max(other.maxMem);
        self.sumDisk += other.sumDisk;
        self.maxDisk = self.maxDisk.max(other.maxDisk);
        self.sumAffectedRows += other.sumAffectedRows;
        self.sumKVTotal += other.sumKVTotal;
        self.sumPDTotal += other.sumPDTotal;
        self.sumBackoffTotal += other.sumBackoffTotal;
        self.sumWriteSQLRespTotal += other.sumWriteSQLRespTotal;
        self.sumTidbCPU += other.sumTidbCPU;
        self.sumTikvCPU += other.sumTikvCPU;
        self.sumResultRows += other.sumResultRows;
        self.maxResultRows = self.maxResultRows.max(other.maxResultRows);
        self.minResultRows = self.minResultRows.min(other.minResultRows);
        self.firstSeen = self.firstSeen.min(other.firstSeen);
        self.lastSeen = self.lastSeen.max(other.lastSeen);
        self.planCacheHits += other.planCacheHits;
        self.execRetryCount += other.execRetryCount;
        self.execRetryTime += other.execRetryTime;
        self.StmtRUSummary.Merge(&other.StmtRUSummary);
        self.StmtNetworkTrafficSummary
            .Merge(Some(&other.StmtNetworkTrafficSummary));
        self.planCacheUnqualifiedCount += other.planCacheUnqualifiedCount;
        self.storageKV |= other.storageKV;
        self.storageMPP |= other.storageMPP;
        self.sumMemArbitration += other.sumMemArbitration;
        self.maxMemArbitration = self.maxMemArbitration.max(other.maxMemArbitration);
    }
}

/// 按全局最大长度截断 SQL，便于展示。
pub fn formatSQL(sql: &str) -> String {
    let max = StmtSummaryByDigestMap
        .lock()
        .expect("global statement summary map poisoned")
        .maxSQLLength();
    let max =
        usize::try_from(max).expect("negative max SQL length bypassed system-variable validation");
    format_sql_with_limit(sql, max)
}

/// 按给定上限在 UTF-8 边界截断 SQL；超长时追加长度标记。
fn format_sql_with_limit(sql: &str, max: usize) -> String {
    let length = sql.len();
    if length <= max {
        return sql.to_owned();
    }
    let mut boundary = max;
    while boundary > 0 && !sql.is_char_boundary(boundary) {
        boundary -= 1;
    }
    format!("{}(len:{length})", &sql[..boundary])
}

/// 将 backoff 类型计数格式化为字符串；空映射返回 `None`。
pub fn formatBackoffTypes(backoffMap: &HashMap<String, i32>) -> Option<String> {
    if backoffMap.is_empty() {
        return None;
    }
    let mut values = backoffMap.iter().collect::<Vec<_>>();
    values.sort_by(|(left_name, left_count), (right_name, right_count)| {
        right_count
            .cmp(left_count)
            .then_with(|| left_name.cmp(right_name))
    });
    Some(
        values
            .into_iter()
            .map(|(kind, count)| format!("{kind}:{count}"))
            .collect::<Vec<_>>()
            .join(","),
    )
}

/// 整数均值；count≤0 时返回 0。
pub fn avgInt(sum: i64, count: i64) -> i64 {
    if count > 0 { sum / count } else { 0 }
}
/// 浮点均值（由整数和计算）；count≤0 时返回 0。
pub fn avgFloat(sum: i64, count: i64) -> f64 {
    if count > 0 {
        sum as f64 / count as f64
    } else {
        0.0
    }
}
/// 浮点和的均值；count≤0 时返回 0。
pub fn avgSumFloat(sum: f64, count: i64) -> f64 {
    if count > 0 { sum / count as f64 } else { 0.0 }
}
/// 空串转为 `None`，非空则克隆为 `Some`。
pub fn convertEmptyToNil(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_owned())
}

#[derive(Clone, Debug, Default, PartialEq)]
/// Request Unit（RU，请求单元）用量汇总：读写 RU 与总量。
pub struct StmtRUSummary {
    pub SumRRU: f64,
    pub SumWRU: f64,
    pub SumRUWaitDuration: Duration,
    pub MaxRRU: f64,
    pub MaxWRU: f64,
    pub MaxRUWaitDuration: Duration,
    pub SumRUV2: f64,
    pub MaxRUV2: f64,
}

impl StmtRUSummary {
    /// 累加一次执行的 RU 明细与 v2 总量。
    pub fn Add(&mut self, info: Option<&RUDetails>, totalRUV2: f64) {
        if let Some(info) = info {
            let rru = info.RRU();
            self.SumRRU += rru;
            self.MaxRRU = self.MaxRRU.max(rru);
            let wru = info.WRU();
            self.SumWRU += wru;
            self.MaxWRU = self.MaxWRU.max(wru);
            let wait = info.RUWaitDuration();
            self.SumRUWaitDuration += wait;
            self.MaxRUWaitDuration = self.MaxRUWaitDuration.max(wait);
        }
        self.SumRUV2 += totalRUV2;
        self.MaxRUV2 = self.MaxRUV2.max(totalRUV2);
    }

    /// 合并另一份 RU 汇总。
    pub fn Merge(&mut self, other: &StmtRUSummary) {
        self.SumRRU += other.SumRRU;
        self.SumWRU += other.SumWRU;
        self.SumRUWaitDuration += other.SumRUWaitDuration;
        self.MaxRRU = self.MaxRRU.max(other.MaxRRU);
        self.MaxWRU = self.MaxWRU.max(other.MaxWRU);
        self.MaxRUWaitDuration = self.MaxRUWaitDuration.max(other.MaxRUWaitDuration);
        self.SumRUV2 += other.SumRUV2;
        self.MaxRUV2 = self.MaxRUV2.max(other.MaxRUV2);
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 语句相关的 TiKV 网络流量汇总（字节级）。
pub struct StmtNetworkTrafficSummary {
    pub UnpackedBytesSentTiKVTotal: i64,
    pub UnpackedBytesReceivedTiKVTotal: i64,
    pub UnpackedBytesSentTiKVCrossZone: i64,
    pub UnpackedBytesReceivedTiKVCrossZone: i64,
    pub UnpackedBytesSentTiFlashTotal: i64,
    pub UnpackedBytesReceivedTiFlashTotal: i64,
    pub UnpackedBytesSentTiFlashCrossZone: i64,
    pub UnpackedBytesReceivedTiFlashCrossZone: i64,
}

impl StmtNetworkTrafficSummary {
    /// 合并可选的另一份流量汇总。
    pub fn Merge(&mut self, other: Option<&StmtNetworkTrafficSummary>) {
        let Some(other) = other else {
            return;
        };
        self.UnpackedBytesSentTiKVTotal += other.UnpackedBytesSentTiKVTotal;
        self.UnpackedBytesReceivedTiKVTotal += other.UnpackedBytesReceivedTiKVTotal;
        self.UnpackedBytesSentTiKVCrossZone += other.UnpackedBytesSentTiKVCrossZone;
        self.UnpackedBytesReceivedTiKVCrossZone += other.UnpackedBytesReceivedTiKVCrossZone;
        self.UnpackedBytesSentTiFlashTotal += other.UnpackedBytesSentTiFlashTotal;
        self.UnpackedBytesReceivedTiFlashTotal += other.UnpackedBytesReceivedTiFlashTotal;
        self.UnpackedBytesSentTiFlashCrossZone += other.UnpackedBytesSentTiFlashCrossZone;
        self.UnpackedBytesReceivedTiFlashCrossZone += other.UnpackedBytesReceivedTiFlashCrossZone;
    }

    /// 从 TiKV 执行细节累加网络读写字节。
    pub fn Add(&mut self, info: Option<&TiKVExecDetails>) {
        let Some(info) = info else {
            return;
        };
        let snapshot = LoadTiKVExecDetails(Some(info));
        let traffic = &snapshot.TrafficDetails;
        self.UnpackedBytesSentTiKVTotal += traffic.UnpackedBytesSentKVTotal.load(Ordering::Relaxed);
        self.UnpackedBytesReceivedTiKVTotal +=
            traffic.UnpackedBytesReceivedKVTotal.load(Ordering::Relaxed);
        self.UnpackedBytesSentTiKVCrossZone +=
            traffic.UnpackedBytesSentKVCrossZone.load(Ordering::Relaxed);
        self.UnpackedBytesReceivedTiKVCrossZone += traffic
            .UnpackedBytesReceivedKVCrossZone
            .load(Ordering::Relaxed);
        self.UnpackedBytesSentTiFlashTotal +=
            traffic.UnpackedBytesSentMPPTotal.load(Ordering::Relaxed);
        self.UnpackedBytesReceivedTiFlashTotal += traffic
            .UnpackedBytesReceivedMPPTotal
            .load(Ordering::Relaxed);
        self.UnpackedBytesSentTiFlashCrossZone += traffic
            .UnpackedBytesSentMPPCrossZone
            .load(Ordering::Relaxed);
        self.UnpackedBytesReceivedTiFlashCrossZone += traffic
            .UnpackedBytesReceivedMPPCrossZone
            .load(Ordering::Relaxed);
    }
}

/// 当前 Unix 秒时间戳。
fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 将纳秒计数转为 `Duration`（负值按 0）。
fn atomic_duration(nanos: i64) -> Duration {
    Duration::from_nanos(nanos.max(0) as u64)
}

/// 超限二进制计划的占位编码串。
fn binary_plan_discarded_encoded() -> String {
    // tipb.ExplainData.discarded_due_to_too_long is protobuf field 4 (bool),
    // whose canonical wire encoding for true is tag 0x20 followed by 0x01.
    let protobuf = [0x20, 0x01];
    let compressed = snap::raw::Encoder::new()
        .compress_vec(&protobuf)
        .expect("small binary-plan sentinel must be snappy-compressible");
    base64::engine::general_purpose::STANDARD.encode(compressed)
}

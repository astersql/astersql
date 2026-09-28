// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// Package-wide metric initialization and registration.
//
// 包级指标总入口：一次性初始化各子系统 collector、向 Prometheus 注册，
// 以及简化模式下注销部分高开销指标。另含 gRPC channelz 采集器的生命周期管理。
// channelz 用于观测进程内 gRPC 通道与套接字拓扑。

use prometheus::core::Collector;
use prometheus::{Counter, CounterVec, Encoder, GaugeVec, Opts, TextEncoder};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex, Once};

/// 各子系统 Init*Metrics 共用的包级互斥锁，串行化 static mut 赋值。
pub static PACKAGE_INIT_LOCK: Mutex<()> = Mutex::new(());
/// 保证 InitMetrics 只执行一次的 Once。
static INIT_METRICS_ONCE: Once = Once::new();
/// InitMetrics 是否已成功完成。
static INIT_METRICS_DONE: AtomicBool = AtomicBool::new(false);
/// InitMetrics 失败时保存的错误信息。
static INIT_METRICS_ERROR: Mutex<Option<String>> = Mutex::new(None);

/// 按类型标签统计 panic 次数。
pub static PanicCounter: LazyLock<CounterVec> = LazyLock::new(|| {
    CounterVec::new(
        Opts::new("panic_total", "Counter of panic.")
            .namespace("tidb")
            .subsystem("server"),
        &[crate::LblType],
    )
    .expect("valid panic counter")
});

/// 按模块与类型标签记录内存用量。
pub static MemoryUsage: LazyLock<GaugeVec> = LazyLock::new(|| {
    GaugeVec::new(
        Opts::new("memory_usage", "Memory Usage")
            .namespace("tidb")
            .subsystem("server"),
        &[crate::LblModule, crate::LblType],
    )
    .expect("valid memory gauge")
});

// 下列常量沿用 Go 侧模块 / 作用域标签字符串，供各子系统指标打标。
/// session 模块标签。
pub const LabelSession: &str = "session";
/// domain 模块标签。
pub const LabelDomain: &str = "domain";
/// DDL owner 模块标签。
pub const LabelDDLOwner: &str = "ddl-owner";
/// DDL 模块标签。
pub const LabelDDL: &str = "ddl";
/// DDL worker 模块标签。
pub const LabelDDLWorker: &str = "ddl-worker";
/// 分布式重组（dist-reorg）模块标签。
pub const LabelDistReorg: &str = "dist-reorg";
/// DDL syncer 模块标签。
pub const LabelDDLSyncer: &str = "ddl-syncer";
/// GC worker 模块标签。GC 即垃圾回收，清理过期多版本数据。
pub const LabelGCWorker: &str = "gcworker";
/// ANALYZE 统计信息收集模块标签。
pub const LabelAnalyze: &str = "analyze";
/// worker-pool 模块标签。
pub const LabelWorkerPool: &str = "worker-pool";
/// 统计信息（stats）模块标签。
pub const LabelStats: &str = "stats";
/// 批量接收循环标签。
pub const LabelBatchRecvLoop: &str = "batch-recv-loop";
/// 批量发送循环标签。
pub const LabelBatchSendLoop: &str = "batch-send-loop";
/// 操作成功结果标签值。
pub(crate) const OP_SUCC: &str = "ok";
/// 操作失败结果标签值。
pub(crate) const OP_FAILED: &str = "err";
/// TiDB 产品名常量。
pub const TiDB: &str = "tidb";
/// 作用域标签名。
pub const LabelScope: &str = "scope";
/// 全局作用域标签值。
pub const ScopeGlobal: &str = "global";
/// 会话作用域标签值。
pub const ScopeSession: &str = "session";
/// server 子系统名。
pub const Server: &str = "server";
/// TiKV 客户端子系统名。
pub const TiKVClient: &str = "tikvclient";

/// 按错误是否存在返回 "ok" 或 "err" 结果标签。
pub fn RetLabel<E>(err: Option<&E>) -> &'static str {
    if err.is_none() { OP_SUCC } else { OP_FAILED }
}

/// 一次性初始化全部子系统指标；并发重入时复用首次结果。
pub unsafe fn InitMetrics() -> Result<(), prometheus::Error> {
    // Go runs InitMetrics from package init() once. Concurrent re-entry from
    // cargo tests must not drop/replace HistogramVec while other threads read.
    // Go 在包 init 中只跑一次；测试并发重入时不得替换仍在被读的 HistogramVec。
    INIT_METRICS_ONCE.call_once(|| {
        // 按 Go InitMetrics 原有顺序依次初始化各子系统 collector。
        let result = (|| -> Result<(), prometheus::Error> {
            crate::bindinfo::init_bind_info_metrics();
            crate::ddl::InitDDLMetrics();
            crate::distsql::InitDistSQLMetrics();
            crate::domain::InitDomainMetrics();
            crate::executor::InitExecutorMetrics();
            crate::gc_worker::InitGCWorkerMetrics();
            crate::log_backup::InitLogBackupMetrics();
            crate::meta::init_meta_metrics();
            crate::owner::init_owner_metrics();
            crate::rawkv::init_raw_kv_metrics();
            crate::resourcemanager::InitResourceManagerMetrics();
            crate::server::InitServerMetrics();
            crate::session::InitSessionMetrics();
            crate::ru_v2::InitRUV2Metrics();
            crate::sli::InitSliMetrics();
            crate::stats::InitStatsMetrics();
            crate::telemetry::InitTelemetryMetrics()?;
            crate::topsql::InitTopSQLMetrics();
            crate::ttl::InitTTLMetrics();
            crate::external_workload::InitExternalWorkloadMetrics();
            crate::stmtsummary::InitStmtSummaryMetrics();
            astersql_dxf_framework_dxfmetric::InitDistTaskMetrics();
            astersql_ingestor_ingestmetric::InitIngestMetrics();
            crate::resource_group::init_resource_group_metrics();
            crate::globalsort::InitGlobalSortMetrics();
            crate::infoschema::InitInfoSchemaV2Metrics();
            crate::memory::InitMemoryMetrics();
            astersql_timer_metrics::InitTimerMetrics();
            crate::br::InitBRMetrics();
            Ok(())
        })();
        // 失败时记录错误字符串；成功则置位完成标志供后续查询。
        if let Err(err) = result {
            *INIT_METRICS_ERROR.lock().unwrap() = Some(err.to_string());
        } else {
            INIT_METRICS_DONE.store(true, Ordering::SeqCst);
        }
    });
    if INIT_METRICS_DONE.load(Ordering::SeqCst) {
        Ok(())
    } else {
        let message = INIT_METRICS_ERROR
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| "InitMetrics failed".to_string());
        Err(prometheus::Error::Msg(message))
    }
}

/// 克隆并注册单个 Collector 到默认 Prometheus 注册表。
fn register_clone<C>(collector: &C) -> Result<(), prometheus::Error>
where
    C: Collector + Clone + 'static,
{
    prometheus::register(Box::new(collector.clone()))
}

/// 从 Option 中取出已初始化的 Collector 再注册；未初始化则 panic。
unsafe fn register_option<C>(collector: &Option<C>) -> Result<(), prometheus::Error>
where
    C: Collector + Clone + 'static,
{
    register_clone(
        collector
            .as_ref()
            .expect("InitMetrics must run before RegisterMetrics"),
    )
}

/// 展开一组 Option collector 路径并依次 register_option。
macro_rules! register_options {
    ($($collector:path),* $(,)?) => {{
        $(register_option(&$collector)?;)*
        Ok::<(), prometheus::Error>(())
    }};
}

/// 将包内关键指标注册到默认 Prometheus 注册表，并尝试安装 channelz 采集器。
pub unsafe fn RegisterMetrics() -> Result<(), prometheus::Error> {
    register_clone(&*PanicCounter)?;
    register_clone(&*MemoryUsage)?;
    // 以下列表对应 Go RegisterMetrics 中逐项 Register 的 collector。
    register_options!(
        crate::ddl::BatchAddIdxHistogram,
        crate::ddl::DDLCounter,
        crate::ddl::BackfillTotalCounter,
        crate::ddl::BackfillProgressGauge,
        crate::ddl::DDLWorkerHistogram,
        crate::ddl::DDLJobTableDuration,
        crate::ddl::DDLRunningJobCount,
        crate::ddl::DeploySyncerHistogram,
        crate::ddl::HandleJobHistogram,
        crate::ddl::JobsGauge,
        crate::ddl::OwnerHandleSyncerHistogram,
        crate::ddl::UpdateSelfVersionHistogram,
        crate::ddl::AddIndexScanRate,
        crate::ddl::RetryableErrorCount,
        crate::distsql::DistSQLPartialCountHistogram,
        crate::distsql::DistSQLCoprCacheCounter,
        crate::distsql::DistSQLCoprClosestReadCounter,
        crate::distsql::DistSQLCoprRespBodySize,
        crate::distsql::DistSQLQueryHistogram,
        crate::distsql::DistSQLScanKeysHistogram,
        crate::distsql::DistSQLScanKeysPartialHistogram,
        crate::domain::LoadPrivilegeCounter,
        crate::domain::LeaseExpireTime,
        crate::domain::LoadSchemaCounter,
        crate::domain::LoadSchemaDuration,
        crate::domain::HandleSchemaValidate,
        crate::domain::LoadSysVarCacheCounter,
        crate::executor::ExecutorCounter,
        crate::executor::AffectedRowsCounter,
        crate::executor::ExecPhaseDuration,
        crate::executor::OngoingTxnDurationHistogram,
        crate::executor::MppCoordinatorStats,
        crate::executor::MppCoordinatorLatency,
        crate::executor::NetworkTransmissionStats,
        crate::executor::IndexLookUpExecutorDuration,
        crate::executor::IndexLookRowsCounter,
        crate::executor::IndexLookUpExecutorRowNumber,
        crate::executor::IndexLookUpCopTaskCount,
        crate::log_backup::LastCheckpoint,
        crate::log_backup::ExternalStorageCheckpoint,
        crate::log_backup::AdvancerOwner,
        crate::log_backup::AdvancerTickDuration,
        crate::log_backup::GetCheckpointBatchSize,
        crate::log_backup::RegionCheckpointRequest,
        crate::log_backup::RegionCheckpointFailure,
        crate::log_backup::RegionCheckpointSubscriptionEvent,
        crate::log_backup::LogBackupCurrentLastRegionID,
        crate::log_backup::LogBackupCurrentLastRegionLeaderStoreID,
        crate::globalsort::GlobalSortWriteToCloudStorageDuration,
        crate::globalsort::GlobalSortWriteToCloudStorageRate,
        crate::globalsort::GlobalSortReadFromCloudStorageDuration,
        crate::globalsort::GlobalSortReadFromCloudStorageRate,
        crate::globalsort::GlobalSortIngestWorkerCnt,
        crate::globalsort::GlobalSortUploadWorkerCount,
        crate::globalsort::MergeSortWriteBytes,
        crate::globalsort::MergeSortReadBytes,
        crate::infoschema::InfoSchemaV2CacheCounter,
        crate::infoschema::InfoSchemaV2CacheMemUsage,
        crate::infoschema::InfoSchemaV2CacheMemLimit,
        crate::infoschema::InfoSchemaV2CacheObjCnt,
        crate::infoschema::TableByNameDuration,
        crate::br::RestoreTableCreatedCount,
        crate::br::RestoreImportFileSeconds,
        crate::br::RestoreUploadSSTForPiTRSeconds,
        crate::br::RestoreUploadSSTMetaForPiTRSeconds,
        crate::br::MetaKVBatchFiles,
        crate::br::MetaKVBatchFilteredKeys,
        crate::br::MetaKVBatchKeys,
        crate::br::MetaKVBatchSize,
        crate::br::KVApplyBatchDuration,
        crate::br::KVApplyBatchFiles,
        crate::br::KVApplyBatchRegions,
        crate::br::KVApplyBatchSize,
        crate::br::KVApplyRegionFiles,
        crate::memory::GlobalMemArbitrationDuration,
        crate::memory::GlobalMemArbitratorWorkMode,
        crate::memory::GlobalMemArbitratorQuota,
        crate::memory::GlobalMemArbitratorWaitingTask,
        crate::memory::GlobalMemArbitratorRuntimeMemMagnifi,
        crate::memory::GlobalMemArbitratorRootPool,
        crate::memory::GlobalMemArbitratorEventCounter,
        crate::memory::GlobalMemArbitratorTaskExecCounter,
        crate::owner::NEW_SESSION_HISTOGRAM,
        crate::owner::WATCH_OWNER_COUNTER,
        crate::owner::CAMPAIGN_OWNER_COUNTER,
        crate::rawkv::RAW_KV_BATCH_PUT_DURATION_SECONDS,
        crate::rawkv::RAW_KV_BATCH_PUT_BATCH_SIZE,
        crate::resource_group::RUNAWAY_CHECKER_COUNTER,
        crate::resource_group::RUNAWAY_FLUSHER_COUNTER,
        crate::resource_group::RUNAWAY_FLUSHER_ADD_COUNTER,
        crate::resource_group::RUNAWAY_FLUSHER_BATCH_SIZE_HISTOGRAM,
        crate::resource_group::RUNAWAY_FLUSHER_DURATION_HISTOGRAM,
        crate::resource_group::RUNAWAY_FLUSHER_INTERVAL_HISTOGRAM,
        crate::resource_group::RUNAWAY_SYNCER_DURATION_HISTOGRAM,
        crate::resource_group::RUNAWAY_SYNCER_INTERVAL_HISTOGRAM,
        crate::resource_group::RUNAWAY_SYNCER_CHECKPOINT_GAUGE,
        crate::resource_group::RUNAWAY_SYNCER_COUNTER,
        crate::resourcemanager::EMACPUUsageGauge,
        crate::resourcemanager::PoolConcurrencyCounter,
        crate::ru_v2::RUV2ResultChunkCells,
        crate::ru_v2::RUV2ExecutorL1,
        crate::ru_v2::RUV2ExecutorL2,
        crate::ru_v2::RUV2ExecutorL3,
        crate::ru_v2::RUV2ExecutorL5InsertRows,
        crate::ru_v2::RUV2PlanCnt,
        crate::ru_v2::RUV2PlanDeriveStatsPaths,
        crate::ru_v2::RUV2ResourceManagerReadCnt,
        crate::ru_v2::RUV2ResourceManagerWriteCnt,
        crate::ru_v2::RUV2WriteKeys,
        crate::ru_v2::RUV2WriteSize,
        crate::ru_v2::RUV2SessionParserTotal,
        crate::ru_v2::RUV2TxnCnt,
        crate::ru_v2::RUV2TiKVKVEngineCacheMiss,
        crate::ru_v2::RUV2TiKVCoprocessorExecutorIterations,
        crate::ru_v2::RUV2TiKVCoprocessorResponseBytes,
        crate::ru_v2::RUV2TiKVRaftstoreStoreWriteTriggerWB,
        crate::ru_v2::RUV2TiKVStorageProcessedKeysBatchGet,
        crate::ru_v2::RUV2TiKVStorageProcessedKeysGet,
        crate::ru_v2::RUV2TiKVCoprocessorWorkTotal,
        crate::executor::StmtNodeCounter,
        crate::executor::DbStmtNodeCounter,
        crate::server::PacketIOCounter,
        crate::server::QueryDurationHistogram,
        crate::server::QueryRPCHistogram,
        crate::server::QueryProcessedKeyHistogram,
        crate::server::QueryTotalCounter,
        crate::server::ConnGauge,
        crate::server::DisconnectionCounter,
        crate::server::PreparedStmtGauge,
        crate::server::ExecuteErrorCounter,
        crate::server::CriticalErrorCounter,
        crate::server::ServerEventCounter,
        crate::server::TimeJumpBackCounter,
        crate::server::PlanCacheCounter,
        crate::server::PlanCacheMissCounter,
        crate::server::PlanCacheInstanceMemoryUsage,
        crate::server::PlanCacheInstancePlanNumCounter,
        crate::server::PlanCacheProcessDuration,
        crate::server::ReadFromTableCacheCounter,
        crate::server::HandShakeErrorCounter,
        crate::server::GetTokenDurationHistogram,
        crate::server::NumOfMultiQueryHistogram,
        crate::server::TotalQueryProcHistogram,
        crate::server::TotalCopProcHistogram,
        crate::server::TotalCopWaitHistogram,
        crate::server::CopMVCCRatioHistogram,
        crate::server::SlowQueryCounter,
        crate::server::MaxProcs,
        crate::server::GOGC,
        crate::server::ConnIdleDurationHistogram,
        crate::server::ServerInfo,
        crate::server::TokenGauge,
        crate::server::ConfigStatus,
        crate::server::TiFlashQueryTotalCounter,
        crate::server::TiFlashFailedMPPStoreState,
        crate::server::PDAPIExecutionHistogram,
        crate::server::PDAPIRequestCounter,
        crate::server::CPUProfileCounter,
        crate::server::LoadTableCacheDurationHistogram,
        crate::server::RCCheckTSWriteConfilictCounter,
        crate::server::MemoryLimit,
        crate::server::InternalSessions,
        crate::server::ActiveUser,
        crate::server::TLSVersion,
        crate::server::TLSCipher,
        crate::session::AutoIDReqDuration,
        crate::session::SessionExecuteParseDuration,
        crate::session::SessionExecuteCompileDuration,
        crate::session::SessionExecuteRunDuration,
        crate::session::SchemaLeaseErrorCounter,
        crate::session::SessionRetry,
        crate::session::SessionRetryErrorCounter,
        crate::session::SessionRestrictedSQLCounter,
        crate::session::StatementPerTransaction,
        crate::session::TransactionDuration,
        crate::session::StatementDeadlockDetectDuration,
        crate::session::StatementPessimisticRetryCount,
        crate::session::StatementLockKeysCount,
        crate::session::StatementSharedLockKeysCount,
        crate::session::ValidateReadTSFromPDCount,
        crate::session::NonTransactionalDMLCount,
        crate::session::TxnStatusEnteringCounter,
        crate::session::TxnDurationHistogram,
        crate::session::LazyPessimisticUniqueCheckSetCount,
        crate::session::PessimisticDMLDurationByAttempt,
        crate::session::ResourceGroupQueryTotalCounter,
        crate::session::FairLockingUsageCount,
        crate::session::PessimisticLockKeysDuration,
        crate::sli::SmallTxnWriteDuration,
        crate::sli::TxnWriteThroughput,
        crate::stats::AutoAnalyzeHistogram,
        crate::stats::AutoAnalyzeCounter,
        crate::stats::ManualAnalyzeCounter,
        crate::stats::StatsInaccuracyRate,
        crate::stats::PseudoEstimation,
        crate::stats::SyncLoadCounter,
        crate::stats::SyncLoadTimeoutCounter,
        crate::stats::SyncLoadDedupCounter,
        crate::stats::SyncLoadHistogram,
        crate::stats::ReadStatsHistogram,
        crate::stats::StatsCacheCounter,
        crate::stats::StatsCacheGauge,
        crate::stats::StatsHealthyGauge,
        crate::stats::StatsDeltaLoadHistogram,
        crate::stats::StatsDeltaUpdateHistogram,
        crate::stats::StatsUsageUpdateHistogram,
        crate::stats::HistoricalStatsCounter,
        crate::stats::PlanReplayerTaskCounter,
        crate::stats::PlanReplayerRegisterTaskGauge,
        crate::stmtsummary::StmtSummaryWindowRecordCount,
        crate::stmtsummary::StmtSummaryWindowEvictedCount,
        crate::stmtsummary::StmtSummaryEvictedLogCounter,
        crate::topsql::TopSQLIgnoredCounter,
        crate::topsql::TopSQLReportDurationHistogram,
        crate::topsql::TopSQLReportDataHistogram,
        crate::ttl::TTLQueryDuration,
        crate::ttl::TTLProcessedExpiredRowsCounter,
        crate::ttl::TTLJobStatus,
        crate::ttl::TTLTaskStatus,
        crate::ttl::TTLPhaseTime,
        crate::ttl::TTLInsertRowsCount,
        crate::ttl::TTLWatermarkDelay,
        crate::ttl::TTLEventCounter,
        crate::external_workload::ExternalWorkloadTaskCounter,
    )?;
    register_external_metrics(prometheus::default_registry())?;
    crate::tikv_client_metrics::RegisterMetrics(prometheus::default_registry())?;
    setup_channelz_collector()?;
    Ok(())
}

/// Register collectors owned by packages that Go wires through pkg/metrics.
pub(crate) fn register_external_metrics(
    registry: &prometheus::Registry,
) -> Result<(), prometheus::Error> {
    astersql_dxf_framework_dxfmetric::Register(registry)?;
    let ingest = astersql_ingestor_ingestmetric::WriteIngestAPIDuration
        .read()
        .expect("ingest metrics lock poisoned")
        .clone()
        .expect("InitMetrics must run before RegisterMetrics");
    registry.register(Box::new(ingest))?;
    let timer = astersql_timer_metrics::TimerEventCounter
        .get()
        .expect("InitMetrics must run before RegisterMetrics")
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
        .expect("InitMetrics must run before RegisterMetrics");
    registry.register(Box::new(timer))?;
    Ok(())
}

/// 批量注册任意 Collector 列表。
pub fn Register(collectors: Vec<Box<dyn Collector>>) -> Result<(), prometheus::Error> {
    for collector in collectors {
        prometheus::register(collector)?;
    }
    Ok(())
}

/// 采集默认注册表并编码为 Prometheus text exposition format。
///
/// Linux 上 rust-prometheus 会在该注册表自动注册进程 CPU、RSS、
/// 文件描述符和 OS 线程数等采集器。
pub fn GatherText() -> Result<(Vec<u8>, String), prometheus::Error> {
    let encoder = TextEncoder::new();
    let mut body = Vec::new();
    encoder.encode(&prometheus::gather(), &mut body)?;
    Ok((body, encoder.format_type().to_owned()))
}

/// 批量注销 Collector；忽略单项失败。
pub fn Unregister(collectors: Vec<Box<dyn Collector>>) {
    for collector in collectors {
        let _ = prometheus::unregister(collector);
    }
}

/// 当前是否处于简化指标模式。
static MODE: Mutex<bool> = Mutex::new(false);

/// 切换简化模式：simplified=true 时注销部分高开销指标，否则重新注册。
pub unsafe fn ToggleSimplifiedMode(simplified: bool) -> Result<(), prometheus::Error> {
    let mut current = MODE.lock().expect("metrics mode mutex poisoned");
    if *current == simplified {
        return Ok(());
    }
    *current = simplified;

    // 按指标当前状态注册或注销；与 Go toggle 列表保持一致。
    macro_rules! toggle {
        ($metric:expr) => {{
            let metric = $metric
                .as_ref()
                .expect("InitMetrics must run first")
                .clone();
            if simplified {
                let _ = prometheus::unregister(Box::new(metric));
            } else {
                prometheus::register(Box::new(metric))?;
            }
        }};
    }
    toggle!(crate::session::StatementDeadlockDetectDuration);
    toggle!(crate::session::ValidateReadTSFromPDCount);
    toggle!(crate::server::LoadTableCacheDurationHistogram);
    toggle!(crate::sli::TxnWriteThroughput);
    toggle!(crate::sli::SmallTxnWriteDuration);
    toggle!(crate::domain::InfoCacheCounters);
    toggle!(crate::server::ReadFromTableCacheCounter);
    toggle!(crate::server::TiFlashQueryTotalCounter);
    toggle!(crate::server::TiFlashFailedMPPStoreState);
    toggle!(crate::owner::CAMPAIGN_OWNER_COUNTER);
    toggle!(crate::session::NonTransactionalDMLCount);
    if simplified {
        let _ = prometheus::unregister(Box::new((*MemoryUsage).clone()));
    } else {
        prometheus::register(Box::new((*MemoryUsage).clone()))?;
    }
    toggle!(crate::server::TokenGauge);
    for collector in crate::tikv_client_metrics::unused_collectors() {
        if simplified {
            let _ = prometheus::unregister(collector);
        } else {
            prometheus::register(collector)?;
        }
    }
    Ok(())
}

type GrpcChannelzCollector = crate::channelz::ChannelzCollector;

#[cfg(test)]
pub(crate) fn collect_channelz_snapshots_for_test(
    top_channels: &str,
    snapshots: &[(&str, u64, &str)],
) -> Vec<prometheus::proto::MetricFamily> {
    crate::channelz::collect_snapshots_for_test(top_channels, snapshots)
}

/// channelz 采集器单例状态：实例与是否已注册。
#[derive(Default)]
pub(crate) struct ChannelzState {
    collector: Option<GrpcChannelzCollector>,
    registered: bool,
}
/// 进程级 channelz 采集器状态容器。
static GRPC_CHANNELZ_COLLECTOR: LazyLock<Mutex<ChannelzState>> =
    LazyLock::new(|| Mutex::new(ChannelzState::default()));
/// Serializes unit tests that intentionally mutate the process-wide channelz singleton.
#[cfg(test)]
pub(crate) static GRPC_CHANNELZ_TEST_LOCK: Mutex<()> = Mutex::new(());

/// Corresponds to Go `initGrpcChannelzCollectorLocked`; caller must hold the mutex.
/// 对应 Go `initGrpcChannelzCollectorLocked`；调用方须已持有互斥锁。
pub(crate) fn init_grpc_channelz_collector_locked(
    state: &mut ChannelzState,
) -> Result<(), prometheus::Error> {
    if state.collector.is_some() {
        return Ok(());
    }
    state.collector = Some(GrpcChannelzCollector::new()?);
    Ok(())
}

/// Corresponds to Go `setupChannelzCollector`: skipped entirely while running
/// under cargo test / intest (Go `intest.InTest`).
/// 对应 Go `setupChannelzCollector`：在 cargo test / intest 下整段跳过。
pub(crate) fn setup_channelz_collector() -> Result<(), prometheus::Error> {
    // Dependencies are not built with cfg(test); `InTest` covers feature-gated builds,
    // while cfg!(test) covers unit tests of this crate itself (Go intest build tag).
    // 依赖包未必带 cfg(test)；InTest 覆盖 feature 构建，cfg!(test) 覆盖本 crate 单测。
    if cfg!(test) || astersql_util_intest::InTest.load(std::sync::atomic::Ordering::SeqCst) {
        return Ok(());
    }

    let mut state = GRPC_CHANNELZ_COLLECTOR
        .lock()
        .expect("channelz mutex poisoned");
    if let Err(err) = init_grpc_channelz_collector_locked(&mut state) {
        // Go logs a warning and returns; keep registration best-effort.
        // Go 仅打警告后返回；注册保持尽力而为。
        let _ = err;
        return Ok(());
    }
    if state.registered {
        return Ok(());
    }
    register_clone(state.collector.as_ref().unwrap())?;
    state.registered = true;
    Ok(())
}

/// 判断 gRPC target 是否为内部 bufnet 测试通道。
pub fn is_internal_channelz_target(target: &str) -> bool {
    target == "bufnet" || target == "passthrough:///bufnet"
}

/// 判断 socket 是否缺少 remote（内部/空套接字）。
pub fn is_internal_channelz_socket(remote: Option<&str>, remote_name: &str) -> bool {
    remote.is_none() && remote_name.is_empty()
}

/// 测试辅助：停止并清空 channelz 采集器单例状态。
pub fn cleanup_grpc_channelz_collector_for_test() {
    let mut state = GRPC_CHANNELZ_COLLECTOR
        .lock()
        .expect("channelz mutex poisoned");
    stop_grpc_channelz_collector_locked(&mut state);
}

/// 若已注册则先注销，再重置 ChannelzState。
fn stop_grpc_channelz_collector_locked(state: &mut ChannelzState) {
    if state.registered {
        if let Some(collector) = state.collector.as_ref() {
            let _ = prometheus::unregister(Box::new(collector.clone()));
        }
    }
    *state = ChannelzState::default();
}

/// Test helper: lock, init, and return a clone of the singleton collector.
/// 测试辅助：加锁后对单例状态执行回调。
pub(crate) fn with_grpc_channelz_collector_locked<R>(f: impl FnOnce(&mut ChannelzState) -> R) -> R {
    let mut state = GRPC_CHANNELZ_COLLECTOR
        .lock()
        .expect("channelz mutex poisoned");
    f(&mut state)
}

impl ChannelzState {
    /// 返回当前采集器引用（若已创建）。
    pub(crate) fn collector(&self) -> Option<&GrpcChannelzCollector> {
        self.collector.as_ref()
    }

    /// 是否已注册到默认 Prometheus 注册表。
    pub(crate) fn registered(&self) -> bool {
        self.registered
    }
}

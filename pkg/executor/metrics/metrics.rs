// Copyright 2023 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// 执行器侧 Prometheus 指标预绑定与 phase 耗时观察表。
//
// 对应 Go `executor/metrics`：在向量指标上按标签（general/internal、phase 名等）
// 取出具体 Observer/Counter/Gauge，供语句执行、两阶段提交（2PC）、MPP 协调
// 与 IndexLookUp 等路径直接打点。MVCC（多版本并发控制）相关直方图亦在此绑定。

use std::collections::HashMap;

use crate::bindinfo::compat_prometheus as prometheus;
use crate::metrics;

/// 执行阶段名：锁定读路径上的 build（构建执行器）。
// phases
pub const PhaseBuildLocking: &str = "build:locking";
/// 锁定读路径上的 open（打开执行器）。
pub const PhaseOpenLocking: &str = "open:locking";
/// 锁定读路径上的 next（拉取一批行）。
pub const PhaseNextLocking: &str = "next:locking";
/// 锁定读路径上的加锁阶段。
pub const PhaseLockLocking: &str = "lock:locking";
/// 最终执行路径上的 build。
pub const PhaseBuildFinal: &str = "build:final";
/// 最终执行路径上的 open。
pub const PhaseOpenFinal: &str = "open:final";
/// 最终执行路径上的 next。
pub const PhaseNextFinal: &str = "next:final";
/// 最终执行路径上的加锁阶段。
pub const PhaseLockFinal: &str = "lock:final";
/// 两阶段提交的 prewrite（预写）阶段。
pub const PhaseCommitPrewrite: &str = "commit:prewrite";
/// 两阶段提交的 commit（提交）阶段。
pub const PhaseCommitCommit: &str = "commit:commit";
/// 等待分配 commit-ts（提交时间戳）。
pub const PhaseCommitWaitCommitTS: &str = "commit:wait:commit-ts";
/// 等待拿到最新时间戳。
pub const PhaseCommitWaitLatestTS: &str = "commit:wait:latest-ts";
/// 等待本地 latch（闩锁，串行化冲突键）。
pub const PhaseCommitWaitLatch: &str = "commit:wait:local-latch";
/// 等待 prewrite binlog 写出。
pub const PhaseCommitWaitBinlog: &str = "commit:wait:prewrite-binlog";
/// 向客户端写回结果。
pub const PhaseWriteResponse: &str = "write-response";

/// Sends adapter phase durations to the same prebound Prometheus histograms as Go.
pub fn RecordPhaseDuration(phase: &str, internal: bool, duration: std::time::Duration) {
    let label = match phase {
        "build_final" => PhaseBuildFinal,
        "build_locking" => PhaseBuildLocking,
        "open_final" => PhaseOpenFinal,
        "open_locking" => PhaseOpenLocking,
        "next_final" => PhaseNextFinal,
        "next_locking" => PhaseNextLocking,
        "lock_final" => PhaseLockFinal,
        "lock_locking" => PhaseLockLocking,
        "commit_prewrite" => PhaseCommitPrewrite,
        "commit_commit" => PhaseCommitCommit,
        "commit_wait_commit_ts" => PhaseCommitWaitCommitTS,
        "commit_wait_latest_ts" => PhaseCommitWaitLatestTS,
        "commit_wait_latch" => PhaseCommitWaitLatch,
        "commit_wait_binlog" => PhaseCommitWaitBinlog,
        _ => return,
    };
    unsafe {
        let map = if internal {
            &PhaseDurationObserverMapInternal
        } else {
            &PhaseDurationObserverMap
        };
        if let Some(observer) = map.as_ref().and_then(|map| map.get(label)) {
            observer.observe(duration.as_secs_f64());
        }
    }
}

// executor metrics vars
// Go 的包级 var 在 init 阶段赋值；用 Option 表示初始化前为空，避免声称已经接线 Prometheus。
/// 普通会话查询处理耗时直方图。
pub static mut TotalQueryProcHistogramGeneral: Option<prometheus::Observer> = None;
/// 普通会话 Coprocessor（下推到 TiKV 的算子）处理耗时。
pub static mut TotalCopProcHistogramGeneral: Option<prometheus::Observer> = None;
/// 普通会话 Coprocessor 排队等待耗时。
pub static mut TotalCopWaitHistogramGeneral: Option<prometheus::Observer> = None;
/// 普通会话 Coprocessor 读到的 MVCC 版本比例。
pub static mut CopMVCCRatioHistogramGeneral: Option<prometheus::Observer> = None;
/// 普通会话慢查询计数。
pub static mut SlowQueryCounterGeneral: Option<prometheus::Counter> = None;
/// 内部会话查询处理耗时直方图。
pub static mut TotalQueryProcHistogramInternal: Option<prometheus::Observer> = None;
/// 内部会话 Coprocessor 处理耗时。
pub static mut TotalCopProcHistogramInternal: Option<prometheus::Observer> = None;
/// 内部会话 Coprocessor 等待耗时。
pub static mut TotalCopWaitHistogramInternal: Option<prometheus::Observer> = None;
/// 内部会话慢查询计数。
pub static mut SlowQueryCounterInternal: Option<prometheus::Counter> = None;

/// `SELECT ... FOR UPDATE` 首次尝试耗时。
pub static mut SelectForUpdateFirstAttemptDuration: Option<prometheus::Observer> = None;
/// `SELECT ... FOR UPDATE` 重试耗时。
pub static mut SelectForUpdateRetryDuration: Option<prometheus::Observer> = None;
/// 悲观 DML（数据修改语句）首次尝试耗时。
pub static mut DmlFirstAttemptDuration: Option<prometheus::Observer> = None;
/// 悲观 DML 重试耗时。
pub static mut DmlRetryDuration: Option<prometheus::Observer> = None;

// Fair locking 相关计数器按事务/语句、使用/生效四个标签预绑定。
/// Fair locking：事务级“已使用”计数。
pub static mut FairLockingTxnUsedCount: Option<prometheus::Counter> = None;
/// Fair locking：语句级“已使用”计数。
pub static mut FairLockingStmtUsedCount: Option<prometheus::Counter> = None;
/// Fair locking：事务级“实际生效”计数。
pub static mut FairLockingTxnEffectiveCount: Option<prometheus::Counter> = None;
/// Fair locking：语句级“实际生效”计数。
pub static mut FairLockingStmtEffectiveCount: Option<prometheus::Counter> = None;

/// Records the same statement and committed-transaction fair locking labels as Go FinishExecuteStmt.
pub fn RecordFairLockingFinishMetrics(
    stmt_used: bool,
    stmt_effective: bool,
    txn_used: bool,
    txn_effective: bool,
) {
    // InitMetricsVars installs the bound handles before statement execution.
    unsafe {
        if stmt_used {
            if let Some(counter) = &FairLockingStmtUsedCount {
                counter.inc();
            }
        }
        if stmt_effective {
            if let Some(counter) = &FairLockingStmtEffectiveCount {
                counter.inc();
            }
        }
        if txn_used {
            if let Some(counter) = &FairLockingTxnUsedCount {
                counter.inc();
            }
        }
        if txn_effective {
            if let Some(counter) = &FairLockingTxnEffectiveCount {
                counter.inc();
            }
        }
    }
}

/// Observes Go Exec defer's retry and exclusive/shared lock statistics.
pub fn RecordExecLockMetrics(
    retries: usize,
    exclusive_keys: i32,
    shared_keys: i32,
    exclusive_duration: std::time::Duration,
    pessimistic_lock_started: bool,
) {
    unsafe {
        if retries > 0 {
            if let Some(histogram) = &crate::session::StatementPessimisticRetryCount {
                histogram.observe(retries as f64);
            }
        }
        if exclusive_keys > 0 {
            if let Some(histogram) = &crate::session::StatementLockKeysCount {
                histogram.observe(exclusive_keys as f64);
            }
        }
        if shared_keys > 0 {
            if let Some(histogram) = &crate::session::StatementSharedLockKeysCount {
                histogram.observe(shared_keys as f64);
            }
        }
        if pessimistic_lock_started && exclusive_duration > std::time::Duration::ZERO {
            if let Some(histogram) = &crate::session::PessimisticLockKeysDuration {
                histogram.observe(exclusive_duration.as_secs_f64());
            }
        }
    }
}

/// Records TiFlash completion and table-cache use from formal statement flags.
pub fn RecordSupplementaryFinishMetrics(
    tiflash: bool,
    success: bool,
    rfc_error_code: Option<&str>,
    read_from_table_cache: bool,
) {
    unsafe {
        if tiflash {
            if success {
                if let Some(counter) = &TotalTiFlashQuerySuccCounter {
                    counter.inc();
                }
            } else if let Some(counter) = &crate::server::TiFlashQueryTotalCounter {
                let label = crate::server::ExecuteErrorToLabel(rfc_error_code);
                counter
                    .with_label_values(&[label.as_str(), metrics::LblError])
                    .inc();
            }
        }
        if read_from_table_cache {
            if let Some(counter) = &crate::server::ReadFromTableCacheCounter {
                counter.inc();
            }
        }
    }
}

/// Observes Go SessionVars.GetExecuteDuration in the internal or general histogram.
pub fn RecordStatementExecuteRunDuration(internal: bool, duration: std::time::Duration) {
    unsafe {
        let observer = if internal {
            &SessionExecuteRunDurationInternal
        } else {
            &SessionExecuteRunDurationGeneral
        };
        if let Some(observer) = observer {
            observer.observe(duration.as_secs_f64());
        }
    }
}

/// 各执行器类型创建次数（MergeJoin 等）。
pub static mut ExecutorCounterMergeJoinExec: Option<prometheus::Counter> = None;
pub static mut ExecutorCountHashJoinExec: Option<prometheus::Counter> = None;
pub static mut ExecutorCounterHashAggExec: Option<prometheus::Counter> = None;
pub static mut ExecutorStreamAggExec: Option<prometheus::Counter> = None;
pub static mut ExecutorCounterSortExec: Option<prometheus::Counter> = None;
pub static mut ExecutorCounterTopNExec: Option<prometheus::Counter> = None;
pub static mut ExecutorCounterNestedLoopApplyExec: Option<prometheus::Counter> = None;
pub static mut ExecutorCounterIndexLookUpJoin: Option<prometheus::Counter> = None;
pub static mut ExecutorCounterIndexLookUpExecutor: Option<prometheus::Counter> = None;
pub static mut ExecutorCounterIndexMergeReaderExecutor: Option<prometheus::Counter> = None;

/// 会话执行 Run 阶段耗时（internal/general）。
pub static mut SessionExecuteRunDurationInternal: Option<prometheus::Observer> = None;
pub static mut SessionExecuteRunDurationGeneral: Option<prometheus::Observer> = None;
/// TiFlash（列存分析引擎）查询成功总次数。
pub static mut TotalTiFlashQuerySuccCounter: Option<prometheus::Counter> = None;

/// 各执行 phase 在 general（internal=0）标签下的耗时 Observer。
pub static mut ExecBuildLocking: Option<prometheus::Observer> = None;
pub static mut ExecOpenLocking: Option<prometheus::Observer> = None;
pub static mut ExecNextLocking: Option<prometheus::Observer> = None;
pub static mut ExecLockLocking: Option<prometheus::Observer> = None;
pub static mut ExecBuildFinal: Option<prometheus::Observer> = None;
pub static mut ExecOpenFinal: Option<prometheus::Observer> = None;
pub static mut ExecNextFinal: Option<prometheus::Observer> = None;
pub static mut ExecLockFinal: Option<prometheus::Observer> = None;
pub static mut ExecCommitPrewrite: Option<prometheus::Observer> = None;
pub static mut ExecCommitCommit: Option<prometheus::Observer> = None;
pub static mut ExecCommitWaitCommitTS: Option<prometheus::Observer> = None;
pub static mut ExecCommitWaitLatestTS: Option<prometheus::Observer> = None;
pub static mut ExecCommitWaitLatch: Option<prometheus::Observer> = None;
pub static mut ExecCommitWaitBinlog: Option<prometheus::Observer> = None;
pub static mut ExecWriteResponse: Option<prometheus::Observer> = None;
pub static mut ExecUnknown: Option<prometheus::Observer> = None;

/// 各执行 phase 在 internal（internal=1）标签下的耗时 Observer。
pub static mut ExecBuildLockingInternal: Option<prometheus::Observer> = None;
pub static mut ExecOpenLockingInternal: Option<prometheus::Observer> = None;
pub static mut ExecNextLockingInternal: Option<prometheus::Observer> = None;
pub static mut ExecLockLockingInternal: Option<prometheus::Observer> = None;
pub static mut ExecBuildFinalInternal: Option<prometheus::Observer> = None;
pub static mut ExecOpenFinalInternal: Option<prometheus::Observer> = None;
pub static mut ExecNextFinalInternal: Option<prometheus::Observer> = None;
pub static mut ExecLockFinalInternal: Option<prometheus::Observer> = None;
pub static mut ExecCommitPrewriteInternal: Option<prometheus::Observer> = None;
pub static mut ExecCommitCommitInternal: Option<prometheus::Observer> = None;
pub static mut ExecCommitWaitCommitTSInternal: Option<prometheus::Observer> = None;
pub static mut ExecCommitWaitLatestTSInternal: Option<prometheus::Observer> = None;
pub static mut ExecCommitWaitLatchInternal: Option<prometheus::Observer> = None;
pub static mut ExecCommitWaitBinlogInternal: Option<prometheus::Observer> = None;
pub static mut ExecWriteResponseInternal: Option<prometheus::Observer> = None;
pub static mut ExecUnknownInternal: Option<prometheus::Observer> = None;

/// 事务回滚耗时：悲观/乐观 × internal/general。
pub static mut TransactionDurationPessimisticRollbackInternal: Option<prometheus::Observer> = None;
pub static mut TransactionDurationPessimisticRollbackGeneral: Option<prometheus::Observer> = None;
pub static mut TransactionDurationOptimisticRollbackInternal: Option<prometheus::Observer> = None;
pub static mut TransactionDurationOptimisticRollbackGeneral: Option<prometheus::Observer> = None;

/// phase 名 → Observer，供按阶段名统一打点（general）。
pub static mut PhaseDurationObserverMap: Option<HashMap<&'static str, prometheus::Observer>> = None;
/// phase 名 → Observer（internal）。
pub static mut PhaseDurationObserverMapInternal: Option<
    HashMap<&'static str, prometheus::Observer>,
> = None;

/// MPP 协调器：累计注册/活跃/超时/未收到 report 等 Gauge，以及收报延迟。
pub static mut MppCoordinatorStatsTotalRegisteredNumber: Option<prometheus::Gauge> = None;
pub static mut MppCoordinatorStatsActiveNumber: Option<prometheus::Gauge> = None;
pub static mut MppCoordinatorStatsOverTimeNumber: Option<prometheus::Gauge> = None;
pub static mut MppCoordinatorStatsReportNotReceived: Option<prometheus::Gauge> = None;
pub static mut MppCoordinatorLatencyRcvReport: Option<prometheus::Observer> = None;

/// 与 TiKV/TiFlash 间网络传输字节计数（含跨可用区）。
pub static mut ExecutorNetworkTransmissionSentTiKVTotal: Option<prometheus::Counter> = None;
pub static mut ExecutorNetworkTransmissionSentTiKVCrossZone: Option<prometheus::Counter> = None;
pub static mut ExecutorNetworkTransmissionReceivedTiKVTotal: Option<prometheus::Counter> = None;
pub static mut ExecutorNetworkTransmissionReceivedTiKVCrossZone: Option<prometheus::Counter> = None;
pub static mut ExecutorNetworkTransmissionSentTiFlashTotal: Option<prometheus::Counter> = None;
pub static mut ExecutorNetworkTransmissionSentTiFlashCrossZone: Option<prometheus::Counter> = None;
pub static mut ExecutorNetworkTransmissionReceivedTiFlashTotal: Option<prometheus::Counter> = None;
pub static mut ExecutorNetworkTransmissionReceivedTiFlashCrossZone: Option<prometheus::Counter> =
    None;

/// IndexLookUp（先扫索引再回表）行数、下推命中/未命中与 Cop 任务计数。
pub static mut IndexLookUpNormalRowsCounter: Option<prometheus::Counter> = None;
pub static mut IndexLookUpPushDownRowsCounterHit: Option<prometheus::Counter> = None;
pub static mut IndexLookUpPushDownRowsCounterMiss: Option<prometheus::Counter> = None;
pub static mut IndexLookUpExecutorWithPushDownEnabledRowNumber: Option<prometheus::Observer> = None;
pub static mut IndexLookUpExecutorWithPushDownEnabledDuration: Option<prometheus::Observer> = None;
pub static mut IndexLookUpIndexScanCopTasksNormal: Option<prometheus::Counter> = None;
pub static mut IndexLookUpIndexScanCopTasksWithPushDownEnabled: Option<prometheus::Counter> = None;

/// 初始化入口：对应 Go 的 init；Rust 无包级 init，需显式调用。
// init 对应 Go 的 init 函数；Rust 没有包级 init，这里保留显式入口。
pub fn init() {
    InitMetricsVars();
    InitPhaseDurationObserverMap();
}

/// 将 Prometheus 向量按标签预绑定到包级静态变量。
// InitMetricsVars init executor metrics vars.
pub fn InitMetricsVars() {
    // 从已初始化的向量指标取出带标签的子指标。
    macro_rules! bind {
        ($metric:ident; $($label:expr),+ $(,)?) => {{
            metrics::$metric
                .as_ref()
                .expect(concat!(stringify!($metric), " must be initialized first"))
                .with_label_values(&[$($label),+])
        }};
    }

    // 必须在 server/session/executor 向量 Init* 之后调用。
    unsafe {
        TotalQueryProcHistogramGeneral = Some(bind!(TotalQueryProcHistogram; metrics::LblGeneral));
        TotalCopProcHistogramGeneral = Some(bind!(TotalCopProcHistogram; metrics::LblGeneral));
        TotalCopWaitHistogramGeneral = Some(bind!(TotalCopWaitHistogram; metrics::LblGeneral));
        CopMVCCRatioHistogramGeneral = Some(bind!(CopMVCCRatioHistogram; metrics::LblGeneral));
        SlowQueryCounterGeneral = Some(bind!(SlowQueryCounter; metrics::LblGeneral));
        TotalQueryProcHistogramInternal =
            Some(bind!(TotalQueryProcHistogram; metrics::LblInternal));
        TotalCopProcHistogramInternal = Some(bind!(TotalCopProcHistogram; metrics::LblInternal));
        TotalCopWaitHistogramInternal = Some(bind!(TotalCopWaitHistogram; metrics::LblInternal));
        SlowQueryCounterInternal = Some(bind!(SlowQueryCounter; metrics::LblInternal));

        SelectForUpdateFirstAttemptDuration =
            Some(bind!(PessimisticDMLDurationByAttempt; "select-for-update", "first-attempt"));
        SelectForUpdateRetryDuration =
            Some(bind!(PessimisticDMLDurationByAttempt; "select-for-update", "retry"));
        DmlFirstAttemptDuration =
            Some(bind!(PessimisticDMLDurationByAttempt; "dml", "first-attempt"));
        DmlRetryDuration = Some(bind!(PessimisticDMLDurationByAttempt; "dml", "retry"));

        FairLockingTxnUsedCount =
            Some(bind!(FairLockingUsageCount; metrics::LblFairLockingTxnUsed));
        FairLockingStmtUsedCount =
            Some(bind!(FairLockingUsageCount; metrics::LblFairLockingStmtUsed));
        FairLockingTxnEffectiveCount =
            Some(bind!(FairLockingUsageCount; metrics::LblFairLockingTxnEffective));
        FairLockingStmtEffectiveCount =
            Some(bind!(FairLockingUsageCount; metrics::LblFairLockingStmtEffective));

        ExecutorCounterMergeJoinExec = Some(bind!(ExecutorCounter; "MergeJoinExec"));
        ExecutorCountHashJoinExec = Some(bind!(ExecutorCounter; "HashJoinExec"));
        ExecutorCounterHashAggExec = Some(bind!(ExecutorCounter; "HashAggExec"));
        ExecutorStreamAggExec = Some(bind!(ExecutorCounter; "StreamAggExec"));
        ExecutorCounterSortExec = Some(bind!(ExecutorCounter; "SortExec"));
        ExecutorCounterTopNExec = Some(bind!(ExecutorCounter; "TopNExec"));
        ExecutorCounterNestedLoopApplyExec = Some(bind!(ExecutorCounter; "NestedLoopApplyExec"));
        ExecutorCounterIndexLookUpJoin = Some(bind!(ExecutorCounter; "IndexLookUpJoin"));
        ExecutorCounterIndexLookUpExecutor = Some(bind!(ExecutorCounter; "IndexLookUpExecutor"));
        ExecutorCounterIndexMergeReaderExecutor =
            Some(bind!(ExecutorCounter; "IndexMergeReaderExecutor"));

        SessionExecuteRunDurationInternal =
            Some(bind!(SessionExecuteRunDuration; metrics::LblInternal));
        SessionExecuteRunDurationGeneral =
            Some(bind!(SessionExecuteRunDuration; metrics::LblGeneral));
        TotalTiFlashQuerySuccCounter = Some(bind!(TiFlashQueryTotalCounter; "", metrics::LblOK));

        ExecBuildLocking = Some(bind!(ExecPhaseDuration; PhaseBuildLocking, "0"));
        ExecOpenLocking = Some(bind!(ExecPhaseDuration; PhaseOpenLocking, "0"));
        ExecNextLocking = Some(bind!(ExecPhaseDuration; PhaseNextLocking, "0"));
        ExecLockLocking = Some(bind!(ExecPhaseDuration; PhaseLockLocking, "0"));
        ExecBuildFinal = Some(bind!(ExecPhaseDuration; PhaseBuildFinal, "0"));
        ExecOpenFinal = Some(bind!(ExecPhaseDuration; PhaseOpenFinal, "0"));
        ExecNextFinal = Some(bind!(ExecPhaseDuration; PhaseNextFinal, "0"));
        ExecLockFinal = Some(bind!(ExecPhaseDuration; PhaseLockFinal, "0"));
        ExecCommitPrewrite = Some(bind!(ExecPhaseDuration; PhaseCommitPrewrite, "0"));
        ExecCommitCommit = Some(bind!(ExecPhaseDuration; PhaseCommitCommit, "0"));
        ExecCommitWaitCommitTS = Some(bind!(ExecPhaseDuration; PhaseCommitWaitCommitTS, "0"));
        ExecCommitWaitLatestTS = Some(bind!(ExecPhaseDuration; PhaseCommitWaitLatestTS, "0"));
        ExecCommitWaitLatch = Some(bind!(ExecPhaseDuration; PhaseCommitWaitLatch, "0"));
        ExecCommitWaitBinlog = Some(bind!(ExecPhaseDuration; PhaseCommitWaitBinlog, "0"));
        ExecWriteResponse = Some(bind!(ExecPhaseDuration; PhaseWriteResponse, "0"));
        ExecUnknown = Some(bind!(ExecPhaseDuration; "unknown", "0"));

        ExecBuildLockingInternal = Some(bind!(ExecPhaseDuration; PhaseBuildLocking, "1"));
        ExecOpenLockingInternal = Some(bind!(ExecPhaseDuration; PhaseOpenLocking, "1"));
        ExecNextLockingInternal = Some(bind!(ExecPhaseDuration; PhaseNextLocking, "1"));
        ExecLockLockingInternal = Some(bind!(ExecPhaseDuration; PhaseLockLocking, "1"));
        ExecBuildFinalInternal = Some(bind!(ExecPhaseDuration; PhaseBuildFinal, "1"));
        ExecOpenFinalInternal = Some(bind!(ExecPhaseDuration; PhaseOpenFinal, "1"));
        ExecNextFinalInternal = Some(bind!(ExecPhaseDuration; PhaseNextFinal, "1"));
        ExecLockFinalInternal = Some(bind!(ExecPhaseDuration; PhaseLockFinal, "1"));
        ExecCommitPrewriteInternal = Some(bind!(ExecPhaseDuration; PhaseCommitPrewrite, "1"));
        ExecCommitCommitInternal = Some(bind!(ExecPhaseDuration; PhaseCommitCommit, "1"));
        ExecCommitWaitCommitTSInternal =
            Some(bind!(ExecPhaseDuration; PhaseCommitWaitCommitTS, "1"));
        ExecCommitWaitLatestTSInternal =
            Some(bind!(ExecPhaseDuration; PhaseCommitWaitLatestTS, "1"));
        ExecCommitWaitLatchInternal = Some(bind!(ExecPhaseDuration; PhaseCommitWaitLatch, "1"));
        ExecCommitWaitBinlogInternal = Some(bind!(ExecPhaseDuration; PhaseCommitWaitBinlog, "1"));
        ExecWriteResponseInternal = Some(bind!(ExecPhaseDuration; PhaseWriteResponse, "1"));
        ExecUnknownInternal = Some(bind!(ExecPhaseDuration; "unknown", "1"));

        TransactionDurationPessimisticRollbackInternal = Some(
            bind!(TransactionDuration; metrics::LblPessimistic, metrics::LblRollback, metrics::LblInternal),
        );
        TransactionDurationPessimisticRollbackGeneral = Some(
            bind!(TransactionDuration; metrics::LblPessimistic, metrics::LblRollback, metrics::LblGeneral),
        );
        TransactionDurationOptimisticRollbackInternal = Some(
            bind!(TransactionDuration; metrics::LblOptimistic, metrics::LblRollback, metrics::LblInternal),
        );
        TransactionDurationOptimisticRollbackGeneral = Some(
            bind!(TransactionDuration; metrics::LblOptimistic, metrics::LblRollback, metrics::LblGeneral),
        );

        MppCoordinatorStatsTotalRegisteredNumber = Some(bind!(MppCoordinatorStats; "total"));
        MppCoordinatorStatsActiveNumber = Some(bind!(MppCoordinatorStats; "active"));
        MppCoordinatorStatsOverTimeNumber = Some(bind!(MppCoordinatorStats; "overTime"));
        MppCoordinatorStatsReportNotReceived = Some(bind!(MppCoordinatorStats; "reportNotRcv"));
        MppCoordinatorLatencyRcvReport = Some(bind!(MppCoordinatorLatency; "rcvReports"));

        ExecutorNetworkTransmissionSentTiKVTotal =
            Some(bind!(NetworkTransmissionStats; "sent_tikv_total"));
        ExecutorNetworkTransmissionSentTiKVCrossZone =
            Some(bind!(NetworkTransmissionStats; "sent_tikv_cross_zone"));
        ExecutorNetworkTransmissionReceivedTiKVTotal =
            Some(bind!(NetworkTransmissionStats; "received_tikv_total"));
        ExecutorNetworkTransmissionReceivedTiKVCrossZone =
            Some(bind!(NetworkTransmissionStats; "received_tikv_cross_zone"));
        ExecutorNetworkTransmissionSentTiFlashTotal =
            Some(bind!(NetworkTransmissionStats; "sent_tiflash_total"));
        ExecutorNetworkTransmissionSentTiFlashCrossZone =
            Some(bind!(NetworkTransmissionStats; "sent_tiflash_cross_zone"));
        ExecutorNetworkTransmissionReceivedTiFlashTotal =
            Some(bind!(NetworkTransmissionStats; "received_tiflash_total"));
        ExecutorNetworkTransmissionReceivedTiFlashCrossZone =
            Some(bind!(NetworkTransmissionStats; "received_tiflash_cross_zone"));

        IndexLookUpNormalRowsCounter = Some(bind!(IndexLookRowsCounter; "normal"));
        IndexLookUpPushDownRowsCounterHit =
            Some(bind!(IndexLookRowsCounter; "index_lookup_push_down_hit"));
        IndexLookUpPushDownRowsCounterMiss =
            Some(bind!(IndexLookRowsCounter; "index_lookup_push_down_miss"));
        IndexLookUpExecutorWithPushDownEnabledRowNumber =
            Some(bind!(IndexLookUpExecutorRowNumber; "enable_index_lookup_push_down"));
        IndexLookUpExecutorWithPushDownEnabledDuration =
            Some(bind!(IndexLookUpExecutorDuration; "enable_index_lookup_push_down"));
        IndexLookUpIndexScanCopTasksNormal =
            Some(bind!(IndexLookUpCopTaskCount; "index_scan_normal"));
        IndexLookUpIndexScanCopTasksWithPushDownEnabled =
            Some(bind!(IndexLookUpCopTaskCount; "index_scan_with_lookup_push_down"));
    }
}

/// 将各 phase 的 Observer 填入 general/internal 两张查找表。
// InitPhaseDurationObserverMap init observer map.
pub fn InitPhaseDurationObserverMap() {
    unsafe {
        // Go 这里把预绑定 observer 放入 map；使用 clone 表达同一 observer 句柄被 map 持有。
        PhaseDurationObserverMap = Some(HashMap::from([
            (PhaseBuildLocking, ExecBuildLocking.clone().unwrap()),
            (PhaseOpenLocking, ExecOpenLocking.clone().unwrap()),
            (PhaseNextLocking, ExecNextLocking.clone().unwrap()),
            (PhaseLockLocking, ExecLockLocking.clone().unwrap()),
            (PhaseBuildFinal, ExecBuildFinal.clone().unwrap()),
            (PhaseOpenFinal, ExecOpenFinal.clone().unwrap()),
            (PhaseNextFinal, ExecNextFinal.clone().unwrap()),
            (PhaseLockFinal, ExecLockFinal.clone().unwrap()),
            (PhaseCommitPrewrite, ExecCommitPrewrite.clone().unwrap()),
            (PhaseCommitCommit, ExecCommitCommit.clone().unwrap()),
            (
                PhaseCommitWaitCommitTS,
                ExecCommitWaitCommitTS.clone().unwrap(),
            ),
            (
                PhaseCommitWaitLatestTS,
                ExecCommitWaitLatestTS.clone().unwrap(),
            ),
            (PhaseCommitWaitLatch, ExecCommitWaitLatch.clone().unwrap()),
            (PhaseCommitWaitBinlog, ExecCommitWaitBinlog.clone().unwrap()),
            (PhaseWriteResponse, ExecWriteResponse.clone().unwrap()),
        ]));
        PhaseDurationObserverMapInternal = Some(HashMap::from([
            (PhaseBuildLocking, ExecBuildLockingInternal.clone().unwrap()),
            (PhaseOpenLocking, ExecOpenLockingInternal.clone().unwrap()),
            (PhaseNextLocking, ExecNextLockingInternal.clone().unwrap()),
            (PhaseLockLocking, ExecLockLockingInternal.clone().unwrap()),
            (PhaseBuildFinal, ExecBuildFinalInternal.clone().unwrap()),
            (PhaseOpenFinal, ExecOpenFinalInternal.clone().unwrap()),
            (PhaseNextFinal, ExecNextFinalInternal.clone().unwrap()),
            (PhaseLockFinal, ExecLockFinalInternal.clone().unwrap()),
            (
                PhaseCommitPrewrite,
                ExecCommitPrewriteInternal.clone().unwrap(),
            ),
            (PhaseCommitCommit, ExecCommitCommitInternal.clone().unwrap()),
            (
                PhaseCommitWaitCommitTS,
                ExecCommitWaitCommitTSInternal.clone().unwrap(),
            ),
            (
                PhaseCommitWaitLatestTS,
                ExecCommitWaitLatestTSInternal.clone().unwrap(),
            ),
            (
                PhaseCommitWaitLatch,
                ExecCommitWaitLatchInternal.clone().unwrap(),
            ),
            (
                PhaseCommitWaitBinlog,
                ExecCommitWaitBinlogInternal.clone().unwrap(),
            ),
            (
                PhaseWriteResponse,
                ExecWriteResponseInternal.clone().unwrap(),
            ),
        ]));
    }
}

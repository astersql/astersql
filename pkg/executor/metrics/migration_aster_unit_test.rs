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

// executor metrics 迁移回归：校验 Go 侧预绑定指标与 phase map 初始化完整。

use astersql_executor_metrics::executor_metrics as subject;
use prometheus::core::Collector;

/// 读取 Collector 描述中的可变标签名列表。
fn variable_labels<C: Collector>(collector: &C) -> Vec<String> {
    collector.desc()[0].variable_labels.clone()
}

/// 初始化全链路指标后，断言标签、phase 表、计数器与 Observer 均可打点。
#[test]
fn initializes_all_go_prebound_metrics_and_phase_maps() {
    // 先注册向量指标，再预绑定执行器侧静态句柄。
    astersql_executor_metrics::server::InitServerMetrics();
    astersql_executor_metrics::session::InitSessionMetrics();
    astersql_executor_metrics::metric_executor::InitExecutorMetrics();
    subject::init();

    unsafe {
        assert_eq!(
            variable_labels(
                astersql_executor_metrics::metrics::ExecPhaseDuration
                    .as_ref()
                    .unwrap()
            ),
            ["phase", "internal"],
        );
        assert_eq!(
            variable_labels(
                astersql_executor_metrics::metrics::FairLockingUsageCount
                    .as_ref()
                    .unwrap()
            ),
            ["type"],
        );

        // general / internal 两张表应各含 15 个已知 phase。
        let general = subject::PhaseDurationObserverMap.as_ref().unwrap();
        let internal = subject::PhaseDurationObserverMapInternal.as_ref().unwrap();
        assert_eq!(general.len(), 15);
        assert_eq!(internal.len(), 15);
        for phase in [
            subject::PhaseBuildLocking,
            subject::PhaseOpenLocking,
            subject::PhaseNextLocking,
            subject::PhaseLockLocking,
            subject::PhaseBuildFinal,
            subject::PhaseOpenFinal,
            subject::PhaseNextFinal,
            subject::PhaseLockFinal,
            subject::PhaseCommitPrewrite,
            subject::PhaseCommitCommit,
            subject::PhaseCommitWaitCommitTS,
            subject::PhaseCommitWaitLatestTS,
            subject::PhaseCommitWaitLatch,
            subject::PhaseCommitWaitBinlog,
            subject::PhaseWriteResponse,
        ] {
            assert!(general.contains_key(phase), "missing general phase {phase}");
            assert!(
                internal.contains_key(phase),
                "missing internal phase {phase}"
            );
        }

        // 慢查询计数器可独立自增。
        subject::SlowQueryCounterGeneral.as_ref().unwrap().inc();
        subject::SlowQueryCounterInternal
            .as_ref()
            .unwrap()
            .inc_by(2.0);
        assert_eq!(
            subject::SlowQueryCounterGeneral.as_ref().unwrap().get(),
            1.0
        );
        assert_eq!(
            subject::SlowQueryCounterInternal.as_ref().unwrap().get(),
            2.0
        );

        // 经 map 打点应落到同一预绑定 Observer。
        let before = subject::ExecBuildLocking
            .as_ref()
            .unwrap()
            .get_sample_count();
        general[subject::PhaseBuildLocking].observe(0.25);
        assert_eq!(
            subject::ExecBuildLocking
                .as_ref()
                .unwrap()
                .get_sample_count(),
            before + 1,
        );
        subject::RecordPhaseDuration(
            "build_locking",
            false,
            std::time::Duration::from_millis(250),
        );
        assert_eq!(
            subject::ExecBuildLocking
                .as_ref()
                .unwrap()
                .get_sample_count(),
            before + 2
        );
        let fair_before = subject::FairLockingStmtUsedCount.as_ref().unwrap().get();
        subject::RecordFairLockingFinishMetrics(true, true, true, true);
        assert_eq!(
            subject::FairLockingStmtUsedCount.as_ref().unwrap().get(),
            fair_before + 1.0
        );
        let tiflash_before = subject::TotalTiFlashQuerySuccCounter
            .as_ref()
            .unwrap()
            .get();
        let cache_before = astersql_executor_metrics::server::ReadFromTableCacheCounter
            .as_ref()
            .unwrap()
            .get();
        subject::RecordSupplementaryFinishMetrics(true, true, None, true);
        assert_eq!(
            subject::TotalTiFlashQuerySuccCounter
                .as_ref()
                .unwrap()
                .get(),
            tiflash_before + 1.0
        );
        assert_eq!(
            astersql_executor_metrics::server::ReadFromTableCacheCounter
                .as_ref()
                .unwrap()
                .get(),
            cache_before + 1.0
        );
        let tiflash_error = astersql_executor_metrics::server::TiFlashQueryTotalCounter
            .as_ref()
            .unwrap()
            .with_label_values(&["global:2", astersql_executor_metrics::metrics::LblError]);
        let before_error = tiflash_error.get();
        subject::RecordSupplementaryFinishMetrics(true, false, Some("global:2"), false);
        assert_eq!(tiflash_error.get(), before_error + 1.0);
        let run_before = subject::SessionExecuteRunDurationGeneral
            .as_ref()
            .unwrap()
            .get_sample_count();
        subject::RecordStatementExecuteRunDuration(false, std::time::Duration::from_millis(250));
        assert_eq!(
            subject::SessionExecuteRunDurationGeneral
                .as_ref()
                .unwrap()
                .get_sample_count(),
            run_before + 1
        );
        let shared_before = astersql_executor_metrics::session::StatementSharedLockKeysCount
            .as_ref()
            .unwrap()
            .get_sample_count();
        subject::RecordExecLockMetrics(2, 3, 4, std::time::Duration::from_millis(250), true);
        assert_eq!(
            astersql_executor_metrics::session::StatementSharedLockKeysCount
                .as_ref()
                .unwrap()
                .get_sample_count(),
            shared_before + 1
        );

        // Go InitMetricsVars 中每个预绑定句柄都必须完成初始化。
        macro_rules! assert_initialized {
            ($($metric:ident),+ $(,)?) => {
                $(assert!(subject::$metric.is_some(), "{} is not initialized", stringify!($metric));)+
            };
        }
        assert_initialized!(
            TotalQueryProcHistogramGeneral,
            TotalCopProcHistogramGeneral,
            TotalCopWaitHistogramGeneral,
            CopMVCCRatioHistogramGeneral,
            SlowQueryCounterGeneral,
            TotalQueryProcHistogramInternal,
            TotalCopProcHistogramInternal,
            TotalCopWaitHistogramInternal,
            SlowQueryCounterInternal,
            SelectForUpdateFirstAttemptDuration,
            SelectForUpdateRetryDuration,
            DmlFirstAttemptDuration,
            DmlRetryDuration,
            FairLockingTxnUsedCount,
            FairLockingStmtUsedCount,
            FairLockingTxnEffectiveCount,
            FairLockingStmtEffectiveCount,
            ExecutorCounterMergeJoinExec,
            ExecutorCountHashJoinExec,
            ExecutorCounterHashAggExec,
            ExecutorStreamAggExec,
            ExecutorCounterSortExec,
            ExecutorCounterTopNExec,
            ExecutorCounterNestedLoopApplyExec,
            ExecutorCounterIndexLookUpJoin,
            ExecutorCounterIndexLookUpExecutor,
            ExecutorCounterIndexMergeReaderExecutor,
            SessionExecuteRunDurationInternal,
            SessionExecuteRunDurationGeneral,
            TotalTiFlashQuerySuccCounter,
            ExecBuildLocking,
            ExecOpenLocking,
            ExecNextLocking,
            ExecLockLocking,
            ExecBuildFinal,
            ExecOpenFinal,
            ExecNextFinal,
            ExecLockFinal,
            ExecCommitPrewrite,
            ExecCommitCommit,
            ExecCommitWaitCommitTS,
            ExecCommitWaitLatestTS,
            ExecCommitWaitLatch,
            ExecCommitWaitBinlog,
            ExecWriteResponse,
            ExecUnknown,
            ExecBuildLockingInternal,
            ExecOpenLockingInternal,
            ExecNextLockingInternal,
            ExecLockLockingInternal,
            ExecBuildFinalInternal,
            ExecOpenFinalInternal,
            ExecNextFinalInternal,
            ExecLockFinalInternal,
            ExecCommitPrewriteInternal,
            ExecCommitCommitInternal,
            ExecCommitWaitCommitTSInternal,
            ExecCommitWaitLatestTSInternal,
            ExecCommitWaitLatchInternal,
            ExecCommitWaitBinlogInternal,
            ExecWriteResponseInternal,
            ExecUnknownInternal,
            TransactionDurationPessimisticRollbackInternal,
            TransactionDurationPessimisticRollbackGeneral,
            TransactionDurationOptimisticRollbackInternal,
            TransactionDurationOptimisticRollbackGeneral,
            MppCoordinatorStatsTotalRegisteredNumber,
            MppCoordinatorStatsActiveNumber,
            MppCoordinatorStatsOverTimeNumber,
            MppCoordinatorStatsReportNotReceived,
            MppCoordinatorLatencyRcvReport,
            ExecutorNetworkTransmissionSentTiKVTotal,
            ExecutorNetworkTransmissionSentTiKVCrossZone,
            ExecutorNetworkTransmissionReceivedTiKVTotal,
            ExecutorNetworkTransmissionReceivedTiKVCrossZone,
            ExecutorNetworkTransmissionSentTiFlashTotal,
            ExecutorNetworkTransmissionSentTiFlashCrossZone,
            ExecutorNetworkTransmissionReceivedTiFlashTotal,
            ExecutorNetworkTransmissionReceivedTiFlashCrossZone,
            IndexLookUpNormalRowsCounter,
            IndexLookUpPushDownRowsCounterHit,
            IndexLookUpPushDownRowsCounterMiss,
            IndexLookUpExecutorWithPushDownEnabledRowNumber,
            IndexLookUpExecutorWithPushDownEnabledDuration,
            IndexLookUpIndexScanCopTasksNormal,
            IndexLookUpIndexScanCopTasksWithPushDownEnabled,
        );
    }
}

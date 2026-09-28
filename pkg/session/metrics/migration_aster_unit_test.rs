// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 会话 metrics 迁移回归：校验 `InitMetricsVars` 的 label 别名与幂等性。
//
// 确认会话侧句柄与包级 Prometheus 指标共享同一底层序列，
// 且重复初始化不会更换底层 Counter / Histogram。

use astersql_session_metrics::{metrics, session_metrics};

/// 验证非事务/事务/遥测标签绑定正确，且二次 `InitMetricsVars` 仍指向同一指标。
#[test]
fn init_metrics_vars_matches_go_labels_aliases_and_is_idempotent() {
    session_metrics::InitMetricsVars();

    unsafe {
        macro_rules! counter_alias {
            ($alias:expr, $source:expr) => {{
                let alias = $alias.as_ref().expect("counter alias initialized").clone();
                let source = $source;
                let before = source.get();
                alias.inc();
                assert_eq!(source.get(), before + 1.0);
            }};
        }
        macro_rules! observer_alias {
            ($alias:expr, $source:expr) => {{
                let alias = $alias.as_ref().expect("observer alias initialized").clone();
                let source = $source;
                let before = source.get_sample_count();
                alias.observe(0.25);
                assert_eq!(source.get_sample_count(), before + 1);
            }};
        }

        let non_transactional = metrics::NonTransactionalDMLCount
            .as_ref()
            .expect("source non-transactional counter initialized");
        counter_alias!(
            session_metrics::NonTransactionalDeleteCount,
            non_transactional.with_label_values(&["delete"])
        );
        counter_alias!(
            session_metrics::NonTransactionalInsertCount,
            non_transactional.with_label_values(&["insert"])
        );
        counter_alias!(
            session_metrics::NonTransactionalUpdateCount,
            non_transactional.with_label_values(&["update"])
        );

        let statements = metrics::StatementPerTransaction
            .as_ref()
            .expect("statement histogram initialized");
        observer_alias!(
            session_metrics::StatementPerTransactionPessimisticOKInternal,
            statements.with_label_values(&[
                metrics::LblPessimistic,
                metrics::LblOK,
                metrics::LblInternal
            ])
        );
        observer_alias!(
            session_metrics::StatementPerTransactionPessimisticOKGeneral,
            statements.with_label_values(&[
                metrics::LblPessimistic,
                metrics::LblOK,
                metrics::LblGeneral
            ])
        );
        observer_alias!(
            session_metrics::StatementPerTransactionPessimisticErrorInternal,
            statements.with_label_values(&[
                metrics::LblPessimistic,
                metrics::LblError,
                metrics::LblInternal
            ])
        );
        observer_alias!(
            session_metrics::StatementPerTransactionPessimisticErrorGeneral,
            statements.with_label_values(&[
                metrics::LblPessimistic,
                metrics::LblError,
                metrics::LblGeneral
            ])
        );
        observer_alias!(
            session_metrics::StatementPerTransactionOptimisticOKInternal,
            statements.with_label_values(&[
                metrics::LblOptimistic,
                metrics::LblOK,
                metrics::LblInternal
            ])
        );
        observer_alias!(
            session_metrics::StatementPerTransactionOptimisticOKGeneral,
            statements.with_label_values(&[
                metrics::LblOptimistic,
                metrics::LblOK,
                metrics::LblGeneral
            ])
        );
        observer_alias!(
            session_metrics::StatementPerTransactionOptimisticErrorInternal,
            statements.with_label_values(&[
                metrics::LblOptimistic,
                metrics::LblError,
                metrics::LblInternal
            ])
        );
        observer_alias!(
            session_metrics::StatementPerTransactionOptimisticErrorGeneral,
            statements.with_label_values(&[
                metrics::LblOptimistic,
                metrics::LblError,
                metrics::LblGeneral
            ])
        );

        let durations = metrics::TransactionDuration
            .as_ref()
            .expect("transaction duration histogram initialized");
        observer_alias!(
            session_metrics::TransactionDurationPessimisticCommitInternal,
            durations.with_label_values(&[
                metrics::LblPessimistic,
                metrics::LblCommit,
                metrics::LblInternal
            ])
        );
        observer_alias!(
            session_metrics::TransactionDurationPessimisticCommitGeneral,
            durations.with_label_values(&[
                metrics::LblPessimistic,
                metrics::LblCommit,
                metrics::LblGeneral
            ])
        );
        observer_alias!(
            session_metrics::TransactionDurationPessimisticAbortInternal,
            durations.with_label_values(&[
                metrics::LblPessimistic,
                metrics::LblAbort,
                metrics::LblInternal
            ])
        );
        observer_alias!(
            session_metrics::TransactionDurationPessimisticAbortGeneral,
            durations.with_label_values(&[
                metrics::LblPessimistic,
                metrics::LblAbort,
                metrics::LblGeneral
            ])
        );
        observer_alias!(
            session_metrics::TransactionDurationOptimisticCommitInternal,
            durations.with_label_values(&[
                metrics::LblOptimistic,
                metrics::LblCommit,
                metrics::LblInternal
            ])
        );
        observer_alias!(
            session_metrics::TransactionDurationOptimisticCommitGeneral,
            durations.with_label_values(&[
                metrics::LblOptimistic,
                metrics::LblCommit,
                metrics::LblGeneral
            ])
        );
        observer_alias!(
            session_metrics::TransactionDurationOptimisticAbortInternal,
            durations.with_label_values(&[
                metrics::LblOptimistic,
                metrics::LblAbort,
                metrics::LblInternal
            ])
        );
        observer_alias!(
            session_metrics::TransactionDurationOptimisticAbortGeneral,
            durations.with_label_values(&[
                metrics::LblOptimistic,
                metrics::LblAbort,
                metrics::LblGeneral
            ])
        );

        let retry = metrics::SessionRetry
            .as_ref()
            .expect("retry histogram initialized");
        observer_alias!(
            session_metrics::TransactionRetryInternal,
            retry.with_label_values(&[metrics::LblInternal])
        );
        observer_alias!(
            session_metrics::TransactionRetryGeneral,
            retry.with_label_values(&[metrics::LblGeneral])
        );
        let compile = metrics::SessionExecuteCompileDuration
            .as_ref()
            .expect("compile histogram initialized");
        observer_alias!(
            session_metrics::SessionExecuteCompileDurationInternal,
            compile.with_label_values(&[metrics::LblInternal])
        );
        observer_alias!(
            session_metrics::SessionExecuteCompileDurationGeneral,
            compile.with_label_values(&[metrics::LblGeneral])
        );
        let parse = metrics::SessionExecuteParseDuration
            .as_ref()
            .expect("parse histogram initialized");
        observer_alias!(
            session_metrics::SessionExecuteParseDurationInternal,
            parse.with_label_values(&[metrics::LblInternal])
        );
        observer_alias!(
            session_metrics::SessionExecuteParseDurationGeneral,
            parse.with_label_values(&[metrics::LblGeneral])
        );

        let telemetry =
            metrics::telemetry::init_telemetry_metrics().expect("telemetry metrics initialized");
        counter_alias!(
            session_metrics::TelemetryCTEUsageRecurCTE,
            telemetry.cte.with_label_values(&["recurCTE"])
        );
        counter_alias!(
            session_metrics::TelemetryCTEUsageNonRecurCTE,
            telemetry.cte.with_label_values(&["nonRecurCTE"])
        );
        counter_alias!(
            session_metrics::TelemetryCTEUsageNotCTE,
            telemetry.cte.with_label_values(&["notCTE"])
        );
        counter_alias!(
            session_metrics::TelemetryMultiSchemaChangeUsage,
            telemetry.multi_schema_change.clone()
        );
        counter_alias!(
            session_metrics::TelemetryFlashbackClusterUsage,
            telemetry.flashback_cluster.clone()
        );
        counter_alias!(
            session_metrics::TelemetryTablePartitionUsage,
            telemetry.table_partition[0].clone()
        );
        counter_alias!(
            session_metrics::TelemetryTablePartitionListUsage,
            telemetry.table_partition[1].clone()
        );
        counter_alias!(
            session_metrics::TelemetryTablePartitionRangeUsage,
            telemetry.table_partition[2].clone()
        );
        counter_alias!(
            session_metrics::TelemetryTablePartitionHashUsage,
            telemetry.table_partition[3].clone()
        );
        counter_alias!(
            session_metrics::TelemetryTablePartitionRangeColumnsUsage,
            telemetry.table_partition[4].clone()
        );
        counter_alias!(
            session_metrics::TelemetryTablePartitionRangeColumnsGt1Usage,
            telemetry.table_partition[5].clone()
        );
        counter_alias!(
            session_metrics::TelemetryTablePartitionRangeColumnsGt2Usage,
            telemetry.table_partition[6].clone()
        );
        counter_alias!(
            session_metrics::TelemetryTablePartitionRangeColumnsGt3Usage,
            telemetry.table_partition[7].clone()
        );
        counter_alias!(
            session_metrics::TelemetryTablePartitionListColumnsUsage,
            telemetry.table_partition[8].clone()
        );
        counter_alias!(
            session_metrics::TelemetryTablePartitionMaxPartitionsUsage,
            telemetry.table_partition[9].clone()
        );
        counter_alias!(
            session_metrics::TelemetryTablePartitionCreateIntervalUsage,
            telemetry.table_partition[10].clone()
        );
        counter_alias!(
            session_metrics::TelemetryTablePartitionAddIntervalUsage,
            telemetry.table_partition[11].clone()
        );
        counter_alias!(
            session_metrics::TelemetryTablePartitionDropIntervalUsage,
            telemetry.table_partition[12].clone()
        );
        counter_alias!(
            session_metrics::TelemetryExchangePartitionUsage,
            telemetry.exchange_partition.clone()
        );
        counter_alias!(
            session_metrics::TelemetryTableCompactPartitionUsage,
            telemetry.table_partition[13].clone()
        );
        counter_alias!(
            session_metrics::TelemetryReorganizePartitionUsage,
            telemetry.table_partition[14].clone()
        );
        counter_alias!(
            session_metrics::TelemetryLockUserUsage,
            telemetry.account_lock.with_label_values(&["lockUser"])
        );
        counter_alias!(
            session_metrics::TelemetryUnlockUserUsage,
            telemetry.account_lock.with_label_values(&["unlockUser"])
        );
        counter_alias!(
            session_metrics::TelemetryCreateOrAlterUserUsage,
            telemetry
                .account_lock
                .with_label_values(&["createOrAlterUser"])
        );
        counter_alias!(
            session_metrics::TelemetryIndexMerge,
            telemetry.index_merge.clone()
        );
        counter_alias!(
            session_metrics::TelemetryStoreBatchedUsage,
            telemetry.store_batched_query.clone()
        );

        // 再次初始化后继续 inc，计数应在同一底层指标上累加。
        let delete = non_transactional.with_label_values(&["delete"]);
        let delete_before = delete.get();
        session_metrics::InitMetricsVars();
        session_metrics::NonTransactionalDeleteCount
            .as_ref()
            .expect("delete counter remains initialized")
            .inc();
        assert_eq!(
            delete.get(),
            delete_before + 1.0,
            "reinitialization must keep the same underlying Go metric"
        );
    }
}

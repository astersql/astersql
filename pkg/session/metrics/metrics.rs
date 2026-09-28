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

// Session-local handles derived from the package-wide TiDB metrics.
//
// 从包级 TiDB Prometheus 指标中派生出会话侧带标签的 Counter / Histogram 句柄。
// 悲观/乐观事务（Pessimistic/Optimistic）、非事务 DML、编译/解析耗时与遥测用量
// 均在此绑定具体 label 组合，供会话运行时直接 `inc` / `observe`。

use crate::metrics;
use std::sync::Mutex;

/// Prometheus 计数器别名。
pub type Counter = prometheus::Counter;
/// Prometheus 直方图观察者别名（用于耗时/分布）。
pub type Observer = prometheus::Histogram;

/// 初始化互斥锁，保证 `InitMetricsVars` 可重入且线程安全。
static INIT_LOCK: Mutex<()> = Mutex::new(());

/// 非事务 DELETE 计数器（label=delete）。
pub static mut NonTransactionalDeleteCount: Option<Counter> = None;
/// 非事务 INSERT 计数器（label=insert）。
pub static mut NonTransactionalInsertCount: Option<Counter> = None;
/// 非事务 UPDATE 计数器（label=update）。
pub static mut NonTransactionalUpdateCount: Option<Counter> = None;

/// 悲观事务成功路径下每事务语句数直方图（内部会话）。
pub static mut StatementPerTransactionPessimisticOKInternal: Option<Observer> = None;
/// 悲观事务成功路径下每事务语句数直方图（普通用户会话）。
pub static mut StatementPerTransactionPessimisticOKGeneral: Option<Observer> = None;
/// 悲观事务失败路径下每事务语句数直方图（内部会话）。
pub static mut StatementPerTransactionPessimisticErrorInternal: Option<Observer> = None;
/// 悲观事务失败路径下每事务语句数直方图（普通用户会话）。
pub static mut StatementPerTransactionPessimisticErrorGeneral: Option<Observer> = None;
/// 乐观事务成功路径下每事务语句数直方图（内部会话）。
pub static mut StatementPerTransactionOptimisticOKInternal: Option<Observer> = None;
/// 乐观事务成功路径下每事务语句数直方图（普通用户会话）。
pub static mut StatementPerTransactionOptimisticOKGeneral: Option<Observer> = None;
/// 乐观事务失败路径下每事务语句数直方图（内部会话）。
pub static mut StatementPerTransactionOptimisticErrorInternal: Option<Observer> = None;
/// 乐观事务失败路径下每事务语句数直方图（普通用户会话）。
pub static mut StatementPerTransactionOptimisticErrorGeneral: Option<Observer> = None;
/// 悲观事务提交耗时直方图（内部会话）。
pub static mut TransactionDurationPessimisticCommitInternal: Option<Observer> = None;
/// 悲观事务提交耗时直方图（普通用户会话）。
pub static mut TransactionDurationPessimisticCommitGeneral: Option<Observer> = None;
/// 悲观事务中止耗时直方图（内部会话）。
pub static mut TransactionDurationPessimisticAbortInternal: Option<Observer> = None;
/// 悲观事务中止耗时直方图（普通用户会话）。
pub static mut TransactionDurationPessimisticAbortGeneral: Option<Observer> = None;
/// 乐观事务提交耗时直方图（内部会话）。
pub static mut TransactionDurationOptimisticCommitInternal: Option<Observer> = None;
/// 乐观事务提交耗时直方图（普通用户会话）。
pub static mut TransactionDurationOptimisticCommitGeneral: Option<Observer> = None;
/// 乐观事务中止耗时直方图（内部会话）。
pub static mut TransactionDurationOptimisticAbortInternal: Option<Observer> = None;
/// 乐观事务中止耗时直方图（普通用户会话）。
pub static mut TransactionDurationOptimisticAbortGeneral: Option<Observer> = None;
/// 事务重试次数直方图（内部会话）。
pub static mut TransactionRetryInternal: Option<Observer> = None;
/// 事务重试次数直方图（普通用户会话）。
pub static mut TransactionRetryGeneral: Option<Observer> = None;

/// 语句编译（Compile，生成执行计划）耗时直方图（内部会话）。
pub static mut SessionExecuteCompileDurationInternal: Option<Observer> = None;
/// 语句编译耗时直方图（普通用户会话）。
pub static mut SessionExecuteCompileDurationGeneral: Option<Observer> = None;
/// 语句解析（Parse）耗时直方图（内部会话）。
pub static mut SessionExecuteParseDurationInternal: Option<Observer> = None;
/// 语句解析耗时直方图（普通用户会话）。
pub static mut SessionExecuteParseDurationGeneral: Option<Observer> = None;

/// 遥测：递归 CTE（公用表表达式）使用计数。
pub static mut TelemetryCTEUsageRecurCTE: Option<Counter> = None;
/// 遥测：非递归 CTE 使用计数。
pub static mut TelemetryCTEUsageNonRecurCTE: Option<Counter> = None;
/// 遥测：非 CTE 查询计数。
pub static mut TelemetryCTEUsageNotCTE: Option<Counter> = None;
/// 遥测：多 schema 变更（Multi Schema Change）使用计数。
pub static mut TelemetryMultiSchemaChangeUsage: Option<Counter> = None;
/// 遥测：集群闪回（Flashback Cluster）使用计数。
pub static mut TelemetryFlashbackClusterUsage: Option<Counter> = None;

/// 遥测：表分区总体使用计数。
pub static mut TelemetryTablePartitionUsage: Option<Counter> = None;
/// 遥测：LIST 分区使用计数。
pub static mut TelemetryTablePartitionListUsage: Option<Counter> = None;
/// 遥测：RANGE 分区使用计数。
pub static mut TelemetryTablePartitionRangeUsage: Option<Counter> = None;
/// 遥测：HASH 分区使用计数。
pub static mut TelemetryTablePartitionHashUsage: Option<Counter> = None;
/// 遥测：RANGE COLUMNS 分区使用计数。
pub static mut TelemetryTablePartitionRangeColumnsUsage: Option<Counter> = None;
/// 遥测：RANGE COLUMNS 列数 >1 的使用计数。
pub static mut TelemetryTablePartitionRangeColumnsGt1Usage: Option<Counter> = None;
/// 遥测：RANGE COLUMNS 列数 >2 的使用计数。
pub static mut TelemetryTablePartitionRangeColumnsGt2Usage: Option<Counter> = None;
/// 遥测：RANGE COLUMNS 列数 >3 的使用计数。
pub static mut TelemetryTablePartitionRangeColumnsGt3Usage: Option<Counter> = None;
/// 遥测：LIST COLUMNS 分区使用计数。
pub static mut TelemetryTablePartitionListColumnsUsage: Option<Counter> = None;
/// 遥测：最大分区数相关使用计数。
pub static mut TelemetryTablePartitionMaxPartitionsUsage: Option<Counter> = None;
/// 遥测：创建 INTERVAL 分区使用计数。
pub static mut TelemetryTablePartitionCreateIntervalUsage: Option<Counter> = None;
/// 遥测：添加 INTERVAL 分区使用计数。
pub static mut TelemetryTablePartitionAddIntervalUsage: Option<Counter> = None;
/// 遥测：删除 INTERVAL 分区使用计数。
pub static mut TelemetryTablePartitionDropIntervalUsage: Option<Counter> = None;
/// 遥测：交换分区（EXCHANGE PARTITION）使用计数。
pub static mut TelemetryExchangePartitionUsage: Option<Counter> = None;
/// 遥测：压缩分区（COMPACT PARTITION）使用计数。
pub static mut TelemetryTableCompactPartitionUsage: Option<Counter> = None;
/// 遥测：重组分区（REORGANIZE PARTITION）使用计数。
pub static mut TelemetryReorganizePartitionUsage: Option<Counter> = None;

/// 遥测：锁定用户（LOCK USER）使用计数。
pub static mut TelemetryLockUserUsage: Option<Counter> = None;
/// 遥测：解锁用户使用计数。
pub static mut TelemetryUnlockUserUsage: Option<Counter> = None;
/// 遥测：创建或修改用户使用计数。
pub static mut TelemetryCreateOrAlterUserUsage: Option<Counter> = None;

/// 遥测：索引合并（Index Merge）使用计数。
pub static mut TelemetryIndexMerge: Option<Counter> = None;
/// 遥测：Store 批处理查询使用计数。
pub static mut TelemetryStoreBatchedUsage: Option<Counter> = None;

/// Mirrors Go package initialization.
/// 对应 Go 包初始化：调用 [`InitMetricsVars`]。
pub fn init() {
    InitMetricsVars();
}

/// Binds every session handle to the corresponding package-wide metric and label set.
/// 将每个会话句柄绑定到包级指标及对应 label 组合；幂等可重复调用。
pub fn InitMetricsVars() {
    let _guard = INIT_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    unsafe {
        // 若包级指标尚未创建，先初始化会话指标注册表。
        if metrics::NonTransactionalDMLCount.is_none() {
            metrics::InitSessionMetrics();
        }

        // 按 DML 类型拆分非事务计数器。
        let non_transactional = metrics::NonTransactionalDMLCount
            .as_ref()
            .expect("InitSessionMetrics initializes NonTransactionalDMLCount");
        NonTransactionalDeleteCount = Some(non_transactional.with_label_values(&["delete"]));
        NonTransactionalInsertCount = Some(non_transactional.with_label_values(&["insert"]));
        NonTransactionalUpdateCount = Some(non_transactional.with_label_values(&["update"]));

        // 按事务模式（悲观/乐观）×结果（OK/Error）×来源（Internal/General）绑定直方图。
        let statements = metrics::StatementPerTransaction
            .as_ref()
            .expect("InitSessionMetrics initializes StatementPerTransaction");
        StatementPerTransactionPessimisticOKInternal = Some(statements.with_label_values(&[
            metrics::LblPessimistic,
            metrics::LblOK,
            metrics::LblInternal,
        ]));
        StatementPerTransactionPessimisticOKGeneral = Some(statements.with_label_values(&[
            metrics::LblPessimistic,
            metrics::LblOK,
            metrics::LblGeneral,
        ]));
        StatementPerTransactionPessimisticErrorInternal = Some(statements.with_label_values(&[
            metrics::LblPessimistic,
            metrics::LblError,
            metrics::LblInternal,
        ]));
        StatementPerTransactionPessimisticErrorGeneral = Some(statements.with_label_values(&[
            metrics::LblPessimistic,
            metrics::LblError,
            metrics::LblGeneral,
        ]));
        StatementPerTransactionOptimisticOKInternal = Some(statements.with_label_values(&[
            metrics::LblOptimistic,
            metrics::LblOK,
            metrics::LblInternal,
        ]));
        StatementPerTransactionOptimisticOKGeneral = Some(statements.with_label_values(&[
            metrics::LblOptimistic,
            metrics::LblOK,
            metrics::LblGeneral,
        ]));
        StatementPerTransactionOptimisticErrorInternal = Some(statements.with_label_values(&[
            metrics::LblOptimistic,
            metrics::LblError,
            metrics::LblInternal,
        ]));
        StatementPerTransactionOptimisticErrorGeneral = Some(statements.with_label_values(&[
            metrics::LblOptimistic,
            metrics::LblError,
            metrics::LblGeneral,
        ]));

        // 按事务模式 × 提交/中止 × 来源绑定耗时直方图。
        let durations = metrics::TransactionDuration
            .as_ref()
            .expect("InitSessionMetrics initializes TransactionDuration");
        TransactionDurationPessimisticCommitInternal = Some(durations.with_label_values(&[
            metrics::LblPessimistic,
            metrics::LblCommit,
            metrics::LblInternal,
        ]));
        TransactionDurationPessimisticCommitGeneral = Some(durations.with_label_values(&[
            metrics::LblPessimistic,
            metrics::LblCommit,
            metrics::LblGeneral,
        ]));
        TransactionDurationPessimisticAbortInternal = Some(durations.with_label_values(&[
            metrics::LblPessimistic,
            metrics::LblAbort,
            metrics::LblInternal,
        ]));
        TransactionDurationPessimisticAbortGeneral = Some(durations.with_label_values(&[
            metrics::LblPessimistic,
            metrics::LblAbort,
            metrics::LblGeneral,
        ]));
        TransactionDurationOptimisticCommitInternal = Some(durations.with_label_values(&[
            metrics::LblOptimistic,
            metrics::LblCommit,
            metrics::LblInternal,
        ]));
        TransactionDurationOptimisticCommitGeneral = Some(durations.with_label_values(&[
            metrics::LblOptimistic,
            metrics::LblCommit,
            metrics::LblGeneral,
        ]));
        TransactionDurationOptimisticAbortInternal = Some(durations.with_label_values(&[
            metrics::LblOptimistic,
            metrics::LblAbort,
            metrics::LblInternal,
        ]));
        TransactionDurationOptimisticAbortGeneral = Some(durations.with_label_values(&[
            metrics::LblOptimistic,
            metrics::LblAbort,
            metrics::LblGeneral,
        ]));

        // 事务重试按内部/普通会话拆分。
        let retries = metrics::SessionRetry
            .as_ref()
            .expect("InitSessionMetrics initializes SessionRetry");
        TransactionRetryInternal = Some(retries.with_label_values(&[metrics::LblInternal]));
        TransactionRetryGeneral = Some(retries.with_label_values(&[metrics::LblGeneral]));

        // 编译与解析阶段耗时按内部/普通会话拆分。
        let compile = metrics::SessionExecuteCompileDuration
            .as_ref()
            .expect("InitSessionMetrics initializes SessionExecuteCompileDuration");
        SessionExecuteCompileDurationInternal =
            Some(compile.with_label_values(&[metrics::LblInternal]));
        SessionExecuteCompileDurationGeneral =
            Some(compile.with_label_values(&[metrics::LblGeneral]));
        let parse = metrics::SessionExecuteParseDuration
            .as_ref()
            .expect("InitSessionMetrics initializes SessionExecuteParseDuration");
        SessionExecuteParseDurationInternal =
            Some(parse.with_label_values(&[metrics::LblInternal]));
        SessionExecuteParseDurationGeneral = Some(parse.with_label_values(&[metrics::LblGeneral]));

        // 遥测计数器：CTE、分区、账户锁定、Index Merge 等功能用量。
        let telemetry = metrics::telemetry::init_telemetry_metrics()
            .expect("valid telemetry metric descriptors");
        TelemetryCTEUsageRecurCTE = Some(telemetry.cte.with_label_values(&["recurCTE"]));
        TelemetryCTEUsageNonRecurCTE = Some(telemetry.cte.with_label_values(&["nonRecurCTE"]));
        TelemetryCTEUsageNotCTE = Some(telemetry.cte.with_label_values(&["notCTE"]));
        TelemetryMultiSchemaChangeUsage = Some(telemetry.multi_schema_change.clone());
        TelemetryFlashbackClusterUsage = Some(telemetry.flashback_cluster.clone());

        TelemetryTablePartitionUsage = Some(telemetry.table_partition[0].clone());
        TelemetryTablePartitionListUsage = Some(telemetry.table_partition[1].clone());
        TelemetryTablePartitionRangeUsage = Some(telemetry.table_partition[2].clone());
        TelemetryTablePartitionHashUsage = Some(telemetry.table_partition[3].clone());
        TelemetryTablePartitionRangeColumnsUsage = Some(telemetry.table_partition[4].clone());
        TelemetryTablePartitionRangeColumnsGt1Usage = Some(telemetry.table_partition[5].clone());
        TelemetryTablePartitionRangeColumnsGt2Usage = Some(telemetry.table_partition[6].clone());
        TelemetryTablePartitionRangeColumnsGt3Usage = Some(telemetry.table_partition[7].clone());
        TelemetryTablePartitionListColumnsUsage = Some(telemetry.table_partition[8].clone());
        TelemetryTablePartitionMaxPartitionsUsage = Some(telemetry.table_partition[9].clone());
        TelemetryTablePartitionCreateIntervalUsage = Some(telemetry.table_partition[10].clone());
        TelemetryTablePartitionAddIntervalUsage = Some(telemetry.table_partition[11].clone());
        TelemetryTablePartitionDropIntervalUsage = Some(telemetry.table_partition[12].clone());
        TelemetryExchangePartitionUsage = Some(telemetry.exchange_partition.clone());
        TelemetryTableCompactPartitionUsage = Some(telemetry.table_partition[13].clone());
        TelemetryReorganizePartitionUsage = Some(telemetry.table_partition[14].clone());

        TelemetryLockUserUsage = Some(telemetry.account_lock.with_label_values(&["lockUser"]));
        TelemetryUnlockUserUsage = Some(telemetry.account_lock.with_label_values(&["unlockUser"]));
        TelemetryCreateOrAlterUserUsage = Some(
            telemetry
                .account_lock
                .with_label_values(&["createOrAlterUser"]),
        );
        TelemetryIndexMerge = Some(telemetry.index_merge.clone());
        TelemetryStoreBatchedUsage = Some(telemetry.store_batched_query.clone());
    }
}

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

// Executor（执行器）相关 Prometheus 指标。
//
// 执行器按物理执行计划逐步产出行；本模块统计昂贵算子次数、语句类型、各执行阶段
// 耗时、进行中事务时长、MPP（大规模并行处理）协调器状态、影响行数、网络传输，
// 以及 IndexLookUp（先扫索引再回表）执行器的耗时与行数。

use crate::bindinfo::compat_prometheus::{
    CounterCompat as _, GaugeCompat as _, MetricCompat as _, ObserverCompat as _,
};
use crate::bindinfo::{compat_metricscommon as metricscommon, compat_prometheus as prometheus};
use crate::*;

// 下列 Option 对应 Go 在 InitExecutorMetrics 运行前尚未赋值的包级指标。
/// 昂贵执行器（expensive executor）触发次数。
// ExecutorCounter records the number of expensive executors.
pub static mut ExecutorCounter: Option<prometheus::CounterVec> = None;
/// 按语句类型统计的 StmtNode 次数。
// StmtNodeCounter records the number of statement with the same type.
pub static mut StmtNodeCounter: Option<prometheus::CounterVec> = None;
/// 按库名与语句类型统计的 StmtNode 次数。
// DbStmtNodeCounter records the number of statement with the same type and db.
pub static mut DbStmtNodeCounter: Option<prometheus::CounterVec> = None;
/// 各执行阶段耗时 Summary。
// ExecPhaseDuration records the duration of each execution phase.
pub static mut ExecPhaseDuration: Option<prometheus::SummaryVec> = None;
/// 进行中事务已持续时长直方图。
// OngoingTxnDurationHistogram records the duration of ongoing transactions.
pub static mut OngoingTxnDurationHistogram: Option<prometheus::HistogramVec> = None;
/// MPP 协调器实例数及相关事件 Gauge。
// MppCoordinatorStats records the number of mpp coordinator instances and related events.
pub static mut MppCoordinatorStats: Option<prometheus::GaugeVec> = None;
/// MPP 协调器操作延迟直方图。
// MppCoordinatorLatency records latencies of mpp coordinator operations.
pub static mut MppCoordinatorLatency: Option<prometheus::HistogramVec> = None;
/// 影响行数计数（按 SQL 类型）。
// AffectedRowsCounter records the number of affected rows.
pub static mut AffectedRowsCounter: Option<prometheus::CounterVec> = None;

// 这些 counter 由 AffectedRowsCounter.WithLabelValues 派生，保持普通 DML 与 NT-DML 标签区别。
/// Insert 影响行数 Counter。
pub static mut AffectedRowsCounterInsert: Option<prometheus::Counter> = None;
/// Update 影响行数 Counter。
pub static mut AffectedRowsCounterUpdate: Option<prometheus::Counter> = None;
/// Delete 影响行数 Counter。
pub static mut AffectedRowsCounterDelete: Option<prometheus::Counter> = None;
/// Replace 影响行数 Counter。
pub static mut AffectedRowsCounterReplace: Option<prometheus::Counter> = None;
/// 非事务 DML（NT-DML）Update 影响行数。
pub static mut AffectedRowsCounterNTDMLUpdate: Option<prometheus::Counter> = None;
/// 非事务 DML Delete 影响行数。
pub static mut AffectedRowsCounterNTDMLDelete: Option<prometheus::Counter> = None;
/// 非事务 DML Insert 影响行数。
pub static mut AffectedRowsCounterNTDMLInsert: Option<prometheus::Counter> = None;
/// 非事务 DML Replace 影响行数。
pub static mut AffectedRowsCounterNTDMLReplace: Option<prometheus::Counter> = None;

/// 查询网络传输字节数。
// NetworkTransmissionStats records the network transmission for queries.
pub static mut NetworkTransmissionStats: Option<prometheus::CounterVec> = None;
/// IndexLookUp 执行耗时直方图。
// IndexLookUpExecutorDuration records the duration of index look up executor.
pub static mut IndexLookUpExecutorDuration: Option<prometheus::HistogramVec> = None;
/// IndexLookUp 下推行数计数。
// IndexLookRowsCounter records the number of rows in index look up executor.
pub static mut IndexLookRowsCounter: Option<prometheus::CounterVec> = None;
/// 单次 IndexLookUp 扫描行数直方图。
// IndexLookUpExecutorRowNumber records the number of rows scanned in one index look up executor.
pub static mut IndexLookUpExecutorRowNumber: Option<prometheus::HistogramVec> = None;
/// IndexLookUp 产生的 Coprocessor 任务数。
// IndexLookUpCopTaskCount records the number of cop tasks in index look up executor.
pub static mut IndexLookUpCopTaskCount: Option<prometheus::CounterVec> = None;

/// 初始化全部 Executor 指标，并派生固定标签的影响行数 Counter。
// InitExecutorMetrics 对应 Go 初始化函数，依次构造执行器、语句、事务、MPP、影响行数、网络和索引回表指标。
pub fn InitExecutorMetrics() {
    let _init_guard = crate::metrics::PACKAGE_INIT_LOCK
        .lock()
        .expect("metrics init lock poisoned");
    let executor_counter = metricscommon::NewCounterVec(
        prometheus::CounterOpts {
            Namespace: "tidb",
            Subsystem: "executor",
            Name: "expensive_total",
            Help: "Counter of Expensive Executors.",
        },
        vec![LblType],
    );

    let stmt_node_counter = metricscommon::NewCounterVec(
        prometheus::CounterOpts {
            Namespace: "tidb",
            Subsystem: "executor",
            Name: "statement_total",
            Help: "Counter of StmtNode.",
        },
        // 标签次序与 Go 完全一致，避免改变已有时序维度。
        vec![LblType, LblDb, LblResourceGroup],
    );

    let db_stmt_node_counter = metricscommon::NewCounterVec(
        prometheus::CounterOpts {
            Namespace: "tidb",
            Subsystem: "executor",
            Name: "statement_db_total",
            Help: "Counter of StmtNode by Database.",
        },
        vec![LblDb, LblType],
    );

    let exec_phase_duration = metricscommon::NewSummaryVec(
        prometheus::SummaryOpts {
            Namespace: "tidb",
            Subsystem: "executor",
            Name: "phase_duration_seconds",
            Help: "Summary of each execution phase duration.",
        },
        vec![LblPhase, LblInternal],
    );

    let ongoing_txn_duration = metricscommon::NewHistogramVec(
        prometheus::HistogramOpts {
            Namespace: "tidb",
            Subsystem: "executor",
            Name: "ongoing_txn_duration_seconds",
            Help: "Bucketed histogram of processing time (s) of ongoing transactions.",
            // 60 秒起始、倍数 2、15 个桶，保持 Go 的约 273 小时覆盖范围。
            Buckets: prometheus::ExponentialBuckets(60.0, 2.0, 15),
        },
        vec![LblType],
    );

    let mpp_coordinator_stats = metricscommon::NewGaugeVec(
        prometheus::GaugeOpts {
            Namespace: "tidb",
            Subsystem: "executor",
            Name: "mpp_coordinator_stats",
            Help: "Mpp Coordinator related stats",
        },
        vec![LblType],
    );

    let mpp_coordinator_latency = metricscommon::NewHistogramVec(
        prometheus::HistogramOpts {
            Namespace: "tidb",
            Subsystem: "executor",
            Name: "mpp_coordinator_latency",
            Help: "Bucketed histogram of processing time (ms) of mpp coordinator operations.",
            Buckets: prometheus::ExponentialBuckets(0.001, 2.0, 28),
        },
        vec![LblType],
    );

    let affected_rows_counter = metricscommon::NewCounterVec(
        prometheus::CounterOpts {
            Namespace: "tidb",
            Subsystem: "executor",
            Name: "affected_rows",
            Help: "Counters of server affected rows.",
        },
        vec![LblSQLType],
    );

    // Go 从同一个 CounterVec 绑定八个固定标签；clone 仅表达多个句柄共享 collector 的意图。
    let affected_insert = affected_rows_counter
        .clone()
        .WithLabelValues(vec!["Insert"]);
    let affected_update = affected_rows_counter
        .clone()
        .WithLabelValues(vec!["Update"]);
    let affected_delete = affected_rows_counter
        .clone()
        .WithLabelValues(vec!["Delete"]);
    let affected_replace = affected_rows_counter
        .clone()
        .WithLabelValues(vec!["Replace"]);
    let affected_ntdml_update = affected_rows_counter
        .clone()
        .WithLabelValues(vec!["NTDML-Update"]);
    let affected_ntdml_delete = affected_rows_counter
        .clone()
        .WithLabelValues(vec!["NTDML-Delete"]);
    let affected_ntdml_insert = affected_rows_counter
        .clone()
        .WithLabelValues(vec!["NTDML-Insert"]);
    let affected_ntdml_replace = affected_rows_counter
        .clone()
        .WithLabelValues(vec!["NTDML-Replace"]);

    let network_transmission_stats = metricscommon::NewCounterVec(
        prometheus::CounterOpts {
            Namespace: "tidb",
            Subsystem: "executor",
            Name: "network_transmission",
            Help: "Counter of network transmission bytes.",
        },
        vec![LblType],
    );

    let index_lookup_duration = metricscommon::NewHistogramVec(
        prometheus::HistogramOpts {
            Namespace: "tidb",
            Subsystem: "executor",
            Name: "index_lookup_execute_duration_seconds",
            Help: "Bucketed histogram of processing time (s) in running index-lookup executor.",
            // 100 微秒起始的 30 个指数桶，保持 Go 的约 15 小时上界。
            Buckets: prometheus::ExponentialBuckets(0.0001, 2.0, 30),
        },
        vec![LblType],
    );

    let index_lookup_rows = metricscommon::NewCounterVec(
        prometheus::CounterOpts {
            Namespace: "tidb",
            Subsystem: "executor",
            Name: "index_lookup_rows",
            Help: "Counter of index lookup push-down rows.",
        },
        vec![LblType],
    );

    let index_lookup_row_number = metricscommon::NewHistogramVec(
        prometheus::HistogramOpts {
            Namespace: "tidb",
            Subsystem: "executor",
            Name: "index_lookup_row_number",
            Help: "Row number for each index lookup executor",
            Buckets: prometheus::ExponentialBuckets(1.0, 2.0, 10),
        },
        vec![LblType],
    );

    let index_lookup_cop_tasks = metricscommon::NewCounterVec(
        prometheus::CounterOpts {
            Namespace: "tidb",
            Subsystem: "executor",
            Name: "index_lookup_cop_task_count",
            Help: "Counter for index lookup cop tasks",
        },
        vec![LblType],
    );

    // 与 Go 的 InitMetrics 调用期写入对应；不处理并发访问或重复初始化。
    unsafe {
        ExecutorCounter = Some(executor_counter);
        StmtNodeCounter = Some(stmt_node_counter);
        DbStmtNodeCounter = Some(db_stmt_node_counter);
        ExecPhaseDuration = Some(exec_phase_duration);
        OngoingTxnDurationHistogram = Some(ongoing_txn_duration);
        MppCoordinatorStats = Some(mpp_coordinator_stats);
        MppCoordinatorLatency = Some(mpp_coordinator_latency);
        AffectedRowsCounter = Some(affected_rows_counter);
        AffectedRowsCounterInsert = Some(affected_insert);
        AffectedRowsCounterUpdate = Some(affected_update);
        AffectedRowsCounterDelete = Some(affected_delete);
        AffectedRowsCounterReplace = Some(affected_replace);
        AffectedRowsCounterNTDMLUpdate = Some(affected_ntdml_update);
        AffectedRowsCounterNTDMLDelete = Some(affected_ntdml_delete);
        AffectedRowsCounterNTDMLInsert = Some(affected_ntdml_insert);
        AffectedRowsCounterNTDMLReplace = Some(affected_ntdml_replace);
        NetworkTransmissionStats = Some(network_transmission_stats);
        IndexLookUpExecutorDuration = Some(index_lookup_duration);
        IndexLookRowsCounter = Some(index_lookup_rows);
        IndexLookUpExecutorRowNumber = Some(index_lookup_row_number);
        IndexLookUpCopTaskCount = Some(index_lookup_cop_tasks);
    }
}

/// Increment the initialized statement counters.
pub fn IncStatementCounter(statement_type: &str, database: &str, resource_group: &str) {
    unsafe {
        if let Some(counter) = StmtNodeCounter.as_ref() {
            counter
                .WithLabelValues(&[statement_type, database, resource_group])
                .inc();
        }
        if let Some(counter) = DbStmtNodeCounter.as_ref() {
            counter.WithLabelValues(&[database, statement_type]).inc();
        }
    }
}

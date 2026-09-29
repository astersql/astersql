// Copyright 2018 PingCAP, Inc.
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

// Session 执行路径相关 Prometheus 指标与共享标签常量。
//
// 覆盖 SQL 解析/编译/执行耗时、事务与重试、悲观锁（Pessimistic Lock）、
// Schema Lease 错误、资源组查询计数等。标签常量供 metrics 包内其它模块复用。
// 本文件只构造指标句柄，不开启会话或访问 PD。

use crate::bindinfo::compat_prometheus::{
    CounterCompat as _, GaugeCompat as _, MetricCompat as _, ObserverCompat as _,
};
use crate::bindinfo::{compat_metricscommon as metricscommon, compat_prometheus as prometheus};
use crate::*;

// 这里只创建 Prometheus 指标描述与句柄，不会注册采集器、开启会话、访问 PD 或执行事务。

// Session 指标。Option 表示 Go 包变量在 InitSessionMetrics 前的零值状态。
/// Auto ID（自增主键等）分配请求耗时。
pub static mut AutoIDReqDuration: Option<prometheus::Histogram> = None;
/// Session 解析 SQL 耗时。
pub static mut SessionExecuteParseDuration: Option<prometheus::HistogramVec> = None;
/// Session 编译/优化（生成执行计划）耗时。
pub static mut SessionExecuteCompileDuration: Option<prometheus::HistogramVec> = None;
/// Session 运行执行器耗时。
pub static mut SessionExecuteRunDuration: Option<prometheus::HistogramVec> = None;
/// Schema Lease（元数据租约）错误次数；租约保证节点看到一致的 schema 版本。
pub static mut SchemaLeaseErrorCounter: Option<prometheus::CounterVec> = None;
/// Session 重试次数分布。
pub static mut SessionRetry: Option<prometheus::HistogramVec> = None;
/// Session 重试错误次数。
pub static mut SessionRetryErrorCounter: Option<prometheus::CounterVec> = None;
/// 内部受限 SQL 执行次数。
pub static mut SessionRestrictedSQLCounter: Option<prometheus::Counter> = None;
/// 每个事务内的语句数分布。
pub static mut StatementPerTransaction: Option<prometheus::HistogramVec> = None;
/// 事务总耗时（含重试）。
pub static mut TransactionDuration: Option<prometheus::HistogramVec> = None;
/// 语句死锁检测耗时。
pub static mut StatementDeadlockDetectDuration: Option<prometheus::Histogram> = None;
/// 悲观事务语句重试次数分布。
pub static mut StatementPessimisticRetryCount: Option<prometheus::Histogram> = None;
/// 单条语句加排他锁的键数量。
pub static mut StatementLockKeysCount: Option<prometheus::Histogram> = None;
/// 单条语句加共享锁的键数量。
pub static mut StatementSharedLockKeysCount: Option<prometheus::Histogram> = None;
/// 向 PD 校验读时间戳（read ts）的次数。
pub static mut ValidateReadTSFromPDCount: Option<prometheus::Counter> = None;
/// 非事务 DML（分批删除等）次数。
pub static mut NonTransactionalDMLCount: Option<prometheus::CounterVec> = None;
/// 事务进入某状态的次数。
pub static mut TxnStatusEnteringCounter: Option<prometheus::CounterVec> = None;
/// 事务各状态停留时长。
pub static mut TxnDurationHistogram: Option<prometheus::HistogramVec> = None;
/// 将 `tidb_constraint_check_in_place` 设为 false（延迟唯一性检查）的次数。
pub static mut LazyPessimisticUniqueCheckSetCount: Option<prometheus::Counter> = None;
/// 悲观 DML 按首次尝试/重试区分的耗时。
pub static mut PessimisticDMLDurationByAttempt: Option<prometheus::HistogramVec> = None;
/// 按资源组统计的查询总数。
pub static mut ResourceGroupQueryTotalCounter: Option<prometheus::CounterVec> = None;
/// Fair Locking（公平加锁）使用/生效次数。
pub static mut FairLockingUsageCount: Option<prometheus::CounterVec> = None;
/// 悲观锁加锁耗时（子系统名保持 tikvclient 以兼容历史）。
pub static mut PessimisticLockKeysDuration: Option<prometheus::Histogram> = None;

/// Return the registered process-wide transaction-state counter vector.
///
/// Session sub-crates use this accessor instead of constructing parallel,
/// unregistered metrics with the same Prometheus names.
pub fn TxnStatusEnteringCounterVec() -> prometheus::CounterVec {
    unsafe {
        // `session.rs` is also path-included by executor/session metric facade
        // crates, where the package-wide `metrics::InitMetrics` entry point is
        // intentionally absent. Reuse an existing registered vector when the
        // process metrics package initialized it; otherwise initialize this
        // self-contained session metric set.
        if TxnStatusEnteringCounter.is_none() {
            InitSessionMetrics();
        }
        TxnStatusEnteringCounter
            .as_ref()
            .expect("transaction-state counter initialized")
            .clone()
    }
}

/// Return the registered process-wide transaction-state duration histogram.
pub fn TxnDurationHistogramVec() -> prometheus::HistogramVec {
    unsafe {
        if TxnDurationHistogram.is_none() {
            InitSessionMetrics();
        }
        TxnDurationHistogram
            .as_ref()
            .expect("transaction-state histogram initialized")
            .clone()
    }
}

fn counter(subsystem: &'static str, name: &'static str, help: &'static str) -> prometheus::Counter {
    metricscommon::NewCounter(prometheus::CounterOpts {
        Namespace: "tidb",
        Subsystem: subsystem,
        Name: name,
        Help: help,
        ..Default::default()
    })
}

fn counter_vec(
    subsystem: &'static str,
    name: &'static str,
    help: &'static str,
    labels: &[&'static str],
) -> prometheus::CounterVec {
    metricscommon::NewCounterVec(
        prometheus::CounterOpts {
            Namespace: "tidb",
            Subsystem: subsystem,
            Name: name,
            Help: help,
            ..Default::default()
        },
        labels.to_vec(),
    )
}

fn histogram(
    subsystem: &'static str,
    name: &'static str,
    help: &'static str,
    buckets: Vec<f64>,
) -> prometheus::Histogram {
    metricscommon::NewHistogram(prometheus::HistogramOpts {
        Namespace: "tidb",
        Subsystem: subsystem,
        Name: name,
        Help: help,
        Buckets: buckets,
        ..Default::default()
    })
}

fn histogram_vec(
    subsystem: &'static str,
    name: &'static str,
    help: &'static str,
    buckets: Vec<f64>,
    labels: &[&'static str],
) -> prometheus::HistogramVec {
    metricscommon::NewHistogramVec(
        prometheus::HistogramOpts {
            Namespace: "tidb",
            Subsystem: subsystem,
            Name: name,
            Help: help,
            Buckets: buckets,
            ..Default::default()
        },
        labels.to_vec(),
    )
}

// InitSessionMetrics 按 Go 顺序构造解析、执行、重试、悲观锁和资源组指标，不在这里注册它们。
/// 初始化 Session 相关全部指标句柄。
pub fn InitSessionMetrics() {
    unsafe {
        // 40 微秒起始、2 倍增长的 28 桶覆盖 auto ID 与解析/编译耗时。
        AutoIDReqDuration = Some(histogram(
            "meta",
            "autoid_duration_seconds",
            "Bucketed histogram of processing time (s) in parse SQL.",
            prometheus::ExponentialBuckets(0.00004, 2.0, 28),
        ));
        SessionExecuteParseDuration = Some(histogram_vec(
            "session",
            "parse_duration_seconds",
            "Bucketed histogram of processing time (s) in parse SQL.",
            prometheus::ExponentialBuckets(0.00004, 2.0, 28),
            &[LblSQLType],
        ));
        // 构建计划可能执行语句或分配 table ID，故沿用可覆盖长耗时的桶范围。
        SessionExecuteCompileDuration = Some(histogram_vec(
            "session",
            "compile_duration_seconds",
            "Bucketed histogram of processing time (s) in query optimize.",
            prometheus::ExponentialBuckets(0.00004, 2.0, 28),
            &[LblSQLType],
        ));
        SessionExecuteRunDuration = Some(histogram_vec(
            "session",
            "execute_duration_seconds",
            "Bucketed histogram of processing time (s) in running executor.",
            prometheus::ExponentialBuckets(0.0001, 2.0, 30),
            &[LblSQLType],
        ));
        SchemaLeaseErrorCounter = Some(counter_vec(
            "session",
            "schema_lease_error_total",
            "Counter of schema lease error",
            &[LblType],
        ));
        SessionRetry = Some(histogram_vec(
            "session",
            "retry_num",
            "Bucketed histogram of session retry count.",
            (0..21).map(|i| i as f64).collect(),
            &[LblScope],
        ));
        SessionRetryErrorCounter = Some(counter_vec(
            "session",
            "retry_error_total",
            "Counter of session retry error.",
            &[LblSQLType, LblType],
        ));
        SessionRestrictedSQLCounter = Some(counter(
            "session",
            "restricted_sql_total",
            "Counter of internal restricted sql.",
        ));
        StatementPerTransaction = Some(histogram_vec(
            "session",
            "transaction_statement_num",
            "Bucketed histogram of statements count in each transaction.",
            prometheus::ExponentialBuckets(1.0, 2.0, 16),
            &[LblTxnMode, LblType, LblScope],
        ));
        TransactionDuration = Some(histogram_vec(
            "session",
            "transaction_duration_seconds",
            "Bucketed histogram of a transaction execution duration, including retry.",
            prometheus::ExponentialBuckets(0.001, 2.0, 28),
            &[LblTxnMode, LblType, LblScope],
        ));
        StatementDeadlockDetectDuration = Some(histogram(
            "session",
            "statement_deadlock_detect_duration_seconds",
            "Bucketed histogram of a statement deadlock detect duration.",
            prometheus::ExponentialBuckets(0.001, 2.0, 28),
        ));
        StatementPessimisticRetryCount = Some(histogram(
            "session",
            "statement_pessimistic_retry_count",
            "Bucketed histogram of statement pessimistic retry count",
            prometheus::ExponentialBuckets(1.0, 2.0, 16),
        ));
        StatementLockKeysCount = Some(histogram(
            "session",
            "statement_lock_keys_count",
            "Keys locking for a single statement",
            prometheus::ExponentialBuckets(1.0, 2.0, 21),
        ));
        StatementSharedLockKeysCount = Some(histogram(
            "session",
            "statement_shared_lock_keys_count",
            "Keys locking for a single statement",
            prometheus::ExponentialBuckets(1.0, 2.0, 21),
        ));
        ValidateReadTSFromPDCount = Some(counter(
            "session",
            "validate_read_ts_from_pd_count",
            "Counter of validating read ts by getting a timestamp from PD",
        ));
        NonTransactionalDMLCount = Some(counter_vec(
            "session",
            "non_transactional_dml_count",
            "Counter of non-transactional delete",
            &[LblType],
        ));
        TxnStatusEnteringCounter = Some(counter_vec(
            "session",
            "txn_state_entering_count",
            "How many times transactions enter this state",
            &[LblType],
        ));
        TxnDurationHistogram = Some(histogram_vec(
            "session",
            "txn_state_seconds",
            "Bucketed histogram of different states of a transaction.",
            prometheus::ExponentialBuckets(0.0005, 2.0, 29),
            &[LblType, LblHasLock],
        ));
        LazyPessimisticUniqueCheckSetCount = Some(counter(
            "session",
            "lazy_pessimistic_unique_check_set_count",
            "Counter of setting tidb_constraint_check_in_place to false, note that it doesn't count the default value set by tidb config",
        ));
        PessimisticDMLDurationByAttempt = Some(histogram_vec(
            "session",
            "transaction_pessimistic_dml_duration_by_attempt",
            "Bucketed histogram of duration of pessimistic DMLs, distinguished by first attempt and retries",
            prometheus::ExponentialBuckets(0.001, 2.0, 28),
            &[LblType, LblPhase],
        ));
        ResourceGroupQueryTotalCounter = Some(counter_vec(
            "session",
            "resource_group_query_total",
            "Counter of the total number of queries for the resource group",
            &[LblName, LblResourceGroup],
        ));
        FairLockingUsageCount = Some(counter_vec(
            "session",
            "transaction_fair_locking_usage",
            "The counter of statements and transactions in which fair locking is used or takes effect",
            &[LblType],
        ));
        // 此指标从 client-go 移入；为兼容历史版本，子系统名必须继续使用 tikvclient。
        PessimisticLockKeysDuration = Some(histogram(
            "tikvclient",
            "pessimistic_lock_keys_duration",
            "tidb txn pessimistic lock keys duration",
            prometheus::ExponentialBuckets(0.001, 2.0, 19),
        ));
    }
}

// 标签常量按 Go 源文件顺序保留，供本包其它共享。
/// 不可重试错误标签。
pub const LblUnretryable: &str = "unretryable";
/// 达到最大重试次数标签。
pub const LblReachMax: &str = "reach_max";
/// 成功结果标签。
pub const LblOK: &str = "ok";
/// 错误结果标签。
pub const LblError: &str = "error";
/// 提交标签。
pub const LblCommit: &str = "commit";
/// 中止标签。
pub const LblAbort: &str = "abort";
/// 回滚标签。
pub const LblRollback: &str = "rollback";
/// 通用 type 标签名。
pub const LblType: &str = "type";
/// 数据库名标签。
pub const LblDb: &str = "db";
/// 结果标签名。
pub const LblResult: &str = "result";
/// SQL 类型标签名。
pub const LblSQLType: &str = "sql_type";
pub const LblSQLTypeDDL: &str = "ddl";
pub const LblSQLTypeRead: &str = "read";
pub const LblSQLTypeWrite: &str = "write";
pub const LblSQLTypeAnalyze: &str = "analyze";
pub const LblSQLTypeOther: &str = "other";
pub const LblEngine: &str = "engine";
pub const LblEngineTiKV: &str = "tikv";
pub const LblEngineTiFlash: &str = "tiflash";
/// Coprocessor 类型标签名。
pub const LblCoprType: &str = "copr_type";
/// 一般（用户）SQL 标签值。
pub const LblGeneral: &str = "general";
/// 内部 SQL 标签值。
pub const LblInternal: &str = "internal";
/// 事务模式标签名。
pub const LblTxnMode: &str = "txn_mode";
/// 悲观事务模式标签值。
pub const LblPessimistic: &str = "pessimistic";
/// 乐观事务模式标签值。
pub const LblOptimistic: &str = "optimistic";
/// 存储标签名。
pub const LblStore: &str = "store";
/// 地址标签名。
pub const LblAddress: &str = "address";
/// batch_get 操作标签值。
pub const LblBatchGet: &str = "batch_get";
/// get 操作标签值。
pub const LblGet: &str = "get";
/// lock_keys 操作标签值。
pub const LblLockKeys: &str = "lock_keys";
/// 是否在事务中标签名。
pub const LblInTxn: &str = "in_txn";
/// 版本标签名。
pub const LblVersion: &str = "version";
/// Git hash 标签名。
pub const LblHash: &str = "hash";
/// CTE 类型标签名。
pub const LblCTEType: &str = "cte_type";
/// 账户锁定标签名。
pub const LblAccountLock: &str = "account_lock";
/// 空闲状态标签值。
pub const LblIdle: &str = "idle";
/// 正在执行 SQL 状态标签值。
pub const LblRunning: &str = "executing_sql";
/// 等待锁状态标签值。
pub const LblLockWaiting: &str = "waiting_for_lock";
/// 提交中状态标签值。
pub const LblCommitting: &str = "committing";
/// 回滚中状态标签值。
pub const LblRollingBack: &str = "rolling_back";
/// 是否持有锁标签名。
pub const LblHasLock: &str = "has_lock";
/// 阶段标签名。
pub const LblPhase: &str = "phase";
/// 模块标签名。
pub const LblModule: &str = "module";
/// RC 读路径 CheckTS 标签值。
pub const LblRCReadCheckTS: &str = "read_check";
/// RC 写路径 CheckTS 标签值。
pub const LblRCWriteCheckTS: &str = "write_check";
/// 资源组标签名。
pub const LblResourceGroup: &str = "resource_group";
/// 名称标签名。
pub const LblName: &str = "name";
/// Fair Locking：事务使用了该特性。
pub const LblFairLockingTxnUsed: &str = "txn-used";
/// Fair Locking：事务实际生效。
pub const LblFairLockingTxnEffective: &str = "txn-effective";
/// Fair Locking：语句使用了该特性。
pub const LblFairLockingStmtUsed: &str = "stmt-used";
/// Fair Locking：语句实际生效。
pub const LblFairLockingStmtEffective: &str = "stmt-effective";
/// 作用域标签名。
pub const LblScope: &str = "scope";
// TLS cipher 标签与 server.rs 的 TLSCipher CounterVec 对应。
/// TLS cipher 标签名。
pub const LblCipher: &str = "cipher";

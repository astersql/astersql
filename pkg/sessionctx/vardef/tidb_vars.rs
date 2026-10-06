// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// TiDB 专有系统变量定义：变量名、默认值、全局原子状态与转换辅助。
//
// 系统变量控制优化器、DDL、事务、慢日志、内存配额等行为。
// 本文件由 Go `tidb_vars.go` 迁移，保留声明顺序与并发语义；
// 变量名常量供 SET/SHOW 与 SysVar 表注册，Def* 为默认值，静态 Atomic* 为进程级可变状态。

// 本文件由 pkg/sessionctx/vardef/tidb_vars.go 迁移而来，保留系统变量名、默认值、
// 并发全局状态和转换函数的 Go 行为。

#![allow(dead_code, non_snake_case, non_upper_case_globals, unused_variables)]

use std::ops::BitOr;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::{LazyLock, RwLock};

// 生成带 Load/Store（SeqCst）的原子包装类型，镜像 Go `go.uber.org/atomic`。
macro_rules! atomic_value {
    ($(#[$meta:meta])* $name:ident, $atomic:ty, $value:ty) => {
        $(#[$meta])*
        pub struct $name($atomic);
        impl $name {
            pub const fn new(value: $value) -> Self {
                Self(<$atomic>::new(value))
            }
            pub fn Load(&self) -> $value {
                self.0.load(Ordering::SeqCst)
            }
            pub fn Store(&self, value: $value) {
                self.0.store(value, Ordering::SeqCst)
            }
        }
    };
}

atomic_value!(
    /// 布尔原子包装。
    AtomicBoolValue,
    AtomicBool,
    bool
);
atomic_value!(AtomicI32Value, AtomicI32, i32);
atomic_value!(AtomicI64Value, AtomicI64, i64);
atomic_value!(AtomicU32Value, AtomicU32, u32);
atomic_value!(AtomicU64Value, AtomicU64, u64);

/// 以 `f64` 位模式存于 `AtomicU64` 的浮点原子值。
pub struct AtomicF64Value(AtomicU64);
impl AtomicF64Value {
    /// 用浮点值构造（按 IEEE754 位存储）。
    pub const fn new(value: f64) -> Self {
        Self(AtomicU64::new(value.to_bits()))
    }
    /// 原子读取浮点值。
    pub fn Load(&self) -> f64 {
        f64::from_bits(self.0.load(Ordering::SeqCst))
    }
    /// 原子写入浮点值。
    pub fn Store(&self, value: f64) {
        self.0.store(value.to_bits(), Ordering::SeqCst)
    }
}

/// 用 `RwLock<String>` 模拟的“原子”字符串，供全局可变字符串配置。
pub struct AtomicStringValue(RwLock<String>);
impl AtomicStringValue {
    /// 用初始字符串构造。
    pub fn new(value: &str) -> Self {
        Self(RwLock::new(value.to_owned()))
    }
    /// 读锁克隆当前字符串。
    pub fn Load(&self) -> String {
        self.0.read().expect("atomic string poisoned").clone()
    }
    /// 写锁替换字符串内容。
    pub fn Store(&self, value: impl Into<String>) {
        *self.0.write().expect("atomic string poisoned") = value.into();
    }
}

/// 无限制的慢日志速率限制器：`Allow` 恒为 true。
pub struct UnlimitedRateLimiter;
impl UnlimitedRateLimiter {
    /// 构造无限制速率限制器。
    pub const fn new() -> Self {
        Self
    }
    /// 始终允许通过（无节流）。
    pub fn Allow(&self) -> bool {
        true
    }
}

#[derive(Default)]
/// 全局慢日志多维规则表（规则 ID → 规则文本）。
pub struct GlobalSlowLogRulesValue {
    /// 规则 ID 到规则定义字符串的映射。
    pub RulesMap: std::collections::HashMap<i64, String>,
}

// Go imports（保留依赖边界，待 Rust crate 接线）：
// 	"fmt"
// 	"math"
// 	"strconv"
// 	"strings"
// 	goatomic "sync/atomic"
// 	"time"
//
// 	"github.com/pingcap/tidb/pkg/config"
// 	"github.com/pingcap/tidb/pkg/config/kerneltype"
// 	"github.com/pingcap/tidb/pkg/executor/join/joinversion"
// 	"github.com/pingcap/tidb/pkg/parser/mysql"
// 	"github.com/pingcap/tidb/pkg/sessionctx/slowlogrule"
// 	"github.com/pingcap/tidb/pkg/util/memory"
// 	"github.com/pingcap/tidb/pkg/util/paging"
// 	"github.com/pingcap/tidb/pkg/util/size"
// 	"github.com/pingcap/tipb/go-tipb"
// 	"go.uber.org/atomic"
// 	"golang.org/x/time/rate"

/*
    Steps to add a new TiDB specific system variable:

    1. Add a new variable name with comment in this file.
    2. Add the default value of the new variable in this file.
    3. Add SysVar instance in 'defaultSysVars' slice.
*/

// TiDB system variable names that only in session scope.
// Go const block：以下常量按原声明顺序逐项迁移。
// —— 会话作用域 TiDB 系统变量名 ——
/// DDL 慢操作阈值（毫秒）变量名。
pub const TiDBDDLSlowOprThreshold: &str = "ddl_slow_threshold";

// TiDBSnapshot is used for reading history data, the default value is empty string.
// The value can be a datetime string like '2017-11-11 20:20:20' or a tso string. When this variable is set, the session reads history data of that time.
pub const TiDBSnapshot: &str = "tidb_snapshot";

// TiDBOptAggPushDown is used to enable/disable the optimizer rule of aggregation push down.
pub const TiDBOptAggPushDown: &str = "tidb_opt_agg_push_down";

// TiDBOptDeriveTopN is used to enable/disable the optimizer rule of deriving topN.
pub const TiDBOptDeriveTopN: &str = "tidb_opt_derive_topn";

// TiDBOptCartesianBCJ is used to disable/enable broadcast cartesian join in MPP mode
pub const TiDBOptCartesianBCJ: &str = "tidb_opt_broadcast_cartesian_join";

pub const TiDBOptMPPOuterJoinFixedBuildSide: &str = "tidb_opt_mpp_outer_join_fixed_build_side";

// TiDBOptDistinctAggPushDown is used to decide whether agg with distinct should be pushed to tikv/tiflash.
pub const TiDBOptDistinctAggPushDown: &str = "tidb_opt_distinct_agg_push_down";

// TiDBOptSkewDistinctAgg is used to indicate the distinct agg has data skew
pub const TiDBOptSkewDistinctAgg: &str = "tidb_opt_skew_distinct_agg";

// TiDBOpt3StageDistinctAgg is used to indicate whether to plan and execute the distinct agg in 3 stages
pub const TiDBOpt3StageDistinctAgg: &str = "tidb_opt_three_stage_distinct_agg";

// TiDBOptEnable3StageMultiDistinctAgg is used to indicate whether to plan and execute the multi distinct agg in 3 stages
pub const TiDBOptEnable3StageMultiDistinctAgg: &str =
    "tidb_opt_enable_three_stage_multi_distinct_agg";

pub const TiDBOptExplainNoEvaledSubQuery: &str = "tidb_opt_enable_non_eval_scalar_subquery";

// TiDBBCJThresholdSize is used to limit the size of small table for mpp broadcast join.
// Its unit is bytes, if the size of small table is larger than it, we will not use bcj.
pub const TiDBBCJThresholdSize: &str = "tidb_broadcast_join_threshold_size";

// TiDBBCJThresholdCount is used to limit the count of small table for mpp broadcast join.
// If we can't estimate the size of one side of join child, we will check if its row number exceeds this limitation.
pub const TiDBBCJThresholdCount: &str = "tidb_broadcast_join_threshold_count";

// TiDBPreferBCJByExchangeDataSize indicates the method used to choose mpp broadcast join
pub const TiDBPreferBCJByExchangeDataSize: &str =
    "tidb_prefer_broadcast_join_by_exchange_data_size";

// TiDBOptWriteRowID is used to enable/disable the operations of insert、replace and update to _tidb_rowid.
pub const TiDBOptWriteRowID: &str = "tidb_opt_write_row_id";

// TiDBAutoAnalyzeRatio will run if (table modify count)/(table row count) is greater than this value.
pub const TiDBAutoAnalyzeRatio: &str = "tidb_auto_analyze_ratio";

// TiDBAutoAnalyzeStartTime will run if current time is within start time and end time.
pub const TiDBAutoAnalyzeStartTime: &str = "tidb_auto_analyze_start_time";
pub const TiDBAutoAnalyzeEndTime: &str = "tidb_auto_analyze_end_time";

// TiDBChecksumTableConcurrency is used to speed up the ADMIN CHECKSUM TABLE
// statement, when a table has multiple indices, those indices can be
// scanned concurrently, with the cost of higher system performance impact.
pub const TiDBChecksumTableConcurrency: &str = "tidb_checksum_table_concurrency";

// TiDBCurrentTS is used to get the current transaction timestamp.
// It is read-only.
pub const TiDBCurrentTS: &str = "tidb_current_ts";

// TiDBLastTxnInfo is used to get the last transaction info within the current session.
pub const TiDBLastTxnInfo: &str = "tidb_last_txn_info";

// TiDBLastQueryInfo is used to get the last query info within the current session.
pub const TiDBLastQueryInfo: &str = "tidb_last_query_info";

// TiDBLastDDLInfo is used to get the last ddl info within the current session.
pub const TiDBLastDDLInfo: &str = "tidb_last_ddl_info";

// TiDBLastPlanReplayerToken is used to get the last plan replayer token within the current session
pub const TiDBLastPlanReplayerToken: &str = "tidb_last_plan_replayer_token";

// TiDBConfig is a read-only variable that shows the config of the current server.
pub const TiDBConfig: &str = "tidb_config";

// TiDBBatchInsert is used to enable/disable auto-split insert data. If set this option on, insert executor will automatically
// insert data into multiple batches and use a single txn for each batch. This will be helpful when inserting large data.
pub const TiDBBatchInsert: &str = "tidb_batch_insert";

// TiDBBatchDelete is used to enable/disable auto-split delete data. If set this option on, delete executor will automatically
// split data into multiple batches and use a single txn for each batch. This will be helpful when deleting large data.
pub const TiDBBatchDelete: &str = "tidb_batch_delete";

// TiDBBatchCommit is used to enable/disable auto-split the transaction.
// If set this option on, the transaction will be committed when it reaches stmt-count-limit and starts a new transaction.
pub const TiDBBatchCommit: &str = "tidb_batch_commit";

// TiDBDMLBatchSize is used to split the insert/delete data into small batches.
// It only takes effort when tidb_batch_insert/tidb_batch_delete is on.
// Its default value is 20000. When the row size is large, 20k rows could be larger than 100MB.
// User could change it to a smaller one to avoid breaking the transaction size limitation.
pub const TiDBDMLBatchSize: &str = "tidb_dml_batch_size";
/// Maximum rows deleted by one materialized-view-log purge batch.
pub const TiDBMLogPurgeBatchSize: &str = "tidb_mlog_purge_batch_size";
/// Minimum target row deletion rate for adaptive MLog purge.
pub const TiDBMLogPurgeMinRate: &str = "tidb_mlog_purge_min_rate";
/// Fraction of the available schedule window allocated to MLog purge.
pub const TiDBMLogPurgeRateBudgetRatio: &str = "tidb_mlog_purge_rate_budget_ratio";
/// TiFlash thread limit for MLog purge DELETE statements.
pub const TiDBMLogPurgeDeleteTiFlashThreads: &str = "tidb_mlog_purge_delete_tiflash_threads";

// The following session variables controls the memory quota during query execution.

// TiDBMemQuotaQuery controls the memory quota of a query.
pub const TiDBMemQuotaQuery: &str = "tidb_mem_quota_query"; // Bytes.;
// TiDBMemQuotaApplyCache controls the memory quota of a query.
pub const TiDBMemQuotaApplyCache: &str = "tidb_mem_quota_apply_cache";

// TiDBGeneralLog is used to log every query in the server in info level.
pub const TiDBGeneralLog: &str = "tidb_general_log";

// TiDBTraceEvent controls the experimental trace event instrumentation.
pub const TiDBTraceEvent: &str = "tidb_trace_event";

// TiDBLogFileMaxDays is used to log every query in the server in info level.
pub const TiDBLogFileMaxDays: &str = "tidb_log_file_max_days";

// TiDBPProfSQLCPU is used to add label sql label to pprof result.
pub const TiDBPProfSQLCPU: &str = "tidb_pprof_sql_cpu";

// TiDBRetryLimit is the maximum number of retries when committing a transaction.
pub const TiDBRetryLimit: &str = "tidb_retry_limit";

// TiDBDisableTxnAutoRetry disables transaction auto retry.
// Deprecated: This variable is deprecated, please do not use this variable.
pub const TiDBDisableTxnAutoRetry: &str = "tidb_disable_txn_auto_retry";

// TiDBEnableChunkRPC enables TiDB to use Chunk format for coprocessor requests.
pub const TiDBEnableChunkRPC: &str = "tidb_enable_chunk_rpc";

// TiDBOptimizerSelectivityLevel is used to control the selectivity estimation level.
pub const TiDBOptimizerSelectivityLevel: &str = "tidb_optimizer_selectivity_level";

// TiDBOptIndexPruneThreshold is used to control the threshold for index pruning optimization.
pub const TiDBOptIndexPruneThreshold: &str = "tidb_opt_index_prune_threshold";

// TiDBOptimizerEnableNewOnlyFullGroupByCheck is used to open the newly only_full_group_by check by maintaining functional dependency.
pub const TiDBOptimizerEnableNewOnlyFullGroupByCheck: &str =
    "tidb_enable_new_only_full_group_by_check";

pub const TiDBOptimizerEnableOuterJoinReorder: &str = "tidb_enable_outer_join_reorder";

// TiDBOptimizerEnableNAAJ is used to open the newly null-aware anti join
pub const TiDBOptimizerEnableNAAJ: &str = "tidb_enable_null_aware_anti_join";

// TiDBTxnMode is used to control the transaction behavior.
pub const TiDBTxnMode: &str = "tidb_txn_mode";

// TiDBRowFormatVersion is used to control tidb row format version current.
pub const TiDBRowFormatVersion: &str = "tidb_row_format_version";

// TiDBEnableRowLevelChecksum is used to control whether to append checksum to row values.
pub const TiDBEnableRowLevelChecksum: &str = "tidb_enable_row_level_checksum";

// TiDBEnableTablePartition is used to control table partition feature.
// The valid value include auto/on/off:
// on or auto: enable table partition if the partition type is implemented.
// off: always disable table partition.
pub const TiDBEnableTablePartition: &str = "tidb_enable_table_partition";

// TiDBEnableListTablePartition is used to control list table partition feature.
// Deprecated: This variable is deprecated, please do not use this variable.
pub const TiDBEnableListTablePartition: &str = "tidb_enable_list_partition";

// TiDBSkipIsolationLevelCheck is used to control whether to return error when set unsupported transaction
// isolation level.
pub const TiDBSkipIsolationLevelCheck: &str = "tidb_skip_isolation_level_check";

// TiDBLowResolutionTSO is used for reading data with low resolution TSO which is updated once every two seconds
pub const TiDBLowResolutionTSO: &str = "tidb_low_resolution_tso";

// TiDBReplicaRead is used for reading data from replicas, followers for example.
pub const TiDBReplicaRead: &str = "tidb_replica_read";

// TiDBAdaptiveClosestReadThreshold is for reading data from closest replicas(with same 'zone' label).
// TiKV client should send read request to the closest replica(leader/follower) if the estimated response
// size exceeds this threshold; otherwise, this request should be sent to leader.
// This variable only take effect when `tidb_replica_read` is 'closest-adaptive'.
pub const TiDBAdaptiveClosestReadThreshold: &str = "tidb_adaptive_closest_read_threshold";

// TiDBAllowRemoveAutoInc indicates whether a user can drop the auto_increment column attribute or not.
pub const TiDBAllowRemoveAutoInc: &str = "tidb_allow_remove_auto_inc";

// TiDBMultiStatementMode enables multi statement at the risk of SQL injection
// provides backwards compatibility
pub const TiDBMultiStatementMode: &str = "tidb_multi_statement_mode";

// TiDBEvolvePlanTaskMaxTime controls the max time of a single evolution task.
pub const TiDBEvolvePlanTaskMaxTime: &str = "tidb_evolve_plan_task_max_time";

// TiDBEvolvePlanTaskStartTime is the start time of evolution task.
pub const TiDBEvolvePlanTaskStartTime: &str = "tidb_evolve_plan_task_start_time";
// TiDBEvolvePlanTaskEndTime is the end time of evolution task.
pub const TiDBEvolvePlanTaskEndTime: &str = "tidb_evolve_plan_task_end_time";

// TiDBSlowLogThreshold is used to set the slow log threshold in the server.
pub const TiDBSlowLogThreshold: &str = "tidb_slow_log_threshold";

// TiDBSlowLogRules defines multi-dimensional trigger rules for flexible slow log control.
pub const TiDBSlowLogRules: &str = "tidb_slow_log_rules";

// TiDBSlowLogMaxPerSec is the maximum number of slow logs that can be recorded per second in the server.
// The default value is 0, which means no rate limiting is applied.
pub const TiDBSlowLogMaxPerSec: &str = "tidb_slow_log_max_per_sec";

// TiDBSlowTxnLogThreshold is used to set the slow transaction log threshold in the server.
pub const TiDBSlowTxnLogThreshold: &str = "tidb_slow_txn_log_threshold";

// TiDBRecordPlanInSlowLog is used to log the plan of the slow query.
pub const TiDBRecordPlanInSlowLog: &str = "tidb_record_plan_in_slow_log";

// TiDBEnableSlowLog enables TiDB to log slow queries.
pub const TiDBEnableSlowLog: &str = "tidb_enable_slow_log";

// TiDBCheckMb4ValueInUTF8 is used to control whether to enable the check wrong utf8 value.
pub const TiDBCheckMb4ValueInUTF8: &str = "tidb_check_mb4_value_in_utf8";

// TiDBFoundInPlanCache indicates whether the last statement was found in plan cache
pub const TiDBFoundInPlanCache: &str = "last_plan_from_cache";

// TiDBFoundInBinding indicates whether the last statement was matched with the hints in the binding.
pub const TiDBFoundInBinding: &str = "last_plan_from_binding";

// TiDBAllowAutoRandExplicitInsert indicates whether explicit insertion on auto_random column is allowed.
pub const TiDBAllowAutoRandExplicitInsert: &str = "allow_auto_random_explicit_insert";

// TiDBTxnReadTS indicates the next transaction should be staleness transaction and provide the startTS
pub const TiDBTxnReadTS: &str = "tx_read_ts";

// TiDBReadStaleness indicates the staleness duration for following statement
pub const TiDBReadStaleness: &str = "tidb_read_staleness";

// TiDBEnablePaging indicates whether paging is enabled in coprocessor requests.
pub const TiDBEnablePaging: &str = "tidb_enable_paging";

// TiDBReadConsistency indicates whether the autocommit read statement goes through TiKV RC.
pub const TiDBReadConsistency: &str = "tidb_read_consistency";

// TiDBSysdateIsNow is the name of the `tidb_sysdate_is_now` system variable
pub const TiDBSysdateIsNow: &str = "tidb_sysdate_is_now";

// RequireSecureTransport indicates the secure mode for data transport
pub const RequireSecureTransport: &str = "require_secure_transport";

// TiFlashFastScan indicates whether use fast scan in tiflash.
pub const TiFlashFastScan: &str = "tiflash_fastscan";

// TiDBEnableUnsafeSubstitute indicates whether to enable generate column takes unsafe substitute.
pub const TiDBEnableUnsafeSubstitute: &str = "tidb_enable_unsafe_substitute";

// TiDBEnableTiFlashReadForWriteStmt indicates whether to enable TiFlash to read for write statements.
pub const TiDBEnableTiFlashReadForWriteStmt: &str = "tidb_enable_tiflash_read_for_write_stmt";

// TiDBUseAlloc indicates whether the last statement used chunk alloc
pub const TiDBUseAlloc: &str = "last_sql_use_alloc";

// TiDBExplicitRequestSourceType indicates the source of the request, it's a complement of RequestSourceType.
// The value maybe "lightning", "br", "dumpling" etc.
pub const TiDBExplicitRequestSourceType: &str = "tidb_request_source_type";

// 以下变量同时具备 SESSION 与 GLOBAL 作用域（可按会话或全局 SET）。
// TiDB system variable names that both in session and global scope.
// Go const block：以下常量按原声明顺序逐项迁移。
// TiDBBuildStatsConcurrency specifies the number of concurrent workers used for analyzing tables or partitions.
// When multiple tables or partitions are specified in the analyze statement, TiDB will process them concurrently.
// —— 实例/全局作用域 TiDB 系统变量名（含并发、优化器代价因子等）——
pub const TiDBBuildStatsConcurrency: &str = "tidb_build_stats_concurrency";

// TiDBBuildSamplingStatsConcurrency is used to control the concurrency of building stats using sampling.
// 1. The number of concurrent workers to merge FMSketches and Sample Data from different regions.
// 2. The number of concurrent workers to build TopN and Histogram concurrently.
// Additionally, this setting controls the concurrency for building NDV (Number of Distinct Values) for special indexes,
// such as generated columns composed indexes.
pub const TiDBBuildSamplingStatsConcurrency: &str = "tidb_build_sampling_stats_concurrency";

// TiDBDistSQLScanConcurrency is used to set the concurrency of a distsql scan task.
// A distsql scan task can be a table scan or a index scan, which may be distributed to many TiKV nodes.
// Higher concurrency may reduce latency, but with the cost of higher memory usage and system performance impact.
// If the query has a LIMIT clause, high concurrency makes the system do much more work than needed.
pub const TiDBDistSQLScanConcurrency: &str = "tidb_distsql_scan_concurrency";

// TiDBQueryCopStoreLimit limits TiKV cop request concurrency per store within one query.
// Zero disables the query-scoped limit.
pub const TiDBQueryCopStoreLimit: &str = "tidb_query_cop_store_limit";

// TiDBAnalyzeDistSQLScanConcurrency is the number of concurrent workers to scan regions to collect statistics (FMSketch, Samples).
// For auto analyze, the value is controlled by tidb_sysproc_scan_concurrency variable.
// This variable was introduced in v7.6.0 to separate the scan concurrency of ANALYZE operations from normal queries. See: https://github.com/pingcap/tidb/pull/48829
// For versions earlier than v7.6.0, the scan concurrency of regions during ANALYZE is controlled by the tidb_distsql_scan_concurrency variable.
// Starting from v7.6.0, this variable also controls the scan concurrency of index serial scans during ANALYZE. See: https://github.com/pingcap/tidb/pull/50639
// For versions earlier than v7.6.0, the scan concurrency of index serial scans during ANALYZE is controlled by the tidb_index_serial_scan_concurrency variable.
// Maximum child Region tasks grouped with a main Region in an Analyze store batch; 0 disables batching.
pub const TiDBAnalyzeStoreBatchSize: &str = "tidb_analyze_store_batch_size";
pub const DefTiDBAnalyzeStoreBatchSize: i64 = 4;
// Tasks run serially with buffered results; cap batches to limit latency and TiKV memory.
pub const MaxTiDBAnalyzeStoreBatchSize: u64 = 8;

pub const TiDBAnalyzeDistSQLScanConcurrency: &str = "tidb_analyze_distsql_scan_concurrency";

// TiDBOptInSubqToJoinAndAgg is used to enable/disable the optimizer rule of rewriting IN subquery.
pub const TiDBOptInSubqToJoinAndAgg: &str = "tidb_opt_insubq_to_join_and_agg";

// TiDBOptPreferRangeScan is used to enable/disable the optimizer to always prefer range scan over table scan, ignoring their costs.
pub const TiDBOptPreferRangeScan: &str = "tidb_opt_prefer_range_scan";

// TiDBOptEnableNoDecorrelateInSelect is used to control whether to enable the NO_DECORRELATE hint for subqueries in the select list.
pub const TiDBOptEnableNoDecorrelateInSelect: &str = "tidb_opt_enable_no_decorrelate_in_select";

// TiDBOptEnableAlternativeLogicalPlans controls whether the optimizer may build
// an extra non-decorrelate logical alternative when decorrelation does not
// produce an equivalent same-order index join candidate.
pub const TiDBOptEnableAlternativeLogicalPlans: &str = "tidb_opt_enable_alternative_logical_plans";

// TiDBEnableSemiJoinRewrite controls automatic rewrite of semi-join to
// inner-join with aggregation (equivalent to SEMI_JOIN_REWRITE() hint).
pub const TiDBOptEnableSemiJoinRewrite: &str = "tidb_opt_enable_semi_join_rewrite";

// TiDBOptEnableCorrelationAdjustment is used to indicates if enable correlation adjustment.
pub const TiDBOptEnableCorrelationAdjustment: &str = "tidb_opt_enable_correlation_adjustment";

// TiDBOptLimitPushDownThreshold determines if push Limit or TopN down to TiKV forcibly.
pub const TiDBOptLimitPushDownThreshold: &str = "tidb_opt_limit_push_down_threshold";

// TiDBOptCorrelationThreshold is a guard to enable row count estimation using column order correlation.
pub const TiDBOptCorrelationThreshold: &str = "tidb_opt_correlation_threshold";

// TiDBOptCorrelationExpFactor is an exponential factor to control heuristic approach when tidb_opt_correlation_threshold is not satisfied.
pub const TiDBOptCorrelationExpFactor: &str = "tidb_opt_correlation_exp_factor";

// TiDBOptRiskEqSkewRatio controls the amount of skew is applied to equal predicate estimation when a value is not found in TopN/buckets.
pub const TiDBOptRiskEqSkewRatio: &str = "tidb_opt_risk_eq_skew_ratio";

// TiDBOptRiskRangeSkewRatio controls the amount of skew that is applied to range predicate estimation when a range falls within a bucket or outside the histogram bucket range.
pub const TiDBOptRiskRangeSkewRatio: &str = "tidb_opt_risk_range_skew_ratio";

// TiDBOptRiskScaleNDVSkewRatio controls the NDV estimation risk strategy for scaling NDV estimation.
pub const TiDBOptRiskScaleNDVSkewRatio: &str = "tidb_opt_scale_ndv_skew_ratio";

// TiDBOptRiskGroupNDVSkewRatio controls the NDV estimation risk strategy for multi-column operations
// including GROUP BY, JOIN, and DISTINCT operations.
// When 0: uses conservative estimate (max of individual column NDVs, production default)
// When > 0: blends conservative and exponential backoff estimates (0.1=mostly conservative, 1.0=full exponential)
pub const TiDBOptRiskGroupNDVSkewRatio: &str = "tidb_opt_group_ndv_skew_ratio";

// TiDBOptAlwaysKeepJoinKey indicates the optimizer to always keep join keys during optimization.
// Join keys are crucial for join optimization like Join Order and Join Algorithm selection, removing
// join keys might lead to suboptimal plans in some cases.
pub const TiDBOptAlwaysKeepJoinKey: &str = "tidb_opt_always_keep_join_key";

// TiDBOptCartesianJoinOrderThreshold controls whether to allow do Cartesian Join first in Join Reorder.
// This variable is used as a penalty to trade off the risk and join order quality.
// When 0: never do Cartesian Join first.
// When > 0: allow Cartesian Join if cost(cartesian join) * threshold < cost(non cartesian join).
pub const TiDBOptCartesianJoinOrderThreshold: &str = "tidb_opt_cartesian_join_order_threshold";

// TiDBOptCPUFactor is the CPU cost of processing one expression for one row.
pub const TiDBOptCPUFactor: &str = "tidb_opt_cpu_factor";
// TiDBOptCopCPUFactor is the CPU cost of processing one expression for one row in coprocessor.
pub const TiDBOptCopCPUFactor: &str = "tidb_opt_copcpu_factor";
// TiDBOptTiFlashConcurrencyFactor is concurrency number of tiflash computation.
pub const TiDBOptTiFlashConcurrencyFactor: &str = "tidb_opt_tiflash_concurrency_factor";
// TiDBOptNetworkFactor is the network cost of transferring 1 byte data.
pub const TiDBOptNetworkFactor: &str = "tidb_opt_network_factor";
// TiDBOptScanFactor is the IO cost of scanning 1 byte data on TiKV.
pub const TiDBOptScanFactor: &str = "tidb_opt_scan_factor";
// TiDBOptDescScanFactor is the IO cost of scanning 1 byte data on TiKV in desc order.
pub const TiDBOptDescScanFactor: &str = "tidb_opt_desc_factor";
// TiDBOptSeekFactor is the IO cost of seeking the start value in a range on TiKV or TiFlash.
pub const TiDBOptSeekFactor: &str = "tidb_opt_seek_factor";
// TiDBOptMemoryFactor is the memory cost of storing one tuple.
pub const TiDBOptMemoryFactor: &str = "tidb_opt_memory_factor";
// TiDBOptDiskFactor is the IO cost of reading/writing one byte to temporary disk.
pub const TiDBOptDiskFactor: &str = "tidb_opt_disk_factor";
// TiDBOptConcurrencyFactor is the CPU cost of additional one goroutine.
pub const TiDBOptConcurrencyFactor: &str = "tidb_opt_concurrency_factor";

// The following optimizer cost factors represent a multiplier for each optimizer physical operator.
// These factors are used to adjust the cost of each operator to influence the optimizer's plan selection.
pub const TiDBOptIndexScanCostFactor: &str = "tidb_opt_index_scan_cost_factor";
pub const TiDBOptIndexReaderCostFactor: &str = "tidb_opt_index_reader_cost_factor";
pub const TiDBOptTableReaderCostFactor: &str = "tidb_opt_table_reader_cost_factor";
pub const TiDBOptTableFullScanCostFactor: &str = "tidb_opt_table_full_scan_cost_factor";
pub const TiDBOptTableRangeScanCostFactor: &str = "tidb_opt_table_range_scan_cost_factor";
pub const TiDBOptTableRowIDScanCostFactor: &str = "tidb_opt_table_rowid_scan_cost_factor";
pub const TiDBOptTableTiFlashScanCostFactor: &str = "tidb_opt_table_tiflash_scan_cost_factor";
pub const TiDBOptIndexLookupCostFactor: &str = "tidb_opt_index_lookup_cost_factor";
pub const TiDBOptIndexMergeCostFactor: &str = "tidb_opt_index_merge_cost_factor";
pub const TiDBOptSortCostFactor: &str = "tidb_opt_sort_cost_factor";
pub const TiDBOptTopNCostFactor: &str = "tidb_opt_topn_cost_factor";
pub const TiDBOptLimitCostFactor: &str = "tidb_opt_limit_cost_factor";
pub const TiDBOptStreamAggCostFactor: &str = "tidb_opt_stream_agg_cost_factor";
pub const TiDBOptHashAggCostFactor: &str = "tidb_opt_hash_agg_cost_factor";
pub const TiDBOptMergeJoinCostFactor: &str = "tidb_opt_merge_join_cost_factor";
pub const TiDBOptHashJoinCostFactor: &str = "tidb_opt_hash_join_cost_factor";
pub const TiDBOptIndexJoinCostFactor: &str = "tidb_opt_index_join_cost_factor";
pub const TiDBOptIndexJoinMaxScanRowsRatio: &str = "tidb_opt_index_join_max_scan_rows_ratio";

// The following selectivity factors represent a multiplier for the selectivity of each predicate.
// These factors are used to determine the selectivity of predicates in the optimizer's cost model.
// TiDBOptSelectivityFactor: If one condition can't be calculated,
// we will assume that the selectivity of this condition is 0.8 by default.
pub const TiDBOptSelectivityFactor: &str = "tidb_opt_selectivity_factor";

// TiDBOptForceInlineCTE is used to enable/disable inline CTE
pub const TiDBOptForceInlineCTE: &str = "tidb_opt_force_inline_cte";

// TiDBIndexJoinBatchSize is used to set the batch size of an index lookup join.
// The index lookup join fetches batches of data from outer executor and constructs ranges for inner executor.
// This value controls how much of data in a batch to do the index join.
// Large value may reduce the latency but consumes more system resource.
pub const TiDBIndexJoinBatchSize: &str = "tidb_index_join_batch_size";

// TiDBIndexLookupSize is used for index lookup executor.
// The index lookup executor first scan a batch of handles from a index, then use those handles to lookup the table
// rows, this value controls how much of handles in a batch to do a lookup task.
// Small value sends more RPCs to TiKV, consume more system resource.
// Large value may do more work than needed if the query has a limit.
pub const TiDBIndexLookupSize: &str = "tidb_index_lookup_size";

// TiDBIndexLookupConcurrency is used for index lookup executor.
// A lookup task may have 'tidb_index_lookup_size' of handles at maximum, the handles may be distributed
// in many TiKV nodes, we execute multiple concurrent index lookup tasks concurrently to reduce the time
// waiting for a task to finish.
// Set this value higher may reduce the latency but consumes more system resource.
// tidb_index_lookup_concurrency is deprecated, use tidb_executor_concurrency instead.
pub const TiDBIndexLookupConcurrency: &str = "tidb_index_lookup_concurrency";

// TiDBIndexLookupJoinConcurrency is used for index lookup join executor.
// IndexLookUpJoin starts "tidb_index_lookup_join_concurrency" inner workers
// to fetch inner rows and join the matched (outer, inner) row pairs.
// tidb_index_lookup_join_concurrency is deprecated, use tidb_executor_concurrency instead.
pub const TiDBIndexLookupJoinConcurrency: &str = "tidb_index_lookup_join_concurrency";

// TiDBIndexSerialScanConcurrency is used for controlling the concurrency of index scan operation
// when we need to keep the data output order the same as the order of index data.
// Deprecated: Use tidb_executor_concurrency for sequential scans and tidb_analyze_distsql_scan_concurrency for ANALYZE.
// Before v5.0.0, this variable was used to control the concurrency of index scan operations for both regular queries and ANALYZE statements. See: https://github.com/pingcap/tidb/pull/16999
// From version v5.0.0 up to (and including) v8.0.0, this variable was used only to control the concurrency of index scan operations for ANALYZE statements. See: https://github.com/pingcap/tidb/pull/50639
pub const TiDBIndexSerialScanConcurrency: &str = "tidb_index_serial_scan_concurrency";

// TiDBMaxChunkSize is used to control the max chunk size during query execution.
pub const TiDBMaxChunkSize: &str = "tidb_max_chunk_size";

// TiDBAllowBatchCop means if we should send batch coprocessor to TiFlash. It can be set to 0, 1 and 2.
// 0 means never use batch cop, 1 means use batch cop in case of aggregation and join, 2, means to force sending batch cop for any query.
// The default value is 0
pub const TiDBAllowBatchCop: &str = "tidb_allow_batch_cop";

// TiDBShardRowIDBits means all the tables created in the current session will be sharded.
// The default value is 0
pub const TiDBShardRowIDBits: &str = "tidb_shard_row_id_bits";

// TiDBPreSplitRegions means all the tables created in the current session will be pre-splited.
// The default value is 0
pub const TiDBPreSplitRegions: &str = "tidb_pre_split_regions";

// TiDBAllowMPPExecution means if we should use mpp way to execute query or not.
// Default value is `true`, means to be determined by the optimizer.
// Value set to `false` means never use mpp.
pub const TiDBAllowMPPExecution: &str = "tidb_allow_mpp";

// TiDBAllowTiFlashCop means we only use MPP mode to query data.
// Default value is `true`, means to be determined by the optimizer.
// Value set to `false` means we may fall back to TiFlash cop plan if possible.
pub const TiDBAllowTiFlashCop: &str = "tidb_allow_tiflash_cop";

// TiDBHashExchangeWithNewCollation means if hash exchange is supported when new collation is on.
// Default value is `true`, means support hash exchange when new collation is on.
// Value set to `false` means not support hash exchange when new collation is on.
pub const TiDBHashExchangeWithNewCollation: &str = "tidb_hash_exchange_with_new_collation";

// TiDBEnforceMPPExecution means if we should enforce mpp way to execute query or not.
// Default value is `false`, means to be determined by variable `tidb_allow_mpp`.
// Value set to `true` means enforce use mpp.
// Note if you want to set `tidb_enforce_mpp` to `true`, you must set `tidb_allow_mpp` to `true` first.
pub const TiDBEnforceMPPExecution: &str = "tidb_enforce_mpp";

// TiDBMaxTiFlashThreads is the maximum number of threads to execute the request which is pushed down to tiflash.
// Default value is -1, means it will not be pushed down to tiflash.
// If the value is bigger than -1, it will be pushed down to tiflash and used to create db context in tiflash.
pub const TiDBMaxTiFlashThreads: &str = "tidb_max_tiflash_threads";

// TiDBMaxBytesBeforeTiFlashExternalJoin is the maximum bytes used by a TiFlash join before spill to disk
pub const TiDBMaxBytesBeforeTiFlashExternalJoin: &str =
    "tidb_max_bytes_before_tiflash_external_join";

// TiDBMaxBytesBeforeTiFlashExternalGroupBy is the maximum bytes used by a TiFlash hash aggregation before spill to disk
pub const TiDBMaxBytesBeforeTiFlashExternalGroupBy: &str =
    "tidb_max_bytes_before_tiflash_external_group_by";

// TiDBMaxBytesBeforeTiFlashExternalSort is the maximum bytes used by a TiFlash sort/TopN before spill to disk
pub const TiDBMaxBytesBeforeTiFlashExternalSort: &str =
    "tidb_max_bytes_before_tiflash_external_sort";

// TiFlashMemQuotaQueryPerNode is the maximum bytes used by a TiFlash Query on each TiFlash node
pub const TiFlashMemQuotaQueryPerNode: &str = "tiflash_mem_quota_query_per_node";

// TiFlashQuerySpillRatio is the threshold that TiFlash will trigger auto spill when the memory usage is above this percentage
pub const TiFlashQuerySpillRatio: &str = "tiflash_query_spill_ratio";

// TiFlashHashJoinVersion indicates whether to use hash join implementation v2 in TiFlash.
pub const TiFlashHashJoinVersion: &str = "tiflash_hash_join_version";

// TiDBMPPStoreFailTTL is the unavailable time when a store is detected failed. During that time, tidb will not send any task to
// TiFlash even though the failed TiFlash node has been recovered.
pub const TiDBMPPStoreFailTTL: &str = "tidb_mpp_store_fail_ttl";

// TiDBInitChunkSize is used to control the init chunk size during query execution.
pub const TiDBInitChunkSize: &str = "tidb_init_chunk_size";

// TiDBMinPagingSize is used to control the min paging size in the coprocessor paging protocol.
pub const TiDBMinPagingSize: &str = "tidb_min_paging_size";

// TiDBMaxPagingSize is used to control the max paging size in the coprocessor paging protocol.
pub const TiDBMaxPagingSize: &str = "tidb_max_paging_size";

// TiDBPagingSizeBytes is the byte budget per coprocessor page.
// 0 means disabled (no byte-budget paging).
pub const TiDBPagingSizeBytes: &str = "tidb_paging_size_bytes";

// TiDBEnableCascadesPlanner is used to control whether to enable the cascades planner.
pub const TiDBEnableCascadesPlanner: &str = "tidb_enable_cascades_planner";

/// Opt-in FULL OUTER JOIN support.
pub const TiDBEnableFullOuterJoin: &str = "tidb_enable_full_outer_join";

// TiDBSkipUTF8Check skips the UTF8 validate process, validate UTF8 has performance cost, if we can make sure
// the input string values are valid, we can skip the check.
pub const TiDBSkipUTF8Check: &str = "tidb_skip_utf8_check";

// TiDBSkipASCIICheck skips the ASCII validate process
// old tidb may already have fields with invalid ASCII bytes
// disable ASCII validate can guarantee a safe replication
pub const TiDBSkipASCIICheck: &str = "tidb_skip_ascii_check";

// TiDBHashJoinConcurrency is used for hash join executor.
// The hash join outer executor starts multiple concurrent join workers to probe the hash table.
// tidb_hash_join_concurrency is deprecated, use tidb_executor_concurrency instead.
pub const TiDBHashJoinConcurrency: &str = "tidb_hash_join_concurrency";

// TiDBProjectionConcurrency is used for projection operator.
// This variable controls the worker number of projection operator.
// tidb_projection_concurrency is deprecated, use tidb_executor_concurrency instead.
pub const TiDBProjectionConcurrency: &str = "tidb_projection_concurrency";

// TiDBHashAggPartialConcurrency is used for hash agg executor.
// The hash agg executor starts multiple concurrent partial workers to do partial aggregate works.
// tidb_hashagg_partial_concurrency is deprecated, use tidb_executor_concurrency instead.
pub const TiDBHashAggPartialConcurrency: &str = "tidb_hashagg_partial_concurrency";

// TiDBHashAggFinalConcurrency is used for hash agg executor.
// The hash agg executor starts multiple concurrent final workers to do final aggregate works.
// tidb_hashagg_final_concurrency is deprecated, use tidb_executor_concurrency instead.
pub const TiDBHashAggFinalConcurrency: &str = "tidb_hashagg_final_concurrency";

// TiDBWindowConcurrency is used for window parallel executor.
// tidb_window_concurrency is deprecated, use tidb_executor_concurrency instead.
pub const TiDBWindowConcurrency: &str = "tidb_window_concurrency";

// TiDBMergeJoinConcurrency is used for merge join parallel executor
pub const TiDBMergeJoinConcurrency: &str = "tidb_merge_join_concurrency";

// TiDBStreamAggConcurrency is used for stream aggregation parallel executor.
// tidb_stream_agg_concurrency is deprecated, use tidb_executor_concurrency instead.
pub const TiDBStreamAggConcurrency: &str = "tidb_streamagg_concurrency";

// TiDBIndexMergeIntersectionConcurrency is used for parallel worker of index merge intersection.
pub const TiDBIndexMergeIntersectionConcurrency: &str = "tidb_index_merge_intersection_concurrency";

// TiDBEnableParallelApply is used for parallel apply.
pub const TiDBEnableParallelApply: &str = "tidb_enable_parallel_apply";

// TiDBBackoffLockFast is used for tikv backoff base time in milliseconds.
pub const TiDBBackoffLockFast: &str = "tidb_backoff_lock_fast";

// TiDBBackOffWeight is used to control the max back off time in TiDB.
// The default maximum back off time is a small value.
// BackOffWeight could multiply it to let the user adjust the maximum time for retrying.
// Only positive integers can be accepted, which means that the maximum back off time can only grow.
pub const TiDBBackOffWeight: &str = "tidb_backoff_weight";

// TiDBDDLReorgWorkerCount defines the count of ddl reorg workers.
pub const TiDBDDLReorgWorkerCount: &str = "tidb_ddl_reorg_worker_cnt";

// TiDBDDLFlashbackConcurrency defines the count of ddl flashback workers.
pub const TiDBDDLFlashbackConcurrency: &str = "tidb_ddl_flashback_concurrency";

// TiDBDDLReorgBatchSize defines the transaction batch size of ddl reorg workers.
pub const TiDBDDLReorgBatchSize: &str = "tidb_ddl_reorg_batch_size";

// TiDBDDLErrorCountLimit defines the count of ddl error limit.
pub const TiDBDDLErrorCountLimit: &str = "tidb_ddl_error_count_limit";

// TiDBDDLReorgPriority defines the operations' priority of adding indices.
// It can be: PRIORITY_LOW, PRIORITY_NORMAL, PRIORITY_HIGH
pub const TiDBDDLReorgPriority: &str = "tidb_ddl_reorg_priority";

// TiDBDDLReorgMaxWriteSpeed defines the max write limitation for the lightning local backend
pub const TiDBDDLReorgMaxWriteSpeed: &str = "tidb_ddl_reorg_max_write_speed";

// TiDBEnableAutoIncrementInGenerated disables the mysql compatibility check on using auto-incremented columns in
// expression indexes and generated columns described here https://dev.mysql.com/doc/refman/5.7/en/create-table-generated-columns.html for details.
pub const TiDBEnableAutoIncrementInGenerated: &str = "tidb_enable_auto_increment_in_generated";

// TiDBEnablePointGetCache is used to control whether to enable the point get cache for special scenario.
pub const TiDBEnablePointGetCache: &str = "tidb_enable_point_get_cache";

// TiDBPlacementMode is used to control the mode for placement
pub const TiDBPlacementMode: &str = "tidb_placement_mode";

// TiDBMaxDeltaSchemaCount defines the max length of deltaSchemaInfos.
// deltaSchemaInfos is a queue that maintains the history of schema changes.
pub const TiDBMaxDeltaSchemaCount: &str = "tidb_max_delta_schema_count";

// TiDBScatterRegion will scatter the regions for DDLs when it is "table" or "global", "" indicates not trigger scatter.
pub const TiDBScatterRegion: &str = "tidb_scatter_region";

// TiDBWaitSplitRegionFinish defines the split region behaviour is sync or async.
pub const TiDBWaitSplitRegionFinish: &str = "tidb_wait_split_region_finish";

// TiDBWaitSplitRegionTimeout uses to set the split and scatter region back off time.
pub const TiDBWaitSplitRegionTimeout: &str = "tidb_wait_split_region_timeout";

// TiDBForcePriority defines the operations' priority of all statements.
// It can be "NO_PRIORITY", "LOW_PRIORITY", "HIGH_PRIORITY", "DELAYED"
pub const TiDBForcePriority: &str = "tidb_force_priority";

// TiDBConstraintCheckInPlace indicates to check the constraint when the SQL executing.
// It could hurt the performance of bulking insert when it is ON.
pub const TiDBConstraintCheckInPlace: &str = "tidb_constraint_check_in_place";

// TiDBEnableWindowFunction is used to control whether to enable the window function.
pub const TiDBEnableWindowFunction: &str = "tidb_enable_window_function";

// TiDBEnablePipelinedWindowFunction is used to control whether to use pipelined window function, it only works when tidb_enable_window_function = true.
pub const TiDBEnablePipelinedWindowFunction: &str = "tidb_enable_pipelined_window_function";

// TiDBEnableStrictNotNullCheck is used to control whether to enable strict not-null check for single-row insert in non-strict mode.
pub const TiDBEnableStrictNotNullCheck: &str = "tidb_enable_strict_not_null_check";

// TiDBEnableStrictDoubleTypeCheck is used to control table field double type syntax check.
pub const TiDBEnableStrictDoubleTypeCheck: &str = "tidb_enable_strict_double_type_check";

// TiDBOptProjectionPushDown is used to control whether to pushdown projection to coprocessor.
pub const TiDBOptProjectionPushDown: &str = "tidb_opt_projection_push_down";

// TiDBEnableVectorizedExpression is used to control whether to enable the vectorized expression evaluation.
pub const TiDBEnableVectorizedExpression: &str = "tidb_enable_vectorized_expression";

// TiDBOptJoinReorderThreshold defines the threshold less than which
// we'll choose a rather time-consuming algorithm to calculate the join order.
pub const TiDBOptJoinReorderThreshold: &str = "tidb_opt_join_reorder_threshold";

// TiDBOptEnableAdvancedJoinReorder controls whether to use the advanced join reorder framework.
pub const TiDBOptEnableAdvancedJoinReorder: &str = "tidb_opt_enable_advanced_join_reorder";

// TiDBOptJoinReorderThroughProj enables join reorder to look through projection operators
// when extracting join groups. This allows join reorder to work with derived columns from CTEs,
// views, or subqueries that have expression computations in their SELECT list.
pub const TiDBOptJoinReorderThroughProj: &str = "tidb_opt_join_reorder_through_proj";

// TiDBOptJoinReorderThroughSel enables pushing selection conditions down to
// reordered join trees when applicable.
pub const TiDBOptJoinReorderThroughSel: &str = "tidb_opt_join_reorder_through_sel";

// TiDBSlowQueryFile indicates which slow query log file for SLOW_QUERY table to parse.
pub const TiDBSlowQueryFile: &str = "tidb_slow_query_file";

// TiDBEnableFastAnalyze indicates to use fast analyze.
// Deprecated: This variable is deprecated, please do not use this variable.
pub const TiDBEnableFastAnalyze: &str = "tidb_enable_fast_analyze";

// TiDBExpensiveQueryTimeThreshold indicates the time threshold of expensive query.
pub const TiDBExpensiveQueryTimeThreshold: &str = "tidb_expensive_query_time_threshold";

// TiDBExpensiveTxnTimeThreshold indicates the time threshold of expensive transaction.
pub const TiDBExpensiveTxnTimeThreshold: &str = "tidb_expensive_txn_time_threshold";

// TiDBEnableIndexMerge indicates to generate IndexMergePath.
pub const TiDBEnableIndexMerge: &str = "tidb_enable_index_merge";

// TiDBEnableNoBackslashEscapesInLike controls whether NO_BACKSLASH_ESCAPES affects LIKE default escape.
pub const TiDBEnableNoBackslashEscapesInLike: &str = "tidb_enable_no_backslash_escapes_in_like";

// TiDBEnableNoopFuncs set true will enable using fake funcs(like get_lock release_lock)
pub const TiDBEnableNoopFuncs: &str = "tidb_enable_noop_functions";

// TiDBEnableStmtSummary indicates whether the statement summary is enabled.
pub const TiDBEnableStmtSummary: &str = "tidb_enable_stmt_summary";

// TiDBStmtSummaryInternalQuery indicates whether the statement summary contain internal query.
pub const TiDBStmtSummaryInternalQuery: &str = "tidb_stmt_summary_internal_query";

// TiDBStmtSummaryRefreshInterval indicates the refresh interval in seconds for each statement summary.
pub const TiDBStmtSummaryRefreshInterval: &str = "tidb_stmt_summary_refresh_interval";

// TiDBStmtSummaryHistorySize indicates the history size of each statement summary.
pub const TiDBStmtSummaryHistorySize: &str = "tidb_stmt_summary_history_size";

// TiDBStmtSummaryMaxStmtCount indicates the max number of statements kept in memory.
pub const TiDBStmtSummaryMaxStmtCount: &str = "tidb_stmt_summary_max_stmt_count";

// TiDBStmtSummaryMaxSQLLength indicates the max length of displayed normalized sql and sample sql.
pub const TiDBStmtSummaryMaxSQLLength: &str = "tidb_stmt_summary_max_sql_length";

// TiDBStmtSummaryPersistEvicted controls whether per-record LRU evictions
// in the v2 (persistent) statement summary are persisted to the stmt log.
// Off by default because it adds log volume proportional to eviction rate.
pub const TiDBStmtSummaryPersistEvicted: &str = "tidb_stmt_summary_persist_evicted";

// TiDBStmtSummaryGroupByUser, when enabled, adds the executing user to the
// statement summary grouping key so the same digest run by different users
// produces separate rows. Off by default to avoid cardinality growth.
pub const TiDBStmtSummaryGroupByUser: &str = "tidb_stmt_summary_group_by_user";

// TiDBIgnoreInlistPlanDigest enables TiDB to generate the same plan digest with SQL using different in-list arguments.
pub const TiDBIgnoreInlistPlanDigest: &str = "tidb_ignore_inlist_plan_digest";

// TiDBCapturePlanBaseline indicates whether the capture of plan baselines is enabled.
pub const TiDBCapturePlanBaseline: &str = "tidb_capture_plan_baselines";

// TiDBUsePlanBaselines indicates whether the use of plan baselines is enabled.
pub const TiDBUsePlanBaselines: &str = "tidb_use_plan_baselines";

// TiDBEvolvePlanBaselines indicates whether the evolution of plan baselines is enabled.
pub const TiDBEvolvePlanBaselines: &str = "tidb_evolve_plan_baselines";

// TiDBOptEnableFuzzyBinding indicates whether to enable the universal binding.
pub const TiDBOptEnableFuzzyBinding: &str = "tidb_opt_enable_fuzzy_binding";

// TiDBEnableExtendedStats is kept only for system variable compatibility. Extended statistics support has been removed.
pub const TiDBEnableExtendedStats: &str = "tidb_enable_extended_stats";

// TiDBIsolationReadEngines indicates the tidb only read from the stores whose engine type is involved in IsolationReadEngines.
// Now, only support TiKV and TiFlash.
pub const TiDBIsolationReadEngines: &str = "tidb_isolation_read_engines";

// TiDBStoreLimit indicates the limit of sending request to a store, 0 means without limit.
pub const TiDBStoreLimit: &str = "tidb_store_limit";

// TiDBMetricSchemaStep indicates the step when query metric schema.
pub const TiDBMetricSchemaStep: &str = "tidb_metric_query_step";

// TiDBCDCWriteSource indicates the following data is written by TiCDC if it is not 0.
pub const TiDBCDCWriteSource: &str = "tidb_cdc_write_source";

// TiDBMetricSchemaRangeDuration indicates the range duration when query metric schema.
pub const TiDBMetricSchemaRangeDuration: &str = "tidb_metric_query_range_duration";

// TiDBEnableCollectExecutionInfo indicates that whether execution info is collected.
pub const TiDBEnableCollectExecutionInfo: &str = "tidb_enable_collect_execution_info";

// TiDBExecutorConcurrency is used for controlling the concurrency of all types of executors.
pub const TiDBExecutorConcurrency: &str = "tidb_executor_concurrency";

// TiDBEnableClusteredIndex indicates if clustered index feature is enabled.
pub const TiDBEnableClusteredIndex: &str = "tidb_enable_clustered_index";

// TiDBEnableGlobalIndex means if we could create an global index on a partition table or not.
// Deprecated, will always be ON
pub const TiDBEnableGlobalIndex: &str = "tidb_enable_global_index";

// TiDBPartitionPruneMode indicates the partition prune mode used.
pub const TiDBPartitionPruneMode: &str = "tidb_partition_prune_mode";

// TiDBRedactLog indicates that whether redact log.
pub const TiDBRedactLog: &str = "tidb_redact_log";

// TiDBRestrictedReadOnly is meant for the cloud admin to toggle the cluster read only
pub const TiDBRestrictedReadOnly: &str = "tidb_restricted_read_only";

// TiDBSuperReadOnly is tidb's variant of mysql's super_read_only, which has some differences from mysql's super_read_only.
pub const TiDBSuperReadOnly: &str = "tidb_super_read_only";

// TiDBShardAllocateStep indicates the max size of continuous rowid shard in one transaction.
pub const TiDBShardAllocateStep: &str = "tidb_shard_allocate_step";
// TiDBEnableTelemetry indicates that whether usage data report to PingCAP is enabled.
// Deprecated: it is 'off' always since Telemetry has been removed from TiDB.
pub const TiDBEnableTelemetry: &str = "tidb_enable_telemetry";

// TiDBMemoryUsageAlarmRatio indicates the alarm threshold when memory usage of the tidb-server exceeds.
pub const TiDBMemoryUsageAlarmRatio: &str = "tidb_memory_usage_alarm_ratio";

// TiDBMemoryUsageAlarmKeepRecordNum indicates the number of saved alarm files.
pub const TiDBMemoryUsageAlarmKeepRecordNum: &str = "tidb_memory_usage_alarm_keep_record_num";

// TiDBEnableRateLimitAction indicates whether enabled ratelimit action
pub const TiDBEnableRateLimitAction: &str = "tidb_enable_rate_limit_action";

// TiDBEnableAsyncCommit indicates whether to enable the async commit feature.
pub const TiDBEnableAsyncCommit: &str = "tidb_enable_async_commit";

// TiDBEnable1PC indicates whether to enable the one-phase commit feature.
pub const TiDBEnable1PC: &str = "tidb_enable_1pc";

// TiDBGuaranteeLinearizability indicates whether to guarantee linearizability.
pub const TiDBGuaranteeLinearizability: &str = "tidb_guarantee_linearizability";

// TiDBAnalyzeVersion indicates how tidb collects the analyzed statistics and how use to it.
pub const TiDBAnalyzeVersion: &str = "tidb_analyze_version";

// TiDBAutoAnalyzePartitionBatchSize indicates the batch size for partition tables for auto analyze in dynamic mode
// Deprecated: This variable is deprecated, please do not use this variable.
pub const TiDBAutoAnalyzePartitionBatchSize: &str = "tidb_auto_analyze_partition_batch_size";

// TiDBEnableIndexMergeJoin indicates whether to enable index merge join.
pub const TiDBEnableIndexMergeJoin: &str = "tidb_enable_index_merge_join";

// TiDBTrackAggregateMemoryUsage indicates whether track the memory usage of aggregate function.
pub const TiDBTrackAggregateMemoryUsage: &str = "tidb_track_aggregate_memory_usage";

// TiDBEnableExchangePartition indicates whether to enable exchange partition.
pub const TiDBEnableExchangePartition: &str = "tidb_enable_exchange_partition";

// TiDBAllowFallbackToTiKV indicates the engine types whose unavailability triggers fallback to TiKV.
// Now we only support TiFlash.
pub const TiDBAllowFallbackToTiKV: &str = "tidb_allow_fallback_to_tikv";

// TiDBEnableTopSQL indicates whether the top SQL is enabled.
pub const TiDBEnableTopSQL: &str = "tidb_enable_top_sql";

// TiDBSourceID indicates the source ID of the TiDB server.
pub const TiDBSourceID: &str = "tidb_source_id";

// TiDBTopSQLMaxTimeSeriesCount indicates the max number of statements been collected in each time series.
pub const TiDBTopSQLMaxTimeSeriesCount: &str = "tidb_top_sql_max_time_series_count";

// TiDBTopSQLMaxMetaCount indicates the max capacity of the collect meta per second.
pub const TiDBTopSQLMaxMetaCount: &str = "tidb_top_sql_max_meta_count";

// TiDBEnableLocalTxn indicates whether to enable Local Txn.
pub const TiDBEnableLocalTxn: &str = "tidb_enable_local_txn";

// TiDBEnableMDL indicates whether to enable MDL.
pub const TiDBEnableMDL: &str = "tidb_enable_metadata_lock";

// TiDBTSOClientBatchMaxWaitTime indicates the max value of the TSO Batch Wait interval time of PD client.
pub const TiDBTSOClientBatchMaxWaitTime: &str = "tidb_tso_client_batch_max_wait_time";

// TiDBTxnCommitBatchSize is used to control the batch size of transaction commit related requests sent by TiDB to TiKV.
// If a single transaction has a large amount of writes, you can increase the batch size to improve the batch effect,
// setting too large will exceed TiKV's raft-entry-max-size limit and cause commit failure.
pub const TiDBTxnCommitBatchSize: &str = "tidb_txn_commit_batch_size";

// TiDBEnableTSOFollowerProxy indicates whether to enable the TSO Follower Proxy feature of PD client.
pub const TiDBEnableTSOFollowerProxy: &str = "tidb_enable_tso_follower_proxy";

// PDEnableFollowerHandleRegion indicates whether to enable the PD Follower handle region API.
// TODO: deprecated this variable to use a format like `tidb_enable_pd_follower_handle_region`.
pub const PDEnableFollowerHandleRegion: &str = "pd_enable_follower_handle_region";

// TiDBEnableBatchQueryRegion indicates whether to enable the batch query region feature.
pub const TiDBEnableBatchQueryRegion: &str = "tidb_enable_batch_query_region";

// TiDBEnableOrderedResultMode indicates if stabilize query results.
pub const TiDBEnableOrderedResultMode: &str = "tidb_enable_ordered_result_mode";

// TiDBRemoveOrderbyInSubquery indicates whether to remove ORDER BY in subquery.
pub const TiDBRemoveOrderbyInSubquery: &str = "tidb_remove_orderby_in_subquery";

// TiDBEnablePseudoForOutdatedStats indicates whether use pseudo for outdated stats
pub const TiDBEnablePseudoForOutdatedStats: &str = "tidb_enable_pseudo_for_outdated_stats";

// TiDBRegardNULLAsPoint indicates whether regard NULL as point when optimizing
pub const TiDBRegardNULLAsPoint: &str = "tidb_regard_null_as_point";

// TiDBTmpTableMaxSize indicates the max memory size of temporary tables.
pub const TiDBTmpTableMaxSize: &str = "tidb_tmp_table_max_size";

// TiDBEnableLegacyInstanceScope indicates if instance scope can be set with SET SESSION.
pub const TiDBEnableLegacyInstanceScope: &str = "tidb_enable_legacy_instance_scope";

// TiDBTableCacheLease indicates the read lock lease of a cached table.
pub const TiDBTableCacheLease: &str = "tidb_table_cache_lease";

// TiDBStatsLoadSyncWait indicates the time sql execution will sync-wait for stats load.
pub const TiDBStatsLoadSyncWait: &str = "tidb_stats_load_sync_wait";

// TiDBEnableMutationChecker indicates whether to check data consistency for mutations
pub const TiDBEnableMutationChecker: &str = "tidb_enable_mutation_checker";
// TiDBTxnAssertionLevel indicates how strict the assertion will be, which helps to detect and preventing data &
// index inconsistency problems.
pub const TiDBTxnAssertionLevel: &str = "tidb_txn_assertion_level";

// TiDBIgnorePreparedCacheCloseStmt indicates whether to ignore close-stmt commands for prepared statements.
pub const TiDBIgnorePreparedCacheCloseStmt: &str = "tidb_ignore_prepared_cache_close_stmt";

// TiDBEnableNewCostInterface is a internal switch to indicates whether to use the new cost calculation interface.
pub const TiDBEnableNewCostInterface: &str = "tidb_enable_new_cost_interface";

// TiDBCostModelVersion is a internal switch to indicates the cost model version.
pub const TiDBCostModelVersion: &str = "tidb_cost_model_version";

// TiDBIndexJoinDoubleReadPenaltyCostRate indicates whether to add some penalty cost to IndexJoin and how much of it.
// IndexJoin can cause plenty of extra double read tasks, which consume lots of resources and take a long time.
// Since the number of double read tasks is hard to estimated accurately, we leave this variable to let us can adjust this
// part of cost manually.
pub const TiDBIndexJoinDoubleReadPenaltyCostRate: &str =
    "tidb_index_join_double_read_penalty_cost_rate";

// TiDBBatchPendingTiFlashCount indicates the maximum count of non-available TiFlash tables.
pub const TiDBBatchPendingTiFlashCount: &str = "tidb_batch_pending_tiflash_count";

// TiDBQueryLogMaxLen is used to set the max length of the query in the log.
pub const TiDBQueryLogMaxLen: &str = "tidb_query_log_max_len";

// TiDBEnableNoopVariables is used to indicate if noops appear in SHOW [GLOBAL] VARIABLES
pub const TiDBEnableNoopVariables: &str = "tidb_enable_noop_variables";

// TiDBNonTransactionalIgnoreError is used to ignore error in non-transactional DMLs.
// When set to false, a non-transactional DML returns when it meets the first error.
// When set to true, a non-transactional DML finishes all batches even if errors are met in some batches.
pub const TiDBNonTransactionalIgnoreError: &str = "tidb_nontransactional_ignore_error";

// Fine grained shuffle is disabled when TiFlashFineGrainedShuffleStreamCount is zero.
pub const TiFlashFineGrainedShuffleStreamCount: &str = "tiflash_fine_grained_shuffle_stream_count";
pub const TiFlashFineGrainedShuffleBatchSize: &str = "tiflash_fine_grained_shuffle_batch_size";

// TiDBSimplifiedMetrics controls whether to unregister some unused metrics.
pub const TiDBSimplifiedMetrics: &str = "tidb_simplified_metrics";

// TiDBMemoryDebugModeMinHeapInUse is used to set tidb memory debug mode trigger threshold.
// When set to 0, the function is disabled.
// When set to a negative integer, use memory debug mode to detect the issue of frequent allocation and release of memory.
// We do not actively trigger gc, and check whether the `tracker memory * (1+bias ratio) > heap in use` each 5s.
// When set to a positive integer, use memory debug mode to detect the issue of memory tracking inaccurate.
// We trigger runtime.GC() each 5s, and check whether the `tracker memory * (1+bias ratio) > heap in use`.
pub const TiDBMemoryDebugModeMinHeapInUse: &str = "tidb_memory_debug_mode_min_heap_inuse";
// TiDBMemoryDebugModeAlarmRatio is used set tidb memory debug mode bias ratio. Treat memory bias less than this ratio as noise.
pub const TiDBMemoryDebugModeAlarmRatio: &str = "tidb_memory_debug_mode_alarm_ratio";

// TiDBEnableAnalyzeSnapshot indicates whether to read data on snapshot when collecting statistics.
// When set to false, ANALYZE reads the latest data.
// When set to true, ANALYZE reads data on the snapshot at the beginning of ANALYZE.
pub const TiDBEnableAnalyzeSnapshot: &str = "tidb_enable_analyze_snapshot";

// TiDBDefaultStrMatchSelectivity controls some special cardinality estimation strategy for string match functions (like and regexp).
// When set to 0, Selectivity() will try to evaluate those functions with TopN and NULL in the stats to estimate,
// and the default selectivity and the selectivity for the histogram part will be 0.1.
// When set to (0, 1], Selectivity() will use the value of this variable as the default selectivity of those
// functions instead of the selectionFactor (0.8).
pub const TiDBDefaultStrMatchSelectivity: &str = "tidb_default_string_match_selectivity";

// TiDBEnablePrepPlanCache indicates whether to enable prepared plan cache
pub const TiDBEnablePrepPlanCache: &str = "tidb_enable_prepared_plan_cache";
// TiDBPrepPlanCacheSize indicates the number of cached statements.
// This variable is deprecated, use tidb_session_plan_cache_size instead.
pub const TiDBPrepPlanCacheSize: &str = "tidb_prepared_plan_cache_size";
// TiDBEnablePrepPlanCacheMemoryMonitor indicates whether to enable prepared plan cache monitor
pub const TiDBEnablePrepPlanCacheMemoryMonitor: &str =
    "tidb_enable_prepared_plan_cache_memory_monitor";

// TiDBEnableNonPreparedPlanCache indicates whether to enable non-prepared plan cache.
pub const TiDBEnableNonPreparedPlanCache: &str = "tidb_enable_non_prepared_plan_cache";
// TiDBEnableNonPreparedPlanCacheForDML indicates whether to enable non-prepared plan cache for DML statements.
pub const TiDBEnableNonPreparedPlanCacheForDML: &str =
    "tidb_enable_non_prepared_plan_cache_for_dml";
// TiDBPlanCacheStrategy controls plan cache strategy.
pub const TiDBPlanCacheStrategy: &str = "tidb_plan_cache_strategy";
// TiDBPlanCacheStrategyAll is one strategy value for TiDBPlanCacheStrategy.
pub const TiDBPlanCacheStrategyAll: &str = "all";
// TiDBPlanCacheStrategyHintOnly is one strategy value for TiDBPlanCacheStrategy.
pub const TiDBPlanCacheStrategyHintOnly: &str = "hint_only";
// TiDBNonPreparedPlanCacheSize controls the size of non-prepared plan cache.
// This variable is deprecated, use tidb_session_plan_cache_size instead.
pub const TiDBNonPreparedPlanCacheSize: &str = "tidb_non_prepared_plan_cache_size";
// TiDBPlanCacheMaxPlanSize controls the maximum size of a plan that can be cached.
pub const TiDBPlanCacheMaxPlanSize: &str = "tidb_plan_cache_max_plan_size";
// TiDBPlanCacheInvalidationOnFreshStats controls if plan cache will be invalidated automatically when
// related stats are analyzed after the plan cache is generated.
pub const TiDBPlanCacheInvalidationOnFreshStats: &str =
    "tidb_plan_cache_invalidation_on_fresh_stats";
// TiDBPlanCacheSkipStatsOnBinding controls if plan cache skips stats-version invalidation when
// a SQL binding is matched. Since a binding pins the plan via hints, stats changes cannot alter
// the chosen plan, so invalidating the cache entry on stats updates is unnecessary.
pub const TiDBPlanCacheSkipStatsOnBinding: &str = "tidb_plan_cache_skip_stats_on_binding";
// TiDBSessionPlanCacheSize controls the size of session plan cache.
pub const TiDBSessionPlanCacheSize: &str = "tidb_session_plan_cache_size";

// TiDBEnableInstancePlanCache indicates whether to enable instance plan cache.
// If this variable is false, session-level plan cache will be used.
pub const TiDBEnableInstancePlanCache: &str = "tidb_enable_instance_plan_cache";
// TiDBInstancePlanCacheReservedPercentage indicates the percentage memory to evict.
pub const TiDBInstancePlanCacheReservedPercentage: &str =
    "tidb_instance_plan_cache_reserved_percentage";
// TiDBInstancePlanCacheMaxMemSize indicates the maximum memory size of instance plan cache.
pub const TiDBInstancePlanCacheMaxMemSize: &str = "tidb_instance_plan_cache_max_size";

// TiDBConstraintCheckInPlacePessimistic controls whether to skip certain kinds of pessimistic locks.
pub const TiDBConstraintCheckInPlacePessimistic: &str =
    "tidb_constraint_check_in_place_pessimistic";

// TiDBEnableForeignKey indicates whether to enable foreign key feature.
// TODO(crazycs520): remove this after foreign key GA.
pub const TiDBEnableForeignKey: &str = "tidb_enable_foreign_key";

// TiDBForeignKeyCheckInSharedLock indicates whether to use shared lock for foreign key check.
pub const TiDBForeignKeyCheckInSharedLock: &str = "tidb_foreign_key_check_in_shared_lock";

// TiDBOptRangeMaxSize is the max memory limit for ranges. When the optimizer estimates that the memory usage of complete
// ranges would exceed the limit, it chooses less accurate ranges such as full range. 0 indicates that there is no memory
// limit for ranges.
pub const TiDBOptRangeMaxSize: &str = "tidb_opt_range_max_size";

// TiDBOptAdvancedJoinHint indicates whether the join method hint is compatible with join order hint.
pub const TiDBOptAdvancedJoinHint: &str = "tidb_opt_advanced_join_hint";
// TiDBOptUseInvisibleIndexes indicates whether to use invisible indexes.
pub const TiDBOptUseInvisibleIndexes: &str = "tidb_opt_use_invisible_indexes";
// TiDBAnalyzePartitionConcurrency is the number of concurrent workers to save statistics to the system tables.
pub const TiDBAnalyzePartitionConcurrency: &str = "tidb_analyze_partition_concurrency";
// TiDBMergePartitionStatsConcurrency is deprecated and always returns 1.
pub const TiDBMergePartitionStatsConcurrency: &str = "tidb_merge_partition_stats_concurrency";
// TiDBEnableAsyncMergeGlobalStats indicates whether to enable async merge global stats
pub const TiDBEnableAsyncMergeGlobalStats: &str = "tidb_enable_async_merge_global_stats";
// TiDBOptPrefixIndexSingleScan indicates whether to do some optimizations to avoid double scan for prefix index.
// When set to true, `col is (not) null`(`col` is index prefix column) is regarded as index filter rather than table filter.
pub const TiDBOptPrefixIndexSingleScan: &str = "tidb_opt_prefix_index_single_scan";
// TiDBOptPartialOrderedIndexForTopN indicates whether to enable partial ordered index optimization for TOPN queries.
// Examples of queries that can benefit from this optimization:
// 1. index a -> order by a, b limit
// 2. index a, prefix(b) -> order by a, b limit
pub const TiDBOptPartialOrderedIndexForTopN: &str = "tidb_opt_partial_ordered_index_for_topn";

// TiDBEnableExternalTSRead indicates whether to enable read through an external ts
pub const TiDBEnableExternalTSRead: &str = "tidb_enable_external_ts_read";

// TiDBEnablePlanReplayerCapture indicates whether to enable plan replayer capture
pub const TiDBEnablePlanReplayerCapture: &str = "tidb_enable_plan_replayer_capture";

// Retention duration for non-capture plan replayer files.
pub const TiDBPlanReplayerFileRetentionTime: &str = "tidb_plan_replayer_file_retention_time";

// TiDBEnablePlanReplayerContinuousCapture indicates whether to enable continuous capture
pub const TiDBEnablePlanReplayerContinuousCapture: &str =
    "tidb_enable_plan_replayer_continuous_capture";
// TiDBEnableReusechunk indicates whether to enable chunk alloc
pub const TiDBEnableReusechunk: &str = "tidb_enable_reuse_chunk";

// TiDBStoreBatchSize indicates the batch size of coprocessor in the same store.
pub const TiDBStoreBatchSize: &str = "tidb_store_batch_size";

// MppExchangeCompressionMode indicates the data compression method in mpp exchange operator
pub const MppExchangeCompressionMode: &str = "mpp_exchange_compression_mode";

// MppVersion indicates the mpp-version used to build mpp plan
pub const MppVersion: &str = "mpp_version";

// TiDBPessimisticTransactionFairLocking controls whether fair locking for pessimistic transaction
// is enabled.
pub const TiDBPessimisticTransactionFairLocking: &str = "tidb_pessimistic_txn_fair_locking";

// TiDBEnablePlanCacheForParamLimit controls whether prepare statement with parameterized limit can be cached
pub const TiDBEnablePlanCacheForParamLimit: &str = "tidb_enable_plan_cache_for_param_limit";

// TiDBEnableINLJoinInnerMultiPattern indicates whether enable multi pattern for inner side of inl join
pub const TiDBEnableINLJoinInnerMultiPattern: &str = "tidb_enable_inl_join_inner_multi_pattern";

// TiFlashComputeDispatchPolicy indicates how to dispatch task to tiflash_compute nodes.
pub const TiFlashComputeDispatchPolicy: &str = "tiflash_compute_dispatch_policy";

// TiDBEnablePlanCacheForSubquery controls whether prepare statement with subquery can be cached
pub const TiDBEnablePlanCacheForSubquery: &str = "tidb_enable_plan_cache_for_subquery";

// TiDBOptEnableLateMaterialization indicates whether to enable late materialization
pub const TiDBOptEnableLateMaterialization: &str = "tidb_opt_enable_late_materialization";
// TiDBLoadBasedReplicaReadThreshold is the wait duration threshold to enable replica read automatically.
pub const TiDBLoadBasedReplicaReadThreshold: &str = "tidb_load_based_replica_read_threshold";

// TiDBOptOrderingIdxSelThresh is the threshold for optimizer to consider the ordering index.
pub const TiDBOptOrderingIdxSelThresh: &str = "tidb_opt_ordering_index_selectivity_threshold";

// TiDBOptOrderingIdxSelRatio is the ratio the optimizer will assume applies when non indexed filtering rows are found
// via the ordering index.
pub const TiDBOptOrderingIdxSelRatio: &str = "tidb_opt_ordering_index_selectivity_ratio";

// TiDBOptEnableMPPSharedCTEExecution indicates whether the optimizer try to build shared CTE scan during MPP execution.
pub const TiDBOptEnableMPPSharedCTEExecution: &str = "tidb_opt_enable_mpp_shared_cte_execution";
// TiDBOptFixControl makes the user able to control some details of the optimizer behavior.
pub const TiDBOptFixControl: &str = "tidb_opt_fix_control";

// TiFlashReplicaRead is used to set the policy of TiFlash replica read when the query needs the TiFlash engine.
pub const TiFlashReplicaRead: &str = "tiflash_replica_read";

/// Cluster-level gate for adding TiFlash/columnar-storage replicas.
pub const TiDBColumnarStorageEnabled: &str = "tidb_columnar_storage_enabled";

// TiDBLockUnchangedKeys indicates whether to lock duplicate keys in INSERT IGNORE and REPLACE statements,
// or unchanged unique keys in UPDATE statements, see PR #42210 and #42713
pub const TiDBLockUnchangedKeys: &str = "tidb_lock_unchanged_keys";

// TiDBFastCheckTable enables fast check table.
pub const TiDBFastCheckTable: &str = "tidb_enable_fast_table_check";

// TiDBAnalyzeSkipColumnTypes indicates the column types whose statistics would not be collected when executing the ANALYZE command.
pub const TiDBAnalyzeSkipColumnTypes: &str = "tidb_analyze_skip_column_types";

// TiDBEnableCheckConstraint indicates whether to enable check constraint feature.
pub const TiDBEnableCheckConstraint: &str = "tidb_enable_check_constraint";

// TiDBOptEnableHashJoin indicates whether to enable hash join.
pub const TiDBOptEnableHashJoin: &str = "tidb_opt_enable_hash_join";

// TiDBHashJoinVersion indicates whether to use hash join implementation v2.
pub const TiDBHashJoinVersion: &str = "tidb_hash_join_version";

// TiDBOptIndexJoinBuild is kept for compatibility. Index join build v2 is always enabled now.
pub const TiDBOptIndexJoinBuild: &str = "tidb_opt_index_join_build_v2";

// TiDBOptObjective indicates whether the optimizer should be more stable, predictable or more aggressive.
// Please see comments of SessionVars.OptObjective for details.
pub const TiDBOptObjective: &str = "tidb_opt_objective";

// TiDBEnableParallelHashaggSpill is the name of the `tidb_enable_parallel_hashagg_spill` system variable
pub const TiDBEnableParallelHashaggSpill: &str = "tidb_enable_parallel_hashagg_spill";

// TiDBTxnEntrySizeLimit indicates the max size of a entry in membuf.
pub const TiDBTxnEntrySizeLimit: &str = "tidb_txn_entry_size_limit";

// TiDBSchemaCacheSize indicates the size of infoschema meta data which are cached in V2 implementation.
pub const TiDBSchemaCacheSize: &str = "tidb_schema_cache_size";

// DivPrecisionIncrement indicates the number of digits by which to increase the scale of the result of
// division operations performed with the / operator.
pub const DivPrecisionIncrement: &str = "div_precision_increment";

// TiDBEnableSharedLockPromotion indicates whether the `select for share` statement would be executed
// as `select for update` statements which do acquire pessimistic locks.
pub const TiDBEnableSharedLockPromotion: &str = "tidb_enable_shared_lock_promotion";

// TiDBAccelerateUserCreationUpdate decides whether tidb will load & update the whole user's data in-memory.
pub const TiDBAccelerateUserCreationUpdate: &str = "tidb_accelerate_user_creation_update";

// TiDBEnableCachePrepareStmt indicates whether to support cache prepare stmt in plan cache.
pub const TiDBEnableCachePrepareStmt: &str = "tidb_enable_cache_prepare_stmt";

// TiDBEnableTxnFile controls whether file-based transactions are enabled.
pub const TiDBEnableTxnFile: &str = "tidb_enable_txn_file";

// TiDBTxnFileMinMutationSize is the minimum mutation size for file-based transactions.
pub const TiDBTxnFileMinMutationSize: &str = "tidb_txn_file_min_mutation_size";

// TiDB vars that have only global scope
// Go const block：以下常量按原声明顺序逐项迁移。
// TiDBGCEnable turns garbage collection on or OFF
pub const TiDBGCEnable: &str = "tidb_gc_enable";
// TiDBGCRunInterval sets the interval that GC runs
pub const TiDBGCRunInterval: &str = "tidb_gc_run_interval";
// TiDBGCLifetime sets the retention window of older versions
pub const TiDBGCLifetime: &str = "tidb_gc_life_time";
// TiDBGCConcurrency sets the concurrency of garbage collection. -1 = AUTO value
pub const TiDBGCConcurrency: &str = "tidb_gc_concurrency";
// TiDBGCScanLockMode enables the green GC feature (deprecated)
pub const TiDBGCScanLockMode: &str = "tidb_gc_scan_lock_mode";
// TiDBGCMaxWaitTime sets max time for gc advances the safepoint delayed by active transactions
pub const TiDBGCMaxWaitTime: &str = "tidb_gc_max_wait_time";
// TiDBEnableEnhancedSecurity restricts SUPER users from certain operations.
pub const TiDBEnableEnhancedSecurity: &str = "tidb_enable_enhanced_security";
// TiDBEnableHistoricalStats enables the historical statistics feature (default off)
pub const TiDBEnableHistoricalStats: &str = "tidb_enable_historical_stats";
// TiDBPersistAnalyzeOptions persists analyze options for later analyze and auto-analyze
pub const TiDBPersistAnalyzeOptions: &str = "tidb_persist_analyze_options";
// TiDBEnableColumnTracking enables collecting predicate columns.
// DEPRECATED: This variable is deprecated, please do not use this variable.
pub const TiDBEnableColumnTracking: &str = "tidb_enable_column_tracking";
// TiDBAnalyzeColumnOptions specifies the default column selection strategy for both manual and automatic analyze operations.
// It accepts two values:
// `PREDICATE`: Analyze only the columns that are used in the predicates of the query.
// `ALL`: Analyze all columns in the table.
pub const TiDBAnalyzeColumnOptions: &str = "tidb_analyze_column_options";
pub const TiDBAnalyzeDefaultNumBuckets: &str = "tidb_analyze_default_num_buckets";
pub const TiDBAnalyzeDefaultNumTopN: &str = "tidb_analyze_default_num_topn";
// TiDBDisableColumnTrackingTime records the last time TiDBEnableColumnTracking is set off.
// It is used to invalidate the collected predicate columns after turning off TiDBEnableColumnTracking, which avoids physical deletion.
// It doesn't have cache in memory, and we directly get/set the variable value from/to mysql.tidb.
// DEPRECATED: This variable is deprecated, please do not use this variable.
pub const TiDBDisableColumnTrackingTime: &str = "tidb_disable_column_tracking_time";
// TiDBStatsLoadPseudoTimeout indicates whether to fallback to pseudo stats after load timeout.
pub const TiDBStatsLoadPseudoTimeout: &str = "tidb_stats_load_pseudo_timeout";
// TiDBMemQuotaBindingCache indicates the memory quota for the bind cache.
pub const TiDBMemQuotaBindingCache: &str = "tidb_mem_quota_binding_cache";
// TiDBRCReadCheckTS indicates the tso optimization for read-consistency read is enabled.
pub const TiDBRCReadCheckTS: &str = "tidb_rc_read_check_ts";
// TiDBRCWriteCheckTs indicates whether some special write statements don't get latest tso from PD at RC
pub const TiDBRCWriteCheckTs: &str = "tidb_rc_write_check_ts";
// TiDBCommitterConcurrency controls the number of running concurrent requests in the commit phase.
pub const TiDBCommitterConcurrency: &str = "tidb_committer_concurrency";
// TiDBPipelinedDmlResourcePolicy controls the number of running concurrent requests in the
// pipelined flush action.
pub const TiDBPipelinedDmlResourcePolicy: &str = "tidb_pipelined_dml_resource_policy";
// TiDBEnableBatchDML enables batch dml.
pub const TiDBEnableBatchDML: &str = "tidb_enable_batch_dml";
// TiDBStatsCacheMemQuota records stats cache quota.
pub const TiDBStatsCacheMemQuota: &str = "tidb_stats_cache_mem_quota";
// TiDBMemQuotaAnalyze indicates the memory quota for all analyze jobs.
pub const TiDBMemQuotaAnalyze: &str = "tidb_mem_quota_analyze";
// TiDBEnableAutoAnalyze determines whether TiDB executes automatic analysis.
// In test, we disable it by default. See GlobalSystemVariableInitialValue for details.
pub const TiDBEnableAutoAnalyze: &str = "tidb_enable_auto_analyze";
// TiDBEnableAutoAnalyzePriorityQueue determines whether TiDB executes automatic analysis with priority queue.
// DEPRECATED: This variable is deprecated, please do not use this variable.
pub const TiDBEnableAutoAnalyzePriorityQueue: &str = "tidb_enable_auto_analyze_priority_queue";
// TiDBMemOOMAction indicates what operation TiDB perform when a single SQL statement exceeds
// the memory quota specified by tidb_mem_quota_query and cannot be spilled to disk.
pub const TiDBMemOOMAction: &str = "tidb_mem_oom_action";
// TiDBPrepPlanCacheMemoryGuardRatio is used to prevent [performance.max-memory] from being exceeded
pub const TiDBPrepPlanCacheMemoryGuardRatio: &str = "tidb_prepared_plan_cache_memory_guard_ratio";
// TiDBMaxAutoAnalyzeTime is the max time that auto analyze can run. If auto analyze runs longer than the value, it
// will be killed. 0 indicates that there is no time limit.
pub const TiDBMaxAutoAnalyzeTime: &str = "tidb_max_auto_analyze_time";
// TiDBAutoAnalyzeConcurrency is the concurrency of the auto analyze
pub const TiDBAutoAnalyzeConcurrency: &str = "tidb_auto_analyze_concurrency";
// TiDBEnableDistTask indicates whether to enable the distributed execute background tasks(For example DDL, Import etc).
pub const TiDBEnableDistTask: &str = "tidb_enable_dist_task";
// TiDBMaxDistTaskNodes indicates the max node count that could be used by distributed execution framework.
pub const TiDBMaxDistTaskNodes: &str = "tidb_max_dist_task_nodes";
// TiDBEnableFastCreateTable indicates whether to enable the fast create table feature.
pub const TiDBEnableFastCreateTable: &str = "tidb_enable_fast_create_table";
// TiDBGenerateBinaryPlan indicates whether binary plan should be generated in slow log and statements summary.
pub const TiDBGenerateBinaryPlan: &str = "tidb_generate_binary_plan";
// TiDBEnableDDLAnalyze indicates whether ddl(create/reorg index) is with embedded index analyze.
pub const TiDBEnableDDLAnalyze: &str = "tidb_stats_update_during_ddl";
// TiDBEnableGCAwareMemoryTrack indicates whether to turn-on GC-aware memory track.
pub const TiDBEnableGCAwareMemoryTrack: &str = "tidb_enable_gc_aware_memory_track";
// TiDBEnableTmpStorageOnOOM controls whether to enable the temporary storage for some operators
// when a single SQL statement exceeds the memory quota specified by the memory quota.
pub const TiDBEnableTmpStorageOnOOM: &str = "tidb_enable_tmp_storage_on_oom";
// TiDBDDLEnableFastReorg indicates whether to use lighting backfill process for adding index.
pub const TiDBDDLEnableFastReorg: &str = "tidb_ddl_enable_fast_reorg";
// TiDBDDLDiskQuota used to set disk quota for lightning add index.
pub const TiDBDDLDiskQuota: &str = "tidb_ddl_disk_quota";
// TiDBCloudStorageURI used to set a cloud storage uri for ddl add index and import into.
pub const TiDBCloudStorageURI: &str = "tidb_cloud_storage_uri";
// TiDBAutoBuildStatsConcurrency is the number of concurrent workers to automatically analyze tables or partitions.
// It is very similar to the `tidb_build_stats_concurrency` variable, but it is used for the auto analyze feature.
pub const TiDBAutoBuildStatsConcurrency: &str = "tidb_auto_build_stats_concurrency";
// TiDBSysProcScanConcurrency is used to set the scan concurrency of for backend system processes, like auto-analyze.
// For now, it controls the number of concurrent workers to scan regions to collect statistics (FMSketch, Samples).
pub const TiDBSysProcScanConcurrency: &str = "tidb_sysproc_scan_concurrency";
// TiDBServerMemoryLimit indicates the memory limit of the tidb-server instance.
pub const TiDBServerMemoryLimit: &str = "tidb_server_memory_limit";
// TiDBServerMemoryLimitSessMinSize indicates the minimal memory used of a session, that becomes a candidate for session kill.
pub const TiDBServerMemoryLimitSessMinSize: &str = "tidb_server_memory_limit_sess_min_size";
// TiDBServerMemoryLimitGCTrigger indicates the gc percentage of the TiDBServerMemoryLimit.
pub const TiDBServerMemoryLimitGCTrigger: &str = "tidb_server_memory_limit_gc_trigger";
// TiDBMemArbitratorSoftLimit indicates the soft memory quota limit of the global memory arbitrator
pub const TiDBMemArbitratorSoftLimit: &str = "tidb_mem_arbitrator_soft_limit";
// TiDBMemArbitratorMode indicates work modes of the global memory arbitrator
pub const TiDBMemArbitratorMode: &str = "tidb_mem_arbitrator_mode";
// TiDBMemArbitratorQueryReserved indicates the memory quota query needs to subscribe from the global memory arbitrator before execution
pub const TiDBMemArbitratorQueryReserved: &str = "tidb_mem_arbitrator_query_reserved";
// TiDBMemArbitratorWaitAverse indicates whether the query is wait averse
pub const TiDBMemArbitratorWaitAverse: &str = "tidb_mem_arbitrator_wait_averse";
// TiDBEnableGOGCTuner is to enable GOGC tuner. it can tuner GOGC
pub const TiDBEnableGOGCTuner: &str = "tidb_enable_gogc_tuner";
// TiDBGOGCTunerThreshold is to control the threshold of GOGC tuner.
pub const TiDBGOGCTunerThreshold: &str = "tidb_gogc_tuner_threshold";
// TiDBGOGCTunerMaxValue is the max value of GOGC that GOGC tuner can change to.
pub const TiDBGOGCTunerMaxValue: &str = "tidb_gogc_tuner_max_value";
// TiDBGOGCTunerMinValue is the min value of GOGC that GOGC tuner can change to.
pub const TiDBGOGCTunerMinValue: &str = "tidb_gogc_tuner_min_value";
// TiDBExternalTS is the ts to read through when the `TiDBEnableExternalTsRead` is on
pub const TiDBExternalTS: &str = "tidb_external_ts";
// TiDBTTLJobEnable is used to enable/disable scheduling ttl job
pub const TiDBTTLJobEnable: &str = "tidb_ttl_job_enable";
// TiDBTTLScanBatchSize is used to control the batch size in the SELECT statement for TTL jobs
pub const TiDBTTLScanBatchSize: &str = "tidb_ttl_scan_batch_size";
// TiDBTTLDeleteBatchSize is used to control the batch size in the DELETE statement for TTL jobs
pub const TiDBTTLDeleteBatchSize: &str = "tidb_ttl_delete_batch_size";
// TiDBTTLDeleteRateLimit is used to control the delete rate limit for TTL jobs in each node
pub const TiDBTTLDeleteRateLimit: &str = "tidb_ttl_delete_rate_limit";
// TiDBTTLJobScheduleWindowStartTime is used to restrict the start time of the time window of scheduling the ttl jobs.
pub const TiDBTTLJobScheduleWindowStartTime: &str = "tidb_ttl_job_schedule_window_start_time";
// TiDBTTLJobScheduleWindowEndTime is used to restrict the end time of the time window of scheduling the ttl jobs.
pub const TiDBTTLJobScheduleWindowEndTime: &str = "tidb_ttl_job_schedule_window_end_time";
// TiDBTTLScanWorkerCount indicates the count of the scan workers in each TiDB node
pub const TiDBTTLScanWorkerCount: &str = "tidb_ttl_scan_worker_count";
// TiDBTTLDeleteWorkerCount indicates the count of the delete workers in each TiDB node
pub const TiDBTTLDeleteWorkerCount: &str = "tidb_ttl_delete_worker_count";
// PasswordReuseHistory limit a few passwords to reuse.
pub const PasswordReuseHistory: &str = "password_history";
// PasswordReuseTime limit how long passwords can be reused.
pub const PasswordReuseTime: &str = "password_reuse_interval";
// TiDBHistoricalStatsDuration indicates the duration to remain tidb historical stats
pub const TiDBHistoricalStatsDuration: &str = "tidb_historical_stats_duration";
// TiDBEnableHistoricalStatsForCapture indicates whether use historical stats in plan replayer capture
pub const TiDBEnableHistoricalStatsForCapture: &str = "tidb_enable_historical_stats_for_capture";
// TiDBEnableResourceControl indicates whether resource control feature is enabled
pub const TiDBEnableResourceControl: &str = "tidb_enable_resource_control";
// TiDBResourceControlStrictMode indicates whether resource control strict mode is enabled.
// When strict mode is enabled, user need certain privilege to change session or statement resource group.
pub const TiDBResourceControlStrictMode: &str = "tidb_resource_control_strict_mode";
// TiDBStmtSummaryEnablePersistent indicates whether to enable file persistence for stmtsummary.
pub const TiDBStmtSummaryEnablePersistent: &str = "tidb_stmt_summary_enable_persistent";
// TiDBStmtSummaryFilename indicates the file name written by stmtsummary.
pub const TiDBStmtSummaryFilename: &str = "tidb_stmt_summary_filename";
// TiDBStmtSummaryFileMaxDays indicates how many days the files written by stmtsummary will be kept.
pub const TiDBStmtSummaryFileMaxDays: &str = "tidb_stmt_summary_file_max_days";
// TiDBStmtSummaryFileMaxSize indicates the maximum size (in mb) of a single file written by stmtsummary.
pub const TiDBStmtSummaryFileMaxSize: &str = "tidb_stmt_summary_file_max_size";
// TiDBStmtSummaryFileMaxBackups indicates the maximum number of files written by stmtsummary.
pub const TiDBStmtSummaryFileMaxBackups: &str = "tidb_stmt_summary_file_max_backups";
// TiDBTTLRunningTasks limits the count of running ttl tasks. Default to 0, means 3 times the count of TiKV (or no
// limitation, if the storage is not TiKV).
pub const TiDBTTLRunningTasks: &str = "tidb_ttl_running_tasks";
// AuthenticationLDAPSASLAuthMethodName defines the authentication method used by LDAP SASL authentication plugin
pub const AuthenticationLDAPSASLAuthMethodName: &str = "authentication_ldap_sasl_auth_method_name";
// AuthenticationLDAPSASLCAPath defines the ca certificate to verify LDAP connection in LDAP SASL authentication plugin
pub const AuthenticationLDAPSASLCAPath: &str = "authentication_ldap_sasl_ca_path";
// AuthenticationLDAPSASLTLS defines whether to use TLS connection in LDAP SASL authentication plugin
pub const AuthenticationLDAPSASLTLS: &str = "authentication_ldap_sasl_tls";
// AuthenticationLDAPSASLServerHost defines the server host of LDAP server for LDAP SASL authentication plugin
pub const AuthenticationLDAPSASLServerHost: &str = "authentication_ldap_sasl_server_host";
// AuthenticationLDAPSASLServerPort defines the port of LDAP server for LDAP SASL authentication plugin
pub const AuthenticationLDAPSASLServerPort: &str = "authentication_ldap_sasl_server_port";
// AuthenticationLDAPSASLReferral defines whether to enable LDAP referral for LDAP SASL authentication plugin
pub const AuthenticationLDAPSASLReferral: &str = "authentication_ldap_sasl_referral";
// AuthenticationLDAPSASLUserSearchAttr defines the attribute of username in LDAP server
pub const AuthenticationLDAPSASLUserSearchAttr: &str = "authentication_ldap_sasl_user_search_attr";
// AuthenticationLDAPSASLBindBaseDN defines the `dn` to search the users in. It's used to limit the search scope of TiDB.
pub const AuthenticationLDAPSASLBindBaseDN: &str = "authentication_ldap_sasl_bind_base_dn";
// AuthenticationLDAPSASLBindRootDN defines the `dn` of the user to login the LDAP server and perform search.
pub const AuthenticationLDAPSASLBindRootDN: &str = "authentication_ldap_sasl_bind_root_dn";
// AuthenticationLDAPSASLBindRootPWD defines the password of the user to login the LDAP server and perform search.
pub const AuthenticationLDAPSASLBindRootPWD: &str = "authentication_ldap_sasl_bind_root_pwd";
// AuthenticationLDAPSASLInitPoolSize defines the init size of connection pool to LDAP server for SASL plugin.
pub const AuthenticationLDAPSASLInitPoolSize: &str = "authentication_ldap_sasl_init_pool_size";
// AuthenticationLDAPSASLMaxPoolSize defines the max size of connection pool to LDAP server for SASL plugin.
pub const AuthenticationLDAPSASLMaxPoolSize: &str = "authentication_ldap_sasl_max_pool_size";
// AuthenticationLDAPSimpleAuthMethodName defines the authentication method used by LDAP Simple authentication plugin
pub const AuthenticationLDAPSimpleAuthMethodName: &str =
    "authentication_ldap_simple_auth_method_name";
// AuthenticationLDAPSimpleCAPath defines the ca certificate to verify LDAP connection in LDAP Simple authentication plugin
pub const AuthenticationLDAPSimpleCAPath: &str = "authentication_ldap_simple_ca_path";
// AuthenticationLDAPSimpleTLS defines whether to use TLS connection in LDAP Simple authentication plugin
pub const AuthenticationLDAPSimpleTLS: &str = "authentication_ldap_simple_tls";
// AuthenticationLDAPSimpleServerHost defines the server host of LDAP server for LDAP Simple authentication plugin
pub const AuthenticationLDAPSimpleServerHost: &str = "authentication_ldap_simple_server_host";
// AuthenticationLDAPSimpleServerPort defines the port of LDAP server for LDAP Simple authentication plugin
pub const AuthenticationLDAPSimpleServerPort: &str = "authentication_ldap_simple_server_port";
// AuthenticationLDAPSimpleReferral defines whether to enable LDAP referral for LDAP Simple authentication plugin
pub const AuthenticationLDAPSimpleReferral: &str = "authentication_ldap_simple_referral";
// AuthenticationLDAPSimpleUserSearchAttr defines the attribute of username in LDAP server
pub const AuthenticationLDAPSimpleUserSearchAttr: &str =
    "authentication_ldap_simple_user_search_attr";
// AuthenticationLDAPSimpleBindBaseDN defines the `dn` to search the users in. It's used to limit the search scope of TiDB.
pub const AuthenticationLDAPSimpleBindBaseDN: &str = "authentication_ldap_simple_bind_base_dn";
// AuthenticationLDAPSimpleBindRootDN defines the `dn` of the user to login the LDAP server and perform search.
pub const AuthenticationLDAPSimpleBindRootDN: &str = "authentication_ldap_simple_bind_root_dn";
// AuthenticationLDAPSimpleBindRootPWD defines the password of the user to login the LDAP server and perform search.
pub const AuthenticationLDAPSimpleBindRootPWD: &str = "authentication_ldap_simple_bind_root_pwd";
// AuthenticationLDAPSimpleInitPoolSize defines the init size of connection pool to LDAP server for SASL plugin.
pub const AuthenticationLDAPSimpleInitPoolSize: &str = "authentication_ldap_simple_init_pool_size";
// AuthenticationLDAPSimpleMaxPoolSize defines the max size of connection pool to LDAP server for SASL plugin.
pub const AuthenticationLDAPSimpleMaxPoolSize: &str = "authentication_ldap_simple_max_pool_size";
// TiDBRuntimeFilterTypeName the value of is string, a runtime filter type list split by ",", such as: "IN,MIN_MAX"
pub const TiDBRuntimeFilterTypeName: &str = "tidb_runtime_filter_type";
// TiDBRuntimeFilterModeName the mode of runtime filter, such as "OFF", "LOCAL"
pub const TiDBRuntimeFilterModeName: &str = "tidb_runtime_filter_mode";
// TiDBSkipMissingPartitionStats controls how to handle missing partition stats when merging partition stats to global stats.
// When set to true, skip missing partition stats and continue to merge other partition stats to global stats.
// When set to false, give up merging partition stats to global stats.
pub const TiDBSkipMissingPartitionStats: &str = "tidb_skip_missing_partition_stats";
// TiDBSessionAlias indicates the alias of a session which is used for tracing.
pub const TiDBSessionAlias: &str = "tidb_session_alias";
// TiDBServiceScope indicates the role for tidb for distributed task framework.
pub const TiDBServiceScope: &str = "tidb_service_scope";
// TiDBSchemaVersionCacheLimit defines the capacity size of domain infoSchema cache.
pub const TiDBSchemaVersionCacheLimit: &str = "tidb_schema_version_cache_limit";
// TiDBEnableTiFlashPipelineMode means if we should use pipeline model to execute query or not in tiflash.
// It's deprecated and setting it will not have any effect.
pub const TiDBEnableTiFlashPipelineMode: &str = "tidb_enable_tiflash_pipeline_model";
// TiDBIdleTransactionTimeout indicates the maximum time duration a transaction could be idle, unit is second.
// Any idle transaction will be killed after being idle for `tidb_idle_transaction_timeout` seconds.
// This is similar to https://docs.percona.com/percona-server/5.7/management/innodb_kill_idle_trx.html and https://mariadb.com/kb/en/transaction-timeouts/
pub const TiDBIdleTransactionTimeout: &str = "tidb_idle_transaction_timeout";
// TiDBLowResolutionTSOUpdateInterval defines how often to refresh low resolution timestamps.
pub const TiDBLowResolutionTSOUpdateInterval: &str = "tidb_low_resolution_tso_update_interval";
// TiDBDMLType indicates the execution type of DML in TiDB.
// The value can be STANDARD, BULK.
// Currently, the BULK mode only affects auto-committed DML.
pub const TiDBDMLType: &str = "tidb_dml_type";
// TiFlashHashAggPreAggMode indicates the policy of 1st hashagg.
pub const TiFlashHashAggPreAggMode: &str = "tiflash_hashagg_preaggregation_mode";
// TiDBEnableLazyCursorFetch defines whether to enable the lazy cursor fetch. If it's `OFF`, all results of
// of a cursor will be stored in the tidb node in `EXECUTE` command.
pub const TiDBEnableLazyCursorFetch: &str = "tidb_enable_lazy_cursor_fetch";
// TiDBTSOClientRPCMode controls how the TSO client performs the TSO RPC requests. It internally controls the
// concurrency of the RPC. This variable provides an approach to tune the latency of getting timestamps from PD.
pub const TiDBTSOClientRPCMode: &str = "tidb_tso_client_rpc_mode";
// TiDBCircuitBreakerPDMetadataErrorRateThresholdRatio variable is used to set ratio of errors to trip the circuit breaker for get region calls to PD
// https://github.com/tikv/rfcs/blob/master/text/0115-circuit-breaker.md
pub const TiDBCircuitBreakerPDMetadataErrorRateThresholdRatio: &str =
    "tidb_cb_pd_metadata_error_rate_threshold_ratio";

// TiDBEnableTSValidation controls whether to enable the timestamp validation in client-go.
pub const TiDBEnableTSValidation: &str = "tidb_enable_ts_validation";

// TiDBAdvancerCheckPointLagLimit controls the maximum lag could be tolerated for the checkpoint lag.
// The log backup task will be paused if the checkpoint lag is larger than it.
pub const TiDBAdvancerCheckPointLagLimit: &str = "tidb_advancer_check_point_lag_limit";

// TiDBIndexLookUpPushDownPolicy controls the push down policy of index lookup.
pub const TiDBIndexLookUpPushDownPolicy: &str = "tidb_index_lookup_pushdown_policy";

// TiDB intentional limits, can be raised in the future.
// Go const block：以下常量按原声明顺序逐项迁移。
// MaxConfigurableConcurrency is the maximum number of "threads" (goroutines) that can be specified
// for any type of configuration item that has concurrent workers.
pub const MaxConfigurableConcurrency: i64 = 256;

// MaxShardRowIDBits is the maximum number of bits that can be used for row-id sharding.
pub const MaxShardRowIDBits: i64 = 15;

// MaxPreSplitRegions is the maximum number of regions that can be pre-split.
pub const MaxPreSplitRegions: i64 = 15;

// Pipelined-DML related constants
// Go const block：以下常量按原声明顺序逐项迁移。
// MinPipelinedDMLConcurrency is the minimum acceptable concurrency
pub const MinPipelinedDMLConcurrency: i64 = 1;
// MaxPipelinedDMLConcurrency is the maximum acceptable concurrency
pub const MaxPipelinedDMLConcurrency: i64 = 8192;

// DefaultFlushConcurrency is the default flush concurrency
pub const DefaultFlushConcurrency: i64 = 128;
// DefaultResolveConcurrency is the default resolve_lock concurrency
pub const DefaultResolveConcurrency: i64 = 8;

// ConservativeFlushConcurrency is the flush concurrency in conservative mode
pub const ConservativeFlushConcurrency: i64 = 2;
// ConservativeResolveConcurrency is the resolve_lock concurrency in conservative mode
pub const ConservativeResolveConcurrency: i64 = 2;

// —— TiDB 系统变量默认值（Def*）——
// 与上方变量名常量一一对应，供 SysVar 注册与全局 Atomic 初始化。
// Default TiDB system variable values.
// Go const block：以下常量按原声明顺序逐项迁移。
pub const DefHostname: &str = "localhost";
pub const DefIndexLookupConcurrency: i64 = ConcurrencyUnset;
pub const DefIndexLookupJoinConcurrency: i64 = ConcurrencyUnset;
pub const DefIndexSerialScanConcurrency: i64 = 1;
pub const DefIndexJoinBatchSize: i64 = 25000;
pub const DefIndexLookupSize: i64 = 20000;
pub const DefDistSQLScanConcurrency: i64 = 15;
pub const DefTiDBQueryCopStoreLimit: i64 = 15;
pub const DefAnalyzeDistSQLScanConcurrency: i64 = 4;
pub const DefBuildStatsConcurrency: i64 = 2;
pub const DefBuildSamplingStatsConcurrency: i64 = 2;
pub const DefAutoAnalyzeRatio: f64 = 0.5;
pub const DefAutoAnalyzeStartTime: &str = "00:00 +0000";
pub const DefAutoAnalyzeEndTime: &str = "23:59 +0000";
pub const DefAutoIncrementIncrement: i64 = 1;
pub const DefAutoIncrementOffset: i64 = 1;
pub const DefChecksumTableConcurrency: i64 = 4;
pub const DefSkipUTF8Check: bool = false;
pub const DefSkipASCIICheck: bool = false;
pub const DefOptAggPushDown: bool = false;
pub const DefOptDeriveTopN: bool = false;
pub const DefOptCartesianBCJ: i64 = 1;
pub const DefOptMPPOuterJoinFixedBuildSide: bool = false;
pub const DefOptWriteRowID: bool = false;
pub const DefOptEnableCorrelationAdjustment: bool = true;
pub const DefOptLimitPushDownThreshold: i64 = 5000;
pub const DefOptCorrelationThreshold: f64 = 0.9;
pub const DefOptCorrelationExpFactor: i64 = 1;
pub const DefOptRiskEqSkewRatio: f64 = 0.0;
pub const DefOptRiskRangeSkewRatio: f64 = 0.0;
pub const DefOptRiskScaleNDVSkewRatio: f64 = 1.0;
pub const DefOptRiskGroupNDVSkewRatio: f64 = 0.0;
pub const DefOptAlwaysKeepJoinKey: bool = true;
pub const DefOptCartesianJoinOrderThreshold: f64 = 0.0;
pub const DefOptCPUFactor: f64 = 3.0;
pub const DefOptCopCPUFactor: f64 = 3.0;
pub const DefOptTiFlashConcurrencyFactor: f64 = 24.0;
pub const DefOptNetworkFactor: f64 = 1.0;
pub const DefOptScanFactor: f64 = 1.5;
pub const DefOptDescScanFactor: f64 = 3.0;
pub const DefOptSeekFactor: f64 = 20.0;
pub const DefOptMemoryFactor: f64 = 0.001;
pub const DefOptDiskFactor: f64 = 1.5;
pub const DefOptConcurrencyFactor: f64 = 3.0;
pub const DefOptIndexScanCostFactor: f64 = 1.0;
pub const DefOptIndexReaderCostFactor: f64 = 1.0;
pub const DefOptTableReaderCostFactor: f64 = 1.0;
pub const DefOptTableFullScanCostFactor: f64 = 1.0;
pub const DefOptTableRangeScanCostFactor: f64 = 1.0;
pub const DefOptTableRowIDScanCostFactor: f64 = 1.0;
pub const DefOptTableTiFlashScanCostFactor: f64 = 1.0;
pub const DefOptIndexLookupCostFactor: f64 = 1.0;
pub const DefOptIndexMergeCostFactor: f64 = 1.0;
pub const DefOptSortCostFactor: f64 = 1.0;
pub const DefOptTopNCostFactor: f64 = 1.0;
pub const DefOptLimitCostFactor: f64 = 1.0;
pub const DefOptStreamAggCostFactor: f64 = 1.0;
pub const DefOptHashAggCostFactor: f64 = 1.0;
pub const DefOptMergeJoinCostFactor: f64 = 1.0;
pub const DefOptHashJoinCostFactor: f64 = 1.0;
pub const DefOptIndexJoinCostFactor: f64 = 1.0;
pub const DefOptIndexJoinMaxScanRowsRatio: f64 = 0.0;
pub const DefOptSelectivityFactor: f64 = 0.8;
pub const DefOptForceInlineCTE: bool = false;
pub const DefOptInSubqToJoinAndAgg: bool = true;
pub const DefOptPreferRangeScan: bool = true;
pub const DefOptEnableNoDecorrelateInSelect: bool = false;
pub const DefOptEnableAlternativeLogicalPlans: bool = false;
pub const DefOptEnableSemiJoinRewrite: bool = false;
pub const DefBatchInsert: bool = false;
pub const DefBatchDelete: bool = false;
pub const DefBatchCommit: bool = false;
pub const DefCurretTS: i64 = 0;
pub const DefInitChunkSize: i64 = 32;
pub const DefMinPagingSize: i64 = 128;
pub const DefMaxPagingSize: i64 = 50_000;
pub const DefPagingSizeBytes: i64 = 0;
pub const DefMaxChunkSize: i64 = 1024;
pub const DefDMLBatchSize: i64 = 0;
pub const DefTiDBMLogPurgeBatchSize: u64 = 10_000;
pub const DefTiDBMLogPurgeBatchMinSize: i64 = 1;
pub const DefTiDBMLogPurgeBatchMaxSize: u64 = 1_000_000;
pub const DefTiDBMLogPurgeMinRate: u64 = 2_000;
pub const DefTiDBMLogPurgeRateBudgetRatio: f64 = 0.5;
pub const DefTiDBMLogPurgeDeleteTiFlashThreads: i64 = 0;
pub const DefMaxPreparedStmtCount: i64 = -1;
pub const DefWaitTimeout: i64 = 28800;
pub const DefTiDBMemQuotaApplyCache: i64 = 32 << 20; // 32MB.;
pub const DefTiDBMemQuotaBindingCache: i64 = 64 << 20; // 64MB.;
pub const DefTiDBGeneralLog: bool = false;
pub const DefTiDBTraceEvent: &str = "";
pub const DefTiDBPProfSQLCPU: i64 = 0;
pub const DefTiDBRetryLimit: i64 = 10;
pub const DefTiDBDisableTxnAutoRetry: bool = true;
pub const DefTiDBConstraintCheckInPlace: bool = false;
pub const DefTiDBHashJoinConcurrency: i64 = ConcurrencyUnset;
pub const DefTiDBProjectionConcurrency: i64 = ConcurrencyUnset;
pub const DefBroadcastJoinThresholdSize: i64 = 100 * 1024 * 1024;
pub const DefBroadcastJoinThresholdCount: i64 = 10 * 1024;
pub const DefPreferBCJByExchangeDataSize: bool = false;
pub const DefTiDBOptimizerSelectivityLevel: i64 = 0;
pub const DefTiDBOptIndexPruneThreshold: i64 = 20;
pub const DefTiDBOptimizerEnableNewOFGB: bool = false;
pub const DefTiDBEnableOuterJoinReorder: bool = true;
pub const DefTiDBEnableNAAJ: bool = true;
pub const DefTiDBAllowBatchCop: i64 = 1;
pub const DefShardRowIDBits: i64 = 0;
pub const DefPreSplitRegions: i64 = 0;
pub const DefBlockEncryptionMode: &str = "aes-128-ecb";
pub const DefTiDBAllowMPPExecution: bool = true;
pub const DefTiDBAllowTiFlashCop: bool = false;
pub const DefTiDBHashExchangeWithNewCollation: bool = true;
pub const DefTiDBEnforceMPPExecution: bool = false;
pub const DefTiFlashMaxThreads: i64 = -1;
pub const DefTiFlashMaxBytesBeforeExternalJoin: i64 = -1;
pub const DefTiFlashMaxBytesBeforeExternalGroupBy: i64 = -1;
pub const DefTiFlashMaxBytesBeforeExternalSort: i64 = -1;
pub const DefTiFlashMemQuotaQueryPerNode: i64 = 0;
pub const DefTiFlashQuerySpillRatio: f64 = 0.7;
pub const DefTiFlashHashJoinVersion: &str = "legacy";
pub const DefTiDBEnableTiFlashPipelineMode: bool = true;
pub const DefTiDBMPPStoreFailTTL: &str = "0s";
pub const DefTiDBTxnMode: &str = "pessimistic";
pub const DefTiDBRowFormatV1: i64 = 1;
pub const DefTiDBRowFormatV2: i64 = 2;
pub const DefTiDBDDLReorgWorkerCount: i64 = 4;
pub const DefTiDBDDLReorgBatchSize: i64 = 256;
pub const DefTiDBDDLFlashbackConcurrency: i64 = 64;
pub const DefTiDBDDLErrorCountLimit: i64 = 512;
pub const DefTiDBDDLReorgMaxWriteSpeed: i64 = 0;
pub const DefTiDBMaxDeltaSchemaCount: i64 = 1024;
pub const DefTiDBPlacementMode: &str = PlacementModeStrict;
pub const DefTiDBEnableAutoIncrementInGenerated: bool = false;
pub const DefTiDBHashAggPartialConcurrency: i64 = ConcurrencyUnset;
pub const DefTiDBHashAggFinalConcurrency: i64 = ConcurrencyUnset;
pub const DefTiDBWindowConcurrency: i64 = ConcurrencyUnset;
pub const DefTiDBMergeJoinConcurrency: i64 = 1; // disable optimization by default;
pub const DefTiDBStreamAggConcurrency: i64 = 1;
pub const DefTiDBForcePriority: i64 = 0;
pub const DefEnableWindowFunction: bool = true;
pub const DefEnablePipelinedWindowFunction: bool = true;
pub const DefTiDBEnableStrictNotNullCheck: bool = true;
pub const DefEnableStrictDoubleTypeCheck: bool = true;
pub const DefEnableVectorizedExpression: bool = true;
pub const DefTiDBOptJoinReorderThreshold: i64 = 0;
pub const DefTiDBOptEnableAdvancedJoinReorder: bool = true;
pub const DefTiDBOptJoinReorderThroughProj: bool = false;
pub const DefTiDBOptJoinReorderThroughSel: bool = false;
pub const DefTiDBDDLSlowOprThreshold: i64 = 300;
pub const DefTiDBUseFastAnalyze: bool = false;
pub const DefTiDBSkipIsolationLevelCheck: bool = false;
pub const DefTiDBExpensiveQueryTimeThreshold: i64 = 60; // 60s;
pub const DefTiDBExpensiveTxnTimeThreshold: i64 = 60 * 10; // 10 minutes;
pub const DefTiDBScatterRegion: &str = ScatterOff;
pub const DefTiDBWaitSplitRegionFinish: bool = true;
pub const DefWaitSplitRegionTimeout: i64 = 300; // 300s;
pub const DefTiDBEnableNoopFuncs: &str = Off;
pub const DefTiDBEnableNoopVariables: bool = true;
pub const DefTiDBAllowRemoveAutoInc: bool = false;
pub const DefTiDBUsePlanBaselines: bool = true;
pub const DefTiDBEvolvePlanBaselines: bool = false;
pub const DefTiDBEvolvePlanTaskMaxTime: i64 = 600; // 600s;
pub const DefTiDBEvolvePlanTaskStartTime: &str = "00:00 +0000";
pub const DefTiDBEvolvePlanTaskEndTime: &str = "23:59 +0000";
pub const DefInnodbLockWaitTimeout: i64 = 50; // 50s;
pub const DefTiDBStoreLimit: i64 = 0;
pub const DefTiDBMetricSchemaStep: i64 = 60; // 60s;
pub const DefTiDBMetricSchemaRangeDuration: i64 = 60; // 60s;
pub const DefTiDBFoundInPlanCache: bool = false;
pub const DefTiDBFoundInBinding: bool = false;
pub const DefTiDBEnableCollectExecutionInfo: bool = true;
pub const DefTiDBAllowAutoRandExplicitInsert: bool = false;
pub const DefTiDBEnableClusteredIndex: ClusteredIndexDefMode = ClusteredIndexDefModeOn;
pub const DefTiDBRedactLog: &str = Off;
pub const DefTiDBRestrictedReadOnly: bool = false;
pub const DefTiDBSuperReadOnly: bool = false;
pub const DefTiDBShardAllocateStep: i64 = i64::MAX;
pub const DefTiDBPointGetCache: bool = false;
pub const DefTiDBEnableTelemetry: bool = true;
pub const DefTiDBEnableParallelApply: bool = false;
pub const DefTiDBPartitionPruneMode: &str = "dynamic";
pub const DefTiDBEnableRateLimitAction: bool = false;
pub const DefTiDBEnableAsyncCommit: bool = false;
pub const DefTiDBEnable1PC: bool = false;
pub const DefTiDBGuaranteeLinearizability: bool = true;
pub const DefTiDBAnalyzeVersion: i64 = 2;
// Deprecated: This variable is deprecated, please do not use this variable.
pub const DefTiDBAutoAnalyzePartitionBatchSize: i64 = 8192;
pub const DefTiDBEnableIndexMergeJoin: bool = false;
pub const DefTiDBTrackAggregateMemoryUsage: bool = true;
pub const DefCTEMaxRecursionDepth: i64 = 1000;
pub const DefTiDBTmpTableMaxSize: i64 = 64 << 20; // 64MB.;
pub const DefTiDBEnableLocalTxn: bool = false;
pub const DefTiDBTSOClientBatchMaxWaitTime: f64 = 0.0; // 0ms;
pub const DefTiDBEnableTSOFollowerProxy: bool = false;
pub const DefPDEnableFollowerHandleRegion: bool = true;
pub const DefTiDBEnableBatchQueryRegion: bool = false;
pub const DefTiDBEnableOrderedResultMode: bool = false;
pub const DefTiDBEnablePseudoForOutdatedStats: bool = false;
/// Missing persisted rows retain the historical ability to add replicas.
pub const DefTiDBColumnarStorageEnabled: bool = true;
pub const DefTiDBRegardNULLAsPoint: bool = true;
pub const DefEnablePlacementCheck: bool = true;
pub const DefTimestamp: &str = "0";
pub const DefTimestampFloat: f64 = 0.0;
pub const DefTiDBEnableStmtSummary: bool = true;
pub const DefTiDBStmtSummaryInternalQuery: bool = false;
pub const DefTiDBStmtSummaryRefreshInterval: i64 = 1800;
pub const DefTiDBStmtSummaryHistorySize: i64 = 24;
pub const DefTiDBStmtSummaryMaxStmtCount: i64 = 3000;
pub const DefTiDBStmtSummaryMaxSQLLength: i64 = 32768;
pub const DefTiDBStmtSummaryPersistEvicted: bool = false;
pub const DefTiDBStmtSummaryGroupByUser: bool = false;
pub const DefTiDBCapturePlanBaseline: &str = Off;
pub const DefTiDBIgnoreInlistPlanDigest: bool = true;
pub const DefTiDBEnableIndexMerge: bool = true;
pub const DefTiDBEnableNoBackslashEscapesInLike: bool = true;
pub const DefEnableLegacyInstanceScope: bool = true;
pub const DefTiDBTableCacheLease: i64 = 3; // 3s;
pub const DefTiDBPersistAnalyzeOptions: bool = true;
pub const DefTiDBStatsLoadSyncWait: i64 = 100;
pub const DefTiDBStatsLoadPseudoTimeout: bool = true;
pub const DefSysdateIsNow: bool = false;
pub const DefTiDBEnableParallelHashaggSpill: bool = true;
pub const DefTiDBEnableMutationChecker: bool = false;
pub const DefTiDBTxnAssertionLevel: &str = AssertionOffStr;
pub const DefTiDBIgnorePreparedCacheCloseStmt: bool = false;
pub const DefTiDBBatchPendingTiFlashCount: i64 = 4000;
pub const DefRCReadCheckTS: bool = false;
pub const DefTiDBRemoveOrderbyInSubquery: bool = true;
pub const DefTiDBSkewDistinctAgg: bool = false;
pub const DefTiDB3StageDistinctAgg: bool = true;
pub const DefTiDB3StageMultiDistinctAgg: bool = false;
pub const DefTiDBOptExplainEvaledSubquery: bool = false;
pub const DefTiDBReadStaleness: i64 = 0;
pub const DefTiDBGCMaxWaitTime: i64 = 24 * 60 * 60;
pub const DefMaxAllowedPacket: u64 = 64 << 20;
pub const DefTiDBEnableBatchDML: bool = false;
pub const DefTiDBMemQuotaQuery: i64 = 1_073_741_824; // 1GB.
pub const DefTiDBStatsCacheMemQuota: i64 = 0;
pub const MaxTiDBStatsCacheMemQuota: i64 = 1024 * 1024 * 1024 * 1024; // 1TB;
pub const DefTiDBQueryLogMaxLen: i64 = 4096;
pub const DefRequireSecureTransport: bool = false;
pub const DefTiDBCommitterConcurrency: i64 = 128;
pub const DefTiDBPipelinedDmlResourcePolicy: &str = StrategyStandard;
pub const DefTiDBBatchDMLIgnoreError: bool = false;
pub const DefTiDBMemQuotaAnalyze: i64 = -1;
pub const DefTiDBEnableAutoAnalyze: bool = true;
pub const DefTiDBEnableAutoAnalyzePriorityQueue: bool = true;
pub const DefTiDBAnalyzeColumnOptions: &str = "ALL";
pub const DefTiDBAnalyzeDefaultNumBuckets: u64 = 256;
pub const DefTiDBAnalyzeDefaultNumTopN: u64 = 100;
pub const MinTiDBAnalyzeDefaultNumBuckets: i64 = 1;
pub const MaxTiDBAnalyzeDefaultNumBuckets: u64 = 100_000;
pub const MinTiDBAnalyzeDefaultNumTopN: i64 = 0;
pub const MaxTiDBAnalyzeDefaultNumTopN: u64 = 100_000;
pub const DefTiDBMemOOMAction: &str = "CANCEL";
pub const DefTiDBMaxAutoAnalyzeTime: i64 = 12 * 60 * 60;
pub const DefTiDBAutoAnalyzeConcurrency: i64 = 3;
pub const DefTiDBEnablePrepPlanCache: bool = true;
pub const DefTiDBPrepPlanCacheSize: i64 = 100;
pub const DefTiDBSessionPlanCacheSize: i64 = 100;
pub const DefTiDBEnablePrepPlanCacheMemoryMonitor: bool = true;
pub const DefTiDBPrepPlanCacheMemoryGuardRatio: f64 = 0.1;
pub const DefTiDBEnableWorkloadBasedLearning: bool = false;
pub const DefTiDBWorkloadBasedLearningInterval: i64 = 24 * 3_600_000_000_000;
pub const DefTiDBEnableDistTask: bool = true;
pub const DefTiDBMaxDistTaskNodes: i64 = -1;
pub const DefTiDBEnableFastCreateTable: bool = true;
pub const DefTiDBSimplifiedMetrics: bool = false;
pub const DefTiDBEnablePaging: bool = true;
pub const DefTiFlashFineGrainedShuffleStreamCount: i64 = 0;
pub const DefStreamCountWhenMaxThreadsNotSet: i64 = 8;
pub const DefTiFlashFineGrainedShuffleBatchSize: i64 = 8192;
pub const DefAdaptiveClosestReadThreshold: i64 = 4096;
pub const DefTiDBEnableAnalyzeSnapshot: bool = false;
pub const DefTiDBGenerateBinaryPlan: bool = true;
pub const DefTiDBEnableDDLAnalyze: bool = false;
pub const DefEnableTiDBGCAwareMemoryTrack: bool = false;
pub const DefTiDBDefaultStrMatchSelectivity: i64 = 0;
pub const DefTiDBEnableTmpStorageOnOOM: bool = true;
pub const DefTiDBEnableMDL: bool = true;
pub const DefTiFlashFastScan: bool = false;
pub const DefMemoryUsageAlarmRatio: f64 = 0.7;
pub const DefMemoryUsageAlarmKeepRecordNum: i64 = 5;
pub const DefTiDBEnableFastReorg: bool = true;
pub const DefTiDBDDLDiskQuota: i64 = 100 * 1024 * 1024 * 1024; // 100GB;
pub const DefExecutorConcurrency: i64 = 5;
pub const DefTiDBEnableNonPreparedPlanCache: bool = false;
pub const DefTiDBEnableNonPreparedPlanCacheForDML: bool = true;
pub const DefTiDBPlanCacheStrategy: &str = TiDBPlanCacheStrategyAll;
pub const DefTiDBNonPreparedPlanCacheSize: i64 = 100;
pub const DefTiDBPlanCacheMaxPlanSize: i64 = 2 * 1024 * 1024;
pub const DefTiDBInstancePlanCacheMaxMemSize: i64 = 100 * 1024 * 1024;
pub const MinTiDBInstancePlanCacheMemSize: i64 = 100 * 1024 * 1024;
pub const DefTiDBInstancePlanCacheReservedPercentage: f64 = 0.1;
// MaxDDLReorgBatchSize is exported for testing.
pub const MaxDDLReorgBatchSize: i32 = 10240;
pub const MinDDLReorgBatchSize: i32 = 32;
pub const MinExpensiveQueryTimeThreshold: u64 = 10; // 10s
pub const MinExpensiveTxnTimeThreshold: u64 = 60; // 60s
pub const DefTiDBAutoBuildStatsConcurrency: i64 = DefBuildStatsConcurrency;
pub const DefTiDBSysProcScanConcurrency: i64 = DefAnalyzeDistSQLScanConcurrency;
pub const DefTiDBRcWriteCheckTs: bool = false;
pub const DefTiDBForeignKeyChecks: bool = true;
pub const DefTiDBForeignKeyCheckInSharedLock: bool = false;
pub const DefTiDBOptAdvancedJoinHint: bool = true;
pub const DefTiDBAnalyzePartitionConcurrency: i64 = 2;
pub const DefTiDBOptRangeMaxSize: i64 = 64 * 1024 * 1024; // 64 MB
pub const DefTiDBCostModelVer: i64 = 2;
pub const DefTiDBServerMemoryLimitSessMinSize: i64 = 128 << 20;
pub const DefTiDBServerMemoryLimitGCTrigger: f64 = 0.7;
pub const DefTiDBEnableGOGCTuner: bool = true;
// DefTiDBGOGCTunerThreshold is to limit TiDBGOGCTunerThreshold.
pub const DefTiDBGOGCTunerThreshold: f64 = 0.6;
pub const DefTiDBGOGCMaxValue: i64 = 500;
pub const DefTiDBGOGCMinValue: i64 = 100;
pub const DefTiDBOptPrefixIndexSingleScan: bool = true;
pub const DefTiDBOptPartialOrderedIndexForTopN: &str = "DISABLE";
pub const DefTiDBEnableAsyncMergeGlobalStats: bool = true;
pub const DefTiDBExternalTS: i64 = 0;
pub const DefTiDBEnableExternalTSRead: bool = false;
pub const DefTiDBEnableReusechunk: bool = true;
pub const DefTiDBUseAlloc: bool = false;
pub const DefTiDBEnablePlanReplayerCapture: bool = true;
pub const DefTiDBPlanReplayerFileRetentionTime: i64 = 7 * 24 * 3_600_000_000_000;
pub const DefTiDBIndexMergeIntersectionConcurrency: i64 = ConcurrencyUnset;
pub const DefTiDBTTLJobEnable: bool = true;
pub const DefTiDBTTLScanBatchSize: i64 = 500;
pub const DefTiDBTTLScanBatchMaxSize: i64 = 10240;
pub const DefTiDBTTLScanBatchMinSize: i64 = 1;
pub const DefTiDBTTLDeleteBatchSize: i64 = 100;
pub const DefTiDBTTLDeleteBatchMaxSize: i64 = 10240;
pub const DefTiDBTTLDeleteBatchMinSize: i64 = 1;
pub const DefTiDBTTLDeleteRateLimit: i64 = 0;
pub const DefTiDBTTLRunningTasks: i64 = -1;
pub const DefPasswordReuseHistory: i64 = 0;
pub const DefPasswordReuseTime: i64 = 0;
pub const DefMaxUserConnections: i64 = 0;
pub const DefTiDBStoreBatchSize: i64 = 4;
pub const DefTiDBHistoricalStatsDuration: i64 = 7 * 24 * 3_600_000_000_000;
pub const DefTiDBEnableHistoricalStatsForCapture: bool = false;
pub const DefTiDBTTLJobScheduleWindowStartTime: &str = "00:00 +0000";
pub const DefTiDBTTLJobScheduleWindowEndTime: &str = "23:59 +0000";
pub const DefTiDBTTLScanWorkerCount: i64 = 4;
pub const DefTiDBTTLDeleteWorkerCount: i64 = 4;
pub const DefaultExchangeCompressionMode: ExchangeCompressionMode =
    ExchangeCompressionModeUnspecified;
pub const DefTiDBEnableResourceControl: bool = true;
pub const DefTiDBResourceControlStrictMode: bool = true;
pub const DefTiDBPessimisticTransactionFairLocking: bool = false;
pub const DefTiDBEnablePlanCacheForParamLimit: bool = true;
pub const DefTiDBEnableINLJoinMultiPattern: bool = true;
pub const DefTiFlashComputeDispatchPolicy: &str = DispatchPolicyConsistentHashStr;
pub const DefTiDBEnablePlanCacheForSubquery: bool = true;
pub const DefTiDBLoadBasedReplicaReadThreshold: i64 = 1_000_000_000;
pub const DefTiDBOptEnableLateMaterialization: bool = true;
pub const DefTiDBOptOrderingIdxSelThresh: f64 = 0.0;
pub const DefTiDBOptOrderingIdxSelRatio: f64 = 0.01;
pub const DefTiDBOptEnableMPPSharedCTEExecution: bool = false;
pub const DefTiDBPlanCacheInvalidationOnFreshStats: bool = true;
pub const DefTiDBPlanCacheSkipStatsOnBinding: bool = true;
pub const DefTiDBEnableRowLevelChecksum: bool = false;
pub const DefAuthenticationLDAPSASLAuthMethodName: &str = "SCRAM-SHA-1";
pub const DefAuthenticationLDAPSASLServerPort: i64 = 389;
pub const DefAuthenticationLDAPSASLTLS: bool = false;
pub const DefAuthenticationLDAPSASLUserSearchAttr: &str = "uid";
pub const DefAuthenticationLDAPSASLInitPoolSize: i64 = 10;
pub const DefAuthenticationLDAPSASLMaxPoolSize: i64 = 1000;
pub const DefAuthenticationLDAPSimpleAuthMethodName: &str = "SIMPLE";
pub const DefAuthenticationLDAPSimpleServerPort: i64 = 389;
pub const DefAuthenticationLDAPSimpleTLS: bool = false;
pub const DefAuthenticationLDAPSimpleUserSearchAttr: &str = "uid";
pub const DefAuthenticationLDAPSimpleInitPoolSize: i64 = 10;
pub const DefAuthenticationLDAPSimpleMaxPoolSize: i64 = 1000;
pub const DefTiFlashReplicaRead: &str = AllReplicaStr;
pub const DefTiDBEnableFastCheckTable: bool = true;
pub const DefRuntimeFilterType: &str = "IN";
pub const DefRuntimeFilterMode: &str = "OFF";
pub const DefTiDBLockUnchangedKeys: bool = true;
pub const DefTiDBEnableCheckConstraint: bool = false;
pub const DefTiDBSkipMissingPartitionStats: bool = true;
pub const DefTiDBOptEnableHashJoin: bool = true;
pub const DefTiDBEnableFullOuterJoin: bool = false;
pub const DefTiDBHashJoinVersion: &str = "optimized";
pub const DefTiDBOptIndexJoinBuild: bool = true;
pub const DefTiDBOptObjective: &str = OptObjectiveModerate;
pub const DefTiDBSchemaVersionCacheLimit: i64 = 16;
pub const DefTiDBIdleTransactionTimeout: i64 = 0;
pub const DefTiDBTxnEntrySizeLimit: i64 = 0;
pub const DefTiDBSchemaCacheSize: i64 = 512 * 1024 * 1024;
pub const DefTiDBLowResolutionTSOUpdateInterval: i64 = 2000;
pub const DefDivPrecisionIncrement: i64 = 4;
pub const DefTiDBDMLType: &str = "STANDARD";
pub const DefGroupConcatMaxLen: u64 = 1024;
pub const DefDefaultWeekFormat: &str = "0";
pub const DefTiFlashPreAggMode: &str = ForcePreAggStr;
pub const DefTiDBEnableLazyCursorFetch: bool = false;
pub const DefOptEnableProjectionPushDown: bool = true;
pub const DefTiDBEnableSharedLockPromotion: bool = false;
pub const DefTiDBTSOClientRPCMode: &str = TSOClientRPCModeDefault;
pub const DefTiDBCircuitBreakerPDMetaErrorRateRatio: f64 = 0.0;
pub const DefTiDBAccelerateUserCreationUpdate: bool = false;
pub const DefTiDBEnableTSValidation: bool = true;
pub const DefTiDBLoadBindingTimeout: i64 = 200;
pub const DefTiDBEnableBindingUsage: bool = true;
pub const DefTiDBAdvancerCheckPointLagLimit: i64 = 48 * 3_600_000_000_000;
pub const DefTiDBMemArbitratorSoftLimitText: &str = "0";
pub const DefTiDBMemArbitratorModeText: &str = "priority";
pub const DefTiDBMemArbitratorQueryReservedText: &str = "0";
pub const DefTiDBMemArbitratorWaitAverse: &str = "0";
pub const DefTiDBIndexLookUpPushDownPolicy: &str = IndexLookUpPushDownPolicyHintOnly;
pub const DefEnableCachePrepareStmt: bool = false;
pub const DefTiDBEnableTxnFile: bool = false;
/// Zero delegates the threshold to the TiKV client configuration.
pub const DefTiDBTxnFileMinMutationSize: u64 = 0;
/// Nonzero per-session thresholds smaller than 1 MiB are rejected.
pub const MinTiDBTxnFileMinMutationSize: u64 = 1 << 20;
// DefConnectAttrsSize is the default max aggregate byte size of connection attributes per connection.
// This corresponds to performance_schema_session_connect_attrs_size. In TiDB, -1 means no limit up to 64KB.
pub const DefConnectAttrsSize: i64 = 4096;

// —— 进程级全局可变状态 ——
// 这些 Atomic*/LazyLock 在 SET GLOBAL 等路径被更新，多会话并发可见。
// Process global variables.
// Global process state, kept in the same declaration order as Go.
pub static ProcessGeneralLog: AtomicBoolValue = AtomicBoolValue::new(false);
pub static RunAutoAnalyze: AtomicBoolValue = AtomicBoolValue::new(DefTiDBEnableAutoAnalyze);
pub static EnableAutoAnalyzePriorityQueue: AtomicBoolValue =
    AtomicBoolValue::new(DefTiDBEnableAutoAnalyzePriorityQueue);
// AnalyzeColumnOptions is a global variable that indicates the default column choice for ANALYZE.
// The value of this variable is a string that can be one of the following values:
// "PREDICATE", "ALL".
// The behavior of the analyze operation depends on the value of `tidb_persist_analyze_options`:
// 1. If `tidb_persist_analyze_options` is enabled and the column choice from the analyze options record is set to `default`,
//    the value of `tidb_analyze_column_options` determines the behavior of the analyze operation.
// 2. If `tidb_persist_analyze_options` is disabled, `tidb_analyze_column_options` is used directly to decide
//    whether to analyze all columns or just the predicate columns.
pub static AnalyzeColumnOptions: LazyLock<AtomicStringValue> =
    LazyLock::new(|| AtomicStringValue::new(DefTiDBAnalyzeColumnOptions));
pub static AnalyzeDefaultNumBuckets: AtomicU64Value =
    AtomicU64Value::new(DefTiDBAnalyzeDefaultNumBuckets);
pub static AnalyzeDefaultNumTopN: AtomicU64Value =
    AtomicU64Value::new(DefTiDBAnalyzeDefaultNumTopN);
pub static GlobalLogMaxDays: AtomicI32Value = AtomicI32Value::new(0);
pub static QueryLogMaxLen: AtomicI32Value = AtomicI32Value::new((DefTiDBQueryLogMaxLen) as i32);
pub static EnablePProfSQLCPU: AtomicBoolValue = AtomicBoolValue::new(false);
pub static EnableBatchDML: AtomicBoolValue = AtomicBoolValue::new(false);
pub static EnableTmpStorageOnOOM: AtomicBoolValue =
    AtomicBoolValue::new(DefTiDBEnableTmpStorageOnOOM);
pub static DDLReorgWorkerCounter: AtomicI32 = AtomicI32::new(DefTiDBDDLReorgWorkerCount as i32);
pub static DDLReorgBatchSize: AtomicI32 = AtomicI32::new(DefTiDBDDLReorgBatchSize as i32);
pub static DDLFlashbackConcurrency: AtomicI32 =
    AtomicI32::new(DefTiDBDDLFlashbackConcurrency as i32);
pub static DDLErrorCountLimit: AtomicI64 = AtomicI64::new(DefTiDBDDLErrorCountLimit);
pub static DDLReorgRowFormat: AtomicI64 = AtomicI64::new(DefTiDBRowFormatV2);
pub static DDLReorgMaxWriteSpeed: AtomicI64Value =
    AtomicI64Value::new((DefTiDBDDLReorgMaxWriteSpeed) as i64);
pub static MaxDeltaSchemaCount: AtomicI64 = AtomicI64::new(DefTiDBMaxDeltaSchemaCount);
pub static GlobalSlowLogRateLimiter: UnlimitedRateLimiter = UnlimitedRateLimiter::new();
// DDLSlowOprThreshold is the threshold for ddl slow operations, uint is millisecond.
pub static DDLSlowOprThreshold: AtomicU32Value = AtomicU32Value::new(300);
pub static GlobalSlowLogRules: LazyLock<RwLock<GlobalSlowLogRulesValue>> =
    LazyLock::new(|| RwLock::new(GlobalSlowLogRulesValue::default()));
pub static ForcePriority: AtomicI32Value = AtomicI32Value::new(DefTiDBForcePriority as i32);
pub static MaxOfMaxAllowedPacket: AtomicU64Value = AtomicU64Value::new(1_073_741_824);
pub static ExpensiveQueryTimeThreshold: AtomicU64Value =
    AtomicU64Value::new(DefTiDBExpensiveQueryTimeThreshold as u64);
pub static ExpensiveTxnTimeThreshold: AtomicU64Value =
    AtomicU64Value::new(DefTiDBExpensiveTxnTimeThreshold as u64);
pub static MemoryUsageAlarmRatio: AtomicF64Value = AtomicF64Value::new(DefMemoryUsageAlarmRatio);
pub static MemoryUsageAlarmKeepRecordNum: AtomicI64Value =
    AtomicI64Value::new((DefMemoryUsageAlarmKeepRecordNum) as i64);
pub static EnableLocalTxn: AtomicBoolValue = AtomicBoolValue::new(DefTiDBEnableLocalTxn);
pub static MaxTSOBatchWaitInterval: AtomicF64Value =
    AtomicF64Value::new(DefTiDBTSOClientBatchMaxWaitTime);
pub static EnableTSOFollowerProxy: AtomicBoolValue =
    AtomicBoolValue::new(DefTiDBEnableTSOFollowerProxy);
pub static EnablePDFollowerHandleRegion: AtomicBoolValue =
    AtomicBoolValue::new(DefPDEnableFollowerHandleRegion);
pub static EnableBatchQueryRegion: AtomicBoolValue =
    AtomicBoolValue::new(DefTiDBEnableBatchQueryRegion);
pub static RestrictedReadOnly: AtomicBoolValue = AtomicBoolValue::new(DefTiDBRestrictedReadOnly);
pub static VarTiDBSuperReadOnly: AtomicBoolValue = AtomicBoolValue::new(DefTiDBSuperReadOnly);
pub static PersistAnalyzeOptions: AtomicBoolValue =
    AtomicBoolValue::new(DefTiDBPersistAnalyzeOptions);
pub static TableCacheLease: AtomicI64Value = AtomicI64Value::new((DefTiDBTableCacheLease) as i64);
pub static StatsLoadSyncWait: AtomicI64Value =
    AtomicI64Value::new((DefTiDBStatsLoadSyncWait) as i64);
pub static StatsLoadPseudoTimeout: AtomicBoolValue =
    AtomicBoolValue::new(DefTiDBStatsLoadPseudoTimeout);
pub static MemQuotaBindingCache: AtomicI64Value =
    AtomicI64Value::new((DefTiDBMemQuotaBindingCache) as i64);
pub static GCMaxWaitTime: AtomicI64Value = AtomicI64Value::new((DefTiDBGCMaxWaitTime) as i64);
pub static StatsCacheMemQuota: AtomicI64Value =
    AtomicI64Value::new((DefTiDBStatsCacheMemQuota) as i64);
pub static OOMAction: LazyLock<AtomicStringValue> =
    LazyLock::new(|| AtomicStringValue::new(DefTiDBMemOOMAction));
pub static MaxAutoAnalyzeTime: AtomicI64Value =
    AtomicI64Value::new((DefTiDBMaxAutoAnalyzeTime) as i64);
// variables for plan cache
pub static PreparedPlanCacheMemoryGuardRatio: AtomicF64Value =
    AtomicF64Value::new(DefTiDBPrepPlanCacheMemoryGuardRatio);
pub static EnableInstancePlanCache: AtomicBoolValue = AtomicBoolValue::new(false);
pub static InstancePlanCacheReservedPercentage: AtomicF64Value = AtomicF64Value::new(0.1);
pub static InstancePlanCacheMaxMemSize: AtomicI64Value =
    AtomicI64Value::new((DefTiDBInstancePlanCacheMaxMemSize) as i64);
pub static EnableDistTask: AtomicBoolValue = AtomicBoolValue::new(DefTiDBEnableDistTask);
pub static EnableFastCreateTable: AtomicBoolValue =
    AtomicBoolValue::new(DefTiDBEnableFastCreateTable);
pub static EnableNoopVariables: AtomicBoolValue = AtomicBoolValue::new(DefTiDBEnableNoopVariables);
// Classic 下的 MDL 开关；NextGen 路径不读取此值。
static enableMDL: AtomicBool = AtomicBool::new(false);
pub static AutoAnalyzePartitionBatchSize: AtomicI64Value =
    AtomicI64Value::new((DefTiDBAutoAnalyzePartitionBatchSize) as i64);
pub static AutoAnalyzeConcurrency: AtomicI32Value =
    AtomicI32Value::new((DefTiDBAutoAnalyzeConcurrency) as i32);
// TODO: set value by session variable
pub static EnableWorkloadBasedLearning: AtomicBoolValue =
    AtomicBoolValue::new(DefTiDBEnableWorkloadBasedLearning);
pub static WorkloadBasedLearningInterval: AtomicI64Value =
    AtomicI64Value::new((DefTiDBWorkloadBasedLearningInterval) as i64);
// EnableFastReorg indicates whether to use lightning to enhance DDL reorg performance.
pub static EnableFastReorg: AtomicBoolValue = AtomicBoolValue::new(DefTiDBEnableFastReorg);
// DDLDiskQuota is the temporary variable for set disk quota for lightning
pub static DDLDiskQuota: AtomicU64Value = AtomicU64Value::new((DefTiDBDDLDiskQuota) as u64);
// EnableForeignKey indicates whether to enable foreign key feature.
pub static EnableForeignKey: AtomicBoolValue = AtomicBoolValue::new(true);
pub static EnableRCReadCheckTS: AtomicBoolValue = AtomicBoolValue::new(false);
// EnableRowLevelChecksum indicates whether to append checksum to row values.
pub static EnableRowLevelChecksum: AtomicBoolValue =
    AtomicBoolValue::new(DefTiDBEnableRowLevelChecksum);
pub static LowResolutionTSOUpdateInterval: AtomicU32Value =
    AtomicU32Value::new((DefTiDBLowResolutionTSOUpdateInterval) as u32);

// DefTiDBServerMemoryLimit indicates the default value of TiDBServerMemoryLimit(TotalMem * 80%).
// It should be a const and shouldn't be modified after tidb is started.
pub static DefTiDBServerMemoryLimit: LazyLock<String> =
    LazyLock::new(serverMemoryLimitDefaultValue);
pub static GOGCTunerThreshold: AtomicF64Value = AtomicF64Value::new(DefTiDBGOGCTunerThreshold);
pub static PasswordValidationLength: AtomicI32Value = AtomicI32Value::new((8) as i32);
pub static PasswordValidationMixedCaseCount: AtomicI32Value = AtomicI32Value::new((1) as i32);
pub static PasswordValidtaionNumberCount: AtomicI32Value = AtomicI32Value::new((1) as i32);
pub static PasswordValidationSpecialCharCount: AtomicI32Value = AtomicI32Value::new((1) as i32);
pub static EnableTTLJob: AtomicBoolValue = AtomicBoolValue::new(DefTiDBTTLJobEnable);
pub static TTLScanBatchSize: AtomicI64Value = AtomicI64Value::new((DefTiDBTTLScanBatchSize) as i64);
pub static TTLDeleteBatchSize: AtomicI64Value =
    AtomicI64Value::new((DefTiDBTTLDeleteBatchSize) as i64);
pub static TTLDeleteRateLimit: AtomicI64Value =
    AtomicI64Value::new((DefTiDBTTLDeleteRateLimit) as i64);
pub static TTLJobScheduleWindowStartTime: LazyLock<AtomicStringValue> = LazyLock::new(|| {
    AtomicStringValue::new(&mustParseTime(
        FullDayTimeFormat,
        DefTiDBTTLJobScheduleWindowStartTime,
    ))
});
pub static TTLJobScheduleWindowEndTime: LazyLock<AtomicStringValue> = LazyLock::new(|| {
    AtomicStringValue::new(&mustParseTime(
        FullDayTimeFormat,
        DefTiDBTTLJobScheduleWindowEndTime,
    ))
});
pub static TTLScanWorkerCount: AtomicI32Value =
    AtomicI32Value::new(DefTiDBTTLScanWorkerCount as i32);
pub static TTLDeleteWorkerCount: AtomicI32Value =
    AtomicI32Value::new(DefTiDBTTLDeleteWorkerCount as i32);
pub static PasswordHistory: AtomicI64Value = AtomicI64Value::new(DefPasswordReuseHistory);
pub static PasswordReuseInterval: AtomicI64Value = AtomicI64Value::new(DefPasswordReuseTime);
pub static IsSandBoxModeEnabled: AtomicBoolValue = AtomicBoolValue::new(false);
pub static MaxUserConnectionsValue: AtomicU32Value =
    AtomicU32Value::new(DefMaxUserConnections as u32);
pub static MaxPreparedStmtCountValue: AtomicI64Value = AtomicI64Value::new(DefMaxPreparedStmtCount);
pub static HistoricalStatsDuration: AtomicI64Value =
    AtomicI64Value::new(DefTiDBHistoricalStatsDuration);
pub static EnableHistoricalStatsForCapture: AtomicBoolValue =
    AtomicBoolValue::new(DefTiDBEnableHistoricalStatsForCapture);
pub static TTLRunningTasks: AtomicI32Value = AtomicI32Value::new(DefTiDBTTLRunningTasks as i32);
pub static EnableResourceControl: AtomicBoolValue = AtomicBoolValue::new(false);
pub static EnableResourceControlStrictMode: AtomicBoolValue = AtomicBoolValue::new(true);
pub static EnableCheckConstraint: AtomicBoolValue =
    AtomicBoolValue::new(DefTiDBEnableCheckConstraint);
pub static SkipMissingPartitionStats: AtomicBoolValue =
    AtomicBoolValue::new(DefTiDBSkipMissingPartitionStats);
pub static TiFlashEnablePipelineMode: AtomicBoolValue =
    AtomicBoolValue::new(DefTiDBEnableTiFlashPipelineMode);
pub static ServiceScope: LazyLock<AtomicStringValue> = LazyLock::new(|| AtomicStringValue::new(""));
pub static SchemaVersionCacheLimit: AtomicI64Value =
    AtomicI64Value::new(DefTiDBSchemaVersionCacheLimit);
pub static CloudStorageURI: LazyLock<AtomicStringValue> =
    LazyLock::new(|| AtomicStringValue::new(""));
pub static IgnoreInlistPlanDigest: AtomicBoolValue =
    AtomicBoolValue::new(DefTiDBIgnoreInlistPlanDigest);
pub static TxnEntrySizeLimit: AtomicU64Value = AtomicU64Value::new(DefTiDBTxnEntrySizeLimit as u64);
pub static SchemaCacheSize: AtomicU64Value = AtomicU64Value::new(DefTiDBSchemaCacheSize as u64);
pub static SchemaCacheSizeOriginText: LazyLock<AtomicStringValue> =
    LazyLock::new(|| AtomicStringValue::new(&DefTiDBSchemaCacheSize.to_string()));
pub static AccelerateUserCreationUpdate: AtomicBoolValue =
    AtomicBoolValue::new(DefTiDBAccelerateUserCreationUpdate);
pub static CircuitBreakerPDMetadataErrorRateThresholdRatio: AtomicF64Value =
    AtomicF64Value::new(0.0);
pub static AdvancerCheckPointLagLimit: AtomicI64Value =
    AtomicI64Value::new(DefTiDBAdvancerCheckPointLagLimit);
pub static EnableBindingUsage: AtomicBoolValue = AtomicBoolValue::new(DefTiDBEnableBindingUsage);
pub static ConnectAttrsSize: AtomicI64Value = AtomicI64Value::new(DefConnectAttrsSize);
pub static ConnectAttrsLongestSeen: AtomicI64Value = AtomicI64Value::new(0);
pub static ConnectAttrsLost: AtomicI64Value = AtomicI64Value::new(0);

/// 根据机器总内存推导 `tidb_server_memory_limit` 默认值。
///
/// 能读到总内存时返回 `"80%"`，否则 `"0"`（禁用）。
pub fn serverMemoryLimitDefaultValue() -> String {
    // 探测主机内存；不可用时退回 0，避免误设百分比。
    let system = sysinfo::System::new_all();
    if system.total_memory() != 0 {
        "80%"
    } else {
        "0"
    }
    .to_owned()
}

/// 按 Go 布局校验时间字符串；非法则 panic（用于静态默认值初始化）。
///
/// `LocalDayTimeFormat` 为 `HH:MM`；`FullDayTimeFormat` 含时区偏移。
pub fn mustParseTime(layout: &str, value: &str) -> String {
    // 将 Go 布局映射为 chrono 格式，并断言可解析。
    let format = match layout {
        LocalDayTimeFormat => "%H:%M",
        FullDayTimeFormat => "%H:%M %z",
        _ => panic!("unsupported time layout: {layout}"),
    };
    let valid = if layout == LocalDayTimeFormat {
        chrono::NaiveTime::parse_from_str(value, format).is_ok()
    } else {
        chrono::DateTime::parse_from_str(
            &format!("1970-01-01 {value}"),
            &format!("%Y-%m-%d {format}"),
        )
        .is_ok()
    };
    assert!(valid, "{value} is not in {layout} duration format");
    value.to_owned()
}

// Go const block：以下常量按原声明顺序逐项迁移。
// OptObjectiveModerate is a possible value and the default value for TiDBOptObjective.
// Please see comments of SessionVars.OptObjective for details.
// 优化目标（opt objective）与副本/调度策略字符串取值：
pub const OptObjectiveModerate: &str = "moderate";
// OptObjectiveDeterminate is a possible value for TiDBOptObjective.
pub const OptObjectiveDeterminate: &str = "determinate";

// ForcePreAggStr means 1st hashagg will be pre aggregated.
// AutoStr means TiFlash will decide which policy for 1st hashagg.
// ForceStreamingStr means 1st hashagg will for pass through all blocks.
// Go const block：以下常量按原声明顺序逐项迁移。
pub const ForcePreAggStr: &str = "force_preagg";
pub const AutoStr: &str = "auto";
pub const ForceStreamingStr: &str = "force_streaming";

// Go const block：以下常量按原声明顺序逐项迁移。
// AllReplicaStr is the string value of AllReplicas.
pub const AllReplicaStr: &str = "all_replicas";
// ClosestAdaptiveStr is the string value of ClosestAdaptive.
pub const ClosestAdaptiveStr: &str = "closest_adaptive";
// ClosestReplicasStr is the string value of ClosestReplicas.
pub const ClosestReplicasStr: &str = "closest_replicas";

// Go const block：以下常量按原声明顺序逐项迁移。
// DispatchPolicyRRStr is string value for DispatchPolicyRR.
pub const DispatchPolicyRRStr: &str = "round_robin";
// DispatchPolicyConsistentHashStr is string value for DispatchPolicyConsistentHash.
pub const DispatchPolicyConsistentHashStr: &str = "consistent_hash";
// DispatchPolicyInvalidStr is string value for DispatchPolicyInvalid.
pub const DispatchPolicyInvalidStr: &str = "invalid";

// ConcurrencyUnset means the value the of the concurrency related variable is unset.
pub const ConcurrencyUnset: i64 = -1;

// ExchangeCompressionMode means the compress method used in exchange operator
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Exchange 算子数据压缩模式（MPP 节点间交换）。
///
/// 内层 `i32` 与 tipb 压缩枚举对齐。
pub struct ExchangeCompressionMode(i32);

// Go const block：以下常量按原声明顺序逐项迁移。
// ExchangeCompressionModeNONE indicates no compression
pub const ExchangeCompressionModeNONE: ExchangeCompressionMode = ExchangeCompressionMode(0);
// ExchangeCompressionModeFast indicates fast compression/decompression speed, compression ratio is lower than HC mode
pub const ExchangeCompressionModeFast: ExchangeCompressionMode = ExchangeCompressionMode(1);
// ExchangeCompressionModeHC indicates high compression (HC) ratio mode
pub const ExchangeCompressionModeHC: ExchangeCompressionMode = ExchangeCompressionMode(2);
// ExchangeCompressionModeUnspecified indicates unspecified compress method, let TiDB choose one
pub const ExchangeCompressionModeUnspecified: ExchangeCompressionMode = ExchangeCompressionMode(3);

// RecommendedExchangeCompressionMode indicates recommended compression mode
pub const RecommendedExchangeCompressionMode: ExchangeCompressionMode = ExchangeCompressionModeFast;

pub const exchangeCompressionModeUnspecifiedName: &str = "UNSPECIFIED";

// Name returns the name of ExchangeCompressionMode
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 映射到 tipb 的压缩模式枚举。
///
/// - `None`：不压缩；`Fast`：偏速度；`HighCompression`：偏压缩比。
pub enum CompressionMode {
    None,
    Fast,
    HighCompression,
}

impl ExchangeCompressionMode {
    /// 返回压缩模式的规范名称字符串。
    pub fn Name(self) -> &'static str {
        match self {
            ExchangeCompressionModeUnspecified => exchangeCompressionModeUnspecifiedName,
            ExchangeCompressionModeFast => "FAST",
            ExchangeCompressionModeHC => "HIGH_COMPRESSION",
            _ => "NONE",
        }
    }

    // ToExchangeCompressionMode returns the ExchangeCompressionMode from name
    /// 转换为 tipb `CompressionMode`。
    pub fn ToTipbCompressionMode(self) -> CompressionMode {
        match self {
            ExchangeCompressionModeFast => CompressionMode::Fast,
            ExchangeCompressionModeHC => CompressionMode::HighCompression,
            _ => CompressionMode::None,
        }
    }
}

/// 由名称解析压缩模式；第二返回值表示是否识别成功。
pub fn ToExchangeCompressionMode(name: &str) -> (ExchangeCompressionMode, bool) {
    match name.to_ascii_uppercase().as_str() {
        "NONE" => (ExchangeCompressionModeNONE, true),
        "FAST" => (ExchangeCompressionModeFast, true),
        "HIGH_COMPRESSION" => (ExchangeCompressionModeHC, true),
        exchangeCompressionModeUnspecifiedName => (ExchangeCompressionModeUnspecified, true),
        _ => (ExchangeCompressionModeNONE, false),
    }
}

// ScopeFlag is for system variable whether can be changed in global/session dynamically or not.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 系统变量作用域标志位（可组合 SESSION/GLOBAL/INSTANCE）。
///
/// 作用域决定 SET 能否在会话或全局动态修改该变量。
pub struct ScopeFlag(u8);

/// 允许用 `|` 组合多个作用域标志。
impl BitOr for ScopeFlag {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

// TypeFlag is the SysVar type, which doesn't exactly match MySQL types.
/// SysVar 值类型标志（不完全等同 MySQL 类型）。
pub type TypeFlag = u8;

// Go const block：以下常量按原声明顺序逐项迁移。
// ScopeNone means the system variable can not be changed dynamically.
// 作用域与类型 iota 常量（对齐 Go）：
pub const ScopeNone: ScopeFlag = ScopeFlag(0);
// ScopeGlobal means the system variable can be changed globally.
pub const ScopeGlobal: ScopeFlag = ScopeFlag(1 << 0);
// ScopeSession means the system variable can only be changed in current session.
pub const ScopeSession: ScopeFlag = ScopeFlag(1 << 1);
// ScopeInstance means it is similar to global but doesn't propagate to other TiDB servers.
pub const ScopeInstance: ScopeFlag = ScopeFlag(1 << 2);

// TypeStr is the default
pub const TypeStr: TypeFlag = 0;
// TypeBool for boolean
pub const TypeBool: TypeFlag = 1;
// TypeInt for integer
pub const TypeInt: TypeFlag = 2;
// TypeEnum for Enum
pub const TypeEnum: TypeFlag = 3;
// TypeFloat for Double
pub const TypeFloat: TypeFlag = 4;
// TypeUnsigned for Unsigned integer
pub const TypeUnsigned: TypeFlag = 5;
// TypeTime for time of day (a TiDB extension)
pub const TypeTime: TypeFlag = 6;
// TypeDuration for a golang duration (a TiDB extension)
pub const TypeDuration: TypeFlag = 7;

// On is the canonical string for ON
pub const On: &str = "ON";
// Off is the canonical string for OFF
pub const Off: &str = "OFF";
// Warn means return warnings
pub const Warn: &str = "WARN";
// IntOnly means enable for int type
pub const IntOnly: &str = "INT_ONLY";
// Marker is a special log redact behavior
pub const Marker: &str = "MARKER";

// AssertionStrictStr is a choice of variable TiDBTxnAssertionLevel that means full assertions should be performed,
// even if the performance might be slowed down.
pub const AssertionStrictStr: &str = "STRICT";
// AssertionFastStr is a choice of variable TiDBTxnAssertionLevel that means assertions that doesn't affect
// performance should be performed.
pub const AssertionFastStr: &str = "FAST";
// AssertionOffStr is a choice of variable TiDBTxnAssertionLevel that means no assertion should be performed.
pub const AssertionOffStr: &str = "OFF";
// OOMActionCancel constants represents the valid action configurations for OOMAction "CANCEL".
pub const OOMActionCancel: &str = "CANCEL";
// OOMActionLog constants represents the valid action configurations for OOMAction "LOG".
pub const OOMActionLog: &str = "LOG";

// TSOClientRPCModeDefault is a choice of variable TiDBTSOClientRPCMode. In this mode, the TSO client sends batched
// TSO requests serially.
pub const TSOClientRPCModeDefault: &str = "DEFAULT";
// TSOClientRPCModeParallel is a choice of variable TiDBTSOClientRPCMode. In this mode, the TSO client tries to
// keep approximately 2 batched TSO requests running in parallel. This option tries to reduce the batch-waiting time
// by half, at the expense of about twice the amount of TSO RPC calls.
pub const TSOClientRPCModeParallel: &str = "PARALLEL";
// TSOClientRPCModeParallelFast is a choice of variable TiDBTSOClientRPCMode. In this mode, the TSO client tries to
// keep approximately 4 batched TSO requests running in parallel. This option tries to reduce the batch-waiting time
// by 3/4, at the expense of about 4 times the amount of TSO RPC calls.
pub const TSOClientRPCModeParallelFast: &str = "PARALLEL-FAST";

// StrategyStandard is a choice of variable TiDBPipelinedDmlResourcePolicy,
// the best performance policy
pub const StrategyStandard: &str = "standard";
// StrategyConservative is a choice of variable TiDBPipelinedDmlResourcePolicy,
// a rather conservative policy
pub const StrategyConservative: &str = "conservative";
// StrategyCustom is a choice of variable TiDBPipelinedDmlResourcePolicy,
pub const StrategyCustom: &str = "custom";

// IndexLookUpPushDownPolicyHintOnly indicates only use the hint to decide whether to push down the index lookup or not.
pub const IndexLookUpPushDownPolicyHintOnly: &str = "hint-only";
// IndexLookUpPushDownPolicyAffinityForce indicates to force push down the index lookup for table with affinity options.
pub const IndexLookUpPushDownPolicyAffinityForce: &str = "affinity-force";
// IndexLookUpPushDownPolicyForce indicates to force push down the index lookup for all tables.
pub const IndexLookUpPushDownPolicyForce: &str = "force";

// Global config name list.
// Go const block：以下常量按原声明顺序逐项迁移。
pub const GlobalConfigEnableTopSQL: &str = "enable_resource_metering";
pub const GlobalConfigSourceID: &str = "source_id";

impl ScopeFlag {
    /// 将作用域标志格式化为 `NONE` 或逗号分隔的 SESSION/GLOBAL/INSTANCE。
    pub fn String(self) -> String {
        if self == ScopeNone {
            return "NONE".to_owned();
        }
        let mut scopes = Vec::with_capacity(3);
        if self.0 & ScopeSession.0 != 0 {
            scopes.push("SESSION");
        }
        if self.0 & ScopeGlobal.0 != 0 {
            scopes.push("GLOBAL");
        }
        if self.0 & ScopeInstance.0 != 0 {
            scopes.push("INSTANCE");
        }
        scopes.join(",")
    }
}

// ClusteredIndexDefMode controls the default clustered property for primary key.
/// 主键默认是否使用聚簇索引（clustered index）的模式。
///
/// 聚簇索引：主键与行数据同组织存储，减少回表。
pub type ClusteredIndexDefMode = i32;

// Go const block：以下常量按原声明顺序逐项迁移。
// ClusteredIndexDefModeIntOnly indicates only single int primary key will default be clustered.
pub const ClusteredIndexDefModeIntOnly: ClusteredIndexDefMode = 0;
// ClusteredIndexDefModeOn indicates primary key will default be clustered.
pub const ClusteredIndexDefModeOn: ClusteredIndexDefMode = 1;
// ClusteredIndexDefModeOff indicates primary key will default be non-clustered.
pub const ClusteredIndexDefModeOff: ClusteredIndexDefMode = 2;

// TiDBOptEnableClustered converts enable clustered options to ClusteredIndexDefMode.
/// 将 `tidb_enable_clustered_index` 选项字符串转为模式枚举。
pub fn TiDBOptEnableClustered(opt: &str) -> ClusteredIndexDefMode {
    match opt {
        On => ClusteredIndexDefModeOn,
        Off => ClusteredIndexDefModeOff,
        _ => ClusteredIndexDefModeIntOnly,
    }
}

// Go const block：以下常量按原声明顺序逐项迁移。
// ScatterOff means default, will not scatter region
pub const ScatterOff: &str = "";
// ScatterTable means scatter region at table level
pub const ScatterTable: &str = "table";
// ScatterGlobal means scatter region at global level
pub const ScatterGlobal: &str = "global";

// Go const block：以下常量按原声明顺序逐项迁移。
// PlacementModeStrict indicates all placement operations should be checked strictly in ddl
pub const PlacementModeStrict: &str = "STRICT";
// PlacementModeIgnore indicates ignore all placement operations in ddl
pub const PlacementModeIgnore: &str = "IGNORE";

// Go const block：以下常量按原声明顺序逐项迁移。
// LocalDayTimeFormat is the local format of analyze start time and end time.
pub const LocalDayTimeFormat: &str = "15:04";
// FullDayTimeFormat is the full format of analyze start time and end time.
pub const FullDayTimeFormat: &str = "15:04 -0700";

// SetDDLReorgWorkerCounter sets DDLReorgWorkerCounter count.
// Sysvar validation enforces the range to already be correct.
/// 设置 DDL reorg（重组）工作线程数；调用前应由 SysVar 校验范围。
pub fn SetDDLReorgWorkerCounter(cnt: i32) {
    DDLReorgWorkerCounter.store(cnt, Ordering::SeqCst);
}

// GetDDLReorgWorkerCounter gets DDLReorgWorkerCounter.
/// 读取 DDL reorg 工作线程数。
pub fn GetDDLReorgWorkerCounter() -> i32 {
    DDLReorgWorkerCounter.load(Ordering::SeqCst)
}

// SetDDLFlashbackConcurrency sets DDLFlashbackConcurrency count.
// Sysvar validation enforces the range to already be correct.
/// 设置 DDL flashback 并发度。
pub fn SetDDLFlashbackConcurrency(cnt: i32) {
    DDLFlashbackConcurrency.store(cnt, Ordering::SeqCst);
}

// GetDDLFlashbackConcurrency gets DDLFlashbackConcurrency count.
/// 读取 DDL flashback 并发度。
pub fn GetDDLFlashbackConcurrency() -> i32 {
    DDLFlashbackConcurrency.load(Ordering::SeqCst)
}

// SetDDLReorgBatchSize sets DDLReorgBatchSize size.
// Sysvar validation enforces the range to already be correct.
/// 设置 DDL reorg 批大小。
pub fn SetDDLReorgBatchSize(cnt: i32) {
    DDLReorgBatchSize.store(cnt, Ordering::SeqCst);
}

// GetDDLReorgBatchSize gets DDLReorgBatchSize.
/// 读取 DDL reorg 批大小。
pub fn GetDDLReorgBatchSize() -> i32 {
    DDLReorgBatchSize.load(Ordering::SeqCst)
}

// SetDDLErrorCountLimit sets ddlErrorCountlimit size.
/// 设置 DDL 错误次数上限。
pub fn SetDDLErrorCountLimit(cnt: i64) {
    DDLErrorCountLimit.store(cnt, Ordering::SeqCst);
}

// GetDDLErrorCountLimit gets ddlErrorCountlimit size.
/// 读取 DDL 错误次数上限。
pub fn GetDDLErrorCountLimit() -> i64 {
    DDLErrorCountLimit.load(Ordering::SeqCst)
}

// SetDDLReorgRowFormat sets DDLReorgRowFormat version.
/// 设置 DDL reorg 行格式版本。
pub fn SetDDLReorgRowFormat(format: i64) {
    DDLReorgRowFormat.store(format, Ordering::SeqCst);
}

// GetDDLReorgRowFormat gets DDLReorgRowFormat version.
/// 读取 DDL reorg 行格式版本。
pub fn GetDDLReorgRowFormat() -> i64 {
    DDLReorgRowFormat.load(Ordering::SeqCst)
}

// SetMaxDeltaSchemaCount sets MaxDeltaSchemaCount size.
/// 设置增量 schema 变更缓存条数上限。
pub fn SetMaxDeltaSchemaCount(cnt: i64) {
    MaxDeltaSchemaCount.store(cnt, Ordering::SeqCst);
}

// GetMaxDeltaSchemaCount gets MaxDeltaSchemaCount size.
/// 读取增量 schema 变更缓存条数上限。
pub fn GetMaxDeltaSchemaCount() -> i64 {
    MaxDeltaSchemaCount.load(Ordering::SeqCst)
}

// IsMDLEnabled returns if MDL is enabled.
/// 是否启用元数据锁（MDL，Metadata Lock）。
///
/// MDL 避免 DDL 与 DML 并发导致 “Information schema is changed”。
/// NextGen 内核始终返回 true。
pub fn IsMDLEnabled() -> bool {
    // NextGen：强制开启 MDL，忽略测试中的 SetEnableMDL(false)。
    if kerneltype::IsNextGen() {
        // MDL is very useful to avoid the 'Information schema is changed' error,
        // in next-gen TiDB, MDL is always enabled, as we don't have the compatibility
        // debts.
        // some tests might call SetEnableMDL(false) to disable MDL, but it is not
        // expected in nextgen, we use this branch to ensure MDL is always enabled,
        // even in test.
        return true;
    }
    enableMDL.load(Ordering::SeqCst)
}

// SetEnableMDL sets the MDL enable status.
/// 设置 Classic 内核下的 MDL 开关（NextGen 下 `IsMDLEnabled` 仍恒为 true）。
pub fn SetEnableMDL(enabled: bool) {
    enableMDL.store(enabled, Ordering::SeqCst);
}

// GetDefaultTxnAssertionLevel returns the default assertion level based on kernel type.
// For next-gen, we use strict assertion level to prevent correctness risks.
// For classic, we use off to maintain compatibility.
/// 按内核类型返回默认事务断言级别。
///
/// 事务断言（txn assertion）：提交时校验读写键是否被并发修改；
/// NextGen 用 STRICT 降低正确性风险，Classic 默认 OFF 保持兼容。
pub fn GetDefaultTxnAssertionLevel() -> &'static str {
    if kerneltype::IsNextGen() {
        return AssertionStrictStr;
    }
    AssertionOffStr
}

/// Controls Info-level connection login and logout events.
pub const TiDBEnableConnectionEventLog: &str = "tidb_enable_connection_event_log";
pub const DefTiDBEnableConnectionEventLog: bool = false;
pub static EnableConnectionEventLog: AtomicBoolValue =
    AtomicBoolValue::new(DefTiDBEnableConnectionEventLog);

pub const TiDBExpEmbedJinaAIAPIKey: &str = "tidb_exp_embed_jina_ai_api_key";
pub const TiDBExpEmbedOpenAIAPIKey: &str = "tidb_exp_embed_openai_api_key";
pub const TiDBExpEmbedOpenAIAPIBase: &str = "tidb_exp_embed_openai_api_base";
pub const TiDBExpEmbedCohereAPIKey: &str = "tidb_exp_embed_cohere_api_key";
pub const TiDBExpEmbedHuggingFaceAPIKey: &str = "tidb_exp_embed_huggingface_api_key";
pub const TiDBExpEmbedNvidiaNIMAPIKey: &str = "tidb_exp_embed_nvidia_nim_api_key";
pub const TiDBExpEmbedGeminiAPIKey: &str = "tidb_exp_embed_gemini_api_key";
pub const DefTiDBEmbedOpenAIAPIBase: &str = "https://api.openai.com/v1";
pub static EmbedJinaAPIKey: LazyLock<AtomicStringValue> =
    LazyLock::new(|| AtomicStringValue::new(""));
pub static EmbedOpenAIAPIKey: LazyLock<AtomicStringValue> =
    LazyLock::new(|| AtomicStringValue::new(""));
pub static EmbedOpenAIAPIBase: LazyLock<AtomicStringValue> =
    LazyLock::new(|| AtomicStringValue::new(""));
pub static EmbedCohereAPIKey: LazyLock<AtomicStringValue> =
    LazyLock::new(|| AtomicStringValue::new(""));
pub static EmbedHuggingFaceAPIKey: LazyLock<AtomicStringValue> =
    LazyLock::new(|| AtomicStringValue::new(""));
pub static EmbedNvidiaNIMAPIKey: LazyLock<AtomicStringValue> =
    LazyLock::new(|| AtomicStringValue::new(""));
pub static EmbedGeminiAPIKey: LazyLock<AtomicStringValue> =
    LazyLock::new(|| AtomicStringValue::new(""));
pub static EmbeddingConfigVersion: AtomicU64Value = AtomicU64Value::new(0);
impl AtomicStringValue {
    pub fn Swap(&self, value: impl Into<String>) -> String {
        std::mem::replace(
            &mut *self.0.write().expect("atomic string poisoned"),
            value.into(),
        )
    }
}
impl AtomicU64Value {
    pub fn Inc(&self) -> u64 {
        self.0.fetch_add(1, Ordering::SeqCst) + 1
    }
}

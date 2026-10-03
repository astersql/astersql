// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

//! Concrete session lifecycle, protocol state, and test-runtime boundaries.

use super::transaction::RuntimeSavepoint;
use super::*;

/// ConcreteSession 可变状态：事务、预编译、变量与 DML 报告等。

#[derive(Clone, Copy, Debug)]
pub(super) enum RuntimeTimeZone {
    Named(Tz),
    Fixed(FixedOffset),
}

impl RuntimeTimeZone {
    pub(super) fn parse(requested: &str) -> Option<Self> {
        if requested.eq_ignore_ascii_case("SYSTEM") {
            return Some(Self::Named(chrono_tz::UTC));
        }
        if let Ok(time_zone) = requested.parse::<Tz>() {
            return Some(Self::Named(time_zone));
        }
        let (sign, offset) = requested.split_at_checked(1)?;
        if !matches!(sign, "+" | "-") {
            return None;
        }
        let (hours, minutes) = offset.split_once(':')?;
        let hours = hours.parse::<u32>().ok()?;
        let minutes = minutes.parse::<u32>().ok()?;
        let maximum_minutes = if sign == "-" { 12 * 60 + 59 } else { 14 * 60 };
        let offset_minutes = hours.checked_mul(60)?.checked_add(minutes)?;
        if minutes >= 60 || offset_minutes > maximum_minutes {
            return None;
        }
        let seconds =
            i32::try_from(offset_minutes.checked_mul(60)?).ok()? * if sign == "-" { -1 } else { 1 };
        FixedOffset::east_opt(seconds).map(Self::Fixed)
    }
}

#[derive(Clone)]
pub(super) struct SessionWarning {
    pub(super) level: &'static str,
    pub(super) code: u16,
    pub(super) message: String,
}

impl SessionWarning {
    pub(super) fn warning(message: String) -> Self {
        Self::warning_with_code(1105, message)
    }

    pub(super) fn warning_with_code(code: u16, message: String) -> Self {
        Self {
            level: "Warning",
            code,
            message,
        }
    }

    pub(super) fn note_with_code(code: u16, message: String) -> Self {
        Self {
            level: "Note",
            code,
            message,
        }
    }

    pub(super) fn error_with_code(code: u16, message: String) -> Self {
        Self {
            level: "Error",
            code,
            message,
        }
    }

    pub(super) fn note(message: String) -> Self {
        Self::note_with_code(1105, message)
    }
}

#[derive(Clone)]
pub(super) struct RuntimeForeignKeyDeleteCascade {
    pub parent: astersql_meta_model::TableInfo,
    pub deleted: Vec<HashMap<String, Option<String>>>,
}

pub(super) struct SessionState {
    /// Session-local schema catalog used by CREATE DATABASE/USE.
    pub(super) databases: BTreeSet<String>,
    /// Local temporary tables shadow the Domain catalog only for this session.
    /// Their independently allocated physical IDs keep their KV rows private.
    pub(super) local_temporary_tables: HashMap<(String, String), astersql_meta_model::TableInfo>,
    /// Local table ID streams do not enter Domain's shared allocators.
    pub(super) local_temporary_auto_ids: HashMap<(i64, u8), u64>,
    /// Global temporary tables written by the active transaction. Their rows
    /// are cleared after a successful `ON COMMIT DELETE ROWS` commit.
    pub(super) global_temporary_tables_in_transaction: HashMap<i64, astersql_meta_model::TableInfo>,
    /// The selected schema. Kept here because SQL execution only has `&self`.
    pub(super) current_database: String,
    /// Go `SessionVars.InspectionTableCache`: enabled inspection tables are
    /// materialized once per session and reused by subsequent statements.
    pub(super) inspection_table_cache:
        Option<HashMap<String, Vec<HashMap<String, Option<String>>>>>,
    pub(super) prepared_database_override: Option<String>,
    pub(super) transaction: Option<Box<dyn kv::Transaction>>,
    /// Immutable InfoSchema captured when the active transaction starts.
    pub(super) transaction_info_schema: Option<SchemaRef>,
    /// Non-temporary physical tables written or locked by the transaction.
    pub(super) transaction_related_table_ids: HashSet<i64>,
    pub(super) transaction_locking_table_ids: HashSet<i64>,
    pub(super) transaction_pessimistic: bool,
    pub(super) transaction_explicit_optimistic: bool,
    pub(super) transaction_isolation: String,
    pub(super) transaction_isolation_one_shot: Option<String>,
    pub(super) transaction_isolation_restore: Option<String>,
    pub(super) txn_mode: String,
    pub(super) autocommit: bool,
    pub(super) enable_async_commit: bool,
    pub(super) guarantee_linearizability: bool,
    pub(super) enable_1pc: bool,
    pub(super) tikv_client_read_timeout_ms: u64,
    pub(super) replica_read: String,
    pub(super) last_replica_read_request: Option<RuntimeReplicaReadRequest>,
    pub(super) last_select_request: Option<RuntimeSelectRequest>,
    pub(super) last_import_plan_path: Option<String>,
    pub(super) enable_paging: bool,
    pub(super) min_paging_size: usize,
    pub(super) innodb_lock_wait_timeout_secs: u64,
    pub(super) max_execution_time_ms: u64,
    pub(super) low_resolution_tso: bool,
    pub(super) fair_locking: bool,
    pub(super) enable_noop_functions: bool,
    pub(super) global_enable_noop_functions: bool,
    pub(super) enable_shared_lock_promotion: bool,
    pub(super) foreign_key_checks: bool,
    pub(super) allow_auto_random_explicit_insert: bool,
    /// Session-local row encoder switch, matching Go SessionVars.RowEncoder.Enable.
    pub(super) row_encoder_enabled: bool,
    pub(super) foreign_key_check_in_shared_lock: bool,
    pub(super) constraint_check_in_place: bool,
    pub(super) constraint_check_in_place_pessimistic: bool,
    pub(super) savepoints: Vec<RuntimeSavepoint>,
    /// TTL rows executed after the first active savepoint. They are published
    /// only when the transaction finishes so rollback-to can restore the Go
    /// `TxnCtx.InsertTTLRowsCount` snapshot before touching the counter.
    pub(super) pending_ttl_insert_rows: usize,
    pub(super) held_row_locks: HashSet<RuntimeRowLockKey>,
    pub(super) optimistic_fk_check_keys: HashSet<Vec<u8>>,
    pub(super) adapter_dml_defer_fk_locks: bool,
    pub(super) adapter_dml_statement_staged: bool,
    pub(super) pending_fk_delete_cascades: Vec<RuntimeForeignKeyDeleteCascade>,
    pub(super) runaway_checker: Option<kv::resourcegroup::SharedRunawayChecker>,
    pub(super) runaway_resource_group_name: Option<String>,
    pub(super) txn_mem_buffer_keys: u64,
    pub(super) txn_mem_buffer_bytes: u64,
    pub(super) transaction_read_epoch: u64,
    pub(super) transaction_write_keys: HashSet<RuntimeRowLockKey>,
    /// Optimistic INSERT uniqueness violations that TiKV reports during
    /// prewrite. The key is the conflicting physical row from the transaction
    /// snapshot; keeping it transaction-scoped lets DELETE/savepoint rollback
    /// resolve the violation before commit.
    pub(super) deferred_optimistic_constraint_errors: BTreeMap<Vec<u8>, String>,
    pub(super) optimistic_for_update_keys: HashSet<RuntimeRowLockKey>,
    pub(super) transaction_conflict_context: Option<(String, String)>,
    pub(super) transaction_scope: String,
    pub(super) transaction_store_labels: HashMap<String, String>,
    pub(super) txn_entry_size_limit: usize,
    /// StartTS allocated while executing the current statement. Autocommit
    /// transactions are not retained in `transaction`, but General Log still
    /// has to expose their real start timestamp after execution completes.
    pub(super) statement_txn_start_ts: u64,
    pub(super) last_commit_ts: u64,
    pub(super) last_observed_store_ts: u64,
    pub(super) pessimistic_lock_started: Option<Instant>,
    pub(super) pessimistic_lock_ttl: Duration,
    pub(super) pessimistic_lock_ttl_expired_for_test: bool,
    pub(super) prepared: HashMap<u64, String>,
    pub(super) protocol_prepared_planned: HashMap<u64, bool>,
    pub(super) prepared_planned: HashMap<u64, PreparedPlannedKVSelect>,
    pub(super) next_prepared_id: u64,
    pub(super) last_plan_from_cache: bool,
    pub(super) plan_cache_generation: u64,
    pub(super) process_plan_snapshot: Option<ProcessPlanSnapshot>,
    pub(super) last_statement_hints_for_test: (i64, u64),
    pub(super) last_dml_report: Option<crate::dml_runtime::DmlExecutionReport>,
    /// MySQL OK-packet message produced by the latest statement.
    pub(super) last_message: String,
    /// SQL text recorded in the Go `sessionctx.QueryString` slot.
    pub(super) last_query_string: String,
    pub(super) last_query_info: String,
    /// MySQL protocol write time supplied when the server finishes the statement response.
    pub(super) last_write_sql_resp_duration: Duration,
    pub(super) defer_protocol_finish: bool,
    pub(super) pending_protocol_slow_logs: VecDeque<(
        bool,
        astersql_sessionctx_variable::slow_log::SlowQueryLogItems,
    )>,
    pub(super) slow_log_threshold_ms: u64,
    /// Session-visible rows backing INFORMATION_SCHEMA.SLOW_QUERY.
    pub(super) slow_query_plans: Vec<(String, String)>,
    /// Session-visible rows backing INFORMATION_SCHEMA.STATEMENTS_SUMMARY.
    pub(super) statement_summary_plans: Vec<(String, String)>,
    pub(super) last_explain_for_rows: Option<Vec<Vec<String>>>,
    /// Client capability flags negotiated for this session.
    pub(super) client_capability: u32,
    /// Persistent INFORMATION-function state. Unlike the protocol DML report,
    /// LAST_INSERT_ID survives statements that do not allocate an ID.
    pub(super) info_last_insert_id: u64,
    /// Auto-increment IDs replayed by a retrying statement, in allocation order.
    pub(super) retry_auto_increment_ids: VecDeque<u64>,
    /// Number of rows produced by the immediately preceding result set.
    pub(super) info_found_rows: usize,
    /// MySQL ROW_COUNT(): affected rows for DML, -1 for result-set statements.
    pub(super) info_row_count: i64,
    pub(super) last_autocommit_retry_attempts: usize,
    /// Session-local deterministic equivalent of the commit/TSO failpoint pair.
    pub(super) next_autocommit_retry_tso_failures: Option<usize>,
    pub(super) next_dml_commit_error: Option<String>,
    pub(super) pending_stats_deltas:
        HashMap<i64, (astersql_statistics_handle::StatsTableKey, i64, i64)>,
    pub(super) snapshot_catalog_version: Option<u64>,
    /// KV snapshot selected by `tidb_snapshot`.
    pub(super) snapshot_read_ts: Option<u64>,
    /// One-shot snapshot selected by `SET TRANSACTION ... AS OF`.
    pub(super) pending_stale_read_ts: Option<u64>,
    /// Snapshot pinned by an active read-only stale transaction.
    pub(super) transaction_stale_read_ts: Option<u64>,
    /// Session stale snapshot captured when `tidb_read_staleness` is enabled.
    pub(super) session_stale_read_ts: Option<u64>,
    /// Absolute value of the negative `tidb_read_staleness` interval. The
    /// selected snapshot is refreshed for every statement, as in TiDB.
    pub(super) session_read_staleness_seconds: Option<u64>,
    pub(super) enable_external_ts_read: bool,
    pub(super) external_read_ts: u64,
    /// Schema catalog version observed at a KV TSO returned to this session.
    pub(super) tso_catalog_versions: BTreeMap<u64, u64>,
    /// Database names visible at each TSO observation, including empty schemas.
    pub(super) tso_database_names: BTreeMap<u64, BTreeSet<String>>,
    /// Wall-clock observation points for mock-store TSOs. The mock oracle is
    /// logical-only, so relative staleness resolves through this timeline.
    pub(super) tso_wall_times: Vec<(SystemTime, u64)>,
    pub(super) analyze_concurrency: usize,
    /// Temporary remote scan concurrency used only while restricted ANALYZE runs.
    pub(super) restricted_analyze_scan_concurrency: Option<i32>,
    pub(super) txn_write_throughput_sli: astersql_util_sli::TxnWriteThroughputSLI,
    pub(super) analyze_version: i32,
    /// Go `SessionVars.MemQuotaAnalyze`; negative means unlimited.
    pub(super) analyze_memory_quota: i64,
    /// Go `SessionVars.SQLMode`, kept as the textual mode list.
    pub(super) sql_mode: String,
    pub(super) auto_increment_increment: u64,
    pub(super) auto_increment_offset: u64,
    pub(super) timestamp_override: Option<f64>,
    pub(super) max_connections: u64,
    pub(super) tx_read_only: bool,
    pub(super) global_tx_read_only: bool,
    pub(super) sql_require_primary_key: bool,
    /// Empty means MySQL result character-set conversion is disabled.
    pub(super) character_set_results: String,
    pub(super) dynamic_partition_prune: bool,
    pub(super) isolation_read_engines: String,
    /// Go `SessionVars.EnableRedactLog`: OFF, ON, or MARKER for EXPLAIN/log rendering.
    pub(super) redact_log: String,
    pub(super) allow_mpp: bool,
    pub(super) allow_tiflash_cop: bool,
    pub(super) enforce_mpp: bool,
    /// Go `SessionVars.CopTiFlashConcurrencyFactor` used by MPP cost planning.
    pub(super) tiflash_concurrency_factor: f64,
    /// Go `SessionVars.TiFlashFastScan`, which changes min/max keep-order plans.
    pub(super) tiflash_fastscan: bool,
    /// Go `SessionVars.TiFlashComputeDispatchPolicy`.
    pub(super) tiflash_compute_dispatch_policy:
        astersql_sessionctx_variable::tiflashcompute::DispatchPolicy,
    pub(super) mpp_disabled_explicitly: bool,
    pub(super) stats_load_sync_wait: i64,
    pub(super) opt_index_prune_threshold: i64,
    /// Session-local correlation exponent used by LIMIT/TopN estimation.
    pub(super) correlation_exp_factor: i64,
    /// Session-local ratio used by the ReproHashJoinIssue CBO regression.
    pub(super) repro_hash_join_max_scan_rows_ratio: f64,
    /// Session-local `tidb_opt_fix_control` text used by relational EXPLAIN.
    pub(super) optimizer_fix_control: String,
    pub(super) current_warnings: Vec<SessionWarning>,
    pub(super) last_warnings: Vec<SessionWarning>,
    /// Go `SessionVars.InRestrictedSQL`, set for internal system sessions.
    pub(super) in_restricted_sql: bool,
    /// Text-protocol `PREPARE name FROM '...'` statements, keyed by name.
    pub(super) prepared_by_name: HashMap<String, NamedPreparedStatement>,
    /// Go `SessionVars.UserVars`, assigned by `SET @name = value`.
    pub(super) user_variables: HashMap<String, String>,
    /// User variables whose current datum is a SQL string. The text map alone
    /// cannot distinguish `SET @v = 0` from `SET @v = '0'` during EXECUTE.
    pub(super) string_user_variables: HashSet<String>,
    /// Go `tidb_enable_prepared_plan_cache`.
    pub(super) prepared_plan_cache: bool,
    /// Go `tidb_enable_non_prepared_plan_cache` and its session-local normalized keys.
    pub(super) non_prepared_plan_cache: bool,
    pub(super) non_prepared_plan_cache_keys: HashSet<String>,
    /// Go `tidb_mem_arbitrator_wait_averse`.
    pub(super) mem_arbitrator_wait_averse: String,
    /// Session-local mock of the GLOBAL memory OOM action.
    pub(super) global_mem_oom_action: String,
    /// Go `tidb_mem_arbitrator_query_reserved`.
    pub(super) mem_arbitrator_query_reserved: i64,
    /// 当前语句 SET_VAR 覆盖后的 query-reserved 值。
    pub(super) statement_mem_arbitrator_query_reserved: Option<i64>,
    /// Go `tidb_enable_dist_task`.
    pub(super) dist_task_enabled: bool,
    /// Go `tidb_ddl_enable_fast_reorg`.
    pub(super) ddl_fast_reorg_enabled: bool,
    /// Go `tidb_stats_update_during_ddl`.
    pub(super) ddl_analyze_enabled: bool,
    /// Session defaults consumed by CREATE TABLE metadata construction.
    pub(super) clustered_index_def_mode: astersql_sessionctx_vardef::ClusteredIndexDefMode,
    pub(super) shard_row_id_bits: u64,
    pub(super) pre_split_regions: u64,
    /// Session-scoped DDL scatter mode inherited from the Domain at connect.
    pub(super) scatter_region: String,
    /// Set while replaying a cached plan so Go's `CollectPredicateColumnsPoint`
    /// stays skipped: a cache hit runs no logical optimization.
    pub(super) skip_predicate_collection: bool,
    /// Rows staged in `mysql.expr_pushdown_blacklist`.
    pub(super) expr_pushdown_blacklist: BTreeSet<(String, String, String)>,
    /// Snapshot activated by `ADMIN RELOAD EXPR_PUSHDOWN_BLACKLIST`.
    pub(super) loaded_expr_pushdown_blacklist: BTreeSet<(String, String, String)>,
    /// Original prepared SQL used for transaction digest observation.
    pub(super) observation_sql_override: Option<String>,
    pub(super) current_statement_digest: String,
    pub(super) statement_not_fill_cache: bool,
    pub(super) use_invisible_indexes: bool,
    pub(super) invisible_index_explain_seen: bool,
    pub(super) pessimistic_pause_observed_statement: bool,
    pub(super) current_statement_is_stale: bool,
    pub(super) last_statement_was_stale: bool,
    /// Statement-stable NOW used by repeated AS OF expressions.
    pub(super) current_stale_now_ts: Option<u64>,
    pub(super) stale_statement_observation_error: Option<String>,
}

/// A text-protocol prepared statement. `planned` records whether a previous
/// EXECUTE already produced a plan that the plan cache can reuse.
#[derive(Clone, Debug, Default)]
/// 具名预编译语句：SQL 文本与参数个数。
pub(super) struct NamedPreparedStatement {
    pub(super) sql: String,
    pub(super) database: String,
    pub(super) planned: bool,
    /// Catalog version of the plan recorded by the last successful EXECUTE.
    pub(super) planned_catalog_version: Option<u64>,
    /// Parameter-shape class used by point/range plan selection regressions.
    pub(super) last_parameter_shape: Option<bool>,
    /// Cached transaction contexts: `(in_transaction, has_dirty_tables)`.
    pub(super) cached_transaction_contexts: HashSet<(bool, bool)>,
    /// Canonical physical plan cache entry for a SELECT executed through ExecStmt.
    pub(super) typed_plan_id: Option<u64>,
    pub(super) typed_plan_catalog_version: Option<u64>,
}

impl Default for SessionState {
    fn default() -> Self {
        let databases = [
            "test",
            "mysql",
            "information_schema",
            "performance_schema",
            "sys",
            "metrics_schema",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        Self {
            databases,
            local_temporary_tables: HashMap::new(),
            local_temporary_auto_ids: HashMap::new(),
            global_temporary_tables_in_transaction: HashMap::new(),
            current_database: "test".to_owned(),
            inspection_table_cache: None,
            prepared_database_override: None,
            transaction: None,
            transaction_info_schema: None,
            transaction_related_table_ids: HashSet::new(),
            transaction_locking_table_ids: HashSet::new(),
            transaction_pessimistic: false,
            transaction_explicit_optimistic: false,
            transaction_isolation: "REPEATABLE-READ".to_owned(),
            transaction_isolation_one_shot: None,
            transaction_isolation_restore: None,
            txn_mode: String::new(),
            autocommit: true,
            enable_async_commit: false,
            guarantee_linearizability: true,
            enable_1pc: false,
            tikv_client_read_timeout_ms: 0,
            replica_read: "leader".to_owned(),
            last_replica_read_request: None,
            last_select_request: None,
            last_import_plan_path: None,
            enable_paging: true,
            min_paging_size: 128,
            innodb_lock_wait_timeout_secs: astersql_sessionctx_vardef::DefInnodbLockWaitTimeout
                as u64,
            max_execution_time_ms: 0,
            low_resolution_tso: false,
            fair_locking: false,
            enable_noop_functions: false,
            global_enable_noop_functions: false,
            enable_shared_lock_promotion: false,
            foreign_key_checks: true,
            allow_auto_random_explicit_insert: false,
            row_encoder_enabled: true,
            foreign_key_check_in_shared_lock: false,
            constraint_check_in_place: astersql_sessionctx_vardef::DefTiDBConstraintCheckInPlace,
            constraint_check_in_place_pessimistic: true,
            savepoints: Vec::new(),
            pending_ttl_insert_rows: 0,
            held_row_locks: HashSet::new(),
            optimistic_fk_check_keys: HashSet::new(),
            adapter_dml_defer_fk_locks: false,
            adapter_dml_statement_staged: false,
            pending_fk_delete_cascades: Vec::new(),
            runaway_checker: None,
            runaway_resource_group_name: None,
            txn_mem_buffer_keys: 0,
            txn_mem_buffer_bytes: 0,
            transaction_read_epoch: 0,
            transaction_write_keys: HashSet::new(),
            deferred_optimistic_constraint_errors: BTreeMap::new(),
            optimistic_for_update_keys: HashSet::new(),
            transaction_conflict_context: None,
            transaction_scope: "global".to_owned(),
            transaction_store_labels: HashMap::new(),
            txn_entry_size_limit: DEFAULT_TXN_ENTRY_SIZE_LIMIT,
            statement_txn_start_ts: 0,
            last_commit_ts: 0,
            last_observed_store_ts: 0,
            pessimistic_lock_started: None,
            pessimistic_lock_ttl: Duration::from_secs(20),
            pessimistic_lock_ttl_expired_for_test: false,
            prepared: HashMap::new(),
            protocol_prepared_planned: HashMap::new(),
            prepared_planned: HashMap::new(),
            next_prepared_id: 1,
            last_plan_from_cache: false,
            plan_cache_generation: 0,
            process_plan_snapshot: None,
            last_statement_hints_for_test: (0, 0),
            last_dml_report: None,
            last_message: String::new(),
            last_query_string: String::new(),
            last_query_info: "null".to_owned(),
            last_write_sql_resp_duration: Duration::ZERO,
            defer_protocol_finish: false,
            pending_protocol_slow_logs: VecDeque::new(),
            slow_log_threshold_ms: 300,
            slow_query_plans: Vec::new(),
            statement_summary_plans: Vec::new(),
            last_explain_for_rows: None,
            client_capability: 0,
            info_last_insert_id: 0,
            retry_auto_increment_ids: VecDeque::new(),
            info_found_rows: 0,
            info_row_count: 0,
            last_autocommit_retry_attempts: 1,
            next_autocommit_retry_tso_failures: None,
            next_dml_commit_error: None,
            pending_stats_deltas: HashMap::new(),
            snapshot_catalog_version: None,
            snapshot_read_ts: None,
            pending_stale_read_ts: None,
            transaction_stale_read_ts: None,
            session_stale_read_ts: None,
            session_read_staleness_seconds: None,
            enable_external_ts_read: false,
            external_read_ts: 0,
            tso_catalog_versions: BTreeMap::new(),
            tso_database_names: BTreeMap::new(),
            tso_wall_times: Vec::new(),
            analyze_concurrency: 1,
            restricted_analyze_scan_concurrency: None,
            txn_write_throughput_sli: astersql_util_sli::TxnWriteThroughputSLI::default(),
            analyze_version: astersql_sessionctx_vardef::DefTiDBAnalyzeVersion as i32,
            analyze_memory_quota: astersql_sessionctx_vardef::DefTiDBMemQuotaAnalyze,
            sql_mode: astersql_parser_mysql::r#const::DefaultSQLMode.to_owned(),
            auto_increment_increment: 1,
            auto_increment_offset: 1,
            timestamp_override: None,
            max_connections: 0,
            tx_read_only: false,
            global_tx_read_only: false,
            sql_require_primary_key: false,
            character_set_results: "utf8mb4".to_owned(),
            dynamic_partition_prune: true,
            isolation_read_engines: "tikv,tiflash,tidb".to_owned(),
            redact_log: astersql_sessionctx_vardef::DefTiDBRedactLog.to_owned(),
            allow_mpp: true,
            allow_tiflash_cop: astersql_sessionctx_vardef::DefTiDBAllowTiFlashCop,
            enforce_mpp: false,
            tiflash_concurrency_factor: astersql_sessionctx_vardef::DefOptTiFlashConcurrencyFactor,
            tiflash_fastscan: false,
            tiflash_compute_dispatch_policy:
                astersql_sessionctx_variable::tiflashcompute::DispatchPolicyConsistentHash,
            mpp_disabled_explicitly: false,
            stats_load_sync_wait: astersql_sessionctx_vardef::DefTiDBStatsLoadSyncWait,
            opt_index_prune_threshold: astersql_sessionctx_vardef::DefTiDBOptIndexPruneThreshold,
            correlation_exp_factor: astersql_sessionctx_vardef::DefOptCorrelationExpFactor,
            repro_hash_join_max_scan_rows_ratio:
                astersql_sessionctx_vardef::DefOptIndexJoinMaxScanRowsRatio,
            optimizer_fix_control: String::new(),
            current_warnings: Vec::new(),
            last_warnings: Vec::new(),
            in_restricted_sql: false,
            prepared_by_name: HashMap::new(),
            user_variables: HashMap::new(),
            string_user_variables: HashSet::new(),
            prepared_plan_cache: astersql_sessionctx_vardef::DefTiDBEnablePrepPlanCache,
            non_prepared_plan_cache: false,
            non_prepared_plan_cache_keys: HashSet::new(),
            mem_arbitrator_wait_averse: astersql_sessionctx_vardef::DefTiDBMemArbitratorWaitAverse
                .to_owned(),
            global_mem_oom_action: astersql_sessionctx_vardef::OOMActionLog.to_owned(),
            mem_arbitrator_query_reserved: 0,
            statement_mem_arbitrator_query_reserved: None,
            dist_task_enabled: false,
            ddl_fast_reorg_enabled: true,
            ddl_analyze_enabled: false,
            clustered_index_def_mode: astersql_sessionctx_vardef::DefTiDBEnableClusteredIndex,
            shard_row_id_bits: astersql_sessionctx_vardef::DefShardRowIDBits as u64,
            pre_split_regions: astersql_sessionctx_vardef::DefPreSplitRegions as u64,
            scatter_region: astersql_sessionctx_vardef::ScatterOff.to_owned(),
            skip_predicate_collection: false,
            expr_pushdown_blacklist: BTreeSet::new(),
            loaded_expr_pushdown_blacklist: BTreeSet::new(),
            observation_sql_override: None,
            current_statement_digest: String::new(),
            statement_not_fill_cache: false,
            use_invisible_indexes: false,
            invisible_index_explain_seen: false,
            pessimistic_pause_observed_statement: false,
            current_statement_is_stale: false,
            last_statement_was_stale: false,
            current_stale_now_ts: None,
            stale_statement_observation_error: None,
        }
    }
}

/// 可执行的具体会话：Domain、会话变量、binding、内存跟踪与 SQLKiller。
pub struct ConcreteSession {
    pub(super) inner: Rc<ConcreteSessionInner>,
}

pub struct ConcreteSessionInner {
    pub(super) import_files: RefCell<super::import_file::ImportFiles>,
    pub(super) domain: Arc<Domain>,
    /// 同一 Domain 内所有会话共享的线程安全实例计划缓存。
    pub(super) instance_plan_cache: Arc<astersql_planner_core::InstancePlanCache>,
    pub(super) state: RefCell<SessionState>,
    pub(super) transaction_mdl: Arc<astersql_session_sessmgr::TransactionMDL>,
    pub(super) schema_validator: RefCell<Option<Arc<astersql_infoschema_isvalidator::Validator>>>,
    pub(super) mdl_metadata_error: RefCell<Option<String>>,
    pub(super) mdl_databases: RefCell<HashMap<i64, Arc<astersql_infoschema::infoschema::DBInfo>>>,
    pub(super) mdl_autocommit_write: std::cell::Cell<bool>,
    pub(super) mdl_tables: RefCell<
        HashMap<
            (String, String),
            (
                astersql_statistics_handle::StatsTableKey,
                astersql_meta_model::TableInfo,
            ),
        >,
    >,
    /// Statement-scoped, materialized non-recursive CTEs. Nested queries search
    /// innermost-to-outermost, matching Go's CTE name-resolution order.
    pub(super) cte_scopes: RefCell<Vec<HashMap<String, InsertSelectRows>>>,
    pub(crate) session_vars: Arc<astersql_sessionctx_variable::session::SessionVars>,
    /// Retain the Domain queue across statement contexts and SQL executions.
    pub(super) stats_sync_load: SessionStatsSyncLoadAdapter,
    /// Session-local display timezone for metadata timestamps.
    /// `SessionVars.StmtCtx` is shared through an `Arc`; keep this small
    /// mutable value beside it so `SET time_zone` affects metadata queries
    /// without weakening the session sharing boundary.
    pub(super) time_zone: RefCell<RuntimeTimeZone>,
    pub(super) bindings: RefCell<crate::hint_runtime::SessionBindingCatalog>,
    pub(super) session_manager: Option<Weak<dyn astersql_session_sessmgr::Manager>>,
    pub(super) login_user: Option<String>,
    pub(super) login_host: Option<String>,
    pub(super) authenticated_host: Option<String>,
    pub(super) active_roles: RefCell<Vec<astersql_privilege_privileges::RoleIdentity>>,
    pub(super) has_process_privilege: bool,
    pub(super) connection_id: AtomicU64,
    pub(super) sql_killer: Arc<SQLKiller>,
    pub(super) mem_tracker: RefCell<Box<Tracker>>,
    /// 最近一条语句的 tracker；TestKit 用它校验 OOM action 收尾后的 fallback。
    pub(super) last_statement_tracker: RefCell<Option<Box<Tracker>>>,
    /// 最近语句中真实 CTE 临时文件的峰值字节数。
    pub(super) last_statement_disk_max: std::cell::Cell<i64>,
    pub(super) row_lock_owner: u64,
    pub(super) trace_statement_count: AtomicU64,
}

impl Clone for ConcreteSession {
    fn clone(&self) -> Self {
        Self {
            inner: Rc::clone(&self.inner),
        }
    }
}

impl std::ops::Deref for ConcreteSession {
    type Target = ConcreteSessionInner;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl std::ops::DerefMut for ConcreteSession {
    fn deref_mut(&mut self) -> &mut Self::Target {
        Rc::get_mut(&mut self.inner).expect("cannot mutate a shared session")
    }
}

/// Send-safe result-column metadata consumed by the production protocol worker.
#[derive(Clone, Debug)]
pub struct ConcreteResultField {
    pub column: astersql_meta_model::ColumnInfo,
    pub column_as_name: ast::CIStr,
    pub table_name: ast::CIStr,
    pub table_as_name: ast::CIStr,
    pub db_name: ast::CIStr,
}

/// Session fields required by the MySQL text protocol after one statement.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ConcreteProtocolState {
    pub affected_rows: u64,
    pub last_insert_id: u64,
    pub warning_count: u16,
    pub status: u16,
    pub current_database: String,
}

/// A typed value decoded from one COM_STMT_EXECUTE parameter.
#[derive(Clone, Debug, PartialEq)]
pub enum ConcretePreparedArgument {
    Null,
    Signed(i64),
    Unsigned(u64),
    Float(f64),
    Decimal(String),
    Text(String),
    Bytes(Vec<u8>),
    Temporal(String),
}

impl ConcretePreparedArgument {
    pub(super) fn sql_literal(&self) -> SessionResult<String> {
        Ok(match self {
            Self::Null => "NULL".to_owned(),
            Self::Signed(value) => value.to_string(),
            Self::Unsigned(value) => value.to_string(),
            Self::Float(value) if value.is_finite() => value.to_string(),
            Self::Float(_) => {
                return Err(SessionError::new(
                    "non-finite prepared floating-point parameter",
                ));
            }
            Self::Decimal(value) => {
                value
                    .parse::<rust_decimal::Decimal>()
                    .map_err(|error| session_error("prepared DECIMAL parameter", error))?;
                value.clone()
            }
            Self::Text(value) | Self::Temporal(value) => quote_argument(value),
            Self::Bytes(value) => format!(
                "X'{}'",
                value
                    .iter()
                    .map(|byte| format!("{byte:02X}"))
                    .collect::<String>()
            ),
        })
    }
}

/// Owns one initialized canonical Domain and creates an independent SQL session
/// for each connection while retaining the Domain's shared storage boundary.
pub struct CanonicalSessionFactory {
    domain: Arc<Domain>,
}

impl CanonicalSessionFactory {
    /// Load an already bootstrapped target keyspace without registering a
    /// primary server or modifying that keyspace's schema during construction.
    pub(crate) fn from_crossks_tikv_store(store: astersql_store::TikvStore) -> SessionResult<Self> {
        let mut config = DomainConfig::default();
        config.keyspace = store.GetKeyspace();
        let factory = Self::from_storage(store, config)?;
        for table in ["tidb_ddl_job", "tidb_ddl_history"] {
            if let Err(error) = factory.domain.table_by_name("mysql", table) {
                factory.domain.close();
                return Err(SessionError::new(format!(
                    "target keyspace is not bootstrapped: mysql.{table}: {error}"
                )));
            }
        }
        Ok(factory)
    }

    /// Build the production session boundary from the exact `TikvStore` clone
    /// exposed by the store registry. This constructor never opens PD/TiKV.
    pub fn from_tikv_store(store: astersql_store::TikvStore) -> SessionResult<Self> {
        Self::from_tikv_store_with_server_info_options(store, &[])
    }

    /// Build the serving Domain and pass server-info Syncer options through
    /// its production initialization chain.
    pub fn from_tikv_store_with_server_info_options(
        store: astersql_store::TikvStore,
        options: &[astersql_domain_serverinfo::SyncerOption],
    ) -> SessionResult<Self> {
        let etcd_addrs = if store.has_real_client_runtime() {
            store
                .EtcdAddrs()
                .map_err(|error| session_error("read etcd endpoints", error))?
        } else {
            Vec::new()
        };
        let tls = store.TLSConfig();
        let pd_addrs = if store.has_real_client_runtime() && astersql_config_kerneltype::IsNextGen()
        {
            store
                .GetPDAddrs()
                .map_err(|error| session_error("read cross-keyspace PD endpoints", error))?
        } else {
            Vec::new()
        };
        let etcd_namespace = if etcd_addrs.is_empty() {
            String::new()
        } else {
            store
                .etcd_namespace()
                .map_err(|error| session_error("resolve Domain etcd namespace", error))?
        };
        let mut config = DomainConfig::default();
        config.keyspace = store.GetKeyspace();
        let keyspace_name = config.keyspace.clone();
        let factory = Self::from_storage(store, config)?;
        let workload_config = astersql_config::get_global_config()
            .external_workload
            .clone();
        if astersql_config_deploymode::IsStarter() && workload_config.Enable {
            let meta = (|| -> SessionResult<astersql_extworkload::keyspacepb::KeyspaceMeta> {
                use astersql_store_copr::network_backend::{
                    NetworkPdKeyspaceClient, NetworkSecurity,
                };
                let security = tls.as_ref().map(|tls| NetworkSecurity {
                    ca_path: tls.ca_path.clone(),
                    cert_path: tls.cert_path.clone(),
                    key_path: tls.key_path.clone(),
                });
                let client = NetworkPdKeyspaceClient::connect(
                    &pd_addrs,
                    security.as_ref(),
                    Duration::from_secs(10),
                    "tidb-extworkload",
                )
                .map_err(|e| session_error("load external workload keyspace metadata", e))?;
                let meta = client
                    .load_keyspace_meta(&keyspace_name)
                    .map_err(|e| session_error("load external workload keyspace metadata", e))?;
                Ok(astersql_extworkload::keyspacepb::KeyspaceMeta {
                    id: meta.get_id(),
                    name: meta.get_name().to_owned(),
                    config: meta
                        .get_config()
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect(),
                })
            })();
            install_external_workload_manager(
                &factory.domain,
                &workload_config.Role,
                meta,
                |meta| {
                    let options = astersql_extworkload::config::ExternalWorkload {
                        Enable: true,
                        Role: workload_config.Role.clone(),
                        TidbPool: workload_config.TidbPool.clone(),
                        ControllerAddr: workload_config.ControllerAddr.clone(),
                    };
                    let controller_tls = tls.as_ref().map(|tls| {
                        (
                            tls.ca_path.as_str(),
                            tls.cert_path.as_str(),
                            tls.key_path.as_str(),
                        )
                    });
                    astersql_extworkload::NewManagerWithTLS(
                        &astersql_extworkload::context::Background(),
                        Some(meta),
                        options,
                        controller_tls,
                    )
                    .map_err(|e| {
                        SessionError::new(format!("initialize external workload manager: {e}"))
                    })
                },
            )?;
        }
        let mut ttl_watch_transport = None;
        let mut serving_ddl_runtime = None;
        let mut bootstrap_owner_lock = None;
        let mut starter_owner_client = None;
        if !etcd_addrs.is_empty() {
            let tls_files = tls.as_ref().map(|tls| {
                (
                    tls.ca_path.as_str(),
                    tls.cert_path.as_str(),
                    tls.key_path.as_str(),
                )
            });
            let client =
                astersql_domain_serverinfo::RealEtcdClient::connect(etcd_addrs.clone(), tls_files)
                    .map_err(|error| session_error("connect Domain server-info etcd", error))?
                    .with_namespace(etcd_namespace);
            bootstrap_owner_lock = acquire_bootstrap_upgrade_lock(&factory.domain, || {
                let runtime = Arc::new(
                    tokio::runtime::Runtime::new()
                        .map_err(|e| session_error("bootstrap lock runtime", e))?,
                );
                let lock = runtime
                    .block_on(astersql_owner::AcquireDistributedLock(
                        &astersql_owner::Context::new(),
                        client.raw_client(),
                        format!(
                            "{}{}",
                            client.namespace(),
                            crate::bootstrap::bootstrapOwnerKey
                        ),
                        10,
                    ))
                    .map_err(|e| session_error("acquire bootstrap owner lock", e))?;
                Ok(BootstrapOwnerLock {
                    runtime,
                    lock: Some(lock),
                })
            })?;
            ttl_watch_transport = Some(Arc::new(super::ttl_runtime::EtcdTtlWatchTransport::new(
                client.raw_client(),
                client.namespace().to_owned(),
            ))
                as Arc<dyn super::ttl_runtime::TtlWatchTransport>);
            let id = format!(
                "{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            );
            let client = Arc::new(client);
            starter_owner_client = Some(Arc::clone(&client));
            factory
                .domain
                .install_server_info_syncer(id.clone(), client.clone(), options)
                .map_err(|error| {
                    factory.domain.close();
                    session_error("register Domain server info", error)
                })?;
            let cancellation = astersql_owner::Context::new();
            let owner = astersql_owner::NewOwnerManager(
                cancellation.clone(),
                client.raw_client(),
                "ddl",
                id.clone(),
                format!("{}{}", client.namespace(), astersql_ddl_util::DDLOwnerKey),
            );
            let schema_client = Arc::new(
                astersql_ddl_schemaver::RealEtcdClient::new(client)
                    .map_err(|e| session_error("prepare serving DDL etcd transport", e))?,
            );
            let owner_runtime = Arc::new(
                tokio::runtime::Runtime::new()
                    .map_err(|e| session_error("prepare serving DDL owner runtime", e))?,
            );
            serving_ddl_runtime = Some((owner, owner_runtime, cancellation, schema_client, id));
        }
        if !pd_addrs.is_empty() && !etcd_addrs.is_empty() {
            let tls_files = tls.as_ref().map(|tls| {
                (
                    tls.ca_path.clone(),
                    tls.cert_path.clone(),
                    tls.key_path.clone(),
                )
            });
            Arc::new(super::session_factory::KeyspaceSessionFactory::new(
                pd_addrs,
                etcd_addrs.clone(),
                tls_files,
            ))
            .install_on_domain(&factory.domain, keyspace_name)
            .map_err(|error| {
                factory.domain.close();
                session_error("install cross-keyspace session factory", error.0)
            })?;
        }
        if let Err(error) = BootstrapCanonicalDomain(Arc::clone(&factory.domain)) {
            factory.domain.close();
            return Err(error);
        }
        drop(bootstrap_owner_lock);
        if let Some((owner, owner_runtime, cancellation, schema_client, id)) = serving_ddl_runtime {
            if let Err(error) = super::session_factory::install_serving_ddl_runtime(
                &factory.domain,
                owner,
                owner_runtime,
                cancellation,
                schema_client,
                &id,
                astersql_sessionctx_vardef::GetSchemaLease(),
            ) {
                factory.domain.close();
                return Err(session_error("start serving durable DDL", error));
            }
        }
        if let Err(error) = factory.domain.start(StartMode::Normal) {
            factory.domain.close();
            return Err(session_error("start canonical Domain", error));
        }
        factory.reconcile_configured_starter_bootstrap(|| {
            let Some(client) = starter_owner_client.as_ref() else {
                return Ok(None);
            };
            let runtime = Arc::new(
                tokio::runtime::Runtime::new()
                    .map_err(|e| session_error("starter bootstrap lock runtime", e))?,
            );
            let lock = runtime
                .block_on(astersql_owner::AcquireDistributedLock(
                    &astersql_owner::Context::new(),
                    client.raw_client(),
                    format!(
                        "{}{}",
                        client.namespace(),
                        crate::bootstrap::bootstrapOwnerKey
                    ),
                    10,
                ))
                .map_err(|e| session_error("starter bootstrap owner lock", e))?;
            Ok(Some(BootstrapOwnerLock {
                runtime,
                lock: Some(lock),
            }))
        })?;
        initialize_external_workload_gcv2(&factory.domain);
        if let Err(error) = factory.domain.initialize_stats() {
            BgLogger().log(
                LogLevel::Error,
                "initialize statistics failed",
                [LogField::String("error".to_owned(), error.to_string())],
            );
        }
        if let Err(error) = super::ttl_runtime::start_domain_ttl_job_manager_with_transport(
            &factory.domain,
            ttl_watch_transport,
        ) {
            factory.domain.close();
            return Err(SessionError::new(format!("start TTL job manager: {error}")));
        }
        if let Err(error) = super::mlog_purge::start_domain_mlog_purge_worker(&factory.domain) {
            factory.domain.close();
            return Err(SessionError::new(format!(
                "start MLog purge worker: {error}"
            )));
        }
        Ok(factory)
    }

    fn from_storage<S>(store: S, config: DomainConfig) -> SessionResult<Self>
    where
        S: kv::Storage + Send + Sync + 'static,
    {
        let domain = Arc::new(Domain::new(
            store,
            Arc::new(KvInfoSchemaLoader::new()),
            config,
        ));
        domain
            .init()
            .map_err(|error| session_error("initialize canonical Domain", error))?;
        Ok(Self { domain })
    }

    #[cfg(test)]
    pub(crate) fn from_storage_for_test<S>(store: S) -> SessionResult<Self>
    where
        S: kv::Storage + Send + Sync + 'static,
    {
        let mut config = DomainConfig::default();
        config.schema_lease = Duration::ZERO;
        config.stats_lease = Duration::ZERO;
        let factory = Self::from_storage(store, config)?;
        BootstrapCanonicalDomain(factory.domain.clone())?;
        let cancellation = astersql_owner::Context::new();
        let id = format!("factory-{:p}", Arc::as_ptr(&factory.domain));
        let owner = astersql_owner::NewMockManager(
            cancellation.clone(),
            id.clone(),
            None,
            format!("/serving-factory/{id}"),
        );
        super::session_factory::install_serving_ddl_runtime(
            &factory.domain,
            owner,
            Arc::new(
                tokio::runtime::Runtime::new().map_err(|e| session_error("owner runtime", e))?,
            ),
            cancellation,
            Arc::new(astersql_ddl_schemaver::MemoryEtcdClient::default()),
            &id,
            Duration::from_millis(50),
        )
        .map_err(|e| session_error("serving DDL runtime", e))?;
        factory.reconcile_configured_starter_bootstrap(|| Ok(()))?;
        Ok(factory)
    }

    // Core bootstrap has loaded persisted settings and the normal SQL/DDL
    // runtime is ready before executing starter migrations against regular schemas.
    fn reconcile_configured_starter_bootstrap<G>(
        &self,
        acquire: impl FnOnce() -> SessionResult<G>,
    ) -> SessionResult<()> {
        let result = (|| {
            if let Some(file) = crate::starter_bootstrap_file::load_starter_bootstrap_file()? {
                crate::starter_bootstrap_file::reconcile_starter_bootstrap(
                    &self.domain,
                    &file,
                    &astersql_config::get_global_keyspace_name(),
                    acquire,
                )?;
            }
            Ok(())
        })();
        if result.is_err() {
            self.domain.close();
        }
        result
    }

    /// Return the single Domain shared by every session from this factory.
    pub fn domain(&self) -> &Arc<Domain> {
        &self.domain
    }

    /// Create one session with independent connection/session state.
    pub fn create_session(&self) -> ConcreteSession {
        ConcreteSession::new(Arc::clone(&self.domain))
    }
}

impl ConcreteSession {
    /// Enable or disable Go-compatible inspection-table snapshot caching.
    pub fn SetInspectionTableCacheEnabledForTest(&self, enabled: bool) {
        self.state.borrow_mut().inspection_table_cache = enabled.then(HashMap::new);
    }

    /// Return the cached row count for one inspection table.
    pub fn InspectionTableCacheRowCountForTest(&self, table: &str) -> Option<usize> {
        self.state
            .borrow()
            .inspection_table_cache
            .as_ref()?
            .get(&table.to_ascii_lowercase())
            .map(Vec::len)
    }

    /// Mutate one cached inspection-table cell, matching Go's test-side
    /// `Datum.SetString` operation on `TableSnapshot.Rows`.
    pub fn SetInspectionTableCacheValueForTest(
        &self,
        table: &str,
        row: usize,
        column: &str,
        value: String,
    ) -> SessionResult<()> {
        let mut state = self.state.borrow_mut();
        let cache = state
            .inspection_table_cache
            .as_mut()
            .ok_or_else(|| SessionError::new("inspection table cache is disabled"))?;
        let rows = cache.get_mut(&table.to_ascii_lowercase()).ok_or_else(|| {
            SessionError::new(format!("inspection table {table:?} is not cached"))
        })?;
        let cached_row = rows.get_mut(row).ok_or_else(|| {
            SessionError::new(format!("inspection table {table:?} has no row {row}"))
        })?;
        cached_row.insert(column.to_ascii_lowercase(), Some(value));
        Ok(())
    }

    pub(super) fn allocate_runtime_auto_id(
        &self,
        table_id: i64,
        explicit: Option<u64>,
        kind: u8,
        increment: u64,
        offset: u64,
    ) -> SessionResult<(u64, bool)> {
        let is_local_temporary = self
            .state
            .borrow()
            .local_temporary_tables
            .values()
            .any(|table| table.ID == table_id);
        if !is_local_temporary {
            return self
                .domain
                .allocate_stats_auto_id_with_increment(table_id, explicit, kind, increment, offset)
                .map_err(|error| session_error("allocate table auto ID", error));
        }

        let mut state = self.state.borrow_mut();
        let next = state
            .local_temporary_auto_ids
            .entry((table_id, kind))
            .or_insert(1);
        if let Some(value) = explicit {
            let rebased = value > *next;
            *next = (*next).max(value.saturating_add(1));
            return Ok((value, rebased));
        }
        if increment <= 1 || kind != 0 {
            let value = *next;
            *next = next
                .checked_add(1)
                .ok_or_else(|| SessionError::new("auto-increment allocator overflow"))?;
            return Ok((value, false));
        }
        let offset = if offset == 0 || offset > increment {
            1
        } else {
            offset
        };
        let value = if *next <= offset {
            offset
        } else {
            let distance = *next - offset;
            offset
                .checked_add(
                    distance
                        .checked_add(increment - 1)
                        .and_then(|value| value.checked_div(increment))
                        .and_then(|value| value.checked_mul(increment))
                        .ok_or_else(|| SessionError::new("auto-increment allocator overflow"))?,
                )
                .ok_or_else(|| SessionError::new("auto-increment allocator overflow"))?
        };
        *next = value
            .checked_add(1)
            .ok_or_else(|| SessionError::new("auto-increment allocator overflow"))?;
        Ok((value, false))
    }

    pub(super) fn local_temporary_table(
        &self,
        database: &str,
        table: &str,
    ) -> Option<astersql_meta_model::TableInfo> {
        self.state
            .borrow()
            .local_temporary_tables
            .get(&(database.to_ascii_lowercase(), table.to_ascii_lowercase()))
            .cloned()
    }

    /// Resolve names through the session catalog before the shared Domain.
    pub(super) fn resolve_runtime_table(
        &self,
        database: &str,
        table: &str,
    ) -> Option<astersql_meta_model::TableInfo> {
        self.local_temporary_table(database, table).or_else(|| {
            self.mdl_stats_table(database, table)
                .map(|(_, table)| table)
        })
    }

    /// Acquire and pin metadata at the first real table access in a txn.
    /// Publishing zero before fetching the latest IS prevents schema loops
    /// from acknowledging a DDL during concurrent metadata lookup.
    pub(super) fn mdl_stats_table(
        &self,
        database: &str,
        name: &str,
    ) -> Option<(
        astersql_statistics_handle::StatsTableKey,
        astersql_meta_model::TableInfo,
    )> {
        let lock = {
            let state = self.state.borrow();
            astersql_sessionctx_vardef::IsMDLEnabled()
                && (state.transaction.is_some() || self.mdl_autocommit_write.get())
                && !state.in_restricted_sql
                && state.transaction_stale_read_ts.is_none()
                && !state.current_statement_is_stale
                && state.snapshot_read_ts.is_none()
        };
        if !lock {
            return self.domain.stats_table(database, name);
        }
        let key = (database.to_lowercase(), name.to_lowercase());
        if let Some(table) = self.mdl_tables.borrow().get(&key) {
            return Some(table.clone());
        }
        let initial = self
            .state
            .borrow()
            .transaction_info_schema
            .as_ref()
            .and_then(|schema| {
                schema
                    .ModelTableInfoByName(
                        &astersql_infoschema::CiString::new(database),
                        &astersql_infoschema::CiString::new(name),
                    )
                    .ok()
            })
            .map(|table| {
                (
                    astersql_statistics_handle::StatsTableKey::new(database, name, table.ID),
                    (*table).clone(),
                )
            })
            .or_else(|| self.domain.stats_table(database, name))?;
        if initial.1.TempTableType == astersql_meta_model::TempTableLocal {
            return Some(initial);
        }
        let skip_lock = initial.1.TempTableType == astersql_meta_model::TempTableGlobal;
        if !skip_lock {
            self.transaction_mdl.begin_table(initial.1.ID);
        }
        let schema = self.domain.info_schema();
        let table = schema
            .TableByName(
                &astersql_infoschema::CiString::new(database),
                &astersql_infoschema::CiString::new(name),
            )
            .ok()
            .and_then(|t| t.Meta().model_meta.as_ref().map(|m| (**m).clone()));
        let Some(mut table) = table else {
            self.transaction_mdl.remove_table(initial.1.ID);
            return None;
        };
        if table.State != astersql_meta_model::StatePublic {
            self.transaction_mdl.remove_table(initial.1.ID);
            return None;
        }
        if !skip_lock && table.ID != initial.1.ID {
            self.transaction_mdl.begin_table(table.ID);
            self.transaction_mdl.remove_table(initial.1.ID);
        }
        if !skip_lock {
            self.transaction_mdl
                .finish_table(table.ID, schema.SchemaMetaVersion());
        }
        let read_consistency = {
            let state = self.state.borrow();
            state.transaction_pessimistic
                && state
                    .transaction_isolation
                    .eq_ignore_ascii_case("READ-COMMITTED")
        };
        if table.Revision != initial.1.Revision && !read_consistency {
            let indices: HashMap<_, _> = initial
                .1
                .Indices
                .iter()
                .map(|index| (index.Name.L.as_str(), index.ID))
                .collect();
            for index in &mut table.Indices {
                if index.State == astersql_meta_model::StatePublic
                    && indices
                        .get(index.Name.L.as_str())
                        .is_none_or(|id| *id != index.ID)
                {
                    index.State = astersql_meta_model::StateWriteReorganization;
                }
            }
            let columns: HashMap<_, _> = initial
                .1
                .Columns
                .iter()
                .map(|column| (column.Name.L.as_str(), column.ID))
                .collect();
            for column in &table.Columns {
                if column.State == astersql_meta_model::StatePublic
                    && columns
                        .get(column.Name.L.as_str())
                        .is_some_and(|id| *id != column.ID)
                {
                    self.transaction_mdl.remove_table(table.ID);
                    *self.mdl_metadata_error.borrow_mut() = Some(format!(
                        "Information schema is changed: public column {} has changed",
                        column.Name.O
                    ));
                    return None;
                }
            }
        }
        if let Some(db) = schema.SchemaByName(&astersql_infoschema::CiString::new(database)) {
            self.mdl_databases.borrow_mut().insert(table.DBID, db);
        }
        let result = (
            astersql_statistics_handle::StatsTableKey::new(database, name, table.ID),
            table,
        );
        self.mdl_tables.borrow_mut().insert(key, result.clone());
        Some(result)
    }

    /// Register physical table names before specialized SQL execution paths.
    pub(super) fn register_statement_mdl(&self, statement: &dyn ast::Node) -> SessionResult<()> {
        if !astersql_sessionctx_vardef::IsMDLEnabled() {
            return Ok(());
        }
        struct Tables(Vec<ast::TableName>);
        impl ast::Visitor for Tables {
            fn enter(&mut self, _: &dyn ast::Node) -> bool {
                false
            }
            fn leave(&mut self, _: &dyn ast::Node) -> bool {
                true
            }
            fn enter_table_name(&mut self, table: &ast::TableName) -> bool {
                self.0.push(table.clone());
                false
            }
        }
        let mut tables = Tables(Vec::new());
        statement.accept(&mut tables);
        let current_database = self.current_database();
        for table in tables.0 {
            let database = if table.Schema.L.is_empty() {
                &current_database
            } else {
                &table.Schema.L
            };
            if self
                .local_temporary_table(database, &table.Name.L)
                .is_none()
            {
                self.mdl_stats_table(database, &table.Name.L);
                if let Some(error) = self.mdl_metadata_error.borrow_mut().take() {
                    return Err(SessionError::new(error));
                }
            }
        }
        Ok(())
    }

    pub(super) fn transaction_mdl_schema(&self, base: SchemaRef) -> SchemaRef {
        if self.mdl_tables.borrow().is_empty() {
            return base;
        }
        let mut extended = astersql_infoschema::infoschema::SessionExtendedInfoSchema::new(base);
        for (_, (_, model)) in self.mdl_tables.borrow().iter() {
            if let Some(db) = self.mdl_databases.borrow().get(&model.DBID) {
                // Model DBID is authoritative in Go meta. Retain it on the
                // pinned table rather than inferring from a later schema.
                let mut model = model.clone();
                model.DBID = db.id;
                extended
                    .UpdateTableInfo(
                        (**db).clone(),
                        astersql_infoschema::Table::from_model(model),
                    )
                    .expect("MDL table names are unique within a transaction");
            }
        }
        Arc::new(extended)
    }

    pub fn transaction_mdl(&self) -> Arc<astersql_session_sessmgr::TransactionMDL> {
        Arc::clone(&self.transaction_mdl)
    }

    /// 创建 ConcreteSession，安装谓词简化直通并初始化默认库。
    pub fn new(domain: Arc<Domain>) -> Self {
        astersql_planner_core_operator_logicalop::InstallPredicateSimplificationPassthrough();
        let mut session_vars = astersql_sessionctx_variable::session::SessionVars::default();
        session_vars.SetCurrentDB("test");
        session_vars.PartitionPruneMode =
            astersql_sessionctx_variable::session::PartitionPruneMode::Dynamic;
        session_vars.StmtCtx.StatsLoad.Timeout =
            Duration::from_millis(astersql_sessionctx_vardef::DefTiDBStatsLoadSyncWait as u64);
        let mut state = SessionState::default();
        state.scatter_region = domain.global_scatter_region();
        state.txn_mode = domain.global_txn_mode();
        state.tiflash_compute_dispatch_policy = domain.global_tiflash_compute_dispatch_policy();
        let domain_id = runtime_domain_id(&domain);
        state.txn_entry_size_limit = RUNTIME_GLOBAL_TXN_ENTRY_SIZE_LIMITS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&domain_id)
            .copied()
            .unwrap_or(DEFAULT_TXN_ENTRY_SIZE_LIMIT);
        let mut mem_tracker = NewTracker(LabelForSession, -1);
        mem_tracker.IsRootTrackerOfSess = true;
        let instance_plan_cache = runtime_instance_plan_cache(&domain);
        let session = Self {
            inner: Rc::new(ConcreteSessionInner {
                stats_sync_load: SessionStatsSyncLoadAdapter::new_with_domain(Arc::clone(&domain)),
                domain,
                instance_plan_cache,
                state: RefCell::new(state),
                transaction_mdl: Arc::new(Default::default()),
                schema_validator: RefCell::new(None),
                mdl_metadata_error: RefCell::new(None),
                mdl_databases: RefCell::new(HashMap::new()),
                mdl_autocommit_write: std::cell::Cell::new(false),
                mdl_tables: RefCell::new(HashMap::new()),
                cte_scopes: RefCell::new(Vec::new()),
                import_files: RefCell::new(Default::default()),
                session_vars: Arc::new(session_vars),
                time_zone: RefCell::new(RuntimeTimeZone::Named(chrono_tz::UTC)),
                bindings: RefCell::new(crate::hint_runtime::SessionBindingCatalog::New("test")),
                session_manager: None,
                login_user: None,
                login_host: None,
                authenticated_host: None,
                active_roles: RefCell::new(Vec::new()),
                has_process_privilege: false,
                connection_id: AtomicU64::new(0),
                sql_killer: Arc::new(SQLKiller::default()),
                mem_tracker: RefCell::new(mem_tracker),
                last_statement_tracker: RefCell::new(None),
                last_statement_disk_max: std::cell::Cell::new(0),
                row_lock_owner: NEXT_ROW_LOCK_OWNER.fetch_add(1, Ordering::Relaxed),
                trace_statement_count: AtomicU64::new(0),
            }),
        };
        session.load_persisted_global_variables();
        session
    }

    /// Load the persisted global-variable catalog into a newly created session,
    /// matching TiDB's session bootstrap path after an upgrade or rebootstrap.
    fn load_persisted_global_variables(&self) {
        if let Some(table) = self.resolve_runtime_table("mysql", "global_variables")
            && let Ok(rows) = self.scan_registered_table(&table)
        {
            for (_, row) in rows {
                let (Some(name), Some(value)) = (
                    row.get("variable_name").and_then(Clone::clone),
                    row.get("variable_value").and_then(Clone::clone),
                ) else {
                    continue;
                };
                self.apply_persisted_global_variable(name, value);
            }
        }
        for (name, value) in self.domain.global_system_variables() {
            self.apply_persisted_global_variable(name, value);
        }
    }

    fn apply_persisted_global_variable(&self, name: String, value: String) {
        let name = name.to_ascii_lowercase();
        let _ = self
            .session_vars
            .SetHintSystemVarWithOldState(&name, &value);
        let mut state = self.state.borrow_mut();
        match name.as_str() {
            "sql_mode" => state.sql_mode = value,
            "tidb_enable_paging" => {
                state.enable_paging =
                    matches!(value.to_ascii_lowercase().as_str(), "1" | "on" | "true");
            }
            "tidb_analyze_version" => {
                if let Ok(version) = value.parse() {
                    state.analyze_version = version;
                }
            }
            "max_execution_time" => {
                if let Ok(timeout) = value.parse() {
                    state.max_execution_time_ms = timeout;
                }
            }
            _ => {}
        }
    }
}

/// 具体结果集：列名与行缓冲。
pub struct ConcreteRecordSet {
    pub(super) columns: Vec<String>,
    pub(super) result_fields: Vec<Option<ConcreteResultField>>,
    pub(super) rows: VecDeque<Vec<String>>,
    pub(super) closed: bool,
    store_read: Option<Arc<SQLKiller>>,
}

impl ConcreteRecordSet {
    pub(super) fn new(columns: Vec<String>, rows: Vec<Vec<String>>) -> Self {
        Self {
            result_fields: std::iter::repeat_with(|| None)
                .take(columns.len())
                .collect(),
            columns,
            rows: rows.into(),
            closed: false,
            store_read: None,
        }
    }

    pub(super) fn new_with_fields(
        columns: Vec<String>,
        rows: Vec<Vec<String>>,
        mut result_fields: Vec<Option<ConcreteResultField>>,
    ) -> Self {
        // Derived/CTE projections may not have catalog fields even though the
        // result still has named columns. Keep protocol metadata aligned and
        // represent those fields as unknown instead of panicking in tests.
        result_fields.resize(columns.len(), None);
        result_fields.truncate(columns.len());
        Self {
            columns,
            result_fields,
            rows: rows.into(),
            closed: false,
            store_read: None,
        }
    }

    /// Transport failures belong to result consumption, after Execute has
    /// returned a valid result set. Closing without reading cancels this work.
    pub(super) fn with_store_read(mut self, killer: Arc<SQLKiller>) -> Self {
        self.store_read = Some(killer);
        self
    }

    fn finish_store_read(&mut self) -> SessionResult {
        let Some(killer) = self.store_read.take() else {
            return Ok(());
        };
        let mut response =
            astersql_testkit_testfailpoint::eval_string("tikvclient/tikvStoreSendReqResult");
        killer
            .HandleSignal()
            .map_err(|error| session_error("coprocessor backoff interrupted", error))?;
        if response.is_none() {
            response =
                astersql_testkit_testfailpoint::eval_string("tikvclient/tikvStoreSendReqResult");
            killer
                .HandleSignal()
                .map_err(|error| session_error("coprocessor backoff interrupted", error))?;
        }
        if let Some(response) = response.filter(|value| !value.is_empty() && value != "timeout") {
            return Err(SessionError::new(format!(
                "TiKV store send failed: {response}"
            )));
        }
        Ok(())
    }

    /// Read-only text-protocol column names.
    pub fn columns(&self) -> &[String] {
        &self.columns
    }

    /// Canonical planner result fields used by the MySQL protocol adapter.
    pub fn result_fields(&self) -> &[Option<ConcreteResultField>] {
        &self.result_fields
    }

    /// Consume one row through the production record-set boundary.
    pub fn next_row(&mut self) -> SessionResult<Option<Vec<String>>> {
        if self.closed {
            return Err(SessionError::new("record set is closed"));
        }
        if let Err(error) = self.finish_store_read() {
            self.close()?;
            return Err(error);
        }
        Ok(self.rows.pop_front())
    }

    /// Release buffered rows and make further reads fail.
    pub fn close(&mut self) -> SessionResult {
        self.store_read = None;
        self.rows.clear();
        self.closed = true;
        Ok(())
    }
}

impl TestRecordSet for ConcreteRecordSet {
    fn Columns(&self) -> &[String] {
        self.columns()
    }

    fn Next(&mut self) -> SessionResult<Option<Vec<String>>> {
        self.next_row()
    }

    fn Close(&mut self) -> SessionResult {
        self.close()
    }
}

/// Validate and split SQL with the same quote/comment-aware boundaries used by
/// ConcreteSession's existing multi-statement executor.
pub fn SplitSQLStatements(sql: &str) -> SessionResult<Vec<String>> {
    parse(sql)?;
    Ok(split_statement_sql(sql))
}

/// 测试运行时持有的存储包装。
struct RuntimeStore<S> {
    storage: Mutex<Option<S>>,
    domain: RwLock<Option<Arc<Domain>>>,
}

impl<S: Send + Sync + 'static> TestStore for RuntimeStore<S> {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// 测试 Domain 包装，实现 TestDomain。
pub struct RuntimeDomain {
    domain: Arc<Domain>,
}

impl RuntimeDomain {
    /// 返回绑定的 Domain。
    pub fn domain(&self) -> &Arc<Domain> {
        &self.domain
    }
}

impl TestDomain for RuntimeDomain {
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Insert a fully materialized row through the relational KV/index encoder
/// without touching the auto-ID allocator.  This is the narrow bridge needed
/// by the Go `Table.AddRecord` regression: a second writer can commit a large
/// auto-increment handle without making the local allocator rebase.
pub fn AddRecordWithoutAutoIDRebaseForTest(
    domain: &Arc<Domain>,
    database: &str,
    table_name: &str,
    values: &[(&str, &str)],
) -> SessionResult<()> {
    let session = ConcreteSession::new(Arc::clone(domain));
    let (_, table) = domain
        .stats_table(database, table_name)
        .ok_or_else(|| SessionError::new(format!("unknown DML table {database}.{table_name}")))?;
    let provided = values
        .iter()
        .map(|(column, value)| ((*column).to_ascii_lowercase(), Some((*value).to_owned())))
        .collect::<HashMap<_, _>>();
    let row = table
        .Columns
        .iter()
        .map(|column| {
            (
                column.Name.L.clone(),
                provided.get(&column.Name.L).cloned().flatten(),
            )
        })
        .collect::<HashMap<_, _>>();
    let flags = session.dml_type_flags();
    let allocator_base = table
        .GetAutoIncrementColInfo()
        .and_then(|_| domain.stats_auto_id_base(table.ID, 0));
    let (row_key, row_value) = encode_relational_row(&table, &row, flags)?;
    let mut mutations = vec![(row_key, Some(row_value))];
    mutations.extend(relational_index_mutations(&table, None, Some(&row), flags)?);
    let result = session.apply_relational_mutations(
        table_name,
        "AddRecord",
        mutations,
        unique_lock_keys_for_rows(&table, std::iter::once(&row)),
        1,
        0,
        0,
        0,
    );
    if let Some(base) = allocator_base {
        domain.restore_stats_auto_id_base_for_test(table.ID, 0, base);
    }
    result
}

/// Materialize only secondary-index keys for the Go `TestReplaceLog`
/// dangling-index regression, intentionally omitting the corresponding row.
pub fn CreateDanglingIndexForTest(
    domain: &Arc<Domain>,
    database: &str,
    table_name: &str,
    values: &[(&str, &str)],
) -> SessionResult<()> {
    let session = ConcreteSession::new(Arc::clone(domain));
    let (_, table) = domain
        .stats_table(database, table_name)
        .ok_or_else(|| SessionError::new(format!("unknown DML table {database}.{table_name}")))?;
    let provided = values
        .iter()
        .map(|(column, value)| ((*column).to_ascii_lowercase(), Some((*value).to_owned())))
        .collect::<HashMap<_, _>>();
    let row = table
        .Columns
        .iter()
        .map(|column| {
            (
                column.Name.L.clone(),
                provided.get(&column.Name.L).cloned().flatten(),
            )
        })
        .collect::<HashMap<_, _>>();
    let index_mutations =
        relational_index_mutations(&table, None, Some(&row), session.dml_type_flags())?;
    session.apply_relational_mutations(
        table_name,
        "CreateDanglingIndex",
        index_mutations,
        Vec::new(),
        0,
        0,
        0,
        0,
    )
}

pub(super) fn build_bootstrap_view_table(
    create_sql: &str,
    resolved_columns: Option<Vec<String>>,
) -> SessionResult<astersql_meta_model::TableInfo> {
    let statements = parse(create_sql)?;
    let create = statements
        .first()
        .and_then(|statement| statement.as_any().downcast_ref::<ast::CreateViewStmt>())
        .ok_or_else(|| SessionError::new("bootstrap view SQL did not parse as CREATE VIEW"))?;
    let select = create
        .Select
        .as_any()
        .downcast_ref::<ast::SelectStmt>()
        .ok_or_else(|| SessionError::new("bootstrap view requires a SELECT definition"))?;
    let column_names = if create.Cols.is_empty() {
        if let Some(columns) = resolved_columns {
            columns
        } else {
            select
                .Fields
                .Fields
                .iter()
                .map(|field| {
                    if !field.AsName.O.is_empty() {
                        return Ok(field.AsName.O.clone());
                    }
                    let expression = field.Expr.as_ref().ok_or_else(|| {
                        SessionError::new("bootstrap view field has no expression")
                    })?;
                    match &expression.Kind {
                        ast::ExprKind::Column(column) => Ok(column.Name.O.clone()),
                        _ => Err(SessionError::new(
                            "bootstrap view expression requires an explicit alias",
                        )),
                    }
                })
                .collect::<SessionResult<Vec<_>>>()?
        }
    } else {
        create.Cols.iter().map(|column| column.O.clone()).collect()
    };
    let columns = column_names
        .iter()
        .enumerate()
        .map(|(offset, name)| virtual_system_column(offset as i64 + 1, offset, name))
        .collect();
    Ok(astersql_meta_model::TableInfo {
        Name: create.ViewName.Name.clone(),
        Charset: "utf8mb4".to_owned(),
        Collate: "utf8mb4_bin".to_owned(),
        Columns: columns,
        State: astersql_meta_model::StatePublic,
        View: Some(astersql_meta_model::ViewInfo {
            Algorithm: match create.Algorithm {
                ast::ViewAlgorithm::Undefined => {
                    astersql_meta_model::ast::model::AlgorithmUndefined
                }
                ast::ViewAlgorithm::Merge => astersql_meta_model::ast::model::AlgorithmMerge,
                ast::ViewAlgorithm::Temptable => {
                    astersql_meta_model::ast::model::AlgorithmTemptable
                }
            },
            Definer: Some(create.Definer.clone()),
            Security: match create.Security {
                ast::ViewSecurity::Definer => astersql_meta_model::ast::model::SecurityDefiner,
                ast::ViewSecurity::Invoker => astersql_meta_model::ast::model::SecurityInvoker,
            },
            SelectStmt: create_sql.to_owned(),
            CheckOption: match create.CheckOption {
                ast::ViewCheckOption::Cascaded => {
                    astersql_meta_model::ast::model::CheckOptionCascaded
                }
                ast::ViewCheckOption::Local => astersql_meta_model::ast::model::CheckOptionLocal,
            },
            // ViewInfo.Cols records only the optional column list written after
            // the view name. Inferred SELECT output names belong to TableInfo.Columns.
            Cols: create.Cols.clone(),
        }),
        ..Default::default()
    })
}

fn canonical_bootstrap_version(
    session: &ConcreteSession,
    domain: &Domain,
) -> SessionResult<Option<i64>> {
    if domain.stats_table("mysql", "tidb").is_none() {
        return Ok(None);
    }
    let mut record_sets = session.execute(
        "SELECT VARIABLE_VALUE FROM mysql.tidb WHERE VARIABLE_NAME='tidb_server_version'",
    )?;
    let Some(record_set) = record_sets.first_mut() else {
        return Ok(None);
    };
    let Some(row) = record_set.next_row()? else {
        return Ok(None);
    };
    let value = row
        .first()
        .ok_or_else(|| SessionError::new("bootstrap version row has no value"))?;
    value
        .parse::<i64>()
        .map(Some)
        .map_err(|_| SessionError::new(format!("invalid bootstrap version {value:?}")))
}

fn ensure_canonical_ddl_system_tables(
    session: &ConcreteSession,
    domain: &Domain,
) -> SessionResult<()> {
    for (name, create_sql) in [
        ("tidb_ddl_job", astersql_meta_metadef::CreateTiDBDDLJobTable),
        (
            "tidb_ddl_reorg",
            astersql_meta_metadef::CreateTiDBReorgTable,
        ),
        (
            "tidb_ddl_history",
            astersql_meta_metadef::CreateTiDBDDLHistoryTable,
        ),
        ("tidb_mdl_info", astersql_meta_metadef::CreateTiDBMDLTable),
        (
            "tidb_background_subtask",
            astersql_meta_metadef::CreateTiDBBackgroundSubtaskTable,
        ),
        (
            "tidb_background_subtask_history",
            astersql_meta_metadef::CreateTiDBBackgroundSubtaskHistoryTable,
        ),
        (
            "tidb_ddl_notifier",
            astersql_meta_metadef::CreateTiDBDDLNotifierTable,
        ),
        (
            "tidb_mlog_purge_info",
            astersql_meta_metadef::CreateTiDBMLogPurgeInfoTable,
        ),
        (
            "tidb_mview_refresh_info",
            astersql_meta_metadef::CreateTiDBMViewRefreshInfoTable,
        ),
        (
            "tidb_mlog_purge_hist",
            astersql_meta_metadef::CreateTiDBMLogPurgeHistTable,
        ),
    ] {
        if domain.stats_table("mysql", name).is_none() {
            session.execute(create_sql).map_err(|error| {
                SessionError::new(format!(
                    "bootstrap mysql.{name} from authoritative DDL: {error}"
                ))
            })?;
        }
    }
    Ok(())
}

struct CanonicalBootstrapVariableRuntime<'a> {
    session: &'a ConcreteSession,
}

impl crate::upgrade_run::BootstrapVariableUpgradeRuntime for CanonicalBootstrapVariableRuntime<'_> {
    type Error = SessionError;

    fn insert_global_if_missing(&mut self, name: &str, value: &str) -> Result<(), Self::Error> {
        self.session.execute(&format!(
            "INSERT IGNORE INTO mysql.global_variables (VARIABLE_NAME, VARIABLE_VALUE) \
             VALUES ('{}', '{}')",
            name.replace('\'', "''"),
            value.replace('\'', "''"),
        ))?;
        Ok(())
    }

    fn delete_global_if_equal(&mut self, name: &str, value: &str) -> Result<(), Self::Error> {
        self.session.execute(&format!(
            "DELETE FROM mysql.global_variables WHERE VARIABLE_NAME='{}' AND VARIABLE_VALUE='{}'",
            name.replace('\'', "''"),
            value.replace('\'', "''"),
        ))?;
        Ok(())
    }

    fn update_global_if_equal(
        &mut self,
        name: &str,
        old_value: &str,
        new_value: &str,
    ) -> Result<(), Self::Error> {
        self.session.execute(&format!(
            "UPDATE mysql.global_variables SET VARIABLE_VALUE='{}' \
             WHERE VARIABLE_NAME='{}' AND VARIABLE_VALUE='{}'",
            new_value.replace('\'', "''"),
            name.replace('\'', "''"),
            old_value.replace('\'', "''"),
        ))?;
        Ok(())
    }

    fn upsert_tidb_variable(
        &mut self,
        name: &str,
        value: &str,
        comment: &str,
    ) -> Result<(), Self::Error> {
        self.session.execute(&format!(
            "INSERT INTO mysql.tidb (VARIABLE_NAME, VARIABLE_VALUE, COMMENT) \
             VALUES ('{}', '{}', '{}') ON DUPLICATE KEY UPDATE VARIABLE_VALUE='{}'",
            name.replace('\'', "''"),
            value.replace('\'', "''"),
            comment.replace('\'', "''"),
            value.replace('\'', "''"),
        ))?;
        Ok(())
    }
}

fn upgrade_canonical_domain(
    session: &ConcreteSession,
    previous_bootstrap_version: Option<i64>,
) -> SessionResult<()> {
    if let Some(previous_bootstrap_version) = previous_bootstrap_version {
        crate::upgrade_run::upgrade_bootstrap_variables(
            &mut CanonicalBootstrapVariableRuntime { session },
            previous_bootstrap_version,
        )?;
    }
    if previous_bootstrap_version.is_some_and(|version| version < 211) {
        session.execute(
            "ALTER TABLE mysql.tidb_background_subtask_history \
             ADD COLUMN IF NOT EXISTS summary JSON",
        )?;
    }
    if previous_bootstrap_version.is_some_and(|version| version < crate::upgrade_def::version262) {
        refresh_canonical_binding_digests(session)?;
    }
    Ok(())
}

fn refresh_canonical_binding_digests(session: &ConcreteSession) -> SessionResult<()> {
    use crate::upgrade_run::{
        BindingDigestRefreshAction, BindingDigestRefreshRow, plan_binding_digest_refresh,
    };

    let mut result_sets = session.execute(
        "SELECT bind_sql, default_db, source, plan_digest FROM mysql.bind_info \
         WHERE source != 'builtin' ORDER BY update_time DESC, create_time DESC, bind_sql DESC",
    )?;
    let Some(mut result) = result_sets.pop() else {
        return Err(SessionError::new(
            "v262 bind_info scan returned no result set",
        ));
    };
    let mut rows = Vec::new();
    while let Some(row) = result.next_row()? {
        if row.len() != 4 {
            return Err(SessionError::new(
                "v262 bind_info scan returned an invalid row",
            ));
        }
        rows.push(BindingDigestRefreshRow {
            identity: row[0].clone(),
            bind_sql: row[0].clone(),
            default_db: row[1].clone(),
            source: row[2].clone(),
            plan_digest: (!row[3].is_empty()).then(|| row[3].clone()),
        });
    }
    for action in plan_binding_digest_refresh(rows) {
        let quote = |value: &str| value.replace('\'', "''");
        match action {
            BindingDigestRefreshAction::ClearInvalidPlanDigest { identity } => {
                session.execute(&format!(
                    "UPDATE mysql.bind_info SET plan_digest=NULL WHERE bind_sql='{}'",
                    quote(&identity)
                ))?;
            }
            BindingDigestRefreshAction::DeleteDuplicate { identity } => {
                session.execute(&format!(
                    "UPDATE mysql.bind_info SET status='deleted', sql_digest=NULL, plan_digest=NULL \
                     WHERE bind_sql='{}'",
                    quote(&identity)
                ))?;
            }
            BindingDigestRefreshAction::UpdateDigest {
                identity,
                original_sql,
                sql_digest,
            } => {
                session.execute(&format!(
                    "UPDATE mysql.bind_info SET original_sql='{}', sql_digest='{}' \
                     WHERE bind_sql='{}'",
                    quote(&original_sql),
                    quote(&sql_digest),
                    quote(&identity)
                ))?;
            }
        }
    }
    Ok(())
}

struct CanonicalBootstrapSchemaRuntime<'a> {
    txn: &'a mut dyn kv::Transaction,
    changed: bool,
}
impl crate::bootstrap::BootstrapSchemaRuntime for CanonicalBootstrapSchemaRuntime<'_> {
    type Error = String;
    fn nextgen_schema_version(&mut self) -> Result<i32, String> {
        match self.txn.Get(
            &kv::Context::default(),
            astersql_meta::transaction_meta_string_key(b"BootTableVersion"),
            &[],
        ) {
            Ok(value) if !value.Value.is_empty() => std::str::from_utf8(&value.Value)
                .map_err(|e| e.to_string())?
                .parse()
                .map_err(|e: std::num::ParseIntError| e.to_string()),
            Ok(_) => Ok(0),
            Err(e) if kv::IsErrNotFound(&e) => Ok(0),
            Err(e) => Err(e.to_string()),
        }
    }
    fn create_system_database(
        &mut self,
        database: crate::bootstrap::DatabaseBasicInfo,
    ) -> Result<(), String> {
        let mut meta = astersql_meta::TransactionMutator::new(self.txn);
        if let Some(existing) = meta.get_database(database.id)? {
            if existing.Name.L != database.name {
                return Err(format!(
                    "reserved database ID {} belongs to {}",
                    database.id, existing.Name.O
                ));
            }
            return Ok(());
        }
        if meta
            .list_databases()?
            .iter()
            .any(|existing| existing.Name.L == database.name)
        {
            return Err(format!(
                "system database {} has a different reserved ID",
                database.name
            ));
        }
        meta.create_database(&astersql_meta_model::DBInfo {
            ID: database.id,
            Name: astersql_meta_model::ast::NewCIStr(database.name),
            Charset: "utf8mb4".into(),
            Collate: "utf8mb4_bin".into(),
            State: astersql_meta_model::SchemaState::Public,
            ..Default::default()
        })?;
        self.changed = true;
        Ok(())
    }
    fn create_and_split_system_table(
        &mut self,
        database_id: i64,
        definition: crate::bootstrap::TableBasicInfo,
    ) -> Result<(), String> {
        let mut parser = astersql_parser::New();
        parser.SetSQLMode(astersql_parser_mysql::r#const::ModeNone);
        let statement = parser
            .ParseOneStmt(definition.create_sql, "", "")
            .map_err(|e| e.to_string())?;
        let statement = statement
            .as_any()
            .downcast_ref::<astersql_parser_ast::CreateTableStmt>()
            .ok_or("system table definition is not CREATE TABLE")?;
        let context = astersql_meta_metabuild::NewContext::<(), std::convert::Infallible>(vec![]);
        let mut table =
            astersql_ddl::BuildTableInfoFromAST(&context, statement).map_err(|e| e.to_string())?;
        crate::bootstrap::checkSystemTableConstraint::<String>(
            &crate::bootstrap::SystemTableInfo {
                partitioned: table.Partition.is_some(),
                separate_auto_increment: table.AutoIDCache == 1,
            },
        )
        .map_err(|e| format!("invalid bootstrap table {}: {e:?}", definition.name))?;
        table.ID = definition.id;
        table.DBID = database_id;
        table.State = astersql_meta_model::SchemaState::Public;
        table.UpdateTS = self.txn.StartTS();
        astersql_meta::TransactionMutator::new(self.txn).create_table(database_id, &table)?;
        self.changed = true;
        Ok(())
    }
    fn set_nextgen_schema_version(&mut self, version: i32) -> Result<(), String> {
        self.txn
            .Set(
                astersql_meta::transaction_meta_string_key(b"BootTableVersion"),
                version.to_string().into_bytes(),
            )
            .map_err(|e| e.to_string())
    }
}
fn bootstrap_canonical_nextgen_schemas(domain: &Arc<Domain>) -> SessionResult<()> {
    domain.storage_handle().with_storage(|store| {
        kv::RunInNewTxn(&kv::Context::default(), store, true, |_, txn| {
            let mut runtime = CanonicalBootstrapSchemaRuntime {txn, changed:false};
            crate::bootstrap::bootstrapSchemas(&mut runtime).map_err(|e|kv::errors::New(format!("bootstrap reserved schemas: {e:?}")))?;
            if runtime.changed {
                let version = astersql_meta::TransactionMutator::new(runtime.txn).gen_schema_version().map_err(kv::errors::New)?;
                runtime.txn.Set(astersql_meta::transaction_meta_string_key(format!("Diff:{version}").as_bytes()), serde_json::to_vec(&serde_json::json!({"version":version,"type":0,"schema_id":0,"table_id":0,"regenerate_schema_map":true})).map_err(|e|kv::errors::New(e.to_string()))?)?;
            }
            Ok(())
        })
    }).map_err(|e|session_error("bootstrap NextGen system metadata",e))?;
    domain
        .reload()
        .map_err(|e| session_error("load bootstrapped reserved schemas", e))?;
    // The canonical SQL/statistics bridge also receives tables created directly
    // through Go metadata, with their reserved identities unchanged.
    let registered = domain.stats_context().catalog();
    for database in domain.info_schema().AllSchemas() {
        for table in &database.tables {
            if let Some(table) = &table.model_meta {
                if !registered.contains_key(&(database.name.lower.clone(), table.Name.L.clone())) {
                    domain
                        .register_stats_table(&database.name.lower, table.as_ref().clone())
                        .map_err(|e| session_error("publish bootstrapped system table", e))?;
                }
            }
        }
    }
    Ok(())
}

/// Create the masking table through KV metadata before classic upgrade DDL can
/// consult it. The caller has re-read the version while holding the owner lock.
pub(super) fn init_bootstrap_dependent_tables(
    domain: &Arc<Domain>,
    version: Option<i64>,
) -> SessionResult<()> {
    if astersql_config_kerneltype::IsNextGen()
        || !version.is_some_and(|v| v > 0 && v < crate::upgrade_def::version260)
        || unsafe { crate::upgrade_def::currentBootstrapVersion } < crate::upgrade_def::version260
    {
        return Ok(());
    }
    let mut created = false;
    domain.storage_handle().with_storage(|store| {
        let context = kv::WithInternalSourceType(kv::Context::default(), kv::InternalTxnDDL);
        kv::RunInNewTxn(&context, store, true, |_, txn| {
            created = false;
            let databases = astersql_meta::TransactionMutator::new(txn)
                .list_databases().map_err(kv::errors::New)?;
            let database_id = databases.iter().find(|db| db.Name.L == "mysql")
                .map_or(astersql_meta_metadef::SystemDatabaseID, |db| db.ID);
            let mut runtime = CanonicalBootstrapSchemaRuntime { txn, changed: false };
            if !databases.iter().any(|db| db.Name.L == "mysql") {
                crate::bootstrap::BootstrapSchemaRuntime::create_system_database(
                    &mut runtime,
                    crate::bootstrap::DatabaseBasicInfo { id: database_id, name: "mysql", tables: &[] },
                ).map_err(kv::errors::New)?;
            }
            let tables = astersql_meta::TransactionMutator::new(runtime.txn)
                .list_tables(database_id).map_err(kv::errors::New)?;
            if tables.iter().any(|table| table.Name.L == "tidb_masking_policy") {
                return Ok(());
            }
            let id = kv::IncInt64(runtime.txn, &astersql_meta::transaction_meta_string_key(b"NextGlobalID"), 1)?;
            crate::bootstrap::BootstrapSchemaRuntime::create_and_split_system_table(
                &mut runtime, database_id,
                crate::bootstrap::TableBasicInfo {
                    id, name: "tidb_masking_policy",
                    create_sql: astersql_meta_metadef::CreateTiDBMaskingPolicyTable,
                },
            ).map_err(kv::errors::New)?;
            // Publish the directly created metadata through the canonical schema
            // bridge so subsequent upgrade SQL sees it without running CREATE DDL.
            let version = astersql_meta::TransactionMutator::new(runtime.txn)
                .gen_schema_version().map_err(kv::errors::New)?;
            runtime.txn.Set(astersql_meta::transaction_meta_string_key(format!("Diff:{version}").as_bytes()),
                serde_json::to_vec(&serde_json::json!({"version":version,"type":0,"regenerate_schema_map":true}))
                    .map_err(|e| kv::errors::New(e.to_string()))?)?;
            created = true;
            Ok(())
        })
    }).map_err(|e| session_error("initialize bootstrap-dependent masking table", e))?;
    if created {
        domain
            .reload()
            .map_err(|e| session_error("reload bootstrap-dependent table", e))?;
        let table = domain
            .table_by_name("mysql", "tidb_masking_policy")
            .map_err(|e| session_error("load bootstrap-dependent table", e))?;
        domain
            .register_stats_table("mysql", table.as_ref().clone())
            .map_err(|e| session_error("publish bootstrap-dependent table", e))?;
    }
    Ok(())
}

/// Persist mysql/sys bootstrap metadata into an initialized canonical Domain.
///
/// Virtual INFORMATION_SCHEMA, PERFORMANCE_SCHEMA and METRICS_SCHEMA tables are
/// intentionally absent here: `metadata_catalog` overlays those registries
/// without writing them to TiKV.
pub fn BootstrapCanonicalDomain(domain: Arc<Domain>) -> SessionResult<ConcreteSession> {
    if astersql_config_kerneltype::IsNextGen() {
        bootstrap_canonical_nextgen_schemas(&domain)?;
    }
    let session = ConcreteSession::new(Arc::clone(&domain));
    let previous_bootstrap_version = canonical_bootstrap_version(&session, &domain)?;
    // The serving factory holds the owner lock; this read observes upgrades by other nodes.
    if previous_bootstrap_version.is_some_and(|version| {
        version > 0 && version < unsafe { crate::upgrade_def::currentBootstrapVersion }
    }) && astersql_config_deploymode::IsStarter()
    {
        if let Some(manager) = domain.external_workload_manager() {
            let mut manager = manager
                .lock()
                .expect("external workload manager lock poisoned");
            let terminate = astersql_extworkload::AbortGCV2ForUpgrade(
                &astersql_extworkload::context::Background(),
                Some(manager.as_mut()),
            )
            .map_err(|e| SessionError::new(format!("abort GCV2 worker failed: {e}")))?;
            if terminate {
                if cfg!(test) {
                    return Ok(session);
                }
                return Err(SessionError::new(
                    "GCV2 worker aborted before bootstrap upgrade",
                ));
            }
        }
    }

    init_bootstrap_dependent_tables(&domain, previous_bootstrap_version)?;
    for database in ["mysql", "sys", "test"] {
        session.execute(&format!("CREATE DATABASE IF NOT EXISTS {database}"))?;
    }
    for definition in astersql_meta_metadef::BootstrapSystemTableDefinitions {
        // Fresh bootstrap owns creation; classic upgrades below v260 were
        // initialized directly above. A later restart must preserve user renames.
        if definition.name == "tidb_masking_policy"
            && previous_bootstrap_version.is_some_and(|v| v > 0)
        {
            continue;
        }
        if domain.stats_table("mysql", definition.name).is_none() {
            session.execute(definition.create_sql).map_err(|error| {
                SessionError::new(format!(
                    "bootstrap mysql.{} from authoritative DDL: {error}",
                    definition.name
                ))
            })?;
        }
    }
    ensure_canonical_ddl_system_tables(&session, &domain)?;
    upgrade_canonical_domain(&session, previous_bootstrap_version)?;
    session
        .execute(astersql_meta_metadef::CreateSysConfigTable)
        .map_err(|error| session_error("bootstrap sys.sys_config", error))?;
    session.execute(
        "INSERT IGNORE INTO sys.sys_config (variable, value) VALUES \
         ('statement_truncate_len', '64'), \
         ('statement_performance_analyzer.limit', '100'), \
         ('statement_performance_analyzer.view', NULL), \
         ('diagnostics.allow_i_s_tables', 'OFF'), \
         ('diagnostics.include_raw', 'OFF'), \
         ('ps_thread_trx_info.max_length', '65535')",
    )?;

    // Keep the canonical mock bootstrap's persistent rows aligned with Go's
    // doDMLWorks.  Creating only the table definitions makes later bootstrap
    // and upgrade tests observe an empty mysql.tidb/global_variables table.
    astersql_sessionctx_variable::register_builtin_sysvars();
    let quote_sql = |value: &str| value.replace('\'', "''");
    session.execute(
        "INSERT IGNORE INTO mysql.user (Host, User, authentication_string, plugin) \
         VALUES ('%', 'root', '', 'mysql_native_password')",
    )?;
    let privilege_handle = runtime_privilege_handle(&domain);
    let mut privileges = privilege_handle.Get();
    if !privileges
        .user
        .iter()
        .any(|record| record.base.fullyMatch("root", "%"))
    {
        let mut root = astersql_privilege_privileges::NewUserRecord("%", "root");
        root.Privileges = astersql_privilege_privileges::computePrivMask(
            astersql_privilege_privileges::ALL_GLOBAL_PRIVS,
        ) | astersql_privilege_privileges::GrantPriv;
        privileges.user.push(root);
        privileges.SortUserTable();
        privilege_handle.merge(privileges);
    }
    for variable in astersql_sessionctx_variable::GetSysVars().into_values() {
        if variable.HasGlobalScope() {
            session.execute(&format!(
                "INSERT IGNORE INTO mysql.global_variables (VARIABLE_NAME, VARIABLE_VALUE) \
                 VALUES ('{}', '{}')",
                quote_sql(&variable.Name),
                quote_sql(&variable.Value),
            ))?;
        }
    }
    // SAFETY: bootstrap only reads the shared compatibility version.
    let bootstrap_version = unsafe { crate::upgrade_def::currentBootstrapVersion }.to_string();
    for (name, value, comment) in [
        ("bootstrapped", "True", "Bootstrap flag"),
        (
            "tidb_server_version",
            bootstrap_version.as_str(),
            "TiDB bootstrap version",
        ),
        ("ddl_table_version", "4", "DDL table version"),
        ("new_collation_enabled", "False", "New collation flag"),
        ("system_tz", "CST", "System timezone"),
    ] {
        session.execute(&format!(
            "INSERT INTO mysql.tidb (VARIABLE_NAME, VARIABLE_VALUE, COMMENT) \
             VALUES ('{}', '{}', '{}') ON DUPLICATE KEY UPDATE \
             VARIABLE_VALUE='{}', COMMENT='{}'",
            quote_sql(name),
            quote_sql(value),
            quote_sql(comment),
            quote_sql(value),
            quote_sql(comment),
        ))?;
    }
    if domain.stats_table("sys", "schema_unused_indexes").is_none() {
        let table =
            build_bootstrap_view_table(astersql_meta_metadef::CreateSchemaUnusedIndexesView, None)?;
        domain
            .ddl_create_table("sys", table, true)
            .map_err(|error| session_error("persist sys.schema_unused_indexes view", error))?;
    }
    // Bootstrap may have inserted missing rows after constructing the session.
    // Refresh the authoritative catalog and its hooks before exposing it to the
    // server factory; a previous Domain override must not mask a backfill.
    for name in [
        astersql_sessionctx_vardef::TiDBAnalyzeDefaultNumBuckets,
        astersql_sessionctx_vardef::TiDBAnalyzeDefaultNumTopN,
        astersql_sessionctx_vardef::TiDBPersistAnalyzeOptions,
    ] {
        let mut sets = session.execute(&format!(
            "SELECT variable_value FROM mysql.global_variables WHERE variable_name='{name}'"
        ))?;
        if let Some(set) = sets.first_mut()
            && let Some(row) = set.next_row()?
        {
            let value = &row[0];
            session
                .session_vars
                .ValidateAndSetGlobalSystemVar(name, value, astersql_sessionctx_vardef::ScopeGlobal)
                .map_err(|error| session_error("load bootstrap ANALYZE variable", error))?;
            domain.set_global_system_variable(name, value);
        }
    }
    Ok(session)
}

/// Builds the same concrete Domain/session pair used by the canonical runtime,
/// backed by the real transactional mock KV storage rather than a SQL result map.
/// 创建用于 ANALYZE 测试的 Domain 与 ConcreteSession。
pub fn CreateAnalyzeSession() -> SessionResult<(Arc<Domain>, ConcreteSession)> {
    let storage = Arc::try_unwrap(
        astersql_store_mockstore_mockstorage::NewMockStorage(
            astersql_store_mockstore_mockstorage::KVStore::NewMemoryWithWallClockTSO(),
            None,
        )
        .map_err(|error| session_error("create analyze mock KV storage", error))?,
    )
    .map_err(|_| SessionError::new("analyze mock KV storage retained an unexpected owner"))?;
    let mut config = DomainConfig::default();
    config.schema_lease = Duration::ZERO;
    config.stats_lease = Duration::ZERO;
    if astersql_config::get_global_config()
        .performance
        .enable_stats_cache_mem_quota
    {
        config.stats_cache_capacity = astersql_sessionctx_vardef::StatsCacheMemQuota.Load();
    }
    let domain = Arc::new(Domain::new(
        storage,
        Arc::new(KvInfoSchemaLoader::new()),
        config,
    ));
    domain
        .init()
        .map_err(|error| session_error("initialize analyze Domain", error))?;
    let session = BootstrapCanonicalDomain(Arc::clone(&domain))?;
    if let Err(error) = domain.initialize_stats() {
        BgLogger().log(
            LogLevel::Error,
            "initialize statistics failed",
            [LogField::String("error".to_owned(), error.to_string())],
        );
    }
    Ok((domain, session))
}

/// 具体测试运行时：mock 存储工厂、schema 加载器与 next-gen 钩子。
pub struct ConcreteTestRuntime<S, F> {
    storage_factory: F,
    schema_loader: Arc<dyn InfoSchemaLoader>,
    next_gen: bool,
    update_next_gen: Arc<dyn Fn() + Send + Sync>,
    marker: std::marker::PhantomData<fn() -> S>,
}

impl<S, F> ConcreteTestRuntime<S, F> {
    pub fn new(
        storage_factory: F,
        schema_loader: Arc<dyn InfoSchemaLoader>,
        next_gen: bool,
    ) -> Self {
        Self {
            storage_factory,
            schema_loader,
            next_gen,
            update_next_gen: Arc::new(|| {}),
            marker: std::marker::PhantomData,
        }
    }

    /// 设置 next-gen 升级回调。
    pub fn with_next_gen_update(mut self, update: Arc<dyn Fn() + Send + Sync>) -> Self {
        self.update_next_gen = update;
        self
    }
}

impl<S, F> TestRuntime for ConcreteTestRuntime<S, F>
where
    S: kv::Storage + Send + Sync + 'static,
    F: Fn() -> SessionResult<S> + Send + Sync,
{
    fn SetMaxProcsForTest(&self) {}

    fn IsNextGen(&self) -> bool {
        self.next_gen
    }

    fn UpdateConfigForNextgen(&self) {
        (self.update_next_gen)();
    }

    fn NewMockStore(&self) -> SessionResult<Arc<dyn TestStore>> {
        Ok(Arc::new(RuntimeStore {
            storage: Mutex::new(Some((self.storage_factory)()?)),
            domain: RwLock::new(None),
        }))
    }

    fn BootstrapSession(&self, store: Arc<dyn TestStore>) -> SessionResult<Arc<dyn TestDomain>> {
        let store = store
            .as_any()
            .downcast_ref::<RuntimeStore<S>>()
            .ok_or_else(|| SessionError::new("test store belongs to another runtime"))?;
        let storage = store
            .storage
            .lock()
            .expect("runtime store lock poisoned")
            .take()
            .ok_or_else(|| SessionError::new("test store is already bootstrapped"))?;
        let domain = Arc::new(Domain::new_mock(storage, Arc::clone(&self.schema_loader)));
        domain
            .init()
            .map_err(|error| session_error("bootstrap domain", error))?;
        *store.domain.write().expect("runtime domain lock poisoned") = Some(Arc::clone(&domain));
        Ok(Arc::new(RuntimeDomain { domain }))
    }

    fn CreateSession4Test(&self, store: Arc<dyn TestStore>) -> SessionResult<Arc<dyn TestSession>> {
        let store = store
            .as_any()
            .downcast_ref::<RuntimeStore<S>>()
            .ok_or_else(|| SessionError::new("test store belongs to another runtime"))?;
        let domain = store
            .domain
            .read()
            .expect("runtime domain lock poisoned")
            .clone()
            .ok_or_else(|| SessionError::new("test store is not bootstrapped"))?;
        Ok(Arc::new(ConcreteSession::new(domain)))
    }

    fn ArgsToExpressions(&self, arguments: &[String]) -> Vec<String> {
        arguments.to_vec()
    }
}

struct BootstrapOwnerLock {
    runtime: Arc<tokio::runtime::Runtime>,
    lock: Option<astersql_owner::DistributedLock>,
}

impl Drop for BootstrapOwnerLock {
    fn drop(&mut self) {
        if let Some(lock) = self.lock.take() {
            if let Err(error) = self.runtime.block_on(lock.release()) {
                BgLogger().log(
                    LogLevel::Warn,
                    "release bootstrap owner lock failed",
                    [LogField::String("error".into(), error.to_string())],
                );
            }
        }
    }
}

pub(super) fn acquire_bootstrap_upgrade_lock<G>(
    domain: &Arc<Domain>,
    acquire: impl FnOnce() -> SessionResult<G>,
) -> SessionResult<Option<G>> {
    let session = ConcreteSession::new(domain.clone());
    let version = canonical_bootstrap_version(&session, domain)?;
    if version.is_some_and(|v| v > 0 && v < unsafe { crate::upgrade_def::currentBootstrapVersion })
    {
        acquire().map(Some)
    } else {
        Ok(None)
    }
}

pub(super) fn load_external_gc_lifetime(domain: &Arc<Domain>) -> SessionResult<Duration> {
    let session = ConcreteSession::new(domain.clone());
    // Match getTiDBTableValue: missing rows and storage-read errors use the registered default.
    let stored = session
        .execute("SELECT VARIABLE_VALUE FROM mysql.tidb WHERE VARIABLE_NAME='tikv_gc_life_time'")
        .ok()
        .and_then(|mut records| {
            records
                .first_mut()
                .and_then(|r| r.next_row().ok().flatten())
                .and_then(|row| row.first().cloned())
        });
    let value = stored.unwrap_or_else(|| "10m0s".into());
    let nanos = astersql_sessionctx_variable::parse_go_duration(&value)
        .and_then(|value| u64::try_from(value).ok())
        .ok_or_else(|| SessionError::new(format!("invalid effective GC lifetime {value:?}")))?;
    Ok(Duration::from_nanos(nanos))
}

pub(super) fn initialize_external_workload_gcv2(domain: &Arc<Domain>) {
    let Some(manager) = domain.external_workload_manager() else {
        return;
    };
    {
        let manager = manager
            .lock()
            .expect("external workload manager lock poisoned");
        if manager.Role() != astersql_extworkload::config::RoleMaster
            || !astersql_extworkload::IsKeyspaceUsingKeyspaceLevelGC(manager.Meta())
        {
            return;
        }
    }
    let result = load_external_gc_lifetime(domain).and_then(|lifetime| {
        manager
            .lock()
            .expect("external workload manager lock poisoned")
            .InitializeGCV2(&astersql_extworkload::context::Background(), lifetime)
            .map_err(|e| SessionError::new(e.to_string()))
    });
    if let Err(error) = result {
        BgLogger().log(
            LogLevel::Warn,
            "initialize external workload GCV2 failed",
            [LogField::String("error".into(), error.to_string())],
        );
        domain.set_external_workload_manager(None);
    }
}

pub(super) fn notify_external_workload_gc_lifetime(domain: &Arc<Domain>) {
    let Some(manager) = domain.external_workload_manager() else {
        return;
    };
    if !astersql_extworkload::IsKeyspaceUsingKeyspaceLevelGC(
        manager
            .lock()
            .expect("external workload manager lock poisoned")
            .Meta(),
    ) {
        return;
    }
    let result = load_external_gc_lifetime(domain).and_then(|lifetime| {
        manager
            .lock()
            .expect("external workload manager lock poisoned")
            .UpdateGCLifeTime(&astersql_extworkload::context::Background(), lifetime)
            .map_err(|e| SessionError::new(e.to_string()))
    });
    if let Err(error) = result {
        BgLogger().log(
            LogLevel::Warn,
            "update external workload GC lifetime failed",
            [LogField::String("error".into(), error.to_string())],
        );
    }
}

pub(super) fn install_external_workload_manager(
    domain: &Arc<Domain>,
    role: &str,
    meta: SessionResult<astersql_extworkload::keyspacepb::KeyspaceMeta>,
    create: impl FnOnce(
        &astersql_extworkload::keyspacepb::KeyspaceMeta,
    ) -> SessionResult<Option<Box<dyn astersql_extworkload::Manager>>>,
) -> SessionResult<()> {
    let manager = meta.and_then(|meta| {
        if role == astersql_extworkload::config::RoleGCV2Worker
            && !astersql_extworkload::IsKeyspaceUsingKeyspaceLevelGC(Some(&meta))
        {
            return Err(SessionError::new(
                "external workload GCV2 role requires keyspace-level GC",
            ));
        }
        create(&meta)
    });
    match manager {
        Ok(manager) => domain.set_external_workload_manager(manager),
        Err(error) if role == astersql_extworkload::config::RoleGCV2Worker => {
            domain.close();
            return Err(error);
        }
        Err(error) => BgLogger().log(
            LogLevel::Warn,
            "initialize external workload manager failed",
            [LogField::String("error".into(), error.to_string())],
        ),
    }
    Ok(())
}

impl ConcreteSession {
    /// Starter SQL uses the normal executor with the same internal SQL mode as
    /// Go's bootstrap session. Restore the caller's mode on every error path.
    pub(crate) fn with_starter_restricted_sql<T>(
        &self,
        operation: impl FnOnce() -> SessionResult<T>,
    ) -> SessionResult<T> {
        struct Restore<'a>(&'a ConcreteSession, bool);
        impl Drop for Restore<'_> {
            fn drop(&mut self) {
                self.0.state.borrow_mut().in_restricted_sql = self.1;
            }
        }
        let previous = self.state.borrow().in_restricted_sql;
        self.state.borrow_mut().in_restricted_sql = true;
        let _restore = Restore(self, previous);
        operation()
    }

    pub(crate) fn starter_statement_count(&self, sql: &str) -> SessionResult<usize> {
        let state = self.state.borrow();
        let mode = astersql_parser_mysql::r#const::GetSQLMode(&state.sql_mode)
            .map_err(|e| session_error("parse sql_mode", e))?;
        let mode = astersql_parser_mysql::r#const::DelSQLMode(
            mode,
            astersql_parser_mysql::r#const::ModeNoBackslashEscapes,
        );
        drop(state);
        parse_with_sql_mode(sql, mode).map(|statements| statements.len())
    }

    pub(crate) fn set_starter_clustered_index_mode(&self) {
        self.state.borrow_mut().clustered_index_def_mode =
            astersql_sessionctx_vardef::ClusteredIndexDefModeIntOnly;
    }
}

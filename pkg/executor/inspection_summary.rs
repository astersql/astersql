// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// 巡检摘要执行器：按规则从 `metrics_schema` 汇总指标行。
//
// 摘要规则（summary rule）将一组相关指标表归为一类（如 query-summary、
// wait-events、read-link 等），对时间窗口内的 avg/min/max 做聚合，
// 供 `INFORMATION_SCHEMA` 巡检摘要表展示。

#![allow(non_snake_case)]

use std::collections::{HashMap, HashSet};

/// 从请求中抽出的过滤条件：是否跳过、启用的规则/指标名、分位数列表。
pub struct InspectionSummaryExtractor {
    pub skip_inspection: bool,
    pub rules: HashSet<String>,
    pub metric_names: HashSet<String>,
    pub quantiles: Vec<f64>,
}

#[derive(Clone, Debug, PartialEq)]
/// 单个 metrics 表的元信息：标签列、注释、是否带 quantile。
pub struct MetricDefinition {
    pub labels: Vec<String>,
    pub comment: String,
    pub quantile: f64,
}

#[derive(Clone, Debug, PartialEq)]
/// 摘要结果单元格取值（字符串 / 浮点 / NULL）。
pub enum InspectionSummaryValue {
    String(String),
    Float(f64),
    Null,
}

/// 运行时依赖：查 metric 定义、执行受限 SQL、读写行字段。
pub trait InspectionSummaryRuntime {
    type Context;
    type MetricRow;
    type Error: std::fmt::Display + From<String>;

    fn metric_definition(&self, name: &str) -> Option<MetricDefinition>;
    fn append_warning(&mut self, warning: String);
    fn execute_restricted_sql(
        &mut self,
        context: &mut Self::Context,
        sql: &str,
    ) -> Result<Vec<Self::MetricRow>, Self::Error>;
    fn row_len(&self, row: &Self::MetricRow) -> usize;
    fn row_string(&self, row: &Self::MetricRow, column: usize) -> String;
    fn row_float(&self, row: &Self::MetricRow, column: usize) -> f64;
}

/// 摘要行检索器：按规则表扫描并物化结果行（只执行一次）。
pub struct InspectionSummaryRetriever<R: InspectionSummaryRuntime> {
    pub runtime: R,
    pub retrieved: bool,
    pub extractor: InspectionSummaryExtractor,
    pub time_range_condition: String,
}

// 全局摘要规则表：每个 summary 规则名对应一组 metrics_schema 表名。
// inspectionSummaryRules is used to maintain
// 对应 Go 的全局规则表：每个 summary rule 维护一组需要汇总的 metrics_schema 表名。
pub fn inspectionSummaryRules() -> HashMap<&'static str, Vec<&'static str>> {
    HashMap::from([
        (
            "query-summary",
            vec![
                "tidb_connection_count",
                "tidb_query_duration",
                "tidb_qps_ideal",
                "tidb_qps",
                "tidb_ops_internal",
                "tidb_ops_statement",
                "tidb_failed_query_opm",
                "tidb_slow_query_duration",
                "tidb_slow_query_cop_wait_duration",
                "tidb_slow_query_cop_process_duration",
            ],
        ),
        (
            "wait-events",
            vec![
                "tidb_get_token_duration",
                "tidb_load_schema_duration",
                "tidb_query_duration",
                "tidb_parse_duration",
                "tidb_compile_duration",
                "tidb_execute_duration",
                "tidb_auto_id_request_duration",
                "pd_tso_wait_duration",
                "pd_tso_rpc_duration",
                "tidb_distsql_execution_duration",
                "pd_start_tso_wait_duration",
                "tidb_transaction_local_latch_wait_duration",
                "tidb_transaction_duration",
                "pd_request_rpc_duration",
                "tidb_cop_duration",
                "tidb_batch_client_wait_duration",
                "tidb_batch_client_unavailable_duration",
                "tidb_kv_backoff_duration",
                "tidb_kv_request_duration",
                "pd_client_cmd_duration",
                "tikv_grpc_message_duration",
                "tikv_average_grpc_messge_duration",
                "tikv_channel_full",
                "tikv_scheduler_is_busy",
                "tikv_coprocessor_is_busy",
                "tikv_engine_write_stall",
                "tikv_raftstore_apply_log_avg_duration",
                "tikv_raftstore_apply_log_duration",
                "tikv_raftstore_append_log_avg_duration",
                "tikv_raftstore_append_log_duration",
                "tikv_raftstore_commit_log_avg_duration",
                "tikv_raftstore_commit_log_duration",
                "tikv_raftstore_process_duration",
                "tikv_raftstore_propose_wait_duration",
                "tikv_propose_avg_wait_duration",
                "tikv_raftstore_apply_wait_duration",
                "tikv_apply_avg_wait_duration",
                "tikv_check_split_duration",
                "tikv_storage_async_request_duration",
                "tikv_storage_async_request_avg_duration",
                "tikv_scheduler_command_duration",
                "tikv_scheduler_command_avg_duration",
                "tikv_scheduler_latch_wait_duration",
                "tikv_scheduler_latch_wait_avg_duration",
                "tikv_send_snapshot_duration",
                "tikv_handle_snapshot_duration",
                "tikv_cop_request_durations",
                "tikv_cop_request_duration",
                "tikv_cop_handle_duration",
                "tikv_cop_wait_duration",
                "tikv_engine_max_get_duration",
                "tikv_engine_avg_get_duration",
                "tikv_engine_avg_seek_duration",
                "tikv_engine_write_duration",
                "tikv_wal_sync_max_duration",
                "tikv_wal_sync_duration",
                "tikv_compaction_max_duration",
                "tikv_compaction_duration",
                "tikv_sst_read_max_duration",
                "tikv_sst_read_duration",
                "tikv_write_stall_max_duration",
                "tikv_write_stall_avg_duration",
                "tikv_oldest_snapshots_duration",
                "tikv_ingest_sst_duration",
                "tikv_ingest_sst_avg_duration",
                "tikv_engine_blob_seek_duration",
                "tikv_engine_blob_get_duration",
                "tikv_engine_blob_file_read_duration",
                "tikv_engine_blob_file_write_duration",
                "tikv_engine_blob_file_sync_duration",
                "tikv_lock_manager_waiter_lifetime_avg_duration",
                "tikv_lock_manager_deadlock_detect_duration",
                "tikv_lock_manager_deadlock_detect_avg_duration",
            ],
        ),
        (
            "read-link",
            vec![
                "tidb_get_token_duration",
                "tidb_parse_duration",
                "tidb_compile_duration",
                "pd_tso_rpc_duration",
                "pd_tso_wait_duration",
                "tidb_execute_duration",
                "tidb_expensive_executors_ops",
                "tidb_query_using_plan_cache_ops",
                "tidb_distsql_execution_duration",
                "tidb_distsql_partial_num",
                "tidb_distsql_partial_qps",
                "tidb_distsql_partial_scan_key_num",
                "tidb_distsql_qps",
                "tidb_distsql_scan_key_num",
                "tidb_distsql_copr_cache",
                "tidb_region_cache_ops",
                "tidb_batch_client_pending_req_count",
                "tidb_batch_client_unavailable_duration",
                "tidb_batch_client_wait_duration",
                "tidb_kv_backoff_duration",
                "tidb_kv_backoff_ops",
                "tidb_kv_region_error_ops",
                "tidb_kv_request_duration",
                "tidb_kv_request_ops",
                "tidb_kv_snapshot_ops",
                "tidb_kv_txn_ops",
                "tikv_average_grpc_messge_duration",
                "tikv_grpc_avg_req_batch_size",
                "tikv_grpc_avg_resp_batch_size",
                "tikv_grpc_errors",
                "tikv_grpc_message_duration",
                "tikv_grpc_qps",
                "tikv_grpc_req_batch_size",
                "tikv_grpc_resp_batch_size",
                "tidb_cop_duration",
                "tikv_cop_wait_duration",
                "tikv_coprocessor_is_busy",
                "tikv_coprocessor_request_error",
                "tikv_cop_handle_duration",
                "tikv_cop_kv_cursor_operations",
                "tikv_cop_request_duration",
                "tikv_cop_request_durations",
                "tikv_cop_scan_details",
                "tikv_cop_dag_executors_ops",
                "tikv_cop_dag_requests_ops",
                "tikv_cop_scan_keys_num",
                "tikv_cop_requests_ops",
                "tikv_cop_total_response_size_per_seconds",
                "tikv_cop_total_rocksdb_perf_statistics",
                "tikv_channel_full",
                "tikv_engine_avg_get_duration",
                "tikv_engine_avg_seek_duration",
                "tikv_handle_snapshot_duration",
                "tikv_block_all_cache_hit",
                "tikv_block_bloom_prefix_cache_hit",
                "tikv_block_cache_size",
                "tikv_block_data_cache_hit",
                "tikv_block_filter_cache_hit",
                "tikv_block_index_cache_hit",
                "tikv_engine_get_block_cache_operations",
                "tikv_engine_get_cpu_cache_operations",
                "tikv_engine_get_memtable_operations",
                "tikv_per_read_avg_bytes",
                "tikv_per_read_max_bytes",
            ],
        ),
        (
            "write-link",
            vec![
                "tidb_get_token_duration",
                "tidb_parse_duration",
                "tidb_compile_duration",
                "pd_tso_rpc_duration",
                "pd_tso_wait_duration",
                "tidb_execute_duration",
                "tidb_transaction_duration",
                "tidb_transaction_local_latch_wait_duration",
                "tidb_transaction_ops",
                "tidb_transaction_retry_error_ops",
                "tidb_transaction_retry_num",
                "tidb_transaction_statement_num",
                "tidb_auto_id_qps",
                "tidb_auto_id_request_duration",
                "tidb_region_cache_ops",
                "tidb_kv_backoff_duration",
                "tidb_kv_backoff_ops",
                "tidb_kv_region_error_ops",
                "tidb_kv_request_duration",
                "tidb_kv_request_ops",
                "tidb_kv_snapshot_ops",
                "tidb_kv_txn_ops",
                "tidb_kv_write_num",
                "tidb_kv_write_size",
                "tikv_average_grpc_messge_duration",
                "tikv_grpc_avg_req_batch_size",
                "tikv_grpc_avg_resp_batch_size",
                "tikv_grpc_errors",
                "tikv_grpc_message_duration",
                "tikv_grpc_qps",
                "tikv_grpc_req_batch_size",
                "tikv_grpc_resp_batch_size",
                "tikv_scheduler_command_avg_duration",
                "tikv_scheduler_command_duration",
                "tikv_scheduler_is_busy",
                "tikv_scheduler_keys_read_avg",
                "tikv_scheduler_keys_read",
                "tikv_scheduler_keys_written_avg",
                "tikv_scheduler_keys_written",
                "tikv_scheduler_latch_wait_avg_duration",
                "tikv_scheduler_latch_wait_duration",
                "tikv_scheduler_pending_commands",
                "tikv_scheduler_priority_commands",
                "tikv_scheduler_scan_details",
                "tikv_scheduler_stage",
                "tikv_scheduler_writing_bytes",
                "tikv_propose_avg_wait_duration",
                "tikv_raftstore_propose_wait_duration",
                "tikv_raftstore_append_log_avg_duration",
                "tikv_raftstore_append_log_duration",
                "tikv_raftstore_commit_log_avg_duration",
                "tikv_raftstore_commit_log_duration",
                "tikv_apply_avg_wait_duration",
                "tikv_raftstore_apply_log_avg_duration",
                "tikv_raftstore_apply_log_duration",
                "tikv_raftstore_apply_wait_duration",
                "tikv_engine_wal_sync_operations",
                "tikv_engine_write_duration",
                "tikv_engine_write_operations",
                "tikv_engine_write_stall",
                "tikv_write_stall_avg_duration",
                "tikv_write_stall_max_duration",
                "tikv_write_stall_reason",
            ],
        ),
        (
            "ddl",
            vec![
                "tidb_ddl_add_index_speed",
                "tidb_ddl_batch_add_index_duration",
                "tidb_ddl_deploy_syncer_duration",
                "tidb_ddl_duration",
                "tidb_ddl_meta_opm",
                "tidb_ddl_opm",
                "tidb_ddl_update_self_version_duration",
                "tidb_ddl_waiting_jobs_num",
                "tidb_ddl_worker_duration",
            ],
        ),
        (
            "stats",
            vec![
                "tidb_statistics_auto_analyze_duration",
                "tidb_statistics_auto_analyze_ops",
                "tidb_statistics_pseudo_estimation_ops",
                "tidb_statistics_stats_inaccuracy_rate",
                "tidb_statistics_update_stats_ops",
            ],
        ),
        (
            "gc",
            vec![
                "tidb_gc_action_result_opm",
                "tidb_gc_config",
                "tidb_gc_delete_range_fail_opm",
                "tidb_gc_delete_range_task_status",
                "tidb_gc_duration",
                "tidb_gc_fail_opm",
                "tidb_gc_push_task_duration",
                "tidb_gc_too_many_locks_opm",
                "tidb_gc_worker_action_opm",
                "tikv_engine_blob_gc_duration",
                "tikv_auto_gc_progress",
                "tikv_auto_gc_safepoint",
                "tikv_auto_gc_working",
                "tikv_gc_fail_tasks",
                "tikv_gc_keys",
                "tikv_gc_skipped_tasks",
                "tikv_gc_speed",
                "tikv_gc_tasks_avg_duration",
                "tikv_gc_tasks_duration",
                "tikv_gc_too_busy",
                "tikv_gc_tasks_ops",
            ],
        ),
        (
            "rocksdb",
            vec![
                "tikv_compaction_duration",
                "tikv_compaction_max_duration",
                "tikv_compaction_operations",
                "tikv_compaction_pending_bytes",
                "tikv_compaction_reason",
                "tikv_write_stall_avg_duration",
                "tikv_write_stall_max_duration",
                "tikv_write_stall_reason",
                "store_available_ratio",
                "store_size_amplification",
                "tikv_engine_avg_get_duration",
                "tikv_engine_avg_seek_duration",
                "tikv_engine_blob_bytes_flow",
                "tikv_engine_blob_file_count",
                "tikv_engine_blob_file_read_duration",
                "tikv_engine_blob_file_size",
                "tikv_engine_blob_file_sync_duration",
                "tikv_engine_blob_file_sync_operations",
                "tikv_engine_blob_file_write_duration",
                "tikv_engine_blob_gc_bytes_flow",
                "tikv_engine_blob_gc_duration",
                "tikv_engine_blob_gc_file",
                "tikv_engine_blob_gc_keys_flow",
                "tikv_engine_blob_get_duration",
                "tikv_engine_blob_key_avg_size",
                "tikv_engine_blob_key_max_size",
                "tikv_engine_blob_seek_duration",
                "tikv_engine_blob_seek_operations",
                "tikv_engine_blob_value_avg_size",
                "tikv_engine_blob_value_max_size",
                "tikv_engine_compaction_flow_bytes",
                "tikv_engine_get_block_cache_operations",
                "tikv_engine_get_cpu_cache_operations",
                "tikv_engine_get_memtable_operations",
                "tikv_engine_live_blob_size",
                "tikv_engine_max_get_duration",
                "tikv_engine_max_seek_duration",
                "tikv_engine_seek_operations",
                "tikv_engine_size",
                "tikv_engine_wal_sync_operations",
                "tikv_engine_write_duration",
                "tikv_engine_write_operations",
                "tikv_engine_write_stall",
            ],
        ),
        (
            "pd",
            vec![
                "pd_scheduler_balance_region",
                "pd_balance_scheduler_status",
                "pd_checker_event_count",
                "pd_client_cmd_duration",
                "pd_client_cmd_ops",
                "pd_cluster_metadata",
                "pd_cluster_status",
                "pd_grpc_completed_commands_duration",
                "pd_grpc_completed_commands_rate",
                "pd_request_rpc_duration",
                "pd_request_rpc_ops",
                "pd_request_rpc_duration_avg",
                "pd_handle_transactions_duration",
                "pd_handle_transactions_rate",
                "pd_hotspot_status",
                "pd_label_distribution",
                "pd_operator_finish_duration",
                "pd_operator_step_finish_duration",
                "pd_peer_round_trip_duration",
                "pd_region_health",
                "pd_region_heartbeat_duration",
                "pd_region_label_isolation_level",
                "pd_region_syncer_status",
                "pd_role",
                "pd_schedule_filter",
                "pd_schedule_operator",
                "pd_schedule_store_limit",
                "pd_scheduler_balance_direction",
                "pd_scheduler_balance_leader",
                "pd_scheduler_config",
                "pd_scheduler_op_influence",
                "pd_scheduler_region_heartbeat",
                "pd_scheduler_status",
                "pd_scheduler_store_status",
                "pd_scheduler_tolerant_resource",
                "pd_server_etcd_state",
                "pd_start_tso_wait_duration",
            ],
        ),
        (
            "raftstore",
            vec![
                "tikv_approximate_avg_region_size",
                "tikv_approximate_region_size_histogram",
                "tikv_approximate_region_size",
                "tikv_raftstore_append_log_avg_duration",
                "tikv_raftstore_append_log_duration",
                "tikv_raftstore_commit_log_avg_duration",
                "tikv_raftstore_commit_log_duration",
                "tikv_apply_avg_wait_duration",
                "tikv_raftstore_apply_log_avg_duration",
                "tikv_raftstore_apply_log_duration",
                "tikv_raftstore_apply_wait_duration",
                "tikv_raftstore_process_duration",
                "tikv_raftstore_process_handled",
                "tikv_propose_avg_wait_duration",
                "tikv_raftstore_propose_wait_duration",
                "tikv_raft_dropped_messages",
                "tikv_raft_log_speed",
                "tikv_raft_message_avg_batch_size",
                "tikv_raft_message_batch_size",
                "tikv_raft_proposals_per_ready",
                "tikv_raft_proposals",
                "tikv_raft_sent_messages",
            ],
        ),
    ])
}

impl<R: InspectionSummaryRuntime> InspectionSummaryRetriever<R> {
    /// 物化摘要行：按规则过滤 → 拼 SQL → 执行 → 组装输出列。
    pub fn retrieve(
        &mut self,
        context: &mut R::Context,
    ) -> Result<Vec<Vec<InspectionSummaryValue>>, R::Error> {
        // 已取过或显式跳过则返回空，避免重复扫描。
        if self.retrieved || self.extractor.skip_inspection {
            return Ok(Vec::new());
        }
        self.retrieved = true;
        let mut final_rows = Vec::new();

        // 遍历全部摘要规则及其指标表。
        for (rule, tables) in inspectionSummaryRules() {
            if !self.extractor.rules.is_empty() && !self.extractor.rules.contains(rule) {
                continue;
            }
            for name in tables {
                if !self.extractor.metric_names.is_empty()
                    && !self.extractor.metric_names.contains(name)
                {
                    continue;
                }
                // 缺失的 metrics 表记 warning 后跳过。
                let Some(definition) = self.runtime.metric_definition(name) else {
                    self.runtime
                        .append_warning(format!("metrics table: {name} not found"));
                    continue;
                };
                let mut columns = definition.labels.clone();
                let mut condition = self.time_range_condition.clone();
                // 直方图类指标按分位数过滤；默认取 0.99。
                if definition.quantile > 0.0 {
                    let quantiles = &self.extractor.quantiles;
                    columns.push("quantile".to_string());
                    if quantiles.is_empty() {
                        condition.push_str(" and quantile=0.99");
                    } else {
                        condition.push_str(" and quantile in (");
                        condition.push_str(
                            &quantiles
                                .iter()
                                .map(|quantile| format!("{quantile:.6}"))
                                .collect::<Vec<_>>()
                                .join(","),
                        );
                        condition.push(')');
                    }
                }
                // 无标签时只聚合计值；有标签则 GROUP BY 标签列。
                let sql = if columns.is_empty() {
                    format!(
                        "select avg(value),min(value),max(value) from `metrics_schema`.`{name}` {condition}"
                    )
                } else {
                    let labels = columns.join("`,`");
                    format!(
                        "select avg(value),min(value),max(value),`{labels}` from `metrics_schema`.`{name}` {condition} group by `{labels}` order by `{labels}`"
                    )
                };
                let rows = self
                    .runtime
                    .execute_restricted_sql(context, &sql)
                    .map_err(|error| R::Error::from(format!("execute '{sql}' failed: {error}")))?;
                // instance 标签单独抽出作为实例列，其余拼成 label 字符串。
                let non_instance_label_index = usize::from(
                    definition
                        .labels
                        .first()
                        .is_some_and(|label| label == "instance"),
                );
                const SKIP_COLS: usize = 3;
                for row in rows {
                    let instance = if non_instance_label_index == 0 {
                        String::new()
                    } else {
                        self.runtime.row_string(&row, SKIP_COLS)
                    };
                    let mut label_values = Vec::new();
                    for (index, label) in definition.labels[non_instance_label_index..]
                        .iter()
                        .enumerate()
                    {
                        let mut value = self
                            .runtime
                            .row_string(&row, SKIP_COLS + non_instance_label_index + index);
                        // store 相关标签统一加 store_id: 前缀便于展示。
                        if label == "store" || label == "store_id" {
                            value = format!("store_id:{value}");
                        }
                        label_values.push(value);
                    }
                    let quantile = if definition.quantile > 0.0 {
                        InspectionSummaryValue::Float(
                            self.runtime.row_float(&row, self.runtime.row_len(&row) - 1),
                        )
                    } else {
                        InspectionSummaryValue::Null
                    };
                    final_rows.push(vec![
                        InspectionSummaryValue::String(rule.to_owned()),
                        InspectionSummaryValue::String(instance),
                        InspectionSummaryValue::String(name.to_owned()),
                        InspectionSummaryValue::String(label_values.join(", ")),
                        quantile,
                        InspectionSummaryValue::Float(self.runtime.row_float(&row, 0)),
                        InspectionSummaryValue::Float(self.runtime.row_float(&row, 1)),
                        InspectionSummaryValue::Float(self.runtime.row_float(&row, 2)),
                        InspectionSummaryValue::String(definition.comment.clone()),
                    ]);
                }
            }
        }
        Ok(final_rows)
    }
}

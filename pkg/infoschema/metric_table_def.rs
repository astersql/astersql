// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// METRICS_SCHEMA 指标表定义（PromQL 模板映射）。
//
// 将指标表名映射到 `MetricTableDef`（PromQL、Labels、Quantile、Comment 等）。
// 供 `information_schema` / `metrics_schema` 生成可查询的指标视图；
// 本文件只保存静态定义，不访问 Prometheus，也不执行查询。
// `$LABEL_CONDITIONS` / `$RANGE_DURATION` / `$QUANTILE` 为运行时替换占位符。

// 不访问 Prometheus，也不执行查询或其他业务动作。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

use std::collections::HashMap;
use std::sync::LazyLock;

use crate::metrics_schema::MetricTableDef;

// MetricTableMap 对应 Go 的同名导出映射，供指标 schema 初始化与查询生成逻辑查表。
// LazyLock 近似 Go 包变量初始化；每项缺失的字段沿用 MetricTableDef::EMPTY 中的 Go 零值。
// TODO: 与 Go 源码一致，后续可改为从系统表读取定义。
/// 表名 → MetricTableDef 的全局只读映射，对应 Go `MetricTableMap`。
/// 缺失字段使用 `MetricTableDef::EMPTY` 的零值；LazyLock 近似 Go 包级初始化。
pub static MetricTableMap: LazyLock<HashMap<&'static str, MetricTableDef>> = LazyLock::new(|| {
    HashMap::from([
        // —— TiDB 查询 / QPS / 慢查询等服务器侧指标 ——
        (
            "tidb_query_duration",
            MetricTableDef {
                PromQL: r###"histogram_quantile($QUANTILE, sum(rate(tidb_server_handle_query_duration_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,sql_type,instance))"###,
                Labels: &["instance", "sql_type"],
                Quantile: 0.90,
                Comment: "The quantile of TiDB query durations(second)",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_qps",
            MetricTableDef {
                PromQL: r###"sum(rate(tidb_server_query_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (result,type,instance)"###,
                Labels: &["instance", "type", "result"],
                Comment: "TiDB query processing numbers per second",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_qps_ideal",
            MetricTableDef {
                PromQL: r###"sum(tidb_server_connections) * sum(rate(tidb_server_handle_query_duration_seconds_count[$RANGE_DURATION])) / sum(rate(tidb_server_handle_query_duration_seconds_sum[$RANGE_DURATION]))"###,
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_ops_statement",
            MetricTableDef {
                PromQL: r###"sum(rate(tidb_executor_statement_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)"###,
                Labels: &["instance", "type"],
                Comment: "TiDB statement statistics",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_failed_query_opm",
            MetricTableDef {
                PromQL: r###"sum(increase(tidb_server_execute_error_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type, instance)"###,
                Labels: &["instance", "type"],
                Comment: "TiDB failed query opm",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_slow_query_duration",
            MetricTableDef {
                PromQL: "histogram_quantile($QUANTILE, sum(rate(tidb_server_slow_query_process_duration_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,instance))",
                Labels: &["instance"],
                Quantile: 0.90,
                Comment: "The quantile of TiDB slow query statistics with slow query time(second)",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_slow_query_qps",
            MetricTableDef {
                PromQL: "sum(rate(tidb_server_slow_query_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,sql_type)",
                Labels: &["instance", "sql_type"],
                Comment: "TiDB slow query processing numbers per second",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_slow_query_cop_process_duration",
            MetricTableDef {
                PromQL: "histogram_quantile($QUANTILE, sum(rate(tidb_server_slow_query_cop_duration_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,instance))",
                Labels: &["instance"],
                Quantile: 0.90,
                Comment: "The quantile of TiDB slow query statistics with slow query total cop process time(second)",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_slow_query_cop_wait_duration",
            MetricTableDef {
                PromQL: "histogram_quantile($QUANTILE, sum(rate(tidb_server_slow_query_wait_duration_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,instance))",
                Labels: &["instance"],
                Quantile: 0.90,
                Comment: "The quantile of TiDB slow query statistics with slow query total cop wait time(second)",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_ops_internal",
            MetricTableDef {
                PromQL: "sum(rate(tidb_session_restricted_sql_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "TiDB internal SQL is used by TiDB itself.",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_process_mem_usage",
            MetricTableDef {
                PromQL: "process_resident_memory_bytes{$LABEL_CONDITIONS}",
                Labels: &["instance", "job"],
                Comment: "process rss memory usage",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "rust_process_mem_usage",
            MetricTableDef {
                PromQL: "process_resident_memory_bytes{$LABEL_CONDITIONS}",
                Labels: &["instance", "job"],
                Comment: "AsterSQL process resident memory size in bytes",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "process_cpu_usage",
            MetricTableDef {
                PromQL: "rate(process_cpu_seconds_total{$LABEL_CONDITIONS}[$RANGE_DURATION])",
                Labels: &["instance", "job"],
                ..MetricTableDef::EMPTY
            },
        ),
        // —— TiFlash 计算节点资源指标 ——
        (
            "tiflash_process_cpu_usage",
            MetricTableDef {
                PromQL: "rate(tiflash_proxy_process_cpu_seconds_total{$LABEL_CONDITIONS}[$RANGE_DURATION])",
                Labels: &["instance", "job"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tiflash_cpu_quota",
            MetricTableDef {
                PromQL: "tiflash_system_current_metric_LogicalCPUCores{$LABEL_CONDITIONS}",
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tiflash_resource_manager_resource_unit",
            MetricTableDef {
                PromQL: "sum(rate(tiflash_compute_request_unit[$RANGE_DURATION]))",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_connection_count",
            MetricTableDef {
                PromQL: "tidb_server_connections{$LABEL_CONDITIONS}",
                Labels: &["instance"],
                Comment: "TiDB current connection counts",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_connection_idle_duration",
            MetricTableDef {
                PromQL: r###"histogram_quantile($QUANTILE, sum(rate(tidb_server_conn_idle_duration_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,in_txn,instance))"###,
                Labels: &["instance", "in_txn"],
                Quantile: 0.90,
                Comment: "The quantile of TiDB connection idle durations(second)",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_connection_idle_total_count",
            MetricTableDef {
                PromQL: r###"sum(increase(tidb_server_conn_idle_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (in_txn,instance)"###,
                Labels: &["instance", "in_txn"],
                Comment: "The total count of TiDB connection idle",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_connection_idle_total_time",
            MetricTableDef {
                PromQL: r###"sum(increase(tidb_server_conn_idle_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (in_txn,instance)"###,
                Labels: &["instance", "in_txn"],
                Comment: "The total time of TiDB connection idle",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "node_process_open_fd_count",
            MetricTableDef {
                PromQL: "process_open_fds{$LABEL_CONDITIONS}",
                Labels: &["instance", "job"],
                Comment: "Process opened file descriptors count",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "rust_process_threads",
            MetricTableDef {
                PromQL: "process_threads{$LABEL_CONDITIONS}",
                Labels: &["instance", "job"],
                Comment: "Current number of OS threads in the AsterSQL process",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_event_opm",
            MetricTableDef {
                PromQL: "increase(tidb_server_event_total{$LABEL_CONDITIONS}[$RANGE_DURATION])",
                Labels: &["instance", "type"],
                Comment: "TiDB Server critical events total, including start/close/shutdown/hang etc",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_keep_alive_opm",
            MetricTableDef {
                PromQL: "sum(increase(tidb_monitor_keep_alive_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "TiDB instance monitor average keep alive times",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_prepared_statement_count",
            MetricTableDef {
                PromQL: "tidb_server_prepared_stmts{$LABEL_CONDITIONS}",
                Labels: &["instance"],
                Comment: "TiDB prepare statements count",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_time_jump_back_ops",
            MetricTableDef {
                PromQL: "sum(increase(tidb_monitor_time_jump_back_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "TiDB monitor time jump back count",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_panic_count",
            MetricTableDef {
                Comment: "TiDB instance panic count",
                PromQL: "increase(tidb_server_panic_total{$LABEL_CONDITIONS}[$RANGE_DURATION])",
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_panic_count_total_count",
            MetricTableDef {
                Comment: "The total count of TiDB instance panic",
                PromQL: "sum(increase(tidb_server_panic_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_binlog_error_count",
            MetricTableDef {
                Comment: "TiDB write binlog error, skip binlog count",
                PromQL: "tidb_server_critical_error_total{$LABEL_CONDITIONS}",
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_binlog_error_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tidb_server_critical_error_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total count of TiDB write binlog error and skip binlog",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_get_token_duration",
            MetricTableDef {
                PromQL: "histogram_quantile($QUANTILE, sum(rate(tidb_server_get_token_duration_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,instance))",
                Labels: &["instance"],
                Quantile: 0.99,
                Comment: " The quantile of Duration (us) for getting token, it should be small until concurrency limit is reached(microsecond)",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_handshake_error_opm",
            MetricTableDef {
                PromQL: "sum(increase(tidb_server_handshake_error_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The OPM of TiDB processing handshake error",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_handshake_error_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tidb_server_handshake_error_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total count of TiDB processing handshake error",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_transaction_ops",
            MetricTableDef {
                PromQL: "sum(rate(tidb_session_transaction_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,sql_type,instance)",
                Labels: &["instance", "type", "sql_type"],
                Comment: "TiDB transaction processing counts by type and source. Internal means TiDB inner transaction calls",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_transaction_duration",
            MetricTableDef {
                PromQL: "histogram_quantile($QUANTILE, sum(rate(tidb_session_transaction_duration_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,type,sql_type,instance))",
                Labels: &["instance", "type", "sql_type"],
                Quantile: 0.95,
                Comment: "The quantile of transaction execution durations, including retry(second)",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_transaction_retry_num",
            MetricTableDef {
                PromQL: "histogram_quantile($QUANTILE, sum(rate(tidb_session_retry_num_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,instance))",
                Labels: &["instance"],
                Comment: "The quantile of TiDB transaction retry num",
                Quantile: 0.95,
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_transaction_statement_num",
            MetricTableDef {
                PromQL: "histogram_quantile($QUANTILE, sum(rate(tidb_session_transaction_statement_num_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,instance,sql_type))",
                Labels: &["instance", "sql_type"],
                Comment: "The quantile of TiDB statements numbers within one transaction. Internal means TiDB inner transaction",
                Quantile: 0.95,
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_transaction_retry_error_ops",
            MetricTableDef {
                PromQL: "sum(rate(tidb_session_retry_error_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,sql_type,instance)",
                Labels: &["instance", "type", "sql_type"],
                Comment: "Error numbers of transaction retry",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_transaction_retry_error_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tidb_session_retry_error_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,sql_type,instance)",
                Labels: &["instance", "type", "sql_type"],
                Comment: "The total count of transaction retry",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_transaction_local_latch_wait_duration",
            MetricTableDef {
                PromQL: "histogram_quantile($QUANTILE, sum(rate(tidb_tikvclient_local_latch_wait_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,instance))",
                Labels: &["instance"],
                Comment: "The quantile of TiDB transaction latch wait time on key value storage(second)",
                Quantile: 0.95,
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_parse_duration",
            MetricTableDef {
                PromQL: "histogram_quantile($QUANTILE, sum(rate(tidb_session_parse_duration_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,sql_type,instance))",
                Labels: &["instance", "sql_type"],
                Quantile: 0.95,
                Comment: "The quantile time cost of parsing SQL to AST(second)",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_compile_duration",
            MetricTableDef {
                PromQL: "histogram_quantile($QUANTILE, sum(rate(tidb_session_compile_duration_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le, sql_type,instance))",
                Labels: &["instance", "sql_type"],
                Quantile: 0.95,
                Comment: "The quantile time cost of building the query plan(second)",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_execute_duration",
            MetricTableDef {
                PromQL: "histogram_quantile($QUANTILE, sum(rate(tidb_session_execute_duration_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le, sql_type, instance))",
                Labels: &["instance", "sql_type"],
                Quantile: 0.95,
                Comment: "The quantile time cost of executing the SQL which does not include the time to get the results of the query(second)",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_expensive_executors_ops",
            MetricTableDef {
                Comment: "TiDB executors using more cpu and memory resources",
                PromQL: "sum(rate(tidb_executor_expensive_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,instance)",
                Labels: &["instance", "type"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_query_using_plan_cache_ops",
            MetricTableDef {
                PromQL: "sum(rate(tidb_server_plan_cache_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,instance)",
                Labels: &["instance", "type"],
                Comment: "TiDB plan cache hit ops",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_distsql_execution_duration",
            MetricTableDef {
                PromQL: "histogram_quantile($QUANTILE, sum(rate(tidb_distsql_handle_query_duration_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le, type, instance))",
                Labels: &["instance", "type"],
                Quantile: 0.95,
                Comment: "The quantile durations of distsql execution(second)",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_distsql_qps",
            MetricTableDef {
                PromQL: "sum(rate(tidb_distsql_handle_query_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION]))",
                Labels: &["instance", "type"],
                Comment: "distsql query handling durations per second",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_distsql_partial_qps",
            MetricTableDef {
                PromQL: "sum(rate(tidb_distsql_scan_keys_partial_num_count{$LABEL_CONDITIONS}[$RANGE_DURATION]))",
                Labels: &["instance"],
                Comment: "the numebr of distsql partial scan numbers",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_distsql_scan_key_num",
            MetricTableDef {
                PromQL: "histogram_quantile($QUANTILE, sum(rate(tidb_distsql_scan_keys_num_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,instance))",
                Labels: &["instance"],
                Quantile: 0.95,
                Comment: "The quantile numebr of distsql scan numbers",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_distsql_partial_scan_key_num",
            MetricTableDef {
                PromQL: "histogram_quantile($QUANTILE, sum(rate(tidb_distsql_scan_keys_partial_num_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,instance))",
                Labels: &["instance"],
                Quantile: 0.95,
                Comment: "The quantile numebr of distsql partial scan key numbers",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_distsql_partial_num",
            MetricTableDef {
                PromQL: "histogram_quantile($QUANTILE, sum(rate(tidb_distsql_partial_num_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,instance))",
                Labels: &["instance"],
                Quantile: 0.95,
                Comment: "The quantile of distsql partial numbers per query",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_cop_duration",
            MetricTableDef {
                PromQL: "histogram_quantile($QUANTILE, sum(rate(tidb_tikvclient_request_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,instance))",
                Labels: &["instance"],
                Quantile: 0.95,
                Comment: "The quantile of kv storage coprocessor processing durations",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_kv_backoff_duration",
            MetricTableDef {
                PromQL: "histogram_quantile($QUANTILE, sum(rate(tidb_tikvclient_backoff_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,type,instance))",
                Labels: &["instance", "type"],
                Quantile: 0.95,
                Comment: "The quantile of kv backoff time durations(second)",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_kv_backoff_ops",
            MetricTableDef {
                PromQL: "sum(rate(tidb_tikvclient_backoff_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,instance)",
                Labels: &["instance", "type"],
                Comment: "kv storage backoff times",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_kv_region_error_ops",
            MetricTableDef {
                PromQL: "sum(rate(tidb_tikvclient_region_err_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,instance)",
                Labels: &["instance", "type"],
                Comment: "kv region error times",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_kv_region_error_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tidb_tikvclient_region_err_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,instance)",
                Labels: &["instance", "type"],
                Comment: "The total count of kv region error",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_lock_resolver_ops",
            MetricTableDef {
                PromQL: "sum(rate(tidb_tikvclient_lock_resolver_actions_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,instance)",
                Labels: &["instance", "type"],
                Comment: "lock resolve times",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_lock_resolver_total_num",
            MetricTableDef {
                PromQL: "sum(increase(tidb_tikvclient_lock_resolver_actions_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,instance)",
                Labels: &["instance", "type"],
                Comment: "The total number of lock resolve",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_lock_cleanup_fail_ops",
            MetricTableDef {
                PromQL: "sum(rate(tidb_tikvclient_lock_cleanup_task_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,instance)",
                Labels: &["instance", "type"],
                Comment: "lock cleanup failed ops",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_load_safepoint_fail_ops",
            MetricTableDef {
                PromQL: "sum(rate(tidb_tikvclient_load_safepoint_total{$LABEL_CONDITIONS}[$RANGE_DURATION]))",
                Labels: &["instance", "type"],
                Comment: "safe point update ops",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_kv_request_ops",
            MetricTableDef {
                Comment: "kv request total by instance and command type",
                PromQL: "sum(rate(tidb_tikvclient_request_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance, type)",
                Labels: &["instance", "type"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_kv_request_duration",
            MetricTableDef {
                PromQL: "histogram_quantile($QUANTILE, sum(rate(tidb_tikvclient_request_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,type,store,instance))",
                Labels: &["instance", "type", "store"],
                Quantile: 0.95,
                Comment: "The quantile of kv requests durations by store",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_kv_txn_ops",
            MetricTableDef {
                Comment: "TiDB total kv transaction counts",
                PromQL: "sum(rate(tidb_tikvclient_txn_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_kv_write_num",
            MetricTableDef {
                PromQL: "histogram_quantile($QUANTILE, sum(rate(tidb_tikvclient_txn_write_kv_num_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le, instance))",
                Labels: &["instance"],
                Quantile: 1.0,
                Comment: "The quantile of kv write count per transaction execution",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_kv_write_size",
            MetricTableDef {
                Comment: "The quantile of kv write size per transaction execution",
                PromQL: "histogram_quantile($QUANTILE, sum(rate(tidb_tikvclient_txn_write_size_bytes_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le, instance))",
                Labels: &["instance"],
                Quantile: 1.0,
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_txn_region_num",
            MetricTableDef {
                PromQL: "histogram_quantile($QUANTILE, sum(rate(tidb_tikvclient_txn_regions_num_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le, instance))",
                Labels: &["instance"],
                Comment: "The quantile of regions transaction operates on count",
                Quantile: 0.95,
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_load_safepoint_ops",
            MetricTableDef {
                PromQL: "sum(rate(tidb_tikvclient_load_safepoint_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)",
                Labels: &["instance", "type"],
                Comment: "The OPS of load safe point loading",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_load_safepoint_total_num",
            MetricTableDef {
                PromQL: "sum(increase(tidb_tikvclient_load_safepoint_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)",
                Labels: &["instance", "type"],
                Comment: "The total count of safe point loading",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_kv_snapshot_ops",
            MetricTableDef {
                Comment: "using snapshots total",
                PromQL: "sum(rate(tidb_tikvclient_snapshot_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        // —— PD（Placement Driver，放置驱动）客户端与调度相关指标 ——
        (
            "pd_client_cmd_ops",
            MetricTableDef {
                PromQL: "sum(rate(pd_client_cmd_handle_cmds_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,instance)",
                Labels: &["instance", "type"],
                Comment: "pd client command ops",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_client_cmd_duration",
            MetricTableDef {
                PromQL: "histogram_quantile($QUANTILE, sum(rate(pd_client_cmd_handle_cmds_duration_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le, type,instance))",
                Labels: &["instance", "type"],
                Quantile: 0.95,
                Comment: "The quantile of pd client command durations",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_cmd_fail_ops",
            MetricTableDef {
                PromQL: "sum(rate(pd_client_cmd_handle_failed_cmds_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,instance)",
                Labels: &["instance", "type"],
                Comment: "pd client command fail count",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_cmd_fail_total_count",
            MetricTableDef {
                PromQL: "sum(increase(pd_client_cmd_handle_failed_cmds_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,instance)",
                Labels: &["instance", "type"],
                Comment: "The total count of pd client command fail",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_request_rpc_ops",
            MetricTableDef {
                PromQL: "sum(rate(pd_client_request_handle_requests_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION]))",
                Labels: &["instance", "type"],
                Comment: "pd client handle request operation per second",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_request_rpc_duration",
            MetricTableDef {
                Comment: "The quantile of pd client handle request duration(second)",
                PromQL: "histogram_quantile($QUANTILE, sum(rate(pd_client_request_handle_requests_duration_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,type,instance))",
                Labels: &["instance", "type"],
                Quantile: 0.999,
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_tso_wait_duration",
            MetricTableDef {
                PromQL: "histogram_quantile($QUANTILE, sum(rate(pd_client_cmd_handle_cmds_duration_seconds_bucket{type=\"wait\"}[$RANGE_DURATION])) by (le,instance))",
                Labels: &["instance"],
                Quantile: 0.999,
                Comment: "The quantile duration of a client starting to wait for the TS until received the TS result.",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_tso_rpc_duration",
            MetricTableDef {
                Comment: "The quantile duration of a client sending TSO request until received the response.",
                PromQL: "histogram_quantile($QUANTILE, sum(rate(pd_client_request_handle_requests_duration_seconds_bucket{type=\"tso\"}[$RANGE_DURATION])) by (le,instance))",
                Labels: &["instance"],
                Quantile: 0.999,
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_start_tso_wait_duration",
            MetricTableDef {
                PromQL: "histogram_quantile($QUANTILE, sum(rate(tidb_pdclient_ts_future_wait_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,instance))",
                Labels: &["instance"],
                Quantile: 0.999,
                Comment: "The quantile duration of the waiting time for getting the start timestamp oracle",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_load_schema_duration",
            MetricTableDef {
                Comment: "The quantile of TiDB loading schema time durations by instance",
                PromQL: "histogram_quantile($QUANTILE, sum(rate(tidb_domain_load_schema_duration_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le, instance))",
                Labels: &["instance"],
                Quantile: 0.99,
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_load_schema_ops",
            MetricTableDef {
                Comment: "TiDB loading schema times including both failed and successful ones",
                PromQL: "sum(rate(tidb_domain_load_schema_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)",
                Labels: &["instance", "type"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_schema_lease_error_opm",
            MetricTableDef {
                Comment: "TiDB schema lease error counts",
                PromQL: "sum(increase(tidb_session_schema_lease_error_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_schema_lease_error_total_count",
            MetricTableDef {
                Comment: "The total count of TiDB schema lease error",
                PromQL: "sum(increase(tidb_session_schema_lease_error_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_load_privilege_ops",
            MetricTableDef {
                Comment: "TiDB load privilege counts",
                PromQL: "sum(rate(tidb_domain_load_privilege_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)",
                Labels: &["instance", "type"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_ddl_duration",
            MetricTableDef {
                Comment: "The quantile of TiDB DDL duration statistics",
                PromQL: "histogram_quantile($QUANTILE, sum(rate(tidb_ddl_handle_job_duration_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le, type,instance))",
                Labels: &["instance", "type"],
                Quantile: 0.95,
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_ddl_batch_add_index_duration",
            MetricTableDef {
                Comment: "The quantile of TiDB batch add index durations by histogram buckets",
                PromQL: "histogram_quantile($QUANTILE, sum(rate(tidb_ddl_batch_add_idx_duration_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le, type, instance))",
                Labels: &["instance", "type"],
                Quantile: 0.95,
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_ddl_add_index_speed",
            MetricTableDef {
                Comment: "TiDB add index speed",
                PromQL: "sum(rate(tidb_ddl_add_index_total[$RANGE_DURATION])) by (type)",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_ddl_waiting_jobs_num",
            MetricTableDef {
                Comment: "TiDB ddl request in queue",
                PromQL: "tidb_ddl_waiting_jobs{$LABEL_CONDITIONS}",
                Labels: &["instance", "type"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_ddl_meta_opm",
            MetricTableDef {
                Comment: "TiDB different ddl worker numbers",
                PromQL: "increase(tidb_ddl_worker_operation_total{$LABEL_CONDITIONS}[$RANGE_DURATION])",
                Labels: &["instance", "type"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_ddl_worker_duration",
            MetricTableDef {
                Comment: "The quantile of TiDB ddl worker duration",
                PromQL: "histogram_quantile($QUANTILE, sum(increase(tidb_ddl_worker_operation_duration_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le, type, action, result,instance))",
                Labels: &["instance", "type", "result", "action"],
                Quantile: 0.95,
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_ddl_deploy_syncer_duration",
            MetricTableDef {
                PromQL: "histogram_quantile($QUANTILE, sum(rate(tidb_ddl_deploy_syncer_duration_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le, type, result,instance))",
                Labels: &["instance", "type", "result"],
                Quantile: 0.95,
                Comment: "The quantile of TiDB ddl schema syncer statistics, including init, start, watch, clear function call time cost",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_owner_handle_syncer_duration",
            MetricTableDef {
                Comment: "The quantile of TiDB ddl owner time operations on etcd duration statistics ",
                PromQL: "histogram_quantile($QUANTILE, sum(rate(tidb_ddl_owner_handle_syncer_duration_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le, type, result,instance))",
                Labels: &["instance", "type", "result"],
                Quantile: 0.95,
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_ddl_update_self_version_duration",
            MetricTableDef {
                Comment: "The quantile of TiDB schema syncer version update time duration",
                PromQL: "histogram_quantile($QUANTILE, sum(rate(tidb_ddl_update_self_ver_duration_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le, result,instance))",
                Labels: &["instance", "result"],
                Quantile: 0.95,
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_ddl_opm",
            MetricTableDef {
                Comment: "The quantile of executed DDL jobs per minute",
                PromQL: "sum(rate(tidb_ddl_handle_job_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,instance)",
                Labels: &["instance", "type"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_statistics_auto_analyze_duration",
            MetricTableDef {
                Comment: "The quantile of TiDB auto analyze time durations within 95 percent histogram buckets",
                PromQL: "histogram_quantile($QUANTILE, sum(rate(tidb_statistics_auto_analyze_duration_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,instance))",
                Labels: &["instance"],
                Quantile: 0.95,
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_statistics_auto_analyze_ops",
            MetricTableDef {
                Comment: "TiDB auto analyze query per second",
                PromQL: "sum(rate(tidb_statistics_auto_analyze_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,instance)",
                Labels: &["instance", "type"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_statistics_manual_analyze_ops",
            MetricTableDef {
                Comment: "TiDB manual analyze query per second",
                PromQL: "sum(rate(tidb_statistics_manual_analyze_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,instance)",
                Labels: &["instance", "type"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_statistics_stats_inaccuracy_rate",
            MetricTableDef {
                Comment: "The quantile of TiDB statistics inaccurate rate",
                PromQL: "histogram_quantile($QUANTILE, sum(rate(tidb_statistics_stats_inaccuracy_rate_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,instance))",
                Labels: &["instance"],
                Quantile: 0.95,
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_statistics_pseudo_estimation_ops",
            MetricTableDef {
                Comment: "TiDB optimizer using pseudo estimation counts",
                PromQL: "sum(rate(tidb_statistics_pseudo_estimation_total{$LABEL_CONDITIONS}[$RANGE_DURATION]))",
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_statistics_pseudo_estimation_total_count",
            MetricTableDef {
                Comment: "The total count of TiDB optimizer using pseudo estimation",
                PromQL: "sum(increase(tidb_statistics_pseudo_estimation_total{$LABEL_CONDITIONS}[$RANGE_DURATION]))",
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_statistics_update_stats_ops",
            MetricTableDef {
                Comment: "TiDB updating statistics using feed back counts",
                PromQL: "sum(rate(tidb_statistics_update_stats_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,instance)",
                Labels: &["instance", "type"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_statistics_update_stats_total_count",
            MetricTableDef {
                Comment: "The total count of TiDB updating statistics using feed back",
                PromQL: "sum(increase(tidb_statistics_update_stats_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,instance)",
                Labels: &["instance", "type"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_new_etcd_session_duration",
            MetricTableDef {
                Comment: "The quantile of TiDB new session durations for new etcd sessions",
                PromQL: "histogram_quantile($QUANTILE, sum(rate(tidb_owner_new_session_duration_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,type,result, instance))",
                Labels: &["instance", "type", "result"],
                Quantile: 0.95,
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_owner_watcher_ops",
            MetricTableDef {
                Comment: "TiDB owner watcher counts",
                PromQL: "sum(rate(tidb_owner_watch_owner_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type, result, instance)",
                Labels: &["instance", "type", "result"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_auto_id_qps",
            MetricTableDef {
                Comment: "TiDB auto id requests per second including single table/global auto id processing and single table auto id rebase processing",
                PromQL: "sum(rate(tidb_autoid_operation_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION]))",
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_auto_id_request_duration",
            MetricTableDef {
                Comment: "The quantile of TiDB auto id requests durations",
                PromQL: "histogram_quantile($QUANTILE, sum(rate(tidb_autoid_operation_duration_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le, type,instance))",
                Labels: &["instance", "type"],
                Quantile: 0.95,
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_region_cache_ops",
            MetricTableDef {
                Comment: "TiDB region cache operations count",
                PromQL: "sum(rate(tidb_tikvclient_region_cache_operations_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,result,instance)",
                Labels: &["instance", "type", "result"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_meta_operation_duration",
            MetricTableDef {
                Comment: "The quantile of TiDB meta operation durations including get/set schema and ddl jobs",
                PromQL: "histogram_quantile($QUANTILE, sum(rate(tidb_meta_operation_duration_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le, type,result,instance))",
                Labels: &["instance", "type", "result"],
                Quantile: 0.95,
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_gc_worker_action_opm",
            MetricTableDef {
                Comment: "kv storage garbage collection counts by type",
                PromQL: "sum(increase(tidb_tikvclient_gc_worker_actions_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,instance)",
                Labels: &["instance", "type"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_gc_duration",
            MetricTableDef {
                Comment: "The quantile of kv storage garbage collection time durations",
                PromQL: "histogram_quantile($QUANTILE, sum(rate(tidb_tikvclient_gc_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,instance,stage))",
                Labels: &["instance", "stage"],
                Quantile: 0.95,
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_gc_config",
            MetricTableDef {
                Comment: "kv storage garbage collection config including gc_life_time and gc_run_interval",
                PromQL: "tidb_tikvclient_gc_config{$LABEL_CONDITIONS}",
                Labels: &["instance", "type"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_gc_fail_opm",
            MetricTableDef {
                Comment: "kv storage garbage collection failing counts",
                PromQL: "sum(increase(tidb_tikvclient_gc_failure{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,instance)",
                Labels: &["instance", "type"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_gc_delete_range_fail_opm",
            MetricTableDef {
                Comment: "kv storage unsafe destroy range failed counts",
                PromQL: "sum(increase(tidb_tikvclient_gc_unsafe_destroy_range_failures{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,instance)",
                Labels: &["instance", "type"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_gc_too_many_locks_opm",
            MetricTableDef {
                Comment: "kv storage region garbage collection clean too many locks count",
                PromQL: "sum(increase(tidb_tikvclient_gc_region_too_many_locks[$RANGE_DURATION]))",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_gc_action_result_opm",
            MetricTableDef {
                Comment: "kv storage garbage collection results including failed and successful ones",
                PromQL: "sum(increase(tidb_tikvclient_gc_action_result{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,instance)",
                Labels: &["instance", "type"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_gc_delete_range_task_status",
            MetricTableDef {
                Comment: "kv storage delete range task execution status by type",
                PromQL: "sum(tidb_tikvclient_range_task_stats{$LABEL_CONDITIONS}) by (type, result,instance)",
                Labels: &["instance", "type", "result"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_gc_push_task_duration",
            MetricTableDef {
                Comment: "The quantile of kv storage range worker processing one task duration",
                PromQL: "histogram_quantile($QUANTILE, sum(rate(tidb_tikvclient_range_task_push_duration_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,type,instance))",
                Labels: &["instance", "type"],
                Quantile: 0.95,
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_batch_client_pending_req_count",
            MetricTableDef {
                Comment: "kv storage batch requests in queue",
                PromQL: "sum(tidb_tikvclient_pending_batch_requests{$LABEL_CONDITIONS}) by (store,instance)",
                Labels: &["instance", "store"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_batch_client_wait_duration",
            MetricTableDef {
                Comment: "The quantile of kv storage batch processing durations, the unit is nanosecond",
                PromQL: "histogram_quantile($QUANTILE, sum(rate(tidb_tikvclient_batch_wait_duration_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le, instance))",
                Labels: &["instance"],
                Quantile: 0.95,
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_batch_client_wait_conn_duration",
            MetricTableDef {
                PromQL: "histogram_quantile($QUANTILE, sum(rate(tidb_tikvclient_batch_client_wait_connection_establish_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le, instance))",
                Labels: &["instance"],
                Quantile: 0.95,
                Comment: "The quantile of batch client wait new connection establish durations",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_batch_client_wait_conn_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tidb_tikvclient_batch_client_wait_connection_establish_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total count of batch client wait new connection establish",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_batch_client_wait_conn_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tidb_tikvclient_batch_client_wait_connection_establish_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total time of batch client wait new connection establish",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_batch_client_unavailable_duration",
            MetricTableDef {
                Comment: "The quantile of kv storage batch processing unvailable durations",
                PromQL: "histogram_quantile($QUANTILE, sum(rate(tidb_tikvclient_batch_client_unavailable_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le, instance))",
                Labels: &["instance"],
                Quantile: 0.95,
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "uptime",
            MetricTableDef {
                PromQL: "(time() - process_start_time_seconds{$LABEL_CONDITIONS})",
                Labels: &["instance", "job"],
                Comment: "TiDB uptime since last restart(second)",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "up",
            MetricTableDef {
                PromQL: r###"up{$LABEL_CONDITIONS}"###,
                Labels: &["instance", "job"],
                Comment: "whether the instance is up. 1 is up, 0 is down(off-line)",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_role",
            MetricTableDef {
                PromQL: r###"delta(pd_tso_events{type="save"}[$RANGE_DURATION]) > bool 0"###,
                Labels: &["instance"],
                Comment: "It indicates whether the current PD is the leader or a follower.",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "normal_stores",
            MetricTableDef {
                PromQL: r###"sum(pd_cluster_status{type="store_up_count"}) by (instance)"###,
                Labels: &["instance"],
                Comment: "The count of healthy stores",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "abnormal_stores",
            MetricTableDef {
                PromQL: r###"sum(pd_cluster_status{ type=~"store_disconnected_count|store_unhealth_count|store_low_space_count|store_down_count|store_offline_count|store_tombstone_count"})"###,
                Labels: &["instance", "type"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_scheduler_config",
            MetricTableDef {
                PromQL: r###"pd_config_status{$LABEL_CONDITIONS}"###,
                Labels: &["type"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_region_label_isolation_level",
            MetricTableDef {
                PromQL: r###"pd_regions_label_level{$LABEL_CONDITIONS}"###,
                Labels: &["instance", "type"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_label_distribution",
            MetricTableDef {
                PromQL: r###"pd_cluster_placement_status{$LABEL_CONDITIONS}"###,
                Labels: &["name"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_cluster_status",
            MetricTableDef {
                PromQL: r###"sum(pd_cluster_status{$LABEL_CONDITIONS}) by (instance, type)"###,
                Labels: &["instance", "type"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_cluster_metadata",
            MetricTableDef {
                PromQL: r###"pd_cluster_metadata{$LABEL_CONDITIONS}"###,
                Labels: &["instance", "type"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_region_health",
            MetricTableDef {
                PromQL: r###"sum(pd_regions_status{$LABEL_CONDITIONS}) by (instance, type)"###,
                Labels: &["instance", "type"],
                Comment: "It records the unusual Regions' count which may include pending peers, down peers, extra peers, offline peers, missing peers or learner peers",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_schedule_operator",
            MetricTableDef {
                PromQL: r###"sum(delta(pd_schedule_operators_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,event,instance)"###,
                Labels: &["instance", "type", "event"],
                Comment: "The number of different operators",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_schedule_operator_total_num",
            MetricTableDef {
                PromQL: r###"sum(increase(pd_schedule_operators_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,event,instance)"###,
                Labels: &["instance", "type", "event"],
                Comment: "The total number of different operators",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_operator_finish_duration",
            MetricTableDef {
                PromQL: r###"histogram_quantile($QUANTILE, sum(rate(pd_schedule_finish_operators_duration_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,type))"###,
                Labels: &["type"],
                Quantile: 0.99,
                Comment: "The quantile time consumed when the operator is finished",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_operator_step_finish_duration",
            MetricTableDef {
                PromQL: r###"histogram_quantile($QUANTILE, sum(rate(pd_schedule_finish_operator_steps_duration_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,type))"###,
                Labels: &["type"],
                Quantile: 0.99,
                Comment: "The quantile time consumed when the operator step is finished",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_scheduler_store_status",
            MetricTableDef {
                PromQL: r###"pd_scheduler_store_status{$LABEL_CONDITIONS}"###,
                Labels: &["instance", "address", "store", "type"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "store_available_ratio",
            MetricTableDef {
                PromQL: r###"sum(pd_scheduler_store_status{type="store_available"}) by (address, store) / sum(pd_scheduler_store_status{type="store_capacity"}) by (address, store)"###,
                Labels: &["address", "store"],
                Comment: "It is equal to Store available capacity size over Store capacity size for each TiKV instance",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "store_size_amplification",
            MetricTableDef {
                PromQL: r###"sum(pd_scheduler_store_status{type="region_size"}) by (address, store) / sum(pd_scheduler_store_status{type="store_used"}) by (address, store) * 2^20"###,
                Labels: &["address", "store"],
                Comment: "The size amplification, which is equal to Store Region size over Store used capacity size, of each TiKV instance",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_scheduler_op_influence",
            MetricTableDef {
                PromQL: r###"pd_scheduler_op_influence{$LABEL_CONDITIONS}"###,
                Labels: &["instance", "scheduler", "store", "type"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_scheduler_tolerant_resource",
            MetricTableDef {
                PromQL: r###"pd_scheduler_tolerant_resource{$LABEL_CONDITIONS}"###,
                Labels: &["instance", "scheduler", "source", "target"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_hotspot_status",
            MetricTableDef {
                PromQL: r###"pd_hotspot_status{$LABEL_CONDITIONS}"###,
                Labels: &["instance", "address", "store", "type"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_scheduler_status",
            MetricTableDef {
                PromQL: r###"pd_scheduler_status{$LABEL_CONDITIONS}"###,
                Labels: &["instance", "kind", "type"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_scheduler_balance_leader",
            MetricTableDef {
                PromQL: r###"sum(delta(pd_scheduler_balance_leader{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (address,store,instance,type)"###,
                Labels: &["instance", "address", "store", "type"],
                Comment: "The leader movement details among TiKV instances",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_scheduler_balance_region",
            MetricTableDef {
                PromQL: r###"sum(delta(pd_scheduler_balance_region{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (address,store,instance,type)"###,
                Labels: &["instance", "address", "store", "type"],
                Comment: "The Region movement details among TiKV instances",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_balance_scheduler_status",
            MetricTableDef {
                PromQL: r###"sum(delta(pd_scheduler_event_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type,name)"###,
                Labels: &["instance", "name", "type"],
                Comment: "The inner status of balance leader scheduler",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_checker_event_count",
            MetricTableDef {
                PromQL: r###"sum(delta(pd_checker_event_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (name,instance,type)"###,
                Labels: &["instance", "name", "type"],
                Comment: "The replica/region checker's status",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_schedule_filter",
            MetricTableDef {
                PromQL: r###"sum(delta(pd_schedule_filter{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (store, type, scope, instance)"###,
                Labels: &["instance", "scope", "store", "type"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_scheduler_balance_direction",
            MetricTableDef {
                PromQL: r###"sum(delta(pd_scheduler_balance_direction{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,source,target,instance)"###,
                Labels: &["instance", "source", "target", "type"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_schedule_store_limit",
            MetricTableDef {
                PromQL: r###"pd_schedule_store_limit{$LABEL_CONDITIONS}"###,
                Labels: &["instance", "store", "type"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_grpc_completed_commands_rate",
            MetricTableDef {
                PromQL: r###"sum(rate(grpc_server_handling_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (grpc_method,instance)"###,
                Labels: &["instance", "grpc_method"],
                Comment: "The rate of completing each kind of gRPC commands",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_grpc_completed_commands_duration",
            MetricTableDef {
                PromQL: r###"histogram_quantile($QUANTILE, sum(rate(grpc_server_handling_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,grpc_method,instance))"###,
                Labels: &["instance", "grpc_method"],
                Quantile: 0.99,
                Comment: "The quantile time consumed of completing each kind of gRPC commands",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_handle_transactions_rate",
            MetricTableDef {
                PromQL: r###"sum(rate(pd_txn_handle_txns_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance, result)"###,
                Labels: &["instance", "result"],
                Comment: "The rate of handling etcd transactions",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_handle_transactions_duration",
            MetricTableDef {
                PromQL: r###"histogram_quantile($QUANTILE, sum(rate(pd_txn_handle_txns_duration_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le, instance, result))"###,
                Labels: &["instance", "result"],
                Quantile: 0.99,
                Comment: "The quantile time consumed of handling etcd transactions",
                ..MetricTableDef::EMPTY
            },
        ),
        // —— etcd WAL / 磁盘相关指标（PD 内嵌 etcd） ——
        (
            "etcd_wal_fsync_duration",
            MetricTableDef {
                PromQL: r###"histogram_quantile($QUANTILE, sum(rate(etcd_disk_wal_fsync_duration_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,instance))"###,
                Labels: &["instance"],
                Quantile: 0.99,
                Comment: "The quantile time consumed of writing WAL into the persistent storage",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_peer_round_trip_duration",
            MetricTableDef {
                PromQL: r###"histogram_quantile($QUANTILE, sum(rate(etcd_network_peer_round_trip_time_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,instance,To))"###,
                Labels: &["instance", "To"],
                Quantile: 0.99,
                Comment: "The quantile latency of the network in .99",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "etcd_disk_wal_fsync_rate",
            MetricTableDef {
                PromQL: r###"delta(etcd_disk_wal_fsync_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])"###,
                Labels: &["instance"],
                Comment: "The rate of writing WAL into the persistent storage",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_server_etcd_state",
            MetricTableDef {
                PromQL: r###"pd_server_etcd_state{$LABEL_CONDITIONS}"###,
                Labels: &["instance", "type"],
                Comment: "The current term of Raft",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_request_rpc_duration_avg",
            MetricTableDef {
                PromQL: r###"avg(rate(pd_client_request_handle_requests_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type) / avg(rate(pd_client_request_handle_requests_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type)"###,
                Labels: &["type"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_region_heartbeat_duration",
            MetricTableDef {
                PromQL: r###"round(histogram_quantile($QUANTILE, sum(rate(pd_scheduler_region_heartbeat_latency_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,address, store)), 1000)"###,
                Labels: &["address", "store"],
                Quantile: 0.99,
                Comment: "The quantile of heartbeat latency of each TiKV instance in",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_scheduler_region_heartbeat",
            MetricTableDef {
                PromQL: r###"sum(rate(pd_scheduler_region_heartbeat{$LABEL_CONDITIONS}[$RANGE_DURATION])*60) by (address,instance, store, status,type)"###,
                Labels: &["instance", "address", "status", "store", "type"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_region_syncer_status",
            MetricTableDef {
                PromQL: r###"pd_region_syncer_status{$LABEL_CONDITIONS}"###,
                Labels: &["instance", "type"],
                ..MetricTableDef::EMPTY
            },
        ),
        // —— 资源管控（Resource Manager）相关指标 ——
        (
            "resource_manager_resource_unit",
            MetricTableDef {
                PromQL: r###"sum(rate(resource_manager_resource_unit_read_request_unit_sum{type=~"|tp"}[$RANGE_DURATION])) + sum(rate(resource_manager_resource_unit_write_request_unit_sum{type=~"|tp"}[$RANGE_DURATION]))"###,
                Comment: "The Total RU consumption per second",
                ..MetricTableDef::EMPTY
            },
        ),
        // —— TiKV 存储引擎、Raft、Region 等指标 ——
        (
            "tikv_engine_size",
            MetricTableDef {
                PromQL: r###"sum(tikv_engine_size_bytes{$LABEL_CONDITIONS}) by (instance, type, db)"###,
                Labels: &["instance", "type", "db"],
                Comment: "The storage size per TiKV instance",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_store_size",
            MetricTableDef {
                PromQL: r###"sum(tikv_store_size_bytes{$LABEL_CONDITIONS}) by (instance,type)"###,
                Labels: &["instance", "type"],
                Comment: "The available or capacity size of each TiKV instance",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_cpu_quota",
            MetricTableDef {
                PromQL: "tikv_server_cpu_cores_quota{$LABEL_CONDITIONS}",
                Labels: &["instance"],
                Comment: "The Total CPU quota of each TiKV instance",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_thread_cpu",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_thread_cpu_seconds_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,name)"###,
                Labels: &["instance", "name"],
                Comment: "The CPU usage of each TiKV instance",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_memory",
            MetricTableDef {
                PromQL: r###"avg(process_resident_memory_bytes{$LABEL_CONDITIONS}) by (instance)"###,
                Labels: &["instance"],
                Comment: "The memory usage per TiKV instance",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_io_utilization",
            MetricTableDef {
                PromQL: r###"rate(node_disk_io_time_seconds_total{$LABEL_CONDITIONS}[$RANGE_DURATION])"###,
                Labels: &["instance", "device"],
                Comment: "The I/O utilization per TiKV instance",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_flow_mbps",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_engine_flow_bytes{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type,db)"###,
                Labels: &["instance", "type", "db"],
                Comment: "The total bytes of read and write in each TiKV instance",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_grpc_qps",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_grpc_msg_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)"###,
                Labels: &["instance", "type"],
                Comment: "The QPS per command in each TiKV instance",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_grpc_errors",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_grpc_msg_fail_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)"###,
                Labels: &["instance", "type"],
                Comment: "The OPS of the gRPC message failures",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_grpc_error_total_count",
            MetricTableDef {
                PromQL: r###"sum(increase(tikv_grpc_msg_fail_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)"###,
                Labels: &["instance", "type"],
                Comment: "The total count of the gRPC message failures",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_critical_error",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_critical_error_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance, type)"###,
                Labels: &["instance", "type"],
                Comment: "The OPS of the TiKV critical error",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_critical_error_total_count",
            MetricTableDef {
                PromQL: r###"sum(increase(tikv_critical_error_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance, type)"###,
                Labels: &["instance", "type"],
                Comment: "The total number of the TiKV critical error",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_pd_heartbeat",
            MetricTableDef {
                PromQL: r###"sum(delta(tikv_pd_heartbeat_message_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)"###,
                Labels: &["instance", "type"],
                Comment: "The total number of the gRPC message failures",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_region_count",
            MetricTableDef {
                PromQL: r###"sum(tikv_raftstore_region_count{$LABEL_CONDITIONS}) by (instance,type)"###,
                Labels: &["instance", "type"],
                Comment: "The number of regions on each TiKV instance",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_scheduler_is_busy",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_scheduler_too_busy_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,db,type,stage)"###,
                Labels: &["instance", "db", "type", "stage"],
                Comment: "Indicates occurrences of Scheduler Busy events that make the TiKV instance unavailable temporarily",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_scheduler_is_busy_total_count",
            MetricTableDef {
                PromQL: r###"sum(increase(tikv_scheduler_too_busy_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,db,type,stage)"###,
                Labels: &["instance", "db", "type", "stage"],
                Comment: "The total count of Scheduler Busy events that make the TiKV instance unavailable temporarily",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_channel_full",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_channel_full_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type,db)"###,
                Labels: &["instance", "db", "type"],
                Comment: "The ops of channel full errors on each TiKV instance, it will make the TiKV instance unavailable temporarily",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_channel_full_total_count",
            MetricTableDef {
                PromQL: r###"sum(increase(tikv_channel_full_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type,db)"###,
                Labels: &["instance", "db", "type"],
                Comment: "The total number of channel full errors on each TiKV instance, it will make the TiKV instance unavailable temporarily",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_coprocessor_is_busy",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_coprocessor_request_error{type='full'}[$RANGE_DURATION])) by (instance,db,type)"###,
                Labels: &["instance", "db"],
                Comment: "The ops of Coprocessor Full events that make the TiKV instance unavailable temporarily",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_coprocessor_is_busy_total_count",
            MetricTableDef {
                PromQL: r###"sum(increase(tikv_coprocessor_request_error{type='full'}[$RANGE_DURATION])) by (instance,db,type)"###,
                Labels: &["instance", "db"],
                Comment: "The total count of Coprocessor Full events that make the TiKV instance unavailable temporarily",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_engine_write_stall",
            MetricTableDef {
                PromQL: r###"avg(tikv_engine_write_stall{type="write_stall_percentile99"}) by (instance, db)"###,
                Labels: &["instance", "db"],
                Comment: "Indicates occurrences of Write Stall events that make the TiKV instance unavailable temporarily",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_server_report_failures",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_server_report_failure_msg_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,instance,store_id)"###,
                Labels: &["instance", "store_id", "type"],
                Comment: "The total number of reported failure messages",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_server_report_failures_total_count",
            MetricTableDef {
                PromQL: r###"sum(increase(tikv_server_report_failure_msg_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,instance,store_id)"###,
                Labels: &["instance", "store_id", "type"],
                Comment: "The total number of reported failure messages",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_storage_async_requests",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_storage_engine_async_request_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance, status, type)"###,
                Labels: &["instance", "status", "type"],
                Comment: "The number of different raftstore errors on each TiKV instance",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_storage_async_requests_total_count",
            MetricTableDef {
                PromQL: r###"sum(increase(tikv_storage_engine_async_request_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance, status, type)"###,
                Labels: &["instance", "status", "type"],
                Comment: "The total number of different raftstore errors on each TiKV instance",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_scheduler_stage",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_scheduler_stage_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance, stage,type)"###,
                Labels: &["instance", "stage", "type"],
                Comment: "The number of scheduler state on each TiKV instance",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_scheduler_stage_total_num",
            MetricTableDef {
                PromQL: r###"sum(increase(tikv_scheduler_stage_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance, stage,type)"###,
                Labels: &["instance", "stage", "type"],
                Comment: "The total number of scheduler state on each TiKV instance",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_coprocessor_request_error",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_coprocessor_request_error{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance, reason)"###,
                Labels: &["instance", "reason"],
                Comment: "The number of different coprocessor errors on each TiKV instance",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_coprocessor_request_error_total_count",
            MetricTableDef {
                PromQL: r###"sum(increase(tikv_coprocessor_request_error{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance, reason)"###,
                Labels: &["instance", "reason"],
                Comment: "The total number of different coprocessor errors on each TiKV instance",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_region_change",
            MetricTableDef {
                PromQL: r###"sum(delta(tikv_raftstore_region_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)"###,
                Labels: &["instance", "type"],
                Comment: "The count of region change per TiKV instance",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_leader_missing",
            MetricTableDef {
                PromQL: r###"sum(tikv_raftstore_leader_missing{$LABEL_CONDITIONS}) by (instance)"###,
                Labels: &["instance"],
                Comment: "The count of missing leaders per TiKV instance",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_active_written_leaders",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_region_written_keys_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)"###,
                Labels: &["instance"],
                Comment: "The number of leaders being written on each TiKV instance",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_approximate_region_size",
            MetricTableDef {
                PromQL: r###"histogram_quantile($QUANTILE, sum(rate(tikv_raftstore_region_size_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,instance))"###,
                Labels: &["instance"],
                Quantile: 0.99,
                Comment: "The quantile of approximate Region size, the default value is P99",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_approximate_avg_region_size",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_raftstore_region_size_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) / sum(rate(tikv_raftstore_region_size_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) "###,
                Labels: &["instance"],
                Comment: "The avg approximate Region size",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_approximate_region_size_histogram",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_raftstore_region_size_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION]))"###,
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_region_average_written_bytes",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_region_written_bytes_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance) / sum(rate(tikv_region_written_bytes_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)"###,
                Labels: &["instance"],
                Comment: "The average rate of writing bytes to Regions per TiKV instance",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_region_written_bytes",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_region_written_bytes_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION]))"###,
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_region_average_written_keys",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_region_written_keys_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance) / sum(rate(tikv_region_written_keys_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)"###,
                Labels: &["instance"],
                Comment: "The average rate of written keys to Regions per TiKV instance",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_region_written_keys",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_region_written_keys_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION]))"###,
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_request_batch_avg",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_server_request_batch_ratio_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,instance) / sum(rate(tikv_server_request_batch_ratio_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,instance)"###,
                Labels: &["instance", "type"],
                Comment: "The ratio of request batch output to input per TiKV instance",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_request_batch_ratio",
            MetricTableDef {
                PromQL: r###"histogram_quantile($QUANTILE, sum(rate(tikv_server_request_batch_ratio_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,type,instance))"###,
                Labels: &["instance", "type"],
                Quantile: 0.99,
                Comment: "The quantile ratio of request batch output to input per TiKV instance",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_request_batch_size_avg",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_server_request_batch_size_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,instance) / sum(rate(tikv_server_request_batch_size_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,instance)"###,
                Labels: &["instance", "type"],
                Comment: "The avg size of requests into request batch per TiKV instance",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_request_batch_size",
            MetricTableDef {
                PromQL: r###"histogram_quantile($QUANTILE, sum(rate(tikv_server_request_batch_size_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,type,instance))"###,
                Labels: &["instance", "type"],
                Quantile: 0.99,
                Comment: "The quantile size of requests into request batch per TiKV instance",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_grpc_message_duration",
            MetricTableDef {
                PromQL: r###"histogram_quantile($QUANTILE, sum(rate(tikv_grpc_msg_duration_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,type,instance))"###,
                Labels: &["instance", "type"],
                Quantile: 0.99,
                Comment: "The quantile execution time of gRPC message",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_average_grpc_messge_duration",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_grpc_msg_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,instance) / sum(rate(tikv_grpc_msg_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,instance)"###,
                Labels: &["instance", "type"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_grpc_req_batch_size",
            MetricTableDef {
                PromQL: r###"histogram_quantile($QUANTILE, sum(rate(tikv_server_grpc_req_batch_size_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,instance))"###,
                Labels: &["instance"],
                Quantile: 0.99,
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_grpc_resp_batch_size",
            MetricTableDef {
                PromQL: r###"histogram_quantile($QUANTILE, sum(rate(tikv_server_grpc_resp_batch_size_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,instance))"###,
                Labels: &["instance"],
                Quantile: 0.99,
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_grpc_avg_req_batch_size",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_server_grpc_req_batch_size_sum[$RANGE_DURATION])) / sum(rate(tikv_server_grpc_req_batch_size_count[$RANGE_DURATION]))"###,
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_grpc_avg_resp_batch_size",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_server_grpc_resp_batch_size_sum[$RANGE_DURATION])) / sum(rate(tikv_server_grpc_resp_batch_size_count[$RANGE_DURATION]))"###,
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_raft_message_batch_size",
            MetricTableDef {
                PromQL: r###"histogram_quantile($QUANTILE, sum(rate(tikv_server_raft_message_batch_size_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,instance))"###,
                Labels: &["instance"],
                Quantile: 0.99,
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_raft_message_avg_batch_size",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_server_raft_message_batch_size_sum[$RANGE_DURATION])) / sum(rate(tikv_server_raft_message_batch_size_count[$RANGE_DURATION]))"###,
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_pd_request_ops",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_pd_request_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,instance)"###,
                Labels: &["instance", "type"],
                Comment: "The OPS of requests that TiKV sends to PD",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_pd_request_duration",
            MetricTableDef {
                PromQL: r###"histogram_quantile($QUANTILE, sum(rate(tikv_pd_request_duration_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,instance,type))"###,
                Labels: &["instance", "type"],
                Quantile: 0.99,
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_pd_request_total_count",
            MetricTableDef {
                PromQL: r###"sum(increase(tikv_pd_request_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,instance)"###,
                Labels: &["instance", "type"],
                Comment: "The count of requests that TiKV sends to PD",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_pd_request_total_time",
            MetricTableDef {
                PromQL: r###"sum(increase(tikv_pd_request_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,instance)"###,
                Labels: &["instance", "type"],
                Comment: "The count of requests that TiKV sends to PD",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_pd_request_avg_duration",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_pd_request_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type ,instance) / sum(rate(tikv_pd_request_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,instance)"###,
                Labels: &["instance", "type"],
                Comment: "The time consumed by requests that TiKV sends to PD",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_pd_heartbeats",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_pd_heartbeat_message_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type ,instance)"###,
                Labels: &["instance", "type"],
                Comment: " The total number of PD heartbeat messages",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_pd_validate_peers",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_pd_validate_peer_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type ,instance)"###,
                Labels: &["instance", "type"],
                Comment: "The total number of peers validated by the PD worker",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_raftstore_apply_log_avg_duration",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_raftstore_apply_log_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) / sum(rate(tikv_raftstore_apply_log_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) "###,
                Labels: &["instance"],
                Comment: "The average time consumed when Raft applies log",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_raftstore_apply_log_duration",
            MetricTableDef {
                PromQL: r###"histogram_quantile($QUANTILE, sum(rate(tikv_raftstore_apply_log_duration_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,instance))"###,
                Labels: &["instance"],
                Quantile: 0.99,
                Comment: "The quantile time consumed when Raft applies log",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_raftstore_append_log_avg_duration",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_raftstore_append_log_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) / sum(rate(tikv_raftstore_append_log_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION]))"###,
                Labels: &["instance"],
                Comment: "The avg time consumed when Raft appends log",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_raftstore_append_log_duration",
            MetricTableDef {
                PromQL: r###"histogram_quantile($QUANTILE, sum(rate(tikv_raftstore_append_log_duration_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,instance))"###,
                Labels: &["instance"],
                Quantile: 0.99,
                Comment: "The quantile time consumed when Raft appends log",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_raftstore_commit_log_avg_duration",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_raftstore_commit_log_duration_seconds_sum[$RANGE_DURATION])) / sum(rate(tikv_raftstore_commit_log_duration_seconds_count[$RANGE_DURATION]))"###,
                Comment: "The time consumed when Raft commits log",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_raftstore_commit_log_duration",
            MetricTableDef {
                PromQL: r###"histogram_quantile($QUANTILE, sum(rate(tikv_raftstore_commit_log_duration_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,instance))"###,
                Labels: &["instance"],
                Quantile: 0.99,
                Comment: "The quantile time consumed when Raft commits log",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_ready_handled",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_raftstore_raft_ready_handled_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,instance)"###,
                Labels: &["instance"],
                Comment: "The count of ready handled of Raft",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_raftstore_process_handled",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_raftstore_raft_process_duration_secs_count{$LABEL_CONDITIONS}[$RANGE_DURATION]))"###,
                Labels: &["instance", "type"],
                Comment: "The count of different process type of Raft",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_raftstore_process_duration",
            MetricTableDef {
                PromQL: r###"histogram_quantile($QUANTILE, sum(rate(tikv_raftstore_raft_process_duration_secs_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,instance,type))"###,
                Labels: &["instance", "type"],
                Quantile: 0.99,
                Comment: "The quantile time consumed for peer processes in Raft",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_raft_store_events_duration",
            MetricTableDef {
                PromQL: r###"histogram_quantile($QUANTILE, sum(rate(tikv_raftstore_event_duration_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,type,instance))"###,
                Labels: &["instance", "type"],
                Quantile: 0.99,
                Comment: "The quantile time consumed by raftstore events (P99).99",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_raft_sent_messages",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_raftstore_raft_sent_message_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)"###,
                Labels: &["instance", "type"],
                Comment: "The number of Raft messages sent by each TiKV instance",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_raft_sent_messages_total_num",
            MetricTableDef {
                PromQL: r###"sum(increase(tikv_raftstore_raft_sent_message_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)"###,
                Labels: &["instance", "type"],
                Comment: "The total number of Raft messages sent by each TiKV instance",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_flush_messages",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_server_raft_message_flush_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)"###,
                Labels: &["instance"],
                Comment: "The number of Raft messages flushed by each TiKV instance",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_flush_messages_total_num",
            MetricTableDef {
                PromQL: r###"sum(increase(tikv_server_raft_message_flush_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)"###,
                Labels: &["instance"],
                Comment: "The total number of Raft messages flushed by each TiKV instance",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_receive_messages",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_server_raft_message_recv_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)"###,
                Labels: &["instance"],
                Comment: "The number of Raft messages received by each TiKV instance",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_receive_messages_total_num",
            MetricTableDef {
                PromQL: r###"sum(increase(tikv_server_raft_message_recv_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)"###,
                Labels: &["instance"],
                Comment: "The total number of Raft messages received by each TiKV instance",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_raft_dropped_messages",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_raftstore_raft_dropped_message_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,instance)"###,
                Labels: &["instance", "type"],
                Comment: "The number of dropped Raft messages per type",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_raft_dropped_messages_total",
            MetricTableDef {
                PromQL: r###"sum(increase(tikv_raftstore_raft_dropped_message_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,instance)"###,
                Labels: &["instance", "type"],
                Comment: "The total number of dropped Raft messages per type",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_raft_proposals_per_ready",
            MetricTableDef {
                PromQL: r###"histogram_quantile($QUANTILE, sum(rate(tikv_raftstore_apply_proposal_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le, instance))"###,
                Labels: &["instance"],
                Quantile: 0.99,
                Comment: "The quantile proposal count of all Regions in a mio tick",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_raft_proposals",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_raftstore_proposal_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,instance)"###,
                Labels: &["instance", "type"],
                Comment: "The number of proposals per type in raft",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_raft_proposals_total_num",
            MetricTableDef {
                PromQL: r###"sum(increase(tikv_raftstore_proposal_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,instance)"###,
                Labels: &["instance", "type"],
                Comment: "The total number of proposals per type in raft",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_raftstore_propose_wait_duration",
            MetricTableDef {
                PromQL: r###"histogram_quantile($QUANTILE, sum(rate(tikv_raftstore_request_wait_time_duration_secs_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,instance))"###,
                Labels: &["instance"],
                Quantile: 0.99,
                Comment: "The quantile wait time of each proposal",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_propose_avg_wait_duration",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_raftstore_request_wait_time_duration_secs_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) / sum(rate(tikv_raftstore_request_wait_time_duration_secs_count{$LABEL_CONDITIONS}[$RANGE_DURATION]))"###,
                Labels: &["instance"],
                Comment: "The average wait time of each proposal",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_raftstore_apply_wait_duration",
            MetricTableDef {
                PromQL: r###"histogram_quantile($QUANTILE, sum(rate(tikv_raftstore_apply_wait_time_duration_secs_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,instance))"###,
                Labels: &["instance"],
                Quantile: 0.99,
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_apply_avg_wait_duration",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_raftstore_apply_wait_time_duration_secs_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) / sum(rate(tikv_raftstore_apply_wait_time_duration_secs_count{$LABEL_CONDITIONS}[$RANGE_DURATION]))"###,
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_raft_log_speed",
            MetricTableDef {
                PromQL: r###"avg(rate(tikv_raftstore_propose_log_size_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)"###,
                Labels: &["instance"],
                Comment: "The rate at which peers propose logs",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_admin_apply",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_raftstore_admin_cmd_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,status,instance)"###,
                Labels: &["instance", "type", "status"],
                Comment: "The number of the processed apply command",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_check_split",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_raftstore_check_split_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,instance)"###,
                Labels: &["instance", "type"],
                Comment: "The number of raftstore split checks",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_check_split_duration",
            MetricTableDef {
                PromQL: r###"histogram_quantile($QUANTILE, sum(rate(tikv_raftstore_check_split_duration_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le, instance))"###,
                Labels: &["instance"],
                Quantile: 0.9999,
                Comment: "The quantile of time consumed when running split check in .9999",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_local_reader_reject_requests",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_raftstore_local_read_reject_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance, reason)"###,
                Labels: &["instance", "reason"],
                Comment: "The number of rejections from the local read thread",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_local_reader_execute_requests",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_raftstore_local_read_executed_requests{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)"###,
                Labels: &["instance"],
                Comment: "The number of total requests from the local read thread",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_storage_command_ops",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_storage_command_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,instance)"###,
                Labels: &["instance", "type"],
                Comment: "The total count of different kinds of commands received per seconds",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_storage_async_request_duration",
            MetricTableDef {
                PromQL: r###"histogram_quantile($QUANTILE, sum(rate(tikv_storage_engine_async_request_duration_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,instance,type))"###,
                Labels: &["instance", "type"],
                Quantile: 0.99,
                Comment: "The quantile of time consumed by processing asynchronous snapshot requests",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_storage_async_request_avg_duration",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_storage_engine_async_request_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) / sum(rate(tikv_storage_engine_async_request_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION]))"###,
                Labels: &["instance", "type"],
                Comment: "The time consumed by processing asynchronous snapshot requests",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_scheduler_writing_bytes",
            MetricTableDef {
                PromQL: r###"sum(tikv_scheduler_writing_bytes{$LABEL_CONDITIONS}) by (instance)"###,
                Labels: &["instance"],
                Comment: "The total writing bytes of commands on each stage",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_scheduler_priority_commands",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_scheduler_commands_pri_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (priority,instance)"###,
                Labels: &["instance", "priority"],
                Comment: "The count of different priority commands",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_scheduler_pending_commands",
            MetricTableDef {
                PromQL: r###"sum(tikv_scheduler_contex_total{$LABEL_CONDITIONS}) by (instance)"###,
                Labels: &["instance"],
                Comment: "The count of pending commands per TiKV instance",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_scheduler_command_duration",
            MetricTableDef {
                PromQL: r###"histogram_quantile($QUANTILE, sum(rate(tikv_scheduler_command_duration_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,instance,type))"###,
                Labels: &["instance", "type"],
                Quantile: 0.99,
                Comment: "The quantile of time consumed when executing command",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_scheduler_command_avg_duration",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_scheduler_command_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) / sum(rate(tikv_scheduler_command_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) "###,
                Labels: &["instance", "type"],
                Comment: "The average time consumed when executing command",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_scheduler_latch_wait_duration",
            MetricTableDef {
                PromQL: r###"histogram_quantile($QUANTILE, sum(rate(tikv_scheduler_latch_wait_duration_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,instance,type))"###,
                Labels: &["instance", "type"],
                Quantile: 0.99,
                Comment: "The quantile time which is caused by latch wait in command",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_scheduler_latch_wait_avg_duration",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_scheduler_latch_wait_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) / sum(rate(tikv_scheduler_latch_wait_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) "###,
                Labels: &["instance", "type"],
                Comment: "The average time which is caused by latch wait in command",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_scheduler_processing_read_duration",
            MetricTableDef {
                PromQL: r###"histogram_quantile($QUANTILE, sum(rate(tikv_scheduler_processing_read_duration_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,instance,type))"###,
                Labels: &["instance", "type"],
                Quantile: 0.99,
                Comment: "The quantile time of scheduler processing read in command",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_scheduler_processing_read_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tikv_scheduler_processing_read_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)",
                Labels: &["instance", "type"],
                Comment: "The total count of scheduler processing read in command",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_scheduler_processing_read_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tikv_scheduler_processing_read_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)",
                Labels: &["instance", "type"],
                Comment: "The total time of scheduler processing read in command",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_scheduler_keys_read",
            MetricTableDef {
                PromQL: r###"histogram_quantile($QUANTILE, sum(rate(tikv_scheduler_kv_command_key_read_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,instance,type))"###,
                Labels: &["instance", "type"],
                Quantile: 0.99,
                Comment: "The quantile count of keys read by command",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_scheduler_keys_read_avg",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_scheduler_kv_command_key_read_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) / sum(rate(tikv_scheduler_kv_command_key_read_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) "###,
                Labels: &["instance", "type"],
                Comment: "The average count of keys read by command",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_scheduler_keys_written",
            MetricTableDef {
                PromQL: r###"histogram_quantile($QUANTILE, sum(rate(tikv_scheduler_kv_command_key_write_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,instance,type))"###,
                Labels: &["instance", "type"],
                Quantile: 0.99,
                Comment: "The quantile of count of keys written by a command",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_scheduler_keys_written_avg",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_scheduler_kv_command_key_write_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) / sum(rate(tikv_scheduler_kv_command_key_write_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) "###,
                Labels: &["instance", "type"],
                Comment: "The average count of keys written by a command",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_scheduler_scan_details",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_scheduler_kv_scan_details{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (tag,instance,req,cf)"###,
                Labels: &["instance", "tag", "req", "cf"],
                Comment: "The keys scan details of each CF when executing command",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_scheduler_scan_details_total_num",
            MetricTableDef {
                PromQL: r###"sum(increase(tikv_scheduler_kv_scan_details{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (tag,instance,req,cf)"###,
                Labels: &["instance", "tag", "req", "cf"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_mvcc_versions",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_storage_mvcc_versions_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION]))"###,
                Labels: &["instance"],
                Comment: "The number of versions for each key",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_mvcc_delete_versions",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_storage_mvcc_gc_delete_versions_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)"###,
                Labels: &["instance"],
                Comment: "The number of versions deleted by GC for each key",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_gc_tasks_ops",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_gcworker_gc_tasks_vec{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (task,instance)"###,
                Labels: &["instance", "task"],
                Comment: "The count of GC total tasks processed by gc_worker per second",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_gc_skipped_tasks",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_storage_gc_skipped_counter{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (task,instance)"###,
                Labels: &["instance", "task"],
                Comment: "The count of GC skipped tasks processed by gc_worker",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_gc_fail_tasks",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_gcworker_gc_task_fail_vec{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (task,instance)"###,
                Labels: &["instance", "task"],
                Comment: "The count of GC tasks processed fail by gc_worker",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_gc_too_busy",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_gc_worker_too_busy{$LABEL_CONDITIONS}[$RANGE_DURATION]))"###,
                Labels: &["instance"],
                Comment: "The count of GC worker too busy",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_gc_tasks_duration",
            MetricTableDef {
                PromQL: r###"histogram_quantile($QUANTILE, sum(rate(tikv_gcworker_gc_task_duration_vec_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,task,instance))"###,
                Labels: &["instance", "task"],
                Quantile: 1.0,
                Comment: "The quantile of time consumed when executing GC tasks",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_gc_tasks_avg_duration",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_gcworker_gc_task_duration_vec_sum{}[$RANGE_DURATION])) by (task,instance) / sum(rate(tikv_gcworker_gc_task_duration_vec_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (task,instance)"###,
                Labels: &["instance", "task"],
                Comment: "The time consumed when executing GC tasks",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_gc_keys",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_gcworker_gc_keys{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (tag,cf,instance)"###,
                Labels: &["instance", "tag", "cf"],
                Comment: "The count of keys in write CF affected during GC",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_gc_keys_total_num",
            MetricTableDef {
                PromQL: r###"sum(increase(tikv_gcworker_gc_keys{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (tag,cf,instance)"###,
                Labels: &["instance", "tag", "cf"],
                Comment: "The total number of keys in write CF affected during GC",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_gc_speed",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_storage_mvcc_gc_delete_versions_sum[$RANGE_DURATION]))"###,
                Comment: "The GC keys per second",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_auto_gc_working",
            MetricTableDef {
                PromQL: r###"sum(max_over_time(tikv_gcworker_autogc_status{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,state)"###,
                Labels: &["instance", "state"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_client_task_progress",
            MetricTableDef {
                PromQL: r###"max(tidb_tikvclient_range_task_stats{$LABEL_CONDITIONS}) by (result,type)"###,
                Labels: &["result", "type"],
                Comment: "The progress of tikv client task",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_auto_gc_progress",
            MetricTableDef {
                PromQL: r###"sum(tikv_gcworker_autogc_processed_regions{type="scan"}) by (instance,type) / sum(tikv_raftstore_region_count{type="region"}) by (instance)"###,
                Labels: &["instance"],
                Comment: "Progress of TiKV's GC",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_auto_gc_safepoint",
            MetricTableDef {
                PromQL: r###"max(tikv_gcworker_autogc_safe_point{$LABEL_CONDITIONS}) by (instance) / (2^18)"###,
                Labels: &["instance"],
                Comment: "SafePoint used for TiKV's Auto GC",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_send_snapshot_duration",
            MetricTableDef {
                PromQL: r###"histogram_quantile($QUANTILE, sum(rate(tikv_server_send_snapshot_duration_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,instance))"###,
                Labels: &["instance"],
                Quantile: 0.99,
                Comment: "The quantile of time consumed when sending snapshots",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_handle_snapshot_duration",
            MetricTableDef {
                PromQL: r###"histogram_quantile($QUANTILE, sum(rate(tikv_raftstore_snapshot_duration_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,instance,type))"###,
                Labels: &["instance", "type"],
                Quantile: 0.99,
                Comment: "The quantile of time consumed when handling snapshots",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_snapshot_state_count",
            MetricTableDef {
                PromQL: r###"sum(tikv_raftstore_snapshot_traffic_total{$LABEL_CONDITIONS}) by (type,instance)"###,
                Labels: &["instance", "type"],
                Comment: "The number of snapshots in different states",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_snapshot_state_total_count",
            MetricTableDef {
                PromQL: r###"sum(delta(tikv_raftstore_snapshot_traffic_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,instance)"###,
                Labels: &["instance", "type"],
                Comment: "The total number of snapshots in different states",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_snapshot_size",
            MetricTableDef {
                PromQL: r###"histogram_quantile($QUANTILE, sum(rate(tikv_snapshot_size_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,instance))"###,
                Labels: &["instance"],
                Quantile: 0.9999,
                Comment: "The quantile of snapshot size",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_snapshot_kv_count",
            MetricTableDef {
                PromQL: r###"histogram_quantile($QUANTILE, sum(rate(tikv_snapshot_kv_count_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,instance))"###,
                Labels: &["instance"],
                Quantile: 0.9999,
                Comment: "The quantile of number of KV within a snapshot",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_worker_handled_tasks",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_worker_handled_task_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (name,instance)"###,
                Labels: &["instance", "name"],
                Comment: "The number of tasks handled by worker",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_worker_handled_tasks_total_num",
            MetricTableDef {
                PromQL: r###"sum(increase(tikv_worker_handled_task_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (name,instance)"###,
                Labels: &["instance", "name"],
                Comment: "Total number of tasks handled by worker",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_worker_pending_tasks",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_worker_pending_task_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (name,instance)"###,
                Labels: &["instance", "name"],
                Comment: "Current pending and running tasks of worker",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_worker_pending_tasks_total_num",
            MetricTableDef {
                PromQL: r###"sum(increase(tikv_worker_pending_task_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (name,instance)"###,
                Labels: &["instance", "name"],
                Comment: "Total pending and running tasks of worker",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_futurepool_handled_tasks",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_futurepool_handled_task_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (name,instance)"###,
                Labels: &["instance", "name"],
                Comment: "The number of tasks handled by future_pool",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_futurepool_handled_tasks_total_num",
            MetricTableDef {
                PromQL: r###"sum(increase(tikv_futurepool_handled_task_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (name,instance)"###,
                Labels: &["instance", "name"],
                Comment: "Total number of tasks handled by future_pool",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_futurepool_pending_tasks",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_futurepool_pending_task_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (name,instance)"###,
                Labels: &["instance", "name"],
                Comment: "Current pending and running tasks of future_pool",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_futurepool_pending_tasks_total_num",
            MetricTableDef {
                PromQL: r###"sum(increase(tikv_futurepool_pending_task_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (name,instance)"###,
                Labels: &["instance", "name"],
                Comment: "Total pending and running tasks of future_pool",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_cop_request_durations",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_coprocessor_request_duration_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,instance,req)"###,
                Labels: &["instance", "req"],
                Comment: "The time consumed to handle coprocessor read requests",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_cop_request_duration",
            MetricTableDef {
                PromQL: r###"histogram_quantile($QUANTILE, sum(rate(tikv_coprocessor_request_duration_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,req,instance))"###,
                Labels: &["instance", "req"],
                Quantile: 1.0,
                Comment: "The quantile of time consumed to handle coprocessor read requests",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_cop_requests_ops",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_coprocessor_request_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (req,instance)"###,
                Labels: &["instance", "req"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_cop_scan_keys_num",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_coprocessor_scan_keys_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (req,instance)"###,
                Labels: &["instance", "req"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_cop_kv_cursor_operations",
            MetricTableDef {
                PromQL: r###"histogram_quantile($QUANTILE, avg(rate(tikv_coprocessor_scan_keys_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,req,instance)) "###,
                Labels: &["instance", "req"],
                Quantile: 1.0,
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_cop_total_rocksdb_perf_statistics",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_coprocessor_rocksdb_perf{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (req,metric,instance)"###,
                Labels: &["instance", "req", "metric"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_cop_total_response_size_per_seconds",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_coprocessor_response_bytes{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)"###,
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_cop_handle_duration",
            MetricTableDef {
                PromQL: r###"histogram_quantile($QUANTILE, sum(rate(tikv_coprocessor_request_handle_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,req,instance))"###,
                Labels: &["instance", "req"],
                Quantile: 1.0,
                Comment: "The quantile of time consumed when handling coprocessor requests",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_cop_wait_duration",
            MetricTableDef {
                PromQL: r###"histogram_quantile($QUANTILE, sum(rate(tikv_coprocessor_request_wait_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,req,type,instance))"###,
                Labels: &["instance", "req", "type"],
                Quantile: 1.0,
                Comment: "The quantile of time consumed when coprocessor requests are wait for being handled",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_cop_dag_requests_ops",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_coprocessor_dag_request_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (vec_type,instance)"###,
                Labels: &["instance", "vec_type"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_cop_dag_executors_ops",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_coprocessor_executor_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,instance)"###,
                Labels: &["instance", "type"],
                Comment: "The number of DAG executors per seconds",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_cop_scan_details",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_coprocessor_scan_details{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (tag,req,cf,instance)"###,
                Labels: &["instance", "tag", "req", "cf"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_cop_scan_details_total",
            MetricTableDef {
                PromQL: r###"sum(increase(tikv_coprocessor_scan_details{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (tag,req,cf,instance)"###,
                Labels: &["instance", "tag", "req", "cf"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_threads_state",
            MetricTableDef {
                PromQL: r###"sum(tikv_threads_state{$LABEL_CONDITIONS}) by (instance,state)"###,
                Labels: &["instance", "state"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_threads_io",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_threads_io_bytes_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (name,io,instance)"###,
                Labels: &["instance", "io", "name"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_thread_voluntary_context_switches",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_thread_voluntary_context_switches{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance, name)"###,
                Labels: &["instance", "name"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_thread_nonvoluntary_context_switches",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_thread_nonvoluntary_context_switches{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance, name)"###,
                Labels: &["instance", "name"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_engine_get_cpu_cache_operations",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_engine_get_served{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (db,type,instance)"###,
                Labels: &["instance", "type", "db"],
                Comment: "The count of get l0,l1,l2 operations",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_engine_get_block_cache_operations",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_engine_cache_efficiency{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (db,type,instance)"###,
                Labels: &["instance", "type", "db"],
                Comment: "The count of get memtable operations",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_engine_get_memtable_operations",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_engine_memtable_efficiency{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (db,type,instance)"###,
                Labels: &["instance", "type", "db"],
                Comment: "The count of get memtable operations",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_engine_max_get_duration",
            MetricTableDef {
                PromQL: r###"max(tikv_engine_get_micro_seconds{$LABEL_CONDITIONS}) by (db,type,instance)"###,
                Labels: &["instance", "type", "db"],
                Comment: "The max time consumed when executing get operations, the unit is microsecond",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_engine_avg_get_duration",
            MetricTableDef {
                PromQL: r###"avg(tikv_engine_get_micro_seconds{$LABEL_CONDITIONS}) by (db,type,instance)"###,
                Labels: &["instance", "type", "db"],
                Comment: "The average time consumed when executing get operations, the unit is microsecond",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_engine_seek_operations",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_engine_locate{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (db,type,instance)"###,
                Labels: &["instance", "type", "db"],
                Comment: "The count of seek operations",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_engine_max_seek_duration",
            MetricTableDef {
                PromQL: r###"max(tikv_engine_seek_micro_seconds{$LABEL_CONDITIONS}) by (db,type,instance)"###,
                Labels: &["instance", "type", "db"],
                Comment: "The time consumed when executing seek operation, the unit is microsecond",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_engine_avg_seek_duration",
            MetricTableDef {
                PromQL: r###"avg(tikv_engine_seek_micro_seconds{$LABEL_CONDITIONS}) by (db,type,instance)"###,
                Labels: &["instance", "type", "db"],
                Comment: "The time consumed when executing seek operation, the unit is microsecond",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_engine_write_operations",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_engine_write_served{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (db,type,instance)"###,
                Labels: &["instance", "type", "db"],
                Comment: "The count of write operations",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_engine_write_duration",
            MetricTableDef {
                PromQL: r###"avg(tikv_engine_write_micro_seconds{$LABEL_CONDITIONS}) by (db,type,instance)"###,
                Labels: &["instance", "type", "db"],
                Comment: "The time consumed when executing write operation, the unit is microsecond",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_engine_write_max_duration",
            MetricTableDef {
                PromQL: r###"max(tikv_engine_write_micro_seconds{$LABEL_CONDITIONS}) by (db,type,instance)"###,
                Labels: &["instance", "type", "db"],
                Comment: "The time consumed when executing write operation, the unit is microsecond",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_engine_wal_sync_operations",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_engine_wal_file_synced{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (db,instance)"###,
                Labels: &["instance", "db"],
                Comment: "The count of WAL sync operations",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_wal_sync_max_duration",
            MetricTableDef {
                PromQL: r###"max(tikv_engine_wal_file_sync_micro_seconds{$LABEL_CONDITIONS}) by (db,type,instance)"###,
                Labels: &["instance", "type", "db"],
                Comment: "The max time consumed when executing WAL sync operation, the unit is microsecond",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_wal_sync_duration",
            MetricTableDef {
                PromQL: r###"avg(tikv_engine_wal_file_sync_micro_seconds{$LABEL_CONDITIONS}) by (db,type,instance)"###,
                Labels: &["instance", "type"],
                Comment: "The time consumed when executing WAL sync operation, the unit is microsecond",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_compaction_operations",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_engine_event_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,instance,db)"###,
                Labels: &["instance", "type", "db"],
                Comment: "The count of compaction and flush operations",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_compaction_max_duration",
            MetricTableDef {
                PromQL: r###"max(tikv_engine_compaction_time{$LABEL_CONDITIONS}) by (type,instance,db)"###,
                Labels: &["instance", "type", "db"],
                Comment: "The time consumed when executing the compaction and flush operations",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_compaction_duration",
            MetricTableDef {
                PromQL: r###"avg(tikv_engine_compaction_time{$LABEL_CONDITIONS}) by (type,instance,db)"###,
                Labels: &["instance", "type", "db"],
                Comment: "The time consumed when executing the compaction and flush operations",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_sst_read_max_duration",
            MetricTableDef {
                PromQL: r###"max(tikv_engine_sst_read_micros{$LABEL_CONDITIONS}) by (type,instance,db)"###,
                Labels: &["instance", "type", "db"],
                Comment: "The max time consumed when reading SST files",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_sst_read_duration",
            MetricTableDef {
                PromQL: r###"avg(tikv_engine_sst_read_micros{$LABEL_CONDITIONS}) by (type,instance,db)"###,
                Labels: &["instance", "type", "db"],
                Comment: "The time consumed when reading SST files",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_write_stall_max_duration",
            MetricTableDef {
                PromQL: r###"max(tikv_engine_write_stall{$LABEL_CONDITIONS}) by (type,instance,db)"###,
                Labels: &["instance", "type", "db"],
                Comment: "The time which is caused by write stall",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_write_stall_avg_duration",
            MetricTableDef {
                PromQL: r###"avg(tikv_engine_write_stall{$LABEL_CONDITIONS}) by (type,instance,db)"###,
                Labels: &["instance", "type", "db"],
                Comment: "The time which is caused by write stall",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_memtable_size",
            MetricTableDef {
                PromQL: r###"avg(tikv_engine_memory_bytes{$LABEL_CONDITIONS}) by (type,instance,db,cf)"###,
                Labels: &["instance", "cf", "type", "db"],
                Comment: "The memtable size of each column family",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_memtable_hit",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_engine_memtable_efficiency{type="memtable_hit"}[$RANGE_DURATION])) by (instance,db) / (sum(rate(tikv_engine_memtable_efficiency{}[$RANGE_DURATION])) by (instance,db) + sum(rate(tikv_engine_memtable_efficiency{}[$RANGE_DURATION])) by (instance,db))"###,
                Labels: &["instance", "db"],
                Comment: "The hit rate of memtable",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_block_cache_size",
            MetricTableDef {
                PromQL: r###"topk(20, avg(tikv_engine_block_cache_size_bytes{$LABEL_CONDITIONS}) by(cf, instance, db))"###,
                Labels: &["instance", "cf", "db"],
                Comment: "The block cache size. Broken down by column family if shared block cache is disabled.",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_block_all_cache_hit",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_engine_cache_efficiency{type="block_cache_hit"}[$RANGE_DURATION])) by (db,instance) / (sum(rate(tikv_engine_cache_efficiency{type="block_cache_hit"}[$RANGE_DURATION])) by (db,instance) + sum(rate(tikv_engine_cache_efficiency{type="block_cache_miss"}[$RANGE_DURATION])) by (db,instance))"###,
                Labels: &["instance", "db"],
                Comment: "The hit rate of all block cache",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_block_data_cache_hit",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_engine_cache_efficiency{type="block_cache_data_hit"}[$RANGE_DURATION])) by (db,instance) / (sum(rate(tikv_engine_cache_efficiency{type="block_cache_data_hit"}[$RANGE_DURATION])) by (db,instance) + sum(rate(tikv_engine_cache_efficiency{type="block_cache_data_miss"}[$RANGE_DURATION])) by (db,instance))"###,
                Labels: &["instance", "db"],
                Comment: "The hit rate of data block cache",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_block_filter_cache_hit",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_engine_cache_efficiency{type="block_cache_filter_hit"}[$RANGE_DURATION])) by (db,instance) / (sum(rate(tikv_engine_cache_efficiency{type="block_cache_filter_hit"}[$RANGE_DURATION])) by (db,instance) + sum(rate(tikv_engine_cache_efficiency{type="block_cache_filter_miss"}[$RANGE_DURATION])) by (db,instance))"###,
                Labels: &["instance", "db"],
                Comment: "The hit rate of data block cache",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_block_index_cache_hit",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_engine_cache_efficiency{type="block_cache_index_hit"}[$RANGE_DURATION])) by (db,instance) / (sum(rate(tikv_engine_cache_efficiency{type="block_cache_index_hit"}[$RANGE_DURATION])) by (db,instance) + sum(rate(tikv_engine_cache_efficiency{type="block_cache_index_miss"}[$RANGE_DURATION])) by (db,instance))"###,
                Labels: &["instance", "db"],
                Comment: "The hit rate of data block cache",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_block_bloom_prefix_cache_hit",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_engine_bloom_efficiency{type="bloom_prefix_useful"}[$RANGE_DURATION])) by (db,instance) / sum(rate(tikv_engine_bloom_efficiency{type="bloom_prefix_checked"}[$RANGE_DURATION])) by (db,instance)"###,
                Labels: &["instance", "db"],
                Comment: "The hit rate of data block cache",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_corrrput_keys_flow",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_engine_compaction_num_corrupt_keys{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (db,cf,instance)"###,
                Labels: &["instance", "db", "cf"],
                Comment: "The flow of corrupt operations on keys",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_total_keys",
            MetricTableDef {
                PromQL: r###"sum(tikv_engine_estimate_num_keys{$LABEL_CONDITIONS}) by (db,cf,instance)"###,
                Labels: &["instance", "db", "cf"],
                Comment: "The count of keys in each column family",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_per_read_max_bytes",
            MetricTableDef {
                PromQL: r###"max(tikv_engine_bytes_per_read{$LABEL_CONDITIONS}) by (type,db,instance)"###,
                Labels: &["instance", "type", "db"],
                Comment: "The max bytes per read",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_per_read_avg_bytes",
            MetricTableDef {
                PromQL: r###"avg(tikv_engine_bytes_per_read{$LABEL_CONDITIONS}) by (type,db,instance)"###,
                Labels: &["instance", "type", "db"],
                Comment: "The avg bytes per read",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_per_write_max_bytes",
            MetricTableDef {
                PromQL: r###"max(tikv_engine_bytes_per_write{$LABEL_CONDITIONS}) by (type,db,instance)"###,
                Labels: &["instance", "type", "db"],
                Comment: "The max bytes per write",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_per_write_avg_bytes",
            MetricTableDef {
                PromQL: r###"avg(tikv_engine_bytes_per_write{$LABEL_CONDITIONS}) by (type,db,instance)"###,
                Labels: &["instance", "type", "db"],
                Comment: "The avg bytes per write",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_engine_compaction_flow_bytes",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_engine_compaction_flow_bytes{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (db,type,instance)"###,
                Labels: &["instance", "type", "db"],
                Comment: "The flow rate of compaction operations per type",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_compaction_pending_bytes",
            MetricTableDef {
                PromQL: r###"tikv_engine_pending_compaction_bytes{$LABEL_CONDITIONS}"###,
                Labels: &["instance", "cf", "db"],
                Comment: "The pending bytes to be compacted",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_read_amplication",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_engine_read_amp_flow_bytes{type="read_amp_total_read_bytes"}[$RANGE_DURATION])) by (instance,db) / sum(rate(tikv_engine_read_amp_flow_bytes{type="read_amp_estimate_useful_bytes"}[$RANGE_DURATION])) by (instance,db)"###,
                Labels: &["instance", "db"],
                Comment: "The read amplification per TiKV instance",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_compression_ratio",
            MetricTableDef {
                PromQL: r###"avg(tikv_engine_compression_ratio{$LABEL_CONDITIONS}) by (level,instance,db)"###,
                Labels: &["instance", "level", "db"],
                Comment: "The compression ratio of each level",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_number_of_snapshots",
            MetricTableDef {
                PromQL: r###"tikv_engine_num_snapshots{$LABEL_CONDITIONS}"###,
                Labels: &["instance", "db"],
                Comment: "The number of snapshot of each TiKV instance",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_oldest_snapshots_duration",
            MetricTableDef {
                PromQL: r###"tikv_engine_oldest_snapshot_duration{$LABEL_CONDITIONS}"###,
                Labels: &["instance", "db"],
                Comment: "The time that the oldest unreleased snapshot survivals",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_number_files_at_each_level",
            MetricTableDef {
                PromQL: r###"avg(tikv_engine_num_files_at_level{$LABEL_CONDITIONS}) by (cf, level,db,instance)"###,
                Labels: &["instance", "cf", "level", "db"],
                Comment: "The number of SST files for different column families in each level",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_ingest_sst_duration",
            MetricTableDef {
                PromQL: r###"histogram_quantile($QUANTILE, sum(rate(tikv_snapshot_ingest_sst_duration_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,instance,db))"###,
                Labels: &["instance", "db"],
                Quantile: 0.99,
                Comment: "The quantile of time consumed when ingesting SST files",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_ingest_sst_avg_duration",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_snapshot_ingest_sst_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) / sum(rate(tikv_snapshot_ingest_sst_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION]))"###,
                Labels: &["instance"],
                Comment: "The average time consumed when ingesting SST files",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_stall_conditions_changed_of_each_cf",
            MetricTableDef {
                PromQL: r###"tikv_engine_stall_conditions_changed{$LABEL_CONDITIONS}"###,
                Labels: &["instance", "cf", "type", "db"],
                Comment: "Stall conditions changed of each column family",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_write_stall_reason",
            MetricTableDef {
                PromQL: r###"sum(increase(tikv_engine_write_stall_reason{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (db,type,instance)"###,
                Labels: &["instance", "type", "db"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_compaction_reason",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_engine_compaction_reason{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (db,cf,reason,instance)"###,
                Labels: &["instance", "cf", "reason", "db"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_engine_blob_key_max_size",
            MetricTableDef {
                PromQL: r###"max(tikv_engine_blob_key_size{$LABEL_CONDITIONS}) by (db,instance,type)"###,
                Labels: &["instance", "type", "db"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_engine_blob_key_avg_size",
            MetricTableDef {
                PromQL: r###"avg(tikv_engine_blob_key_size{$LABEL_CONDITIONS}) by (db,instance,type)"###,
                Labels: &["instance", "type", "db"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_engine_blob_value_avg_size",
            MetricTableDef {
                PromQL: r###"avg(tikv_engine_blob_value_size{$LABEL_CONDITIONS}) by (db,instance,type)"###,
                Labels: &["instance", "type", "db"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_engine_blob_value_max_size",
            MetricTableDef {
                PromQL: r###"max(tikv_engine_blob_value_size{$LABEL_CONDITIONS}) by (db,instance,type)"###,
                Labels: &["instance", "type", "db"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_engine_blob_seek_duration",
            MetricTableDef {
                PromQL: r###"avg(tikv_engine_blob_seek_micros_seconds{$LABEL_CONDITIONS}) by (db,type,instance)"###,
                Labels: &["instance", "type", "db"],
                Comment: "the unit is microsecond",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_engine_blob_seek_operations",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_engine_blob_locate{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (db,type,instance)"###,
                Labels: &["instance", "type", "db"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_engine_blob_get_duration",
            MetricTableDef {
                PromQL: r###"avg(tikv_engine_blob_get_micros_seconds{$LABEL_CONDITIONS}) by (type,db,instance)"###,
                Labels: &["instance", "type", "db"],
                Comment: "the unit is microsecond",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_engine_blob_bytes_flow",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_engine_blob_flow_bytes{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,instance,db)"###,
                Labels: &["instance", "type", "db"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_engine_blob_file_read_duration",
            MetricTableDef {
                PromQL: r###"avg(tikv_engine_blob_file_read_micros_seconds{$LABEL_CONDITIONS}) by (type,instance,db)"###,
                Labels: &["instance", "type", "db"],
                Comment: "the unit is microsecond",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_engine_blob_file_write_duration",
            MetricTableDef {
                PromQL: r###"avg(tikv_engine_blob_file_write_micros_seconds{$LABEL_CONDITIONS}) by (type,instance,db)"###,
                Labels: &["instance", "type", "db"],
                Comment: "the unit is microsecond",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_engine_blob_file_sync_operations",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_engine_blob_file_synced{$LABEL_CONDITIONS}[$RANGE_DURATION]))"###,
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_engine_blob_file_sync_duration",
            MetricTableDef {
                PromQL: r###"avg(tikv_engine_blob_file_sync_micros_seconds{$LABEL_CONDITIONS}) by (instance,type,db)"###,
                Labels: &["instance", "type", "db"],
                Comment: "the unit is microsecond",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_engine_blob_file_count",
            MetricTableDef {
                PromQL: r###"avg(tikv_engine_titandb_num_obsolete_blob_file{$LABEL_CONDITIONS}) by (instance,db)"###,
                Labels: &["instance", "db"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_engine_blob_file_size",
            MetricTableDef {
                PromQL: r###"avg(tikv_engine_titandb_obsolete_blob_file_size{$LABEL_CONDITIONS}) by (instance,db)"###,
                Labels: &["instance", "db"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_engine_blob_gc_file",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_engine_blob_gc_file_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,instance,db)"###,
                Labels: &["instance", "type", "db"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_engine_blob_gc_duration",
            MetricTableDef {
                PromQL: r###"avg(tikv_engine_blob_gc_micros_seconds{$LABEL_CONDITIONS}) by (db,instance,type)"###,
                Labels: &["instance", "type", "db"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_engine_blob_gc_bytes_flow",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_engine_blob_gc_flow_bytes{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,instance,db)"###,
                Labels: &["instance", "type", "db"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_engine_blob_gc_keys_flow",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_engine_blob_gc_flow_bytes{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,instance,db)"###,
                Labels: &["instance", "type", "db"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_engine_live_blob_size",
            MetricTableDef {
                PromQL: r###"avg(tikv_engine_titandb_live_blob_size{$LABEL_CONDITIONS}) by (instance,db)"###,
                Labels: &["instance", "db"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_lock_manager_handled_tasks",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_lock_manager_task_counter{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,instance)"###,
                Labels: &["instance", "type"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_lock_manager_waiter_lifetime_duration",
            MetricTableDef {
                PromQL: r###"histogram_quantile($QUANTILE, sum(rate(tikv_lock_manager_waiter_lifetime_duration_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,instance))"###,
                Labels: &["instance"],
                Quantile: 0.99,
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_lock_manager_waiter_lifetime_avg_duration",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_lock_manager_waiter_lifetime_duration_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) / sum(rate(tikv_lock_manager_waiter_lifetime_duration_count{$LABEL_CONDITIONS}[$RANGE_DURATION]))"###,
                Labels: &["instance", "type"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_lock_manager_wait_table",
            MetricTableDef {
                PromQL: r###"sum(max_over_time(tikv_lock_manager_wait_table_status{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,instance)"###,
                Labels: &["instance", "type"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_lock_manager_deadlock_detect_duration",
            MetricTableDef {
                PromQL: r###"histogram_quantile($QUANTILE, sum(rate(tikv_lock_manager_detect_duration_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,instance))"###,
                Labels: &["instance"],
                Quantile: 0.99,
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_lock_manager_deadlock_detect_avg_duration",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_lock_manager_detect_duration_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) / sum(rate(tikv_lock_manager_detect_duration_count{$LABEL_CONDITIONS}[$RANGE_DURATION]))"###,
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_lock_manager_detect_error",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_lock_manager_error_counter{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,instance)"###,
                Labels: &["instance", "type"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_lock_manager_detect_error_total_count",
            MetricTableDef {
                PromQL: r###"sum(increase(tikv_lock_manager_error_counter{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,instance)"###,
                Labels: &["instance", "type"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_lock_manager_deadlock_detector_leader",
            MetricTableDef {
                PromQL: r###"sum(max_over_time(tikv_lock_manager_detector_leader_heartbeat{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)"###,
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_allocator_stats",
            MetricTableDef {
                PromQL: r###"tikv_allocator_stats{$LABEL_CONDITIONS}"###,
                Labels: &["instance", "type"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_backup_range_size",
            MetricTableDef {
                PromQL: r###"histogram_quantile($QUANTILE, sum(rate(tikv_backup_range_size_bytes_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,cf,instance))"###,
                Labels: &["instance", "cf"],
                Quantile: 0.99,
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_backup_duration",
            MetricTableDef {
                PromQL: r###"histogram_quantile($QUANTILE, sum(rate(tikv_backup_request_duration_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,instance))"###,
                Labels: &["instance"],
                Quantile: 0.99,
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_backup_avg_duration",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_backup_request_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) / sum(rate(tikv_backup_request_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION]))"###,
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_backup_flow",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_backup_range_size_bytes_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)"###,
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_disk_read_bytes",
            MetricTableDef {
                PromQL: r###"sum(irate(node_disk_read_bytes_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,device)"###,
                Labels: &["instance", "device"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_disk_write_bytes",
            MetricTableDef {
                PromQL: r###"sum(irate(node_disk_written_bytes_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,device)"###,
                Labels: &["instance", "device"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_backup_range_duration",
            MetricTableDef {
                PromQL: r###"histogram_quantile($QUANTILE, sum(rate(tikv_backup_range_duration_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,type,instance))"###,
                Labels: &["instance", "type"],
                Quantile: 0.99,
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_backup_range_avg_duration",
            MetricTableDef {
                PromQL: r###"sum(rate(tikv_backup_range_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,instance) / sum(rate(tikv_backup_range_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,instance)"###,
                Labels: &["instance", "type"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_backup_errors",
            MetricTableDef {
                PromQL: r###"rate(tikv_backup_error_counter{$LABEL_CONDITIONS}[$RANGE_DURATION])"###,
                Labels: &["instance", "error"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_backup_errors_total_count",
            MetricTableDef {
                PromQL: r###"sum(increase(tikv_backup_error_counter{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,error)"###,
                Labels: &["instance", "error"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "node_virtual_cpus",
            MetricTableDef {
                PromQL: r###"count(node_cpu_seconds_total{mode="user"}) by (instance)"###,
                Labels: &["instance"],
                Comment: "node virtual cpu count",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "node_total_memory",
            MetricTableDef {
                PromQL: r###"node_memory_MemTotal_bytes{$LABEL_CONDITIONS}"###,
                Labels: &["instance"],
                Comment: "total memory in node",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "node_memory_available",
            MetricTableDef {
                PromQL: r###"node_memory_MemAvailable_bytes{$LABEL_CONDITIONS}"###,
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "node_memory_usage",
            MetricTableDef {
                PromQL: r###"100* (1-(node_memory_MemAvailable_bytes{$LABEL_CONDITIONS}/node_memory_MemTotal_bytes{$LABEL_CONDITIONS}))"###,
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "node_memory_swap_used",
            MetricTableDef {
                PromQL: r###"node_memory_SwapTotal_bytes{$LABEL_CONDITIONS} - node_memory_SwapFree_bytes{$LABEL_CONDITIONS}"###,
                Labels: &["instance"],
                Comment: "bytes used of node swap memory",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "node_uptime",
            MetricTableDef {
                PromQL: r###"node_time_seconds{$LABEL_CONDITIONS} - node_boot_time_seconds{$LABEL_CONDITIONS}"###,
                Labels: &["instance"],
                Comment: "node uptime, units are seconds",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "node_load1",
            MetricTableDef {
                PromQL: r###"node_load1{$LABEL_CONDITIONS}"###,
                Labels: &["instance"],
                Comment: "1 minute load averages in node",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "node_load5",
            MetricTableDef {
                PromQL: r###"node_load5{$LABEL_CONDITIONS}"###,
                Labels: &["instance"],
                Comment: "5 minutes load averages in node",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "node_load15",
            MetricTableDef {
                PromQL: r###"node_load15{$LABEL_CONDITIONS}"###,
                Labels: &["instance"],
                Comment: "15 minutes load averages in node",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "node_kernel_interrupts",
            MetricTableDef {
                PromQL: r###"rate(node_intr_total{$LABEL_CONDITIONS}[$RANGE_DURATION]) or irate(node_intr_total{$LABEL_CONDITIONS}[$RANGE_DURATION])"###,
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "node_kernel_forks",
            MetricTableDef {
                PromQL: r###"rate(node_forks_total{$LABEL_CONDITIONS}[$RANGE_DURATION]) or irate(node_forks_total{$LABEL_CONDITIONS}[$RANGE_DURATION])"###,
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "node_kernel_context_switches",
            MetricTableDef {
                PromQL: r###"rate(node_context_switches_total{$LABEL_CONDITIONS}[$RANGE_DURATION]) or irate(node_context_switches_total{$LABEL_CONDITIONS}[$RANGE_DURATION])"###,
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "node_cpu_usage",
            MetricTableDef {
                PromQL: r###"sum(rate(node_cpu_seconds_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (mode,instance) * 100 / count(node_cpu_seconds_total{$LABEL_CONDITIONS}) by (mode,instance) or sum(irate(node_cpu_seconds_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (mode,instance) * 100 / count(node_cpu_seconds_total{$LABEL_CONDITIONS}) by (mode,instance)"###,
                Labels: &["instance", "mode"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "node_memory_free",
            MetricTableDef {
                PromQL: r###"node_memory_MemFree_bytes{$LABEL_CONDITIONS}"###,
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "node_memory_buffers",
            MetricTableDef {
                PromQL: r###"node_memory_Buffers_bytes{$LABEL_CONDITIONS}"###,
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "node_memory_cached",
            MetricTableDef {
                PromQL: r###"node_memory_Cached_bytes{$LABEL_CONDITIONS}"###,
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "node_memory_active",
            MetricTableDef {
                PromQL: r###"node_memory_Active_bytes{$LABEL_CONDITIONS}"###,
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "node_memory_inactive",
            MetricTableDef {
                PromQL: r###"node_memory_Inactive_bytes{$LABEL_CONDITIONS}"###,
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "node_memory_writeback",
            MetricTableDef {
                PromQL: r###"node_memory_Writeback_bytes{$LABEL_CONDITIONS}"###,
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "node_memory_writeback_tmp",
            MetricTableDef {
                PromQL: r###"node_memory_WritebackTmp_bytes{$LABEL_CONDITIONS}"###,
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "node_memory_dirty",
            MetricTableDef {
                PromQL: r###"node_memory_Dirty_bytes{$LABEL_CONDITIONS}"###,
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "node_memory_shared",
            MetricTableDef {
                PromQL: r###"node_memory_Shmem_bytes{$LABEL_CONDITIONS}"###,
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "node_memory_mapped",
            MetricTableDef {
                PromQL: r###"node_memory_Mapped_bytes{$LABEL_CONDITIONS}"###,
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "node_disk_size",
            MetricTableDef {
                PromQL: r###"node_filesystem_size_bytes{$LABEL_CONDITIONS}"###,
                Labels: &["instance", "device", "fstype", "mountpoint"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "node_disk_available_size",
            MetricTableDef {
                PromQL: r###"node_filesystem_avail_bytes{$LABEL_CONDITIONS}"###,
                Labels: &["instance", "device", "fstype", "mountpoint"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "node_disk_state",
            MetricTableDef {
                PromQL: r###"node_filesystem_readonly{$LABEL_CONDITIONS}"###,
                Labels: &["instance", "device", "fstype", "mountpoint"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "node_disk_io_util",
            MetricTableDef {
                PromQL: r###"rate(node_disk_io_time_seconds_total{$LABEL_CONDITIONS}[$RANGE_DURATION]) or irate(node_disk_io_time_seconds_total{$LABEL_CONDITIONS}[$RANGE_DURATION])"###,
                Labels: &["instance", "device"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "node_disk_iops",
            MetricTableDef {
                PromQL: r###"sum(rate(node_disk_reads_completed_total{$LABEL_CONDITIONS}[$RANGE_DURATION]) + rate(node_disk_writes_completed_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,device)"###,
                Labels: &["instance", "device"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "node_disk_write_latency",
            MetricTableDef {
                PromQL: r###"(rate(node_disk_write_time_seconds_total{$LABEL_CONDITIONS}[$RANGE_DURATION])/ rate(node_disk_writes_completed_total{$LABEL_CONDITIONS}[$RANGE_DURATION]))"###,
                Labels: &["instance", "device"],
                Comment: "node disk write latency",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "node_disk_read_latency",
            MetricTableDef {
                PromQL: r###"(rate(node_disk_read_time_seconds_total{$LABEL_CONDITIONS}[$RANGE_DURATION])/ rate(node_disk_reads_completed_total{$LABEL_CONDITIONS}[$RANGE_DURATION]))"###,
                Labels: &["instance", "device"],
                Comment: "node disk read latency",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "node_disk_throughput",
            MetricTableDef {
                PromQL: r###"irate(node_disk_read_bytes_total{$LABEL_CONDITIONS}[$RANGE_DURATION]) + irate(node_disk_written_bytes_total{$LABEL_CONDITIONS}[$RANGE_DURATION])"###,
                Labels: &["instance", "device"],
                Comment: "Units is byte",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "node_disk_usage",
            MetricTableDef {
                PromQL: r###"((node_filesystem_size_bytes{$LABEL_CONDITIONS} - node_filesystem_avail_bytes{$LABEL_CONDITIONS}) / node_filesystem_size_bytes{$LABEL_CONDITIONS}) * 100"###,
                Labels: &["instance", "device"],
                Comment: "Filesystem used space. If is > 80% then is Critical.",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "node_file_descriptor_allocated",
            MetricTableDef {
                PromQL: r###"node_filefd_allocated{$LABEL_CONDITIONS}"###,
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "node_network_in_drops",
            MetricTableDef {
                PromQL: r###"rate(node_network_receive_drop_total{$LABEL_CONDITIONS}[$RANGE_DURATION]) "###,
                Labels: &["instance", "device"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "node_network_out_drops",
            MetricTableDef {
                PromQL: r###"rate(node_network_transmit_drop_total{$LABEL_CONDITIONS}[$RANGE_DURATION])"###,
                Labels: &["instance", "device"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "node_network_in_errors",
            MetricTableDef {
                PromQL: r###"rate(node_network_receive_errs_total{$LABEL_CONDITIONS}[$RANGE_DURATION])"###,
                Labels: &["instance", "device"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "node_network_in_errors_total_count",
            MetricTableDef {
                PromQL: r###"sum(increase(node_network_receive_errs_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by(instance, device)"###,
                Labels: &["instance", "device"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "node_network_out_errors",
            MetricTableDef {
                PromQL: r###"rate(node_network_transmit_errs_total{$LABEL_CONDITIONS}[$RANGE_DURATION])"###,
                Labels: &["instance", "device"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "node_network_out_errors_total_count",
            MetricTableDef {
                PromQL: r###"sum(increase(node_network_transmit_errs_total{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance, device)"###,
                Labels: &["instance", "device"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "node_network_in_traffic",
            MetricTableDef {
                PromQL: r###"rate(node_network_receive_bytes_total{$LABEL_CONDITIONS}[$RANGE_DURATION]) or irate(node_network_receive_bytes_total{$LABEL_CONDITIONS}[$RANGE_DURATION])"###,
                Labels: &["instance", "device"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "node_network_out_traffic",
            MetricTableDef {
                PromQL: r###"rate(node_network_transmit_bytes_total{$LABEL_CONDITIONS}[$RANGE_DURATION]) or irate(node_network_transmit_bytes_total{$LABEL_CONDITIONS}[$RANGE_DURATION])"###,
                Labels: &["instance", "device"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "node_network_in_packets",
            MetricTableDef {
                PromQL: r###"rate(node_network_receive_packets_total{$LABEL_CONDITIONS}[$RANGE_DURATION]) or irate(node_network_receive_packets_total{$LABEL_CONDITIONS}[$RANGE_DURATION])"###,
                Labels: &["instance", "device"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "node_network_out_packets",
            MetricTableDef {
                PromQL: r###"rate(node_network_transmit_packets_total{$LABEL_CONDITIONS}[$RANGE_DURATION]) or irate(node_network_transmit_packets_total{$LABEL_CONDITIONS}[$RANGE_DURATION])"###,
                Labels: &["instance", "device"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "node_network_interface_speed",
            MetricTableDef {
                PromQL: r###"node_network_transmit_queue_length{$LABEL_CONDITIONS}"###,
                Labels: &["instance", "device"],
                Comment: "node_network_transmit_queue_length = transmit_queue_length value of /sys/class/net/<iface>.",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "node_network_utilization_in_hourly",
            MetricTableDef {
                PromQL: r###"sum(increase(node_network_receive_bytes_total{$LABEL_CONDITIONS}[1h]))"###,
                Labels: &["instance", "device"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "node_network_utilization_out_hourly",
            MetricTableDef {
                PromQL: r###"sum(increase(node_network_transmit_bytes_total{$LABEL_CONDITIONS}[1h]))"###,
                Labels: &["instance", "device"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "node_tcp_in_use",
            MetricTableDef {
                PromQL: r###"node_sockstat_TCP_inuse{$LABEL_CONDITIONS}"###,
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "node_tcp_segments_retransmitted",
            MetricTableDef {
                PromQL: r###"rate(node_netstat_Tcp_RetransSegs{$LABEL_CONDITIONS}[$RANGE_DURATION]) or irate(node_netstat_Tcp_RetransSegs{$LABEL_CONDITIONS}[$RANGE_DURATION])"###,
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "node_tcp_connections",
            MetricTableDef {
                PromQL: r###"node_netstat_Tcp_CurrEstab{$LABEL_CONDITIONS}"###,
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "node_processes_running",
            MetricTableDef {
                PromQL: r###"node_procs_running{$LABEL_CONDITIONS}"###,
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "node_processes_blocked",
            MetricTableDef {
                PromQL: r###"node_procs_blocked{$LABEL_CONDITIONS}"###,
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "etcd_wal_fsync_total_count",
            MetricTableDef {
                PromQL: "sum(increase(etcd_disk_wal_fsync_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total count of writing WAL into the persistent storage",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "etcd_wal_fsync_total_time",
            MetricTableDef {
                PromQL: "sum(increase(etcd_disk_wal_fsync_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total time of writing WAL into the persistent storage",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_client_cmd_total_count",
            MetricTableDef {
                PromQL: "sum(increase(pd_client_cmd_handle_cmds_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)",
                Labels: &["instance", "type"],
                Comment: "The total count of pd client command durations",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_client_cmd_total_time",
            MetricTableDef {
                PromQL: "sum(increase(pd_client_cmd_handle_cmds_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)",
                Labels: &["instance", "type"],
                Comment: "The total time of pd client command durations",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_grpc_completed_commands_total_count",
            MetricTableDef {
                PromQL: "sum(increase(grpc_server_handling_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,grpc_method)",
                Labels: &["instance", "grpc_method"],
                Comment: "The total count of completing each kind of gRPC commands",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_grpc_completed_commands_total_time",
            MetricTableDef {
                PromQL: "sum(increase(grpc_server_handling_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,grpc_method)",
                Labels: &["instance", "grpc_method"],
                Comment: "The total time of completing each kind of gRPC commands",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_request_rpc_total_count",
            MetricTableDef {
                PromQL: "sum(increase(pd_client_request_handle_requests_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)",
                Labels: &["instance", "type"],
                Comment: "The total count of pd client handle request duration(second)",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_request_rpc_total_time",
            MetricTableDef {
                PromQL: "sum(increase(pd_client_request_handle_requests_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)",
                Labels: &["instance", "type"],
                Comment: "The total time of pd client handle request duration(second)",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_handle_transactions_total_count",
            MetricTableDef {
                PromQL: "sum(increase(pd_txn_handle_txns_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,result)",
                Labels: &["instance", "result"],
                Comment: "The total count of handling etcd transactions",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_handle_transactions_total_time",
            MetricTableDef {
                PromQL: "sum(increase(pd_txn_handle_txns_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,result)",
                Labels: &["instance", "result"],
                Comment: "The total time of handling etcd transactions",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_operator_finish_total_count",
            MetricTableDef {
                PromQL: "sum(increase(pd_schedule_finish_operators_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type)",
                Labels: &["type"],
                Comment: "The total count of the operator is finished",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_operator_finish_total_time",
            MetricTableDef {
                PromQL: "sum(increase(pd_schedule_finish_operators_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type)",
                Labels: &["type"],
                Comment: "The total time consumed when the operator is finished",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_operator_step_finish_total_count",
            MetricTableDef {
                PromQL: "sum(increase(pd_schedule_finish_operator_steps_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type)",
                Labels: &["type"],
                Comment: "The total count of the operator step is finished",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_operator_step_finish_total_time",
            MetricTableDef {
                PromQL: "sum(increase(pd_schedule_finish_operator_steps_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type)",
                Labels: &["type"],
                Comment: "The total time consumed when the operator step is finished",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_peer_round_trip_total_count",
            MetricTableDef {
                PromQL: "sum(increase(etcd_network_peer_round_trip_time_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,To)",
                Labels: &["instance", "To"],
                Comment: "The total count of the network in .99",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_peer_round_trip_total_time",
            MetricTableDef {
                PromQL: "sum(increase(etcd_network_peer_round_trip_time_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,To)",
                Labels: &["instance", "To"],
                Comment: "The total time of latency of the network in .99",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_region_heartbeat_total_count",
            MetricTableDef {
                PromQL: "sum(increase(pd_scheduler_region_heartbeat_latency_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (address,store)",
                Labels: &["address", "store"],
                Comment: "The total count of heartbeat latency of each TiKV instance in",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_region_heartbeat_total_time",
            MetricTableDef {
                PromQL: "sum(increase(pd_scheduler_region_heartbeat_latency_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (address,store)",
                Labels: &["address", "store"],
                Comment: "The total time of heartbeat latency of each TiKV instance in",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_start_tso_wait_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tidb_pdclient_ts_future_wait_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total count of the waiting for getting the start timestamp oracle",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_start_tso_wait_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tidb_pdclient_ts_future_wait_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total time of duration of the waiting time for getting the start timestamp oracle",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_tso_rpc_total_count",
            MetricTableDef {
                PromQL: "sum(increase(pd_client_request_handle_requests_duration_seconds_count{type=\"tso\"}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total count of a client sending TSO request until received the response.",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_tso_rpc_total_time",
            MetricTableDef {
                PromQL: "sum(increase(pd_client_request_handle_requests_duration_seconds_sum{type=\"tso\"}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total time of a client sending TSO request until received the response.",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_tso_wait_total_count",
            MetricTableDef {
                PromQL: "sum(increase(pd_client_cmd_handle_cmds_duration_seconds_count{type=\"wait\"}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total count of a client starting to wait for the TS until received the TS result.",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "pd_tso_wait_total_time",
            MetricTableDef {
                PromQL: "sum(increase(pd_client_cmd_handle_cmds_duration_seconds_sum{type=\"wait\"}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total time of a client starting to wait for the TS until received the TS result.",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_auto_id_request_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tidb_autoid_operation_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)",
                Labels: &["instance", "type"],
                Comment: "The total count of TiDB auto id requests durations",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_auto_id_request_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tidb_autoid_operation_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)",
                Labels: &["instance", "type"],
                Comment: "The total time of TiDB auto id requests durations",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_batch_client_unavailable_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tidb_tikvclient_batch_client_unavailable_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total count of kv storage batch processing unvailable durations",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_batch_client_unavailable_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tidb_tikvclient_batch_client_unavailable_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total time of kv storage batch processing unvailable durations",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_batch_client_wait_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tidb_tikvclient_batch_wait_duration_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total count of kv storage batch processing durations",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_batch_client_wait_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tidb_tikvclient_batch_wait_duration_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total time of kv storage batch processing durations, the unit is nanosecond",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_compile_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tidb_session_compile_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,sql_type)",
                Labels: &["instance", "sql_type"],
                Comment: "The total count of building the query plan(second)",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_compile_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tidb_session_compile_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,sql_type)",
                Labels: &["instance", "sql_type"],
                Comment: "The total time of cost of building the query plan(second)",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_cop_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tidb_tikvclient_cop_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total count of kv storage coprocessor processing durations",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_cop_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tidb_tikvclient_cop_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total time of kv storage coprocessor processing durations",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_ddl_batch_add_index_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tidb_ddl_batch_add_idx_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)",
                Labels: &["instance", "type"],
                Comment: "The total count of TiDB batch add index durations by histogram buckets",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_ddl_batch_add_index_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tidb_ddl_batch_add_idx_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)",
                Labels: &["instance", "type"],
                Comment: "The total time of TiDB batch add index durations by histogram buckets",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_ddl_deploy_syncer_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tidb_ddl_deploy_syncer_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type,result)",
                Labels: &["instance", "type", "result"],
                Comment: "The total count of TiDB ddl schema syncer statistics, including init, start, watch, clear function call",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_ddl_deploy_syncer_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tidb_ddl_deploy_syncer_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type,result)",
                Labels: &["instance", "type", "result"],
                Comment: "The total time of TiDB ddl schema syncer statistics, including init, start, watch, clear function call time cost",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_ddl_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tidb_ddl_handle_job_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)",
                Labels: &["instance", "type"],
                Comment: "The total count of TiDB DDL duration statistics",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_ddl_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tidb_ddl_handle_job_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)",
                Labels: &["instance", "type"],
                Comment: "The total time of TiDB DDL duration statistics",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_ddl_update_self_version_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tidb_ddl_update_self_ver_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,result)",
                Labels: &["instance", "result"],
                Comment: "The total count of TiDB schema syncer version update",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_ddl_update_self_version_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tidb_ddl_update_self_ver_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,result)",
                Labels: &["instance", "result"],
                Comment: "The total time of TiDB schema syncer version update time duration",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_ddl_worker_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tidb_ddl_worker_operation_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type,result,action)",
                Labels: &["instance", "type", "result", "action"],
                Comment: "The total count of TiDB ddl worker duration",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_ddl_worker_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tidb_ddl_worker_operation_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type,result,action)",
                Labels: &["instance", "type", "result", "action"],
                Comment: "The total time of TiDB ddl worker duration",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_distsql_execution_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tidb_distsql_handle_query_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)",
                Labels: &["instance", "type"],
                Comment: "The total count of distsql execution(second)",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_distsql_execution_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tidb_distsql_handle_query_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)",
                Labels: &["instance", "type"],
                Comment: "The total time of distsql execution(second)",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_distsql_copr_cache",
            MetricTableDef {
                Comment: "The total count of TiDB distsql coprocessor cache",
                PromQL: "sum(rate(tidb_distsql_copr_cache{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (type,instance)",
                Labels: &["instance", "type"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_execute_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tidb_session_execute_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,sql_type)",
                Labels: &["instance", "sql_type"],
                Comment: "The total count of of TiDB executing the SQL",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_execute_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tidb_session_execute_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,sql_type)",
                Labels: &["instance", "sql_type"],
                Comment: "The total time cost of executing the SQL which does not include the time to get the results of the query(second)",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_gc_push_task_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tidb_tikvclient_range_task_push_duration_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)",
                Labels: &["instance", "type"],
                Comment: "The total count of kv storage range worker processing one task duration",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_gc_push_task_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tidb_tikvclient_range_task_push_duration_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)",
                Labels: &["instance", "type"],
                Comment: "The total time of kv storage range worker processing one task duration",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_gc_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tidb_tikvclient_gc_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,stage)",
                Labels: &["instance", "stage"],
                Comment: "The total count of kv storage garbage collection",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_gc_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tidb_tikvclient_gc_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,stage)",
                Labels: &["instance", "stage"],
                Comment: "The total time of kv storage garbage collection time durations",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_get_token_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tidb_server_get_token_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total count of Duration (us) for getting token, it should be small until concurrency limit is reached",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_get_token_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tidb_server_get_token_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total time of Duration (us) for getting token, it should be small until concurrency limit is reached(microsecond)",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_kv_backoff_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tidb_tikvclient_backoff_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)",
                Labels: &["instance", "type"],
                Comment: "The total count of kv backoff",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_kv_backoff_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tidb_tikvclient_backoff_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)",
                Labels: &["instance", "type"],
                Comment: "The total time of kv backoff time durations(second)",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_kv_request_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tidb_tikvclient_request_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type,store)",
                Labels: &["instance", "type", "store"],
                Comment: "The total count of kv requests durations by store",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_kv_request_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tidb_tikvclient_request_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type,store)",
                Labels: &["instance", "type", "store"],
                Comment: "The total time of kv requests durations by store",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_load_schema_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tidb_domain_load_schema_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total count of TiDB loading schema by instance",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_load_schema_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tidb_domain_load_schema_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total time of TiDB loading schema time durations by instance",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_meta_operation_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tidb_meta_operation_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type,result)",
                Labels: &["instance", "type", "result"],
                Comment: "The total count of TiDB meta operation durations including get/set schema and ddl jobs",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_meta_operation_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tidb_meta_operation_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type,result)",
                Labels: &["instance", "type", "result"],
                Comment: "The total time of TiDB meta operation durations including get/set schema and ddl jobs",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_new_etcd_session_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tidb_owner_new_session_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type,result)",
                Labels: &["instance", "type", "result"],
                Comment: "The total count of TiDB new session durations for new etcd sessions",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_new_etcd_session_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tidb_owner_new_session_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type,result)",
                Labels: &["instance", "type", "result"],
                Comment: "The total time of TiDB new session durations for new etcd sessions",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_owner_handle_syncer_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tidb_ddl_owner_handle_syncer_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type,result)",
                Labels: &["instance", "type", "result"],
                Comment: "The total count of TiDB ddl owner operations on etcd ",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_owner_handle_syncer_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tidb_ddl_owner_handle_syncer_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type,result)",
                Labels: &["instance", "type", "result"],
                Comment: "The total time of TiDB ddl owner time operations on etcd duration statistics ",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_parse_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tidb_session_parse_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,sql_type)",
                Labels: &["instance", "sql_type"],
                Comment: "The total count of parsing SQL to AST(second)",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_parse_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tidb_session_parse_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,sql_type)",
                Labels: &["instance", "sql_type"],
                Comment: "The total time cost of parsing SQL to AST(second)",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_query_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tidb_server_handle_query_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,sql_type)",
                Labels: &["instance", "sql_type"],
                Comment: "The total count of TiDB query durations(second)",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_query_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tidb_server_handle_query_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,sql_type)",
                Labels: &["instance", "sql_type"],
                Comment: "The total time of TiDB query durations(second)",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_txn_cmd_duration",
            MetricTableDef {
                PromQL: r###"histogram_quantile($QUANTILE, sum(rate(tidb_tikvclient_txn_cmd_duration_seconds_bucket{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (le,type,instance))"###,
                Labels: &["instance", "type"],
                Quantile: 0.90,
                Comment: "The quantile of TiDB transaction command durations(second)",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_txn_cmd_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tidb_tikvclient_txn_cmd_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)",
                Labels: &["instance", "type"],
                Comment: "The total count of TiDB transaction command",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_txn_cmd_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tidb_tikvclient_txn_cmd_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)",
                Labels: &["instance", "type"],
                Comment: "The total time of TiDB transaction command",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_slow_query_cop_process_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tidb_server_slow_query_cop_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total count of TiDB slow query cop process",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_slow_query_cop_process_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tidb_server_slow_query_cop_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total time of TiDB slow query statistics with slow query total cop process time(second)",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_slow_query_cop_wait_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tidb_server_slow_query_wait_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total count of TiDB slow query cop wait",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_slow_query_cop_wait_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tidb_server_slow_query_wait_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total time of TiDB slow query statistics with slow query total cop wait time(second)",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_slow_query_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tidb_server_slow_query_process_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total count of TiDB slow query",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_slow_query_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tidb_server_slow_query_process_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total time of TiDB slow query statistics with slow query time(second)",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_statistics_auto_analyze_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tidb_statistics_auto_analyze_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total count of TiDB auto analyze",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_statistics_auto_analyze_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tidb_statistics_auto_analyze_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total time of TiDB auto analyze time durations within 95 percent histogram buckets",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_transaction_local_latch_wait_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tidb_tikvclient_local_latch_wait_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total count of TiDB transaction latch wait on key value storage(second)",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_transaction_local_latch_wait_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tidb_tikvclient_local_latch_wait_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total time of TiDB transaction latch wait time on key value storage(second)",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_transaction_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tidb_session_transaction_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type,sql_type)",
                Labels: &["instance", "type", "sql_type"],
                Comment: "The total count of transaction execution durations, including retry(second)",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_transaction_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tidb_session_transaction_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type,sql_type)",
                Labels: &["instance", "type", "sql_type"],
                Comment: "The total time of transaction execution durations, including retry(second)",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_server_maxprocs",
            MetricTableDef {
                PromQL: "tidb_server_maxprocs{$LABEL_CONDITIONS}",
                Labels: &["instance"],
                Comment: "The Total CPU quota of each TiDB instance",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_raftstore_append_log_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tikv_raftstore_append_log_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total count of Raft appends log",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_raftstore_append_log_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tikv_raftstore_append_log_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total time of Raft appends log",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_raftstore_apply_log_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tikv_raftstore_apply_log_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total count of Raft applies log",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_raftstore_apply_log_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tikv_raftstore_apply_log_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total time of Raft applies log",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_raftstore_apply_wait_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tikv_raftstore_apply_wait_time_duration_secs_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_raftstore_apply_wait_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tikv_raftstore_apply_wait_time_duration_secs_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_backup_range_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tikv_backup_range_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)",
                Labels: &["instance", "type"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_backup_range_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tikv_backup_range_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)",
                Labels: &["instance", "type"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_backup_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tikv_backup_request_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_backup_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tikv_backup_request_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_check_split_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tikv_raftstore_check_split_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total count of running split check",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_check_split_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tikv_raftstore_check_split_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total time of time consumed when running split check in .9999",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_raftstore_commit_log_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tikv_raftstore_commit_log_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total count of Raft commits log",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_raftstore_commit_log_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tikv_raftstore_commit_log_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total time of Raft commits log",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_cop_handle_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tikv_coprocessor_request_handle_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,req)",
                Labels: &["instance", "req"],
                Comment: "The total count of tikv coprocessor handling coprocessor requests",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_cop_handle_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tikv_coprocessor_request_handle_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,req)",
                Labels: &["instance", "req"],
                Comment: "The total time of time consumed when handling coprocessor requests",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_cop_request_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tikv_coprocessor_request_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,req)",
                Labels: &["instance", "req"],
                Comment: "The total count of tikv handle coprocessor read requests",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_cop_request_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tikv_coprocessor_request_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,req)",
                Labels: &["instance", "req"],
                Comment: "The total time of time consumed to handle coprocessor read requests",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_cop_wait_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tikv_coprocessor_request_wait_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,req,type)",
                Labels: &["instance", "req", "type"],
                Comment: "The total count of coprocessor requests that wait for being handled",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_cop_wait_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tikv_coprocessor_request_wait_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,req,type)",
                Labels: &["instance", "req", "type"],
                Comment: "The total time of time consumed when coprocessor requests are wait for being handled",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_raft_store_events_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tikv_raftstore_event_duration_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)",
                Labels: &["instance", "type"],
                Comment: "The total count of raftstore events (P99).99",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_raft_store_events_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tikv_raftstore_event_duration_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)",
                Labels: &["instance", "type"],
                Comment: "The total time of raftstore events (P99).99",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_gc_tasks_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tikv_gcworker_gc_task_duration_vec_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,task)",
                Labels: &["instance", "task"],
                Comment: "The total count of executing GC tasks",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_gc_tasks_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tikv_gcworker_gc_task_duration_vec_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,task)",
                Labels: &["instance", "task"],
                Comment: "The total time of time consumed when executing GC tasks",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_grpc_message_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tikv_grpc_msg_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)",
                Labels: &["instance", "type"],
                Comment: "The total count of tikv execution gRPC message",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_grpc_message_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tikv_grpc_msg_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)",
                Labels: &["instance", "type"],
                Comment: "The total time of execution time of gRPC message",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_handle_snapshot_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tikv_raftstore_snapshot_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)",
                Labels: &["instance", "type"],
                Comment: "The total count of tikv handling snapshots",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_handle_snapshot_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tikv_raftstore_snapshot_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)",
                Labels: &["instance", "type"],
                Comment: "The total time of time consumed when handling snapshots",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_ingest_sst_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tikv_snapshot_ingest_sst_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,db)",
                Labels: &["instance", "db"],
                Comment: "The total count of ingesting SST files",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_ingest_sst_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tikv_snapshot_ingest_sst_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,db)",
                Labels: &["instance", "db"],
                Comment: "The total time of time consumed when ingesting SST files",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_lock_manager_deadlock_detect_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tikv_lock_manager_detect_duration_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_lock_manager_deadlock_detect_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tikv_lock_manager_detect_duration_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_lock_manager_waiter_lifetime_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tikv_lock_manager_waiter_lifetime_duration_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_lock_manager_waiter_lifetime_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tikv_lock_manager_waiter_lifetime_duration_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_raftstore_process_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tikv_raftstore_raft_process_duration_secs_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)",
                Labels: &["instance", "type"],
                Comment: "The total count of peer processes in Raft",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_raftstore_process_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tikv_raftstore_raft_process_duration_secs_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)",
                Labels: &["instance", "type"],
                Comment: "The total time of peer processes in Raft",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_raftstore_propose_wait_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tikv_raftstore_request_wait_time_duration_secs_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total count of each proposal",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_raftstore_propose_wait_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tikv_raftstore_request_wait_time_duration_secs_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total time of wait time of each proposal",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_scheduler_command_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tikv_scheduler_command_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)",
                Labels: &["instance", "type"],
                Comment: "The total count of tikv scheduler executing command",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_scheduler_command_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tikv_scheduler_command_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)",
                Labels: &["instance", "type"],
                Comment: "The total time of time consumed when executing command",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_scheduler_latch_wait_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tikv_scheduler_latch_wait_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)",
                Labels: &["instance", "type"],
                Comment: "The total count which is caused by latch wait in command",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_scheduler_latch_wait_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tikv_scheduler_latch_wait_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)",
                Labels: &["instance", "type"],
                Comment: "The total time which is caused by latch wait in command",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_send_snapshot_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tikv_server_send_snapshot_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total count of sending snapshots",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_send_snapshot_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tikv_server_send_snapshot_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total time of time consumed when sending snapshots",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_storage_async_request_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tikv_storage_engine_async_request_duration_seconds_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)",
                Labels: &["instance", "type"],
                Comment: "The total count of processing asynchronous snapshot requests",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_storage_async_request_total_time",
            MetricTableDef {
                PromQL: "sum(increase(tikv_storage_engine_async_request_duration_seconds_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)",
                Labels: &["instance", "type"],
                Comment: "The total time of time consumed by processing asynchronous snapshot requests",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_distsql_partial_num_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tidb_distsql_partial_num_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total count of distsql partial numbers per query",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_distsql_partial_scan_key_num_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tidb_distsql_scan_keys_partial_num_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total count of distsql partial scan key numbers",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_distsql_partial_scan_key_total_num",
            MetricTableDef {
                PromQL: "sum(increase(tidb_distsql_scan_keys_partial_num_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total num of distsql partial scan key numbers",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_distsql_partial_total_num",
            MetricTableDef {
                PromQL: "sum(increase(tidb_distsql_partial_num_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total num of distsql partial numbers per query",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_distsql_scan_key_num_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tidb_distsql_scan_keys_num_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total count of distsql scan numbers",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_distsql_scan_key_total_num",
            MetricTableDef {
                PromQL: "sum(increase(tidb_distsql_scan_keys_num_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total num of distsql scan numbers",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_kv_write_num_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tidb_tikvclient_txn_write_kv_num_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total count of kv write in transaction execution",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_kv_write_size_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tidb_tikvclient_txn_write_size_bytes_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total count of kv write size per transaction execution",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_kv_write_total_num",
            MetricTableDef {
                PromQL: "sum(increase(tidb_tikvclient_txn_write_kv_num_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total num of kv write in transaction execution",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_kv_write_total_size",
            MetricTableDef {
                PromQL: "sum(increase(tidb_tikvclient_txn_write_size_bytes_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total kv write size in transaction execution",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_statistics_stats_inaccuracy_rate_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tidb_statistics_stats_inaccuracy_rate_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total count of TiDB statistics inaccurate rate",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_statistics_stats_inaccuracy_total_rate",
            MetricTableDef {
                PromQL: "sum(increase(tidb_statistics_stats_inaccuracy_rate_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total time of TiDB statistics inaccurate rate",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_transaction_retry_num_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tidb_session_retry_num_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total count of TiDB transaction retry num",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_transaction_retry_total_num",
            MetricTableDef {
                PromQL: "sum(increase(tidb_session_retry_num_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total num of TiDB transaction retry num",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_transaction_statement_num_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tidb_session_transaction_statement_num_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,sql_type)",
                Labels: &["instance", "sql_type"],
                Comment: "The total count of TiDB statements numbers within one transaction. Internal means TiDB inner transaction",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_transaction_statement_total_num",
            MetricTableDef {
                PromQL: "sum(increase(tidb_session_transaction_statement_num_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,sql_type)",
                Labels: &["instance", "sql_type"],
                Comment: "The total num of TiDB statements numbers within one transaction. Internal means TiDB inner transaction",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_txn_region_num_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tidb_tikvclient_txn_regions_num_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total count of regions transaction operates on count",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tidb_txn_region_total_num",
            MetricTableDef {
                PromQL: "sum(increase(tidb_tikvclient_txn_regions_num_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total num of regions transaction operates on count",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_approximate_region_size_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tikv_raftstore_region_size_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total count of approximate Region size, the default value is P99",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_approximate_region_total_size",
            MetricTableDef {
                PromQL: "sum(increase(tikv_raftstore_region_size_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total size of approximate Region size",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_backup_range_size_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tikv_backup_range_size_bytes_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,cf)",
                Labels: &["instance", "cf"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_backup_range_total_size",
            MetricTableDef {
                PromQL: "sum(increase(tikv_backup_range_size_bytes_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,cf)",
                Labels: &["instance", "cf"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_cop_kv_cursor_operations_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tikv_coprocessor_scan_keys_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,req)",
                Labels: &["instance", "req"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_cop_scan_keys_total_num",
            MetricTableDef {
                PromQL: "sum(increase(tikv_coprocessor_scan_keys_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,req)",
                Labels: &["instance", "req"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_cop_total_response_total_size",
            MetricTableDef {
                PromQL: r###"sum(increase(tikv_coprocessor_response_bytes{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)"###,
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_grpc_req_batch_size_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tikv_server_grpc_req_batch_size_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_grpc_req_batch_total_size",
            MetricTableDef {
                PromQL: "sum(increase(tikv_server_grpc_req_batch_size_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_grpc_resp_batch_size_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tikv_server_grpc_resp_batch_size_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_grpc_resp_batch_total_size",
            MetricTableDef {
                PromQL: "sum(increase(tikv_server_grpc_resp_batch_size_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_raft_message_batch_size_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tikv_server_raft_message_batch_size_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_raft_message_batch_total_size",
            MetricTableDef {
                PromQL: "sum(increase(tikv_server_raft_message_batch_size_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_raft_proposals_per_ready_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tikv_raftstore_apply_proposal_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total count of proposal count of all Regions in a mio tick",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_raft_proposals_per_total_ready",
            MetricTableDef {
                PromQL: "sum(increase(tikv_raftstore_apply_proposal_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total proposal count of all Regions in a mio tick",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_request_batch_ratio_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tikv_server_request_batch_ratio_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)",
                Labels: &["instance", "type"],
                Comment: "The total count of request batch output to input per TiKV instance",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_request_batch_size_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tikv_server_request_batch_size_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)",
                Labels: &["instance", "type"],
                Comment: "The total count of request batch per TiKV instance",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_request_batch_total_ratio",
            MetricTableDef {
                PromQL: "sum(increase(tikv_server_request_batch_ratio_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)",
                Labels: &["instance", "type"],
                Comment: "The total ratio of request batch output to input per TiKV instance",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_request_batch_total_size",
            MetricTableDef {
                PromQL: "sum(increase(tikv_server_request_batch_size_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)",
                Labels: &["instance", "type"],
                Comment: "The total size of requests into request batch per TiKV instance",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_scheduler_keys_read_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tikv_scheduler_kv_command_key_read_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)",
                Labels: &["instance", "type"],
                Comment: "The total count of keys read by a command",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_scheduler_keys_total_read",
            MetricTableDef {
                PromQL: "sum(increase(tikv_scheduler_kv_command_key_read_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)",
                Labels: &["instance", "type"],
                Comment: "The total count of keys read by command",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_scheduler_keys_total_written",
            MetricTableDef {
                PromQL: "sum(increase(tikv_scheduler_kv_command_key_write_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)",
                Labels: &["instance", "type"],
                Comment: "The total count of keys written by a command",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_scheduler_keys_written_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tikv_scheduler_kv_command_key_write_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance,type)",
                Labels: &["instance", "type"],
                Comment: "The total count of keys written by a command",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_snapshot_kv_count_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tikv_snapshot_kv_count_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total count of number of KV within a snapshot",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_snapshot_kv_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tikv_snapshot_kv_count_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total number of KV within a snapshot",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_snapshot_size_total_count",
            MetricTableDef {
                PromQL: "sum(increase(tikv_snapshot_size_count{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total count of snapshot size",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_snapshot_total_size",
            MetricTableDef {
                PromQL: "sum(increase(tikv_snapshot_size_sum{$LABEL_CONDITIONS}[$RANGE_DURATION])) by (instance)",
                Labels: &["instance"],
                Comment: "The total size of snapshot size",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_config_rocksdb",
            MetricTableDef {
                PromQL: "tikv_config_rocksdb{$LABEL_CONDITIONS}",
                Labels: &["instance", "cf", "name"],
                Comment: "TiKV rocksdb config value",
                ..MetricTableDef::EMPTY
            },
        ),
        (
            "tikv_config_raftstore",
            MetricTableDef {
                PromQL: "tikv_config_raftstore{$LABEL_CONDITIONS}",
                Labels: &["instance", "name"],
                Comment: "TiKV rocksdb config value",
                ..MetricTableDef::EMPTY
            },
        ),
    ])
});

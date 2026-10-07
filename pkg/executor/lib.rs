// Copyright 2026 AsterSQL.

// SQL 执行器（Executor）crate 根模块。
//
// 将物理执行计划（Physical Plan）落地为可迭代的算子树：表/索引扫描、
// 投影、连接、DML、DDL、ANALYZE、SHOW、MPP 收集等。各子模块对应一类
// 算子或辅助能力；测试通过 `#[path]` 挂到本 crate，便于与源文件同目录存放。

#![allow(dead_code)]

// —— 适配与慢查询日志 ——
/// 执行器与会话/事务边界的适配层。
pub mod adapter;
/// 慢查询日志相关辅助。
pub mod adapter_slow_log;
/// 管理类语句执行（ADMIN）。
pub mod admin;
/// 插件管理相关 ADMIN。
pub mod admin_plugins;
#[cfg(test)]
mod admin_plugins_test;
/// RU v2 的物理计划证据遍历与终端快照。
pub mod statement_ru_plan_walk;
#[cfg(test)]
mod statement_ru_plan_walk_test;
/// RU v2 按执行引擎分摊原始单位和报告。
pub mod statement_ru_reporting;
#[cfg(test)]
mod statement_ru_reporting_test;
/// RU v2 语句结算所需的原始证据与计算辅助。
pub mod statement_ru_result;
#[cfg(test)]
mod statement_ru_result_test;
// —— 统计信息 ANALYZE ——
/// ANALYZE 表/索引统计入口。
pub mod analyze;
/// 列统计采样与汇总。
pub mod analyze_col;
/// 列采样实现细节。
pub mod analyze_col_sampling;
#[cfg(test)]
mod analyze_col_sampling_test;
#[cfg(test)]
mod analyze_col_test;
/// 分区表全局统计合并。
pub mod analyze_global_stats;
/// 索引统计 ANALYZE。
pub mod analyze_idx;
#[cfg(test)]
mod analyze_idx_test;
/// ANALYZE 共用工具。
pub mod analyze_utils;
/// ANALYZE 后台工作线程。
pub mod analyze_worker;
#[cfg(test)]
mod analyze_worker_test;
/// 批处理校验（如批量点查结果）。
pub mod batch_checker;
/// 批量点查（Batch Point Get）。
pub mod batch_point_get;
/// 执行计划绑定（Plan Binding）。
pub mod bind;
#[cfg(test)]
mod bind_test;
/// BRIE（Backup/Restore/Import/Export）相关执行。
pub mod brie;
/// BRIE DDL 渲染与选项工具。
pub mod brie_utils;
/// 由物理计划构建执行器树。
pub mod builder;
#[cfg(test)]
mod builder_contract_test;
/// 检查表与索引一致性。
pub mod check_table_index;
#[cfg(test)]
mod check_table_index_test;
/// 表校验和（Checksum）。
pub mod checksum;
/// 表压缩（Compact）。
pub mod compact_table;
/// 表达式/计划编译辅助。
pub mod compiler;
#[cfg(test)]
mod compiler_test;
/// Coprocessor 下推执行相关。
pub mod coprocessor;
/// 公用表表达式 CTE 执行。
pub mod cte;
/// CTE 物化结果读取。
pub mod cte_table_reader;
/// DDL 语句在执行器侧的入口。
pub mod ddl;
/// DELETE 执行器。
pub mod delete;
/// 会话/任务分离（Detach）。
pub mod detach;
/// 分布式相关表操作。
pub mod distribute;
/// DistSQL（分布式 SQL 请求）执行。
pub mod distsql;
/// Expand 算子（如 CUBE/ROLLUP 展开）。
pub mod expand;
/// EXPLAIN 执行计划输出。
pub mod explain;
/// 外键级联检查与执行。
pub mod foreign_key;
#[cfg(test)]
mod foreign_key_test;
/// GRANT 权限授予。
pub mod grant;
/// IMPORT INTO 批量导入。
pub mod import_into;
/// IMPORT INTO 作业存储与分布式任务取消。
pub mod import_into_storage;
/// Index Merge 多索引合并读取。
pub mod index_merge_reader;
/// Information Schema 虚拟表读取。
pub mod infoschema_reader;
/// INSERT 执行器。
pub mod insert;
/// INSERT 共用逻辑。
pub mod insert_common;
#[cfg(test)]
mod insert_common_test;
/// 巡检（Inspection）共用类型。
pub mod inspection_common;
/// 巡检配置文件。
pub mod inspection_profile;
#[cfg(test)]
mod inspection_profile_test;
/// 巡检结果表。
pub mod inspection_result;
/// 巡检汇总。
pub mod inspection_summary;
/// LOAD DATA 从文件导入。
pub mod load_data;
#[cfg(test)]
mod load_data_test;
/// LOAD STATS 从 JSON 加载统计。
pub mod load_stats;
/// 事务内存缓冲（MemBuffer）上的行/索引读取，供 UnionScan 等使用。
pub mod mem_reader;
#[cfg(test)]
mod mem_reader_test;
/// 集群内存表（如 cluster_config、cluster_log）检索执行器。
pub mod memtable_reader;
/// Prometheus/指标虚拟表读取。
pub mod metrics_reader;
/// MPP（大规模并行处理）结果收集。
pub mod mpp_gather;
#[cfg(test)]
mod mpp_gather_test;
/// 操作 DDL Job（取消/暂停等）。
pub mod operate_ddl_jobs;
#[cfg(test)]
mod operate_ddl_jobs_test;
/// 优化规则黑名单管理。
pub mod opt_rule_blacklist;
#[cfg(test)]
mod opt_rule_blacklist_test;
/// Parallel Apply 并行相关子查询。
pub mod parallel_apply;
/// 分区表运行时裁剪与路由。
pub mod partition_runtime;
/// 物理计划运行时辅助。
pub mod physical_plan_runtime;
/// Plan Replayer 计划重放。
pub mod plan_replayer;
#[cfg(test)]
mod plan_replayer_test;
/// 单点查询（Point Get）。
pub mod point_get;
/// 预处理语句（Prepared Statement）。
pub mod prepared;
/// 投影（Projection）算子。
pub mod projection;
/// 索引推荐。
pub mod recommend_index;
/// 重载表达式下推黑名单。
pub mod reload_expr_pushdown_blacklist;
#[cfg(test)]
mod reload_expr_pushdown_blacklist_test;
/// REPLACE 执行器。
pub mod replace;
#[cfg(test)]
mod replace_test;
/// REVOKE 权限回收。
pub mod revoke;
/// 采样算子。
pub mod sample;
/// SELECT 执行入口相关。
pub mod select;
/// SELECT INTO 写出文件。
pub mod select_into;
/// SET 变量。
pub mod set;
/// SET CONFIG 集群配置。
pub mod set_config;
#[cfg(test)]
mod set_config_test;
/// SHOW 语句族。
pub mod show;
/// SHOW AFFINITY。
pub mod show_affinity;
/// SHOW BDR ROLE。
pub mod show_bdr_role;
#[cfg(test)]
mod show_bdr_role_test;
/// SHOW DDL。
pub mod show_ddl;
/// SHOW DDL JOB QUERIES。
pub mod show_ddl_job_queries;
/// SHOW DDL JOBS。
pub mod show_ddl_jobs;
/// SHOW NEXT_ROW_ID。
pub mod show_next_row_id;
/// SHOW PLACEMENT。
pub mod show_placement;
/// SHOW SLOW QUERIES。
pub mod show_slow_queries;
#[cfg(test)]
mod show_slow_queries_test;
/// SHOW STATS。
pub mod show_stats;
/// Shuffle 算子（数据重分布）。
pub mod shuffle;
/// 简单语句（如 USE、事务控制等）。
pub mod simple;
/// 慢查询表读取。
pub mod slow_query;
/// Region/表 Split。
pub mod split;
/// 语句摘要（Statement Summary）。
pub mod stmtsummary;
/// 表扫描读取器。
pub mod table_reader;
/// TRACE 语句。
pub mod trace;
/// 流量控制相关。
pub mod traffic;
pub mod typed_hash_agg;
#[cfg(test)]
mod typed_hash_agg_test;
pub mod typed_hash_join;
#[cfg(test)]
mod typed_hash_join_test;
pub mod typed_index_lookup;
#[cfg(test)]
mod typed_index_lookup_test;
pub mod typed_index_reader;
#[cfg(test)]
mod typed_index_reader_test;
pub mod typed_kv_scan;
#[cfg(test)]
mod typed_kv_scan_test;
pub mod typed_limit;
#[cfg(test)]
mod typed_limit_test;
pub mod typed_point_get;
#[cfg(test)]
mod typed_point_get_test;
pub mod typed_projection;
#[cfg(test)]
mod typed_projection_test;
pub mod typed_selection;
#[cfg(test)]
mod typed_selection_test;
/// UnionScan：合并存储快照与事务写缓冲。
pub mod union_scan;
/// UPDATE 执行器。
pub mod update;
/// 执行器共用工具（线程池等）。
pub mod utils;
/// 工作负载仓库（Workload Repo）。
pub mod workloadrepo;
/// 写路径共用逻辑。
pub mod write;
#[cfg(test)]
mod write_test;

// —— 单元/集成测试挂载（与源文件分离存放） ——
#[cfg(test)]
#[path = "partition_runtime_test.rs"]
mod partition_runtime_test;

#[cfg(test)]
#[path = "physical_plan_runtime_test.rs"]
mod physical_plan_runtime_test;

#[cfg(test)]
#[path = "partition_table_test.rs"]
mod partition_table_test;

#[cfg(test)]
#[path = "set_test.rs"]
mod set_test;

#[cfg(test)]
#[path = "adapter_slow_log_aster_unit_test.rs"]
mod adapter_slow_log_aster_unit_test;

#[cfg(test)]
#[path = "adapter_slow_log_test.rs"]
mod adapter_slow_log_test;

#[cfg(test)]
#[path = "analyze_utils_test.rs"]
mod analyze_utils_test;

#[cfg(test)]
#[path = "analyze_test.rs"]
mod analyze_test;

#[cfg(test)]
#[path = "sample_test.rs"]
mod sample_test;

#[cfg(test)]
#[path = "bench_gencol_test.rs"]
mod bench_gencol_test;

#[cfg(test)]
#[path = "benchmark_test.rs"]
mod benchmark_test;

#[cfg(test)]
#[path = "show_stats_test.rs"]
mod show_stats_test;

#[cfg(test)]
#[path = "show_placement_test.rs"]
mod show_placement_test;

#[cfg(test)]
#[path = "show_affinity_test.rs"]
mod show_affinity_test;

#[cfg(test)]
#[path = "show_placement_labels_test.rs"]
mod show_placement_labels_test;

#[cfg(test)]
#[path = "show_test.rs"]
mod show_test;

#[cfg(test)]
#[path = "show_ddl_jobs_test.rs"]
mod show_ddl_jobs_test;

#[cfg(test)]
#[path = "grant_test.rs"]
mod grant_test;

#[cfg(test)]
#[path = "revoke_test.rs"]
mod revoke_test;

#[cfg(test)]
#[path = "traffic_test.rs"]
mod traffic_test;

#[cfg(test)]
#[path = "brie_test.rs"]
mod brie_test;

#[cfg(test)]
#[path = "brie_utils_test.rs"]
mod brie_utils_test;

#[cfg(test)]
#[path = "parallel_apply_test.rs"]
mod parallel_apply_test;

#[cfg(test)]
#[path = "builder_index_join_cleanup_test.rs"]
mod builder_index_join_cleanup_test;

#[cfg(test)]
#[path = "join_pkg_test.rs"]
mod join_pkg_test;

#[cfg(test)]
#[path = "copr_cache_test.rs"]
mod copr_cache_test;

#[cfg(test)]
#[path = "tikv_regions_peers_table_test.rs"]
mod tikv_regions_peers_table_test;

#[cfg(test)]
#[path = "historical_stats_test.rs"]
mod historical_stats_test;

#[cfg(test)]
#[path = "executor_failpoint_test.rs"]
mod executor_failpoint_test;

#[cfg(test)]
#[path = "compact_table_test.rs"]
mod compact_table_test;

#[cfg(test)]
#[path = "detach_integration_test.rs"]
mod detach_integration_test;

#[cfg(test)]
#[path = "detach_test.rs"]
mod detach_test;

#[cfg(test)]
#[path = "trace_test.rs"]
mod trace_test;

#[cfg(test)]
#[path = "checksum_test.rs"]
mod checksum_test;

#[cfg(test)]
#[path = "explainfor_test.rs"]
mod explainfor_test;

#[cfg(test)]
#[path = "explain_test.rs"]
mod explain_test;

#[cfg(test)]
#[path = "explain_unit_test.rs"]
mod explain_unit_test;

#[cfg(test)]
#[path = "executor_required_rows_test.rs"]
mod executor_required_rows_test;

#[cfg(test)]
#[path = "table_readers_required_rows_test.rs"]
mod table_readers_required_rows_test;

#[cfg(test)]
#[path = "chunk_size_control_test.rs"]
mod chunk_size_control_test;

#[cfg(test)]
#[path = "slow_query_test.rs"]
mod slow_query_test;

#[cfg(test)]
#[path = "slow_query_sql_test.rs"]
mod slow_query_sql_test;

#[cfg(test)]
#[path = "inspection_result_test.rs"]
mod inspection_result_test;

#[cfg(test)]
#[path = "inspection_summary_test.rs"]
mod inspection_summary_test;

#[cfg(test)]
#[path = "inspection_result_internal_test.rs"]
mod inspection_result_internal_test;

#[cfg(test)]
#[path = "metrics_reader_test.rs"]
mod metrics_reader_test;

#[cfg(test)]
#[path = "hot_regions_history_table_test.rs"]
mod hot_regions_history_table_test;

#[cfg(test)]
#[path = "infoschema_reader_test.rs"]
mod infoschema_reader_test;

#[cfg(test)]
#[path = "infoschema_reader_internal_test.rs"]
mod infoschema_reader_internal_test;

#[cfg(test)]
#[path = "infoschema_reader_keyspace_test.rs"]
mod infoschema_reader_keyspace_test;

#[cfg(test)]
#[path = "infoschema_reader_bench_test.rs"]
mod infoschema_reader_bench_test;

#[cfg(test)]
#[path = "stmtsummary_test.rs"]
mod stmtsummary_test;

#[cfg(test)]
#[path = "adapter_test.rs"]
mod adapter_test;

#[cfg(test)]
#[path = "adapter_internal_test.rs"]
mod adapter_internal_test;

#[cfg(test)]
#[path = "executor_pkg_test.rs"]
mod executor_pkg_test;

#[cfg(test)]
#[path = "pkg_test.rs"]
mod pkg_test;

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;

#[cfg(test)]
#[path = "simple_test.rs"]
mod simple_test;

#[cfg(test)]
#[path = "simple_internal_test.rs"]
mod simple_internal_test;

#[cfg(test)]
#[path = "utils_test.rs"]
mod utils_test;

#[cfg(test)]
#[path = "prepared_test.rs"]
mod prepared_test;

#[cfg(test)]
#[path = "batch_point_get_test.rs"]
mod batch_point_get_test;
#[cfg(test)]
#[path = "cluster_table_test.rs"]
mod cluster_table_test;
#[cfg(test)]
#[path = "distsql_test.rs"]
mod distsql_test;
#[cfg(test)]
#[path = "infoschema_cluster_table_test.rs"]
mod infoschema_cluster_table_test;
#[cfg(test)]
#[path = "memtable_reader_test.rs"]
mod memtable_reader_test;
#[cfg(test)]
#[path = "point_get_test.rs"]
mod point_get_test;
#[cfg(test)]
#[path = "union_scan_test.rs"]
mod union_scan_test;

#[cfg(test)]
#[path = "delete_test.rs"]
mod delete_test;
#[cfg(test)]
#[path = "distribute_table_test.rs"]
mod distribute_table_test;
#[cfg(test)]
#[path = "import_into_test.rs"]
mod import_into_test;
#[cfg(test)]
#[path = "insert_test.rs"]
mod insert_test;
#[cfg(test)]
#[path = "resource_tag_test.rs"]
mod resource_tag_test;
#[cfg(test)]
#[path = "select_into_test.rs"]
mod select_into_test;
#[cfg(test)]
#[path = "select_test.rs"]
mod select_test;
#[cfg(test)]
#[path = "shuffle_test.rs"]
mod shuffle_test;
#[cfg(test)]
#[path = "split_test.rs"]
mod split_test;
#[cfg(test)]
#[path = "temporary_table_test.rs"]
mod temporary_table_test;
#[cfg(test)]
#[path = "update_test.rs"]
mod update_test;
#[cfg(test)]
#[path = "write_concurrent_test.rs"]
mod write_concurrent_test;

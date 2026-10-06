// Copyright 2026 AsterSQL.

// DDL（数据定义语言）crate 的根模块入口。
//
// 本 crate 从 Go(TiDB) 的 `pkg/ddl` 机械迁移而来，承载在线 Schema 变更
//（Online DDL）相关的全部子系统：建表、改列、索引回填（backfill）、
// 分区、放置策略、作业调度与 Owner 选举等。
//
// 本文件主要负责：
// - 声明并组织各功能子模块；
// - 重新导出建表相关的 AST→元数据构建函数；
// - 在测试配置下挂载大量单元/集成测试模块。

#![allow(dead_code)]

use std::sync::atomic::AtomicU32;

/// Go `EnableSplitTableRegion`: gates physical pre-splitting for newly created
/// tables when the backing store supports region splitting.
pub static EnableSplitTableRegion: AtomicU32 = AtomicU32::new(0);

/// 建表语句到 TableInfo/ColumnInfo 的构建逻辑。
mod create_table;

pub use create_table::{
    BuildColumnInfoFromAST, BuildPartialIndexCondition, BuildPartitionInfo, BuildTableInfoFromAST,
    BuildTableInfoWithStmt, MaterializeExpressionIndexColumns, expression_text,
};

pub mod add_column;
pub mod affinity;
pub mod backfill_metrics;
pub mod backfilling;
pub mod backfilling_clean_s3;
pub mod backfilling_dist_executor;
pub mod backfilling_dist_scheduler;
pub mod backfilling_import_cloud;
pub mod backfilling_merge_sort;
pub mod backfilling_merge_temp;
pub mod backfilling_operators;
pub mod backfilling_read_index;
#[cfg(test)]
mod backfilling_read_index_test;
pub mod backfilling_txn_executor;
pub mod cluster;
pub mod column;
pub mod constraint;
pub mod ddl;
pub mod ddl_algorithm;
pub mod ddl_history;
pub mod ddl_running_jobs;
pub mod ddl_tiflash_api;
#[cfg(test)]
mod ddl_tiflash_api_test;
pub mod ddl_workerpool;
pub mod delete_range;
#[cfg(test)]
mod delete_range_test;
pub mod delete_range_util;
pub mod dist_owner;
pub mod doc;
pub mod executor;
pub mod foreign_key;
pub mod generated_column;
pub mod index;
pub mod index_auto_presplit;
#[cfg(test)]
mod index_auto_presplit_test;
pub mod index_cop;
pub mod index_merge_tmp;
#[cfg(test)]
mod index_merge_tmp_test;
pub mod index_presplit;
#[cfg(test)]
mod index_presplit_test;
pub mod job_scheduler;
pub mod job_submitter;
pub mod job_worker;
pub mod masking_policy;
pub mod metabuild;
pub mod mock;
#[cfg(test)]
mod mock_test;
pub mod modify_column;
pub mod multi_schema_change;
pub mod options;
pub mod owner_mgr;
pub mod partition;
pub mod placement_policy;
pub mod reorg;
pub mod reorg_util;
pub mod resource_group;
#[cfg(test)]
mod resource_group_test;
pub mod rollingback;
pub mod sanity_check;
#[cfg(test)]
mod sanity_check_test;
pub mod schema;
pub mod schema_version;
pub mod sequence;
pub mod split_region;
#[cfg(test)]
mod split_region_test;
pub mod stat;
pub mod table;
pub mod table_lock;
pub mod table_mode;
pub mod ttl;

pub use ddl_systable::{SchemaLoader, SchemaLoaderError};

#[cfg(test)]
mod create_table_aster_unit_test;
#[cfg(test)]
mod create_table_test;
#[cfg(test)]
mod create_table_validation_aster_unit_test;
#[cfg(test)]
mod schema_loader_contract_aster_unit_test;

#[cfg(test)]
mod add_column_test;
#[cfg(test)]
mod affinity_test;
#[cfg(test)]
mod attributes_sql_test;
#[cfg(test)]
mod backfill_metrics_test;
#[cfg(test)]
mod backfilling_clean_s3_test;
#[cfg(test)]
mod backfilling_dist_executor_test;
#[cfg(test)]
mod backfilling_dist_scheduler_test;
#[cfg(test)]
mod backfilling_import_cloud_test;
#[cfg(test)]
mod backfilling_merge_sort_test;
#[cfg(test)]
mod backfilling_merge_temp_test;
#[cfg(test)]
mod backfilling_operators_async_test;
#[cfg(test)]
mod backfilling_operators_test;
#[cfg(test)]
mod backfilling_test;
#[cfg(test)]
mod backfilling_txn_executor_test;
#[cfg(test)]
mod bench_test;
#[cfg(test)]
mod cancel_test;
#[cfg(test)]
mod cluster_test;
#[cfg(test)]
mod column_change_test;
#[cfg(test)]
mod column_modify_test;
#[cfg(test)]
mod column_test;
#[cfg(test)]
mod column_type_change_test;
#[cfg(test)]
mod constraint_test;
#[cfg(test)]
mod db_cache_test;
#[cfg(test)]
mod db_change_failpoints_test;
#[cfg(test)]
mod db_change_test;
#[cfg(test)]
mod db_integration_test;
#[cfg(test)]
mod db_rename_test;
#[cfg(test)]
mod db_table_test;
#[cfg(test)]
mod db_test;
#[cfg(test)]
mod ddl_algorithm_test;
#[cfg(test)]
mod ddl_error_test;
#[cfg(test)]
mod ddl_history_test;
#[cfg(test)]
mod ddl_running_jobs_test;
#[cfg(test)]
mod ddl_test;
#[cfg(test)]
mod ddl_workerpool_test;
#[cfg(test)]
mod executor_nokit_test;
#[cfg(test)]
mod executor_test;
#[cfg(test)]
mod export_test;
#[cfg(test)]
mod fail_test;
#[cfg(test)]
mod foreign_key_test;
#[cfg(test)]
mod generated_column_test;
#[cfg(test)]
mod schema_version_test;

#[cfg(test)]
mod index_change_test;
#[cfg(test)]
mod index_cop_test;
#[cfg(test)]
mod index_modify_test;
#[cfg(test)]
mod index_nokit_test;
#[cfg(test)]
mod index_test;
#[cfg(test)]
mod integration_test;
#[cfg(test)]
mod integration_validation_aster_unit_test;
#[cfg(test)]
mod job_scheduler_test;
#[cfg(test)]
mod job_scheduler_testkit_test;
#[cfg(test)]
mod job_submitter_test;
#[cfg(test)]
mod job_worker_test;
#[cfg(test)]
mod main_test;
#[cfg(test)]
mod masking_policy_test;
#[cfg(test)]
mod metabuild_test;
#[cfg(test)]
mod modify_column_test;
#[cfg(test)]
mod multi_schema_change_test;
#[cfg(test)]
mod mv_index_test;
#[cfg(test)]
mod options_test;
#[cfg(test)]
mod owner_mgr_test;
#[cfg(test)]
mod partition_test;
#[cfg(test)]
mod placement_policy_ddl_test;
#[cfg(test)]
mod placement_policy_test;
#[cfg(test)]
mod placement_sql_test;
#[cfg(test)]
mod primary_key_handle_test;
#[cfg(test)]
mod reorg_test;
#[cfg(test)]
mod reorg_util_test;
#[cfg(test)]
mod repair_table_test;
#[cfg(test)]
mod restart_test;
#[cfg(test)]
mod rollingback_test;
#[cfg(test)]
mod schema_test;
#[cfg(test)]
mod sequence_test;
#[cfg(test)]
mod stat_test;
#[cfg(test)]
mod table_lock_rename_aster_unit_test;
#[cfg(test)]
mod table_mode_test;
#[cfg(test)]
mod table_modify_test;
#[cfg(test)]
mod table_rename_aster_unit_test;
#[cfg(test)]
mod table_split_test;
#[cfg(test)]
mod table_test;
#[cfg(test)]
mod tiflash_replica_test;
#[cfg(test)]
mod ttl_test;

pub mod normal_policy;
pub mod persistent_actions;
pub mod persistent_alter_materialized_view_attributes;
pub mod persistent_alter_materialized_view_log_purge;
pub mod persistent_alter_materialized_view_refresh;

pub mod persistent_create_materialized_view_log;
pub mod persistent_create_materialized_view_shadow;
pub mod persistent_create_table;

pub mod persistent_create_materialized_view;

pub mod persistent_drop_column;
pub mod persistent_masking_actions;
pub mod persistent_modify_column;

pub mod storage_class;
#[cfg(test)]
mod storage_class_test;
pub mod storage_class_transition;
#[cfg(test)]
mod storage_class_transition_test;

// Copyright 2025 PingCAP, Inc.
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

// 系统保留全局对象 ID 边界与 `mysql`/`sys` 系统表固定 ID。
//
// 物理 schema 对象（库、表、索引等）使用 i64 ID。用户对象占用 `[1, ReservedGlobalIDLowerBound]`，
// 系统对象占用 `(ReservedGlobalIDLowerBound, ReservedGlobalIDUpperBound]`。本文件按从上界向下
// 逐项减一的方式为各系统表分配稳定 ID，保证与 Go 侧十六进制边界及减法语义一致。
//
// 所有 ID 继续使用 i64，确保十六进制上界和逐项减法与 Go int64 语义一致。

// 以下边界把用户对象 ID 与系统保留对象 ID 分隔开。
/// 物理 schema 对象 ID 的全局上界；历史上前两字节曾规划给多租户，现由 keyspace 替代。
// ReservedGlobalIDUpperBound is the max value of any physical schema object ID.
// due to history reasons, the first 2 bytes are planned to be used for multi
// tenancy, but it's replaced by keyspace.
pub const ReservedGlobalIDUpperBound: i64 = 0x0000FFFFFFFFFFFF;
/// 保留区间下界：用户对象可用范围为 [1, lower]；`(lower, upper]` 留给系统对象。
// ReservedGlobalIDLowerBound reserves 1000 IDs.
// valid usable ID range for user schema objects is [1, ReservedGlobalIDLowerBound].
// (ReservedGlobalIDLowerBound, ReservedGlobalIDUpperBound] is reserved for
// system schema objects.
pub const ReservedGlobalIDLowerBound: i64 = ReservedGlobalIDUpperBound - 1000;
/// 用户 schema 对象 ID 的最大可用值（含下界本身）。
// MaxUserGlobalID is the max value of user schema object ID, inclusive.
pub const MaxUserGlobalID: i64 = ReservedGlobalIDLowerBound;

/// `mysql` 系统库的固定数据库 ID（等于上界）。
// SystemDatabaseID is the database ID of `mysql`.
pub const SystemDatabaseID: i64 = ReservedGlobalIDUpperBound;
/// `mysql.tidb_ddl_job` 表 ID：存放进行中的 DDL（数据定义语言）作业。
// TiDBDDLJobTableID is the table ID of `tidb_ddl_job`.
pub const TiDBDDLJobTableID: i64 = ReservedGlobalIDUpperBound - 1;
/// `mysql.tidb_ddl_reorg` 表 ID：DDL 重组（reorg）进度与区间。
// TiDBDDLReorgTableID is the table ID of `tidb_ddl_reorg`.
pub const TiDBDDLReorgTableID: i64 = ReservedGlobalIDUpperBound - 2;
/// `mysql.tidb_ddl_history` 表 ID：已完成 DDL 作业历史。
// TiDBDDLHistoryTableID is the table ID of `tidb_ddl_history`.
pub const TiDBDDLHistoryTableID: i64 = ReservedGlobalIDUpperBound - 3;
/// `mysql.tidb_mdl_info` 表 ID：元数据锁（MDL）信息。
// TiDBMDLInfoTableID is the table ID of `tidb_mdl_info`.
pub const TiDBMDLInfoTableID: i64 = ReservedGlobalIDUpperBound - 4;
/// `mysql.tidb_background_subtask` 表 ID：分布式框架后台子任务。
// TiDBBackgroundSubtaskTableID is the table ID of `tidb_background_subtask`.
pub const TiDBBackgroundSubtaskTableID: i64 = ReservedGlobalIDUpperBound - 5;
/// `mysql.tidb_background_subtask_history` 表 ID：后台子任务历史。
// TiDBBackgroundSubtaskHistoryTableID is the table ID of `tidb_background_subtask_history`.
pub const TiDBBackgroundSubtaskHistoryTableID: i64 = ReservedGlobalIDUpperBound - 6;
/// `mysql.tidb_ddl_notifier` 表 ID：DDL schema change 事件通知。
// TiDBDDLNotifierTableID is the table ID of `tidb_ddl_notifier`.
pub const TiDBDDLNotifierTableID: i64 = ReservedGlobalIDUpperBound - 7;
/// `mysql.user` 表 ID：账号与全局权限。
// UserTableID is the table ID of `user`.
pub const UserTableID: i64 = ReservedGlobalIDUpperBound - 8;
/// `mysql.password_history` 表 ID：密码历史。
// PasswordHistoryTableID is the table ID of `password_history`.
pub const PasswordHistoryTableID: i64 = ReservedGlobalIDUpperBound - 9;
/// `mysql.global_priv` 表 ID：全局扩展权限（如动态特权相关）。
// GlobalPrivTableID is the table ID of `global_priv`.
pub const GlobalPrivTableID: i64 = ReservedGlobalIDUpperBound - 10;
/// `mysql.db` 表 ID：库级权限。
// DBTableID is the table ID of `db`.
pub const DBTableID: i64 = ReservedGlobalIDUpperBound - 11;
/// `mysql.tables_priv` 表 ID：表级权限。
// TablesPrivTableID is the table ID of `table_priv`.
pub const TablesPrivTableID: i64 = ReservedGlobalIDUpperBound - 12;
/// `mysql.columns_priv` 表 ID：列级权限。
// ColumnsPrivTableID is the table ID of `column_priv`.
pub const ColumnsPrivTableID: i64 = ReservedGlobalIDUpperBound - 13;
/// `mysql.global_variables` 表 ID：持久化全局系统变量。
// GlobalVariablesTableID is the table ID of `global_variables`.
pub const GlobalVariablesTableID: i64 = ReservedGlobalIDUpperBound - 14;
/// `mysql.tidb` 表 ID：TiDB 内部键值（如是否已 bootstrap）。
// TiDBTableID is the table ID of `tidb`.
pub const TiDBTableID: i64 = ReservedGlobalIDUpperBound - 15;
/// `mysql.help_topic` 表 ID：帮助主题（兼容 MySQL）。
// HelpTopicTableID is the table ID of `help_topic`.
pub const HelpTopicTableID: i64 = ReservedGlobalIDUpperBound - 16;
/// `mysql.stats_meta` 表 ID：表级统计信息元数据。
// StatsMetaTableID is the table ID of `stats_meta`.
pub const StatsMetaTableID: i64 = ReservedGlobalIDUpperBound - 17;
/// `mysql.stats_histograms` 表 ID：列/索引直方图元数据。
// StatsHistogramsTableID is the table ID of `stats_histograms`.
pub const StatsHistogramsTableID: i64 = ReservedGlobalIDUpperBound - 18;
/// `mysql.stats_buckets` 表 ID：直方图桶数据。
// StatsBucketsTableID is the table ID of `stats_buckets`.
pub const StatsBucketsTableID: i64 = ReservedGlobalIDUpperBound - 19;
/// `mysql.gc_delete_range` 表 ID：待 GC 删除的键区间。
// GCDeleteRangeTableID is the table ID of `gc_delete_range`.
pub const GCDeleteRangeTableID: i64 = ReservedGlobalIDUpperBound - 20;
/// `mysql.gc_delete_range_done` 表 ID：已完成 DeleteRange 的区间。
// GCDeleteRangeDoneTableID is the table ID of `gc_delete_range_done`.
pub const GCDeleteRangeDoneTableID: i64 = ReservedGlobalIDUpperBound - 21;
/// `mysql.stats_feedback` 表 ID：统计反馈（已废弃但仍需建表兼容）。
// StatsFeedbackTableID is the table ID of `stats_feedback`.
pub const StatsFeedbackTableID: i64 = ReservedGlobalIDUpperBound - 22;
/// `mysql.role_edges` 表 ID：角色继承边。
// RoleEdgesTableID is the table ID of `role_edges`.
pub const RoleEdgesTableID: i64 = ReservedGlobalIDUpperBound - 23;
/// `mysql.default_roles` 表 ID：用户默认角色。
// DefaultRolesTableID is the table ID of `default_roles`.
pub const DefaultRolesTableID: i64 = ReservedGlobalIDUpperBound - 24;
/// `mysql.bind_info` 表 ID：SQL 绑定（binding）信息。
// BindInfoTableID is the table ID of `bind_info`.
pub const BindInfoTableID: i64 = ReservedGlobalIDUpperBound - 25;
/// `mysql.stats_top_n` 表 ID：CMSketch TopN 统计。
// StatsTopNTableID is the table ID of `stats_top_n`.
pub const StatsTopNTableID: i64 = ReservedGlobalIDUpperBound - 26;
/// `mysql.expr_pushdown_blacklist` 表 ID：禁止下推的表达式名单。
// ExprPushdownBlacklistTableID is the table ID of `expr_pushdown_blacklist`.
pub const ExprPushdownBlacklistTableID: i64 = ReservedGlobalIDUpperBound - 27;
/// `mysql.opt_rule_blacklist` 表 ID：禁用的优化规则名单。
// OptRuleBlacklistTableID is the table ID of `opt_rule_blacklist`.
pub const OptRuleBlacklistTableID: i64 = ReservedGlobalIDUpperBound - 28;
/// `mysql.stats_extended` 表 ID：扩展统计信息。
// StatsExtendedTableID is the table ID of `stats_extended`.
pub const StatsExtendedTableID: i64 = ReservedGlobalIDUpperBound - 29;
/// `mysql.stats_fm_sketch` 表 ID：FMSketch 基数估计数据。
// StatsFMSketchTableID is the table ID of `stats_fm_sketch`.
pub const StatsFMSketchTableID: i64 = ReservedGlobalIDUpperBound - 30;
/// `mysql.global_grants` 表 ID：动态特权授予记录。
// GlobalGrantsTableID is the table ID of `global_grants`.
pub const GlobalGrantsTableID: i64 = ReservedGlobalIDUpperBound - 31;
/// `mysql.capture_plan_baselines_blacklist` 表 ID：捕获执行计划基线的过滤规则。
// CapturePlanBaselinesBlacklistTableID is the table ID of `capture_plan_baselines_blacklist`.
pub const CapturePlanBaselinesBlacklistTableID: i64 = ReservedGlobalIDUpperBound - 32;
/// `mysql.column_stats_usage` 表 ID：列统计使用情况。
// ColumnStatsUsageTableID is the table ID of `column_stats_usage`.
pub const ColumnStatsUsageTableID: i64 = ReservedGlobalIDUpperBound - 33;
/// `mysql.table_cache_meta` 表 ID：缓存表元数据锁。
// TableCacheMetaTableID is the table ID of `table_cache_meta`.
pub const TableCacheMetaTableID: i64 = ReservedGlobalIDUpperBound - 34;
/// `mysql.analyze_options` 表 ID：ANALYZE 选项。
// AnalyzeOptionsTableID is the table ID of `analyze_options`.
pub const AnalyzeOptionsTableID: i64 = ReservedGlobalIDUpperBound - 35;
/// `mysql.stats_history` 表 ID：历史统计快照。
// StatsHistoryTableID is the table ID of `stats_history`.
pub const StatsHistoryTableID: i64 = ReservedGlobalIDUpperBound - 36;
/// `mysql.stats_meta_history` 表 ID：历史 stats_meta。
// StatsMetaHistoryTableID is the table ID of `stats_meta_history`.
pub const StatsMetaHistoryTableID: i64 = ReservedGlobalIDUpperBound - 37;
/// `mysql.analyze_jobs` 表 ID：ANALYZE 作业状态。
// AnalyzeJobsTableID is the table ID of `analyze_jobs`.
pub const AnalyzeJobsTableID: i64 = ReservedGlobalIDUpperBound - 38;
/// `mysql.advisory_locks` 表 ID：顾问锁（GET_LOCK/RELEASE_LOCK）。
// AdvisoryLocksTableID is the table ID of `advisory_locks`.
pub const AdvisoryLocksTableID: i64 = ReservedGlobalIDUpperBound - 39;
/// `mysql.plan_replayer_status` 表 ID：Plan Replayer 任务状态。
// PlanReplayerStatusTableID is the table ID of `plan_replayer_status`.
pub const PlanReplayerStatusTableID: i64 = ReservedGlobalIDUpperBound - 40;
/// `mysql.plan_replayer_task` 表 ID：Plan Replayer 捕获任务。
// PlanReplayerTaskTableID is the table ID of `plan_replayer_task`.
pub const PlanReplayerTaskTableID: i64 = ReservedGlobalIDUpperBound - 41;
/// `mysql.stats_table_locked` 表 ID：被锁定而不再自动更新统计的表。
// StatsTableLockedTableID is the table ID of `stats_table_locked`.
pub const StatsTableLockedTableID: i64 = ReservedGlobalIDUpperBound - 42;
/// `mysql.tidb_ttl_table_status` 表 ID：TTL（按时间过期删除）表调度状态。
// TiDBTTLTableStatusTableID is the table ID of `tidb_ttl_table_status`.
pub const TiDBTTLTableStatusTableID: i64 = ReservedGlobalIDUpperBound - 43;
/// `mysql.tidb_ttl_task` 表 ID：TTL 并行扫描子任务。
// TiDBTTLTaskTableID is the table ID of `tidb_ttl_task`.
pub const TiDBTTLTaskTableID: i64 = ReservedGlobalIDUpperBound - 44;
/// `mysql.tidb_ttl_job_history` 表 ID：TTL 作业历史。
// TiDBTTLJobHistoryTableID is the table ID of `tidb_ttl_job_history`.
pub const TiDBTTLJobHistoryTableID: i64 = ReservedGlobalIDUpperBound - 45;
/// `mysql.tidb_global_task` 表 ID：分布式全局任务。
// TiDBGlobalTaskTableID is the table ID of `tidb_global_task`.
pub const TiDBGlobalTaskTableID: i64 = ReservedGlobalIDUpperBound - 46;
/// `mysql.tidb_global_task_history` 表 ID：全局任务历史。
// TiDBGlobalTaskHistoryTableID is the table ID of `tidb_global_task_history`.
pub const TiDBGlobalTaskHistoryTableID: i64 = ReservedGlobalIDUpperBound - 47;
/// `mysql.tidb_import_jobs` 表 ID：IMPORT INTO 作业。
// TiDBImportJobsTableID is the table ID of `tidb_import_jobs`.
pub const TiDBImportJobsTableID: i64 = ReservedGlobalIDUpperBound - 48;
/// `mysql.tidb_runaway_watch` 表 ID：runaway 查询隔离监视条件。
// TiDBRunawayWatchTableID is the table ID of `tidb_runaway_watch`.
pub const TiDBRunawayWatchTableID: i64 = ReservedGlobalIDUpperBound - 49;
/// `mysql.tidb_runaway_queries` 表 ID：被识别为 runaway 的查询记录。
// TiDBRunawayQueriesTableID is the table ID of `tidb_runaway`.
pub const TiDBRunawayQueriesTableID: i64 = ReservedGlobalIDUpperBound - 50;
/// `mysql.tidb_timers` 表 ID：内核定时器。
// TiDBTimersTableID is the table ID of `tidb_timers`.
pub const TiDBTimersTableID: i64 = ReservedGlobalIDUpperBound - 51;
/// `mysql.tidb_runaway_watch_done` 表 ID：已结束的 runaway 监视记录。
// TiDBRunawayWatchDoneTableID is the table ID of `tidb_done_runaway_watch`.
pub const TiDBRunawayWatchDoneTableID: i64 = ReservedGlobalIDUpperBound - 52;
/// `mysql.dist_framework_meta` 表 ID：分布式任务框架节点元信息。
// DistFrameworkMetaTableID is the table ID of `dist_framework_meta`.
pub const DistFrameworkMetaTableID: i64 = ReservedGlobalIDUpperBound - 53;
/// `mysql.request_unit_by_group` 表 ID：按资源组汇总的 RU（Request Unit）消耗。
// RequestUnitByGroupTableID is the table ID of `request_unit_by_group`.
pub const RequestUnitByGroupTableID: i64 = ReservedGlobalIDUpperBound - 54;
/// `mysql.tidb_pitr_id_map` 表 ID：PITR（基于时间点恢复）上下游对象 ID 映射。
// TiDBPITRIDMapTableID is the table ID of `tidb_pitr_id_map`.
pub const TiDBPITRIDMapTableID: i64 = ReservedGlobalIDUpperBound - 55;
/// `mysql.tidb_restore_registry` 表 ID：活跃恢复任务登记，避免冲突。
// TiDBRestoreRegistryTableID is the table ID of `tidb_restore_registry`.
pub const TiDBRestoreRegistryTableID: i64 = ReservedGlobalIDUpperBound - 56;
/// `mysql.index_advisor_results` 表 ID：索引顾问推荐结果。
// IndexAdvisorResultsTableID is the table ID of `index_advisor`.
pub const IndexAdvisorResultsTableID: i64 = ReservedGlobalIDUpperBound - 57;
/// `mysql.tidb_kernel_options` 表 ID：内核可调选项。
// TiDBKernelOptionsTableID is the table ID of `tidb_kernel_options`.
pub const TiDBKernelOptionsTableID: i64 = ReservedGlobalIDUpperBound - 58;
/// `mysql.tidb_workload_values` 表 ID：基于工作负载学习到的取值。
// TiDBWorkloadValuesTableID is the table ID of `tidb_workload_values`.
pub const TiDBWorkloadValuesTableID: i64 = ReservedGlobalIDUpperBound - 59;
/// `sys` 库的固定数据库 ID。
// SysDatabaseID is the database ID of `sys`.
pub const SysDatabaseID: i64 = ReservedGlobalIDUpperBound - 60;
/// `mysql.tidb_softdelete_table_status` 表 ID：软删除表状态。
// TiDBSoftDeleteTableStatusTableID is the table ID of `tidb_softdelete_table_status`.
pub const TiDBSoftDeleteTableStatusTableID: i64 = ReservedGlobalIDUpperBound - 61;
/// `mysql.tidb_masking_policy` 表 ID：列脱敏（masking）策略元数据。
// TiDBMaskingPolicyTableID is the table ID of `tidb_masking_policy`.
pub const TiDBMaskingPolicyTableID: i64 = ReservedGlobalIDUpperBound - 62;

/// 判断 ID 是否落在系统保留区间 `(lower, upper]`（下界本身仍属用户可用）。
// IsReservedID 判断 ID 是否落在系统保留区间 (lower, upper]。
// 下界本身仍是用户可用的最大 ID，上界则属于 mysql 系统数据库。
pub fn IsReservedID(id: i64) -> bool {
    ReservedGlobalIDLowerBound < id && id <= ReservedGlobalIDUpperBound
}

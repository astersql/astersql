// Copyright 2026 AsterSQL.

// InfoSchema（信息模式）crate 根模块。
//
// 汇总元数据缓存、Builder、集群表、指标 schema、SIEVE 缓存与表辅助等子模块，
// 并向会话/执行器导出常用类型。InfoSchema 是会话可见的数据库/表/列等元数据快照。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// 从 schema diff 增量构建 InfoSchema。
pub mod builder;
/// Builder 杂项辅助。
pub mod builder_misc;
/// Placement / 绑定策略等 bundle 构建。
pub mod bundle_builder;
/// InfoSchema 版本缓存（InfoCache）。
pub mod cache;
/// 集群表（CLUSTER_*）相关逻辑。
pub mod cluster;
/// 本包错误类型。
pub mod error;
/// InfoSchema V1 核心实现。
pub mod infoschema;
/// InfoSchema V2（基于 SIEVE 等的增量缓存）。
pub mod infoschema_v2;
/// METRICS_SCHEMA 中各指标表的 PromQL 定义表。
pub mod metric_table_def;
/// InfoSchema V2 SIEVE 状态钩子的指标实现。
pub mod metrics;
/// 指标表定义查询与判定。
pub mod metrics_schema;
/// SIEVE 缓存算法。
pub mod sieve;
/// information_schema 虚拟表实现辅助。
pub mod tables;

pub use builder::{
    ActionType, AffectedOption, Builder, MetadataReader, NewBuilder, SchemaDiff, getKeptAllocators,
};
pub use cache::{Data, InfoCache, NewCache, SchemaRef};
pub use cluster::{
    AppendHostInfoToRows, ClusterSessionContext, ClusterTableCopDestination, ClusterTableDeadlocks,
    ClusterTableMemoryUsage, ClusterTableMemoryUsageOpsHistory, ClusterTableProcesslist,
    ClusterTableSlowLog, ClusterTableStatementsSummary, ClusterTableStatementsSummaryEvicted,
    ClusterTableStatementsSummaryHistory, ClusterTableTiDBIndexUsage, ClusterTableTiDBPlanCache,
    ClusterTableTiDBStatementsStats, ClusterTableTiDBTrx, ClusterTableTrxSummary, Datum,
    GetClusterTableCopDestination, GetInstanceAddr, IsClusterTableByName, ServerInfo,
};
pub use infoschema::{
    AllSchemaNames, CiString, ColumnInfo, DBInfo, FindTableByTblOrPartID, ForeignKeyInfo,
    HasAutoIncrementColumn, IndexInfo, InfoSchema, InfoSchemaError, LoadMaskingPolicies,
    MaskingPolicyInfo, MaskingPolicyLoader, MaskingPolicyRestrictOps, MaskingPolicyStatus,
    MaskingPolicyType, MockInfoSchema, MockInfoSchemaWithSchemaVer, PartitionDefinition,
    PartitionInfo, PolicyInfo, ReferredFKInfo, ResourceGroupInfo, SchemaByTable, Table, TableInfo,
    TableIsSequence, TableIsView, TableItem, bucketCount, infoSchema, isMaskingPolicyTableNotReady,
    loadMaskingPoliciesWithTableIDs, normalizeMaskingPolicyTableIDs, tableBucketIdx,
};
pub use infoschema_v2::{
    Data as V2Data, IsSpecialDB, IsV2, NewData as NewV2Data, NewInfoSchemaV2, infoschemaV2,
};
pub use metric_table_def::MetricTableMap;
pub use metrics_schema::{GetMetricTableDef, IsMetricTable, MetricTableDef};
pub use sieve::{EmptySieveStatusHook, Sieve, SieveStatusHook, newSieve};
pub use tables::{
    FormatStoreServerVersion, FormatTiDBVersion, StoreInfo, StoreLabel, is_tiflash_store,
    is_tiflash_write_node, isTiFlashStore, isTiFlashWriteNode,
};

/// 请求上下文与后台任务等接口占位。
mod interface;
pub use interface::{Background, BackgroundArc, ContextError, RequestContext, TODO, TODOArc};

/// 包级统一错误别名。
pub type Error = astersql_util_dbterror::errors::SharedError;

#[cfg(test)]
#[path = "bench_test.rs"]
mod bench_test;
#[cfg(test)]
#[path = "builder_test.rs"]
mod builder_test;
#[cfg(test)]
#[path = "bundle_builder_test.rs"]
mod bundle_builder_test;
#[cfg(test)]
#[path = "cluster_test.rs"]
mod cluster_test;
#[cfg(test)]
mod go_merge_45_test;
#[cfg(test)]
#[path = "infoschema_nokit_test.rs"]
mod infoschema_nokit_test;
#[cfg(test)]
#[path = "infoschema_test.rs"]
mod infoschema_test;
#[cfg(test)]
#[path = "infoschema_v2_test.rs"]
mod infoschema_v2_test;
#[cfg(test)]
#[path = "infoschemav2_cache_test.rs"]
mod infoschemav2_cache_test;
#[cfg(test)]
#[path = "interface_aster_unit_test.rs"]
mod interface_aster_unit_test;
#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
#[cfg(test)]
#[path = "metrics_schema_test.rs"]
mod metrics_schema_test;
#[cfg(test)]
#[path = "metrics_test.rs"]
mod metrics_test;
#[cfg(test)]
#[path = "sieve_test.rs"]
mod sieve_test;
#[cfg(test)]
#[path = "tables_test.rs"]
mod tables_test;

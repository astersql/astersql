// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

// INFORMATION_SCHEMA 虚拟表定义与集群元数据辅助。
//
// 对应 Go `pkg/infoschema/tables.go`：声明 INFORMATION_SCHEMA 各表名常量、
// 列元数据、内存虚拟表接口，以及集群 ServerInfo 发现、版本格式化、
// TiFlash Store 标签识别、分片信息等辅助函数。
// InfoSchema 是库表元数据的版本化快照；INFORMATION_SCHEMA 以虚拟表形式对外查询。

// Static column defs, metadata construction, and in-memory virtual table interfaces.

#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::sync::{Arc, OnceLock};

use crate::cluster::{ClusterTableTiDBIndexUsage, Datum};
use crate::infoschema::{CiString, ColumnInfo, DBInfo, Table, TableInfo};
use astersql_meta_model as model_dependency;
use astersql_parser_charset as charset;
use astersql_parser_mysql as mysql;

#[path = "catalog_columns_go45.rs"]
mod catalog_columns_go45;

/// 批量声明 INFORMATION_SCHEMA 表名字符串常量的宏。
macro_rules! table_constants {
    ($(($name:ident, $value:literal)),+ $(,)?) => { $(pub const $name: &str = $value;)+ };
}
// 与 Go 常量块对齐的表名：MySQL 兼容表 + TiDB/TiKV/TiFlash 扩展表。
table_constants!(
    (TableSchemata, "SCHEMATA"),
    (TableTables, "TABLES"),
    (TableColumns, "COLUMNS"),
    (TableStatistics, "STATISTICS"),
    (TableCharacterSets, "CHARACTER_SETS"),
    (TableCollations, "COLLATIONS"),
    (TableProfiling, "PROFILING"),
    (TablePartitions, "PARTITIONS"),
    (TableKeyColumn, "KEY_COLUMN_USAGE"),
    (TableReferConst, "REFERENTIAL_CONSTRAINTS"),
    (TableConstraints, "TABLE_CONSTRAINTS"),
    (TableTriggers, "TRIGGERS"),
    (TableUserPrivileges, "USER_PRIVILEGES"),
    (TableSchemaPrivileges, "SCHEMA_PRIVILEGES"),
    (TableTablePrivileges, "TABLE_PRIVILEGES"),
    (TableColumnPrivileges, "COLUMN_PRIVILEGES"),
    (TableEngines, "ENGINES"),
    (TableViews, "VIEWS"),
    (TableRoutines, "ROUTINES"),
    (TableParameters, "PARAMETERS"),
    (TableEvents, "EVENTS"),
    (
        TableCollationCharacterSetApplicability,
        "COLLATION_CHARACTER_SET_APPLICABILITY"
    ),
    (TableProcesslist, "PROCESSLIST"),
    (TableTiDBIndexes, "TIDB_INDEXES"),
    (TableTiDBHotRegions, "TIDB_HOT_REGIONS"),
    (TableTiDBHotRegionsHistory, "TIDB_HOT_REGIONS_HISTORY"),
    (TableTiKVStoreStatus, "TIKV_STORE_STATUS"),
    (TableAnalyzeStatus, "ANALYZE_STATUS"),
    (TableTiKVRegionStatus, "TIKV_REGION_STATUS"),
    (TableTiKVRegionPeers, "TIKV_REGION_PEERS"),
    (TableTiDBServersInfo, "TIDB_SERVERS_INFO"),
    (TableSlowQuery, "SLOW_QUERY"),
    (TableClusterInfo, "CLUSTER_INFO"),
    (TableClusterConfig, "CLUSTER_CONFIG"),
    (TableClusterLog, "CLUSTER_LOG"),
    (TableClusterLoad, "CLUSTER_LOAD"),
    (TableClusterHardware, "CLUSTER_HARDWARE"),
    (TableClusterSystemInfo, "CLUSTER_SYSTEMINFO"),
    (TableTiFlashReplica, "TIFLASH_REPLICA"),
    (
        TableStorageClassTransitions,
        "TIKV_STORAGE_CLASS_TRANSITIONS"
    ),
    (TableInspectionResult, "INSPECTION_RESULT"),
    (TableMetricTables, "METRICS_TABLES"),
    (TableMetricSummary, "METRICS_SUMMARY"),
    (TableMetricSummaryByLabel, "METRICS_SUMMARY_BY_LABEL"),
    (TableInspectionSummary, "INSPECTION_SUMMARY"),
    (TableInspectionRules, "INSPECTION_RULES"),
    (TableDDLJobs, "DDL_JOBS"),
    (TableSequences, "SEQUENCES"),
    (TableStatementsSummary, "STATEMENTS_SUMMARY"),
    (TableStatementsSummaryHistory, "STATEMENTS_SUMMARY_HISTORY"),
    (TableStatementsSummaryEvicted, "STATEMENTS_SUMMARY_EVICTED"),
    (TableTiDBStatementsStats, "TIDB_STATEMENTS_STATS"),
    (TableStorageStats, "TABLE_STORAGE_STATS"),
    (TableTiFlashTables, "TIFLASH_TABLES"),
    (TableTiFlashSegments, "TIFLASH_SEGMENTS"),
    (TableTiFlashIndexes, "TIFLASH_INDEXES"),
    (
        TableClientErrorsSummaryGlobal,
        "CLIENT_ERRORS_SUMMARY_GLOBAL"
    ),
    (
        TableClientErrorsSummaryByUser,
        "CLIENT_ERRORS_SUMMARY_BY_USER"
    ),
    (
        TableClientErrorsSummaryByHost,
        "CLIENT_ERRORS_SUMMARY_BY_HOST"
    ),
    (TableTiDBTrx, "TIDB_TRX"),
    (TableDeadlocks, "DEADLOCKS"),
    (TableDataLockWaits, "DATA_LOCK_WAITS"),
    (TableAttributes, "ATTRIBUTES"),
    (TablePlacementPolicies, "PLACEMENT_POLICIES"),
    (TableTrxSummary, "TRX_SUMMARY"),
    (TableVariablesInfo, "VARIABLES_INFO"),
    (TableUserAttributes, "USER_ATTRIBUTES"),
    (TableMemoryUsage, "MEMORY_USAGE"),
    (TableMemoryUsageOpsHistory, "MEMORY_USAGE_OPS_HISTORY"),
    (TableResourceGroups, "RESOURCE_GROUPS"),
    (TableRunawayWatches, "RUNAWAY_WATCHES"),
    (TableCheckConstraints, "CHECK_CONSTRAINTS"),
    (TableTiDBCheckConstraints, "TIDB_CHECK_CONSTRAINTS"),
    (TableKeywords, "KEYWORDS"),
    (TableTiDBIndexUsage, "TIDB_INDEX_USAGE"),
    (TableTiDBPlanCache, "TIDB_PLAN_CACHE"),
    (TableKeyspaceMeta, "KEYSPACE_META"),
    (TableSchemataExtensions, "SCHEMATA_EXTENSIONS")
);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 虚拟表列的简化类型枚举（对应 MySQL 字段类型子集）。
pub enum ColumnType {
    Varchar,
    Tiny,
    Long,
    Longlong,
    Double,
    Blob,
    MediumBlob,
    LongBlob,
    Timestamp,
    Datetime,
    Decimal,
    Json,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 列元数据：供构建 VirtualTableMeta / TableInfo。
pub struct columnInfo {
    pub name: &'static str,
    pub column_type: ColumnType,
    pub size: u32,
    pub decimal: Option<u8>,
    pub unsigned: bool,
    pub not_null: bool,
    pub primary_key: bool,
    pub binary: bool,
    pub default_value: Option<&'static str>,
    pub comment: &'static str,
}

impl columnInfo {
    /// 构造指定 MySQL 类型的列描述。
    pub const fn typed(name: &'static str, column_type: ColumnType, size: u32) -> Self {
        Self {
            name,
            column_type,
            size,
            decimal: None,
            unsigned: false,
            not_null: false,
            primary_key: false,
            binary: false,
            default_value: None,
            comment: "",
        }
    }

    /// 构造 VARCHAR 列描述。
    pub const fn varchar(name: &'static str, size: u32) -> Self {
        Self::typed(name, ColumnType::Varchar, size)
    }

    /// 构造 BIGINT/整数列描述。
    pub const fn integer(name: &'static str) -> Self {
        Self::typed(name, ColumnType::Longlong, 21)
    }

    /// 对应 Go columnInfo.flag 中的 mysql.UnsignedFlag。
    pub const fn unsigned(mut self) -> Self {
        self.unsigned = true;
        self
    }

    /// 对应 Go columnInfo.flag 中的 mysql.NotNullFlag。
    pub const fn not_null(mut self) -> Self {
        self.not_null = true;
        self
    }

    /// 对应 Go columnInfo.deflt。
    pub const fn with_default(mut self, default_value: &'static str) -> Self {
        self.default_value = Some(default_value);
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 虚拟表元数据：稳定 id、表名与列定义。
pub struct VirtualTableMeta {
    pub id: i64,
    pub name: &'static str,
    pub columns: Vec<columnInfo>,
}

// 注册表中的表名清单（顺序决定默认虚拟表 id）。
const TABLE_NAMES: &[&str] = &[
    TableSchemata,
    TableTables,
    TableColumns,
    TableStatistics,
    TableCharacterSets,
    TableCollations,
    TableProfiling,
    TablePartitions,
    TableKeyColumn,
    TableReferConst,
    TableConstraints,
    TableTriggers,
    TableUserPrivileges,
    TableSchemaPrivileges,
    TableTablePrivileges,
    TableColumnPrivileges,
    TableEngines,
    TableViews,
    TableRoutines,
    TableParameters,
    TableEvents,
    TableCollationCharacterSetApplicability,
    TableProcesslist,
    TableTiDBIndexes,
    TableTiDBHotRegions,
    TableTiDBHotRegionsHistory,
    TableTiKVStoreStatus,
    TableAnalyzeStatus,
    TableTiKVRegionStatus,
    TableTiKVRegionPeers,
    TableTiDBServersInfo,
    TableSlowQuery,
    TableClusterInfo,
    TableClusterConfig,
    TableClusterLog,
    TableClusterLoad,
    TableClusterHardware,
    TableClusterSystemInfo,
    TableTiFlashReplica,
    TableStorageClassTransitions,
    TableInspectionResult,
    TableMetricTables,
    TableMetricSummary,
    TableMetricSummaryByLabel,
    TableInspectionSummary,
    TableInspectionRules,
    TableDDLJobs,
    TableSequences,
    TableStatementsSummary,
    TableStatementsSummaryHistory,
    TableStatementsSummaryEvicted,
    TableTiDBStatementsStats,
    TableStorageStats,
    TableTiFlashTables,
    TableTiFlashSegments,
    TableTiFlashIndexes,
    TableClientErrorsSummaryGlobal,
    TableClientErrorsSummaryByUser,
    TableClientErrorsSummaryByHost,
    TableTiDBTrx,
    TableDeadlocks,
    TableDataLockWaits,
    TableAttributes,
    TablePlacementPolicies,
    TableTrxSummary,
    TableVariablesInfo,
    TableUserAttributes,
    TableMemoryUsage,
    TableMemoryUsageOpsHistory,
    TableResourceGroups,
    TableRunawayWatches,
    TableCheckConstraints,
    TableTiDBCheckConstraints,
    TableKeywords,
    TableTiDBIndexUsage,
    TableTiDBPlanCache,
    TableKeyspaceMeta,
    TableSchemataExtensions,
    ClusterTableTiDBIndexUsage,
];

// 按表名返回列集；JDBC 目录发现依赖的三张表与 Go 定义逐列对齐。
fn default_columns(name: &str) -> Vec<columnInfo> {
    match name {
        TableSchemata => vec![
            columnInfo::varchar("CATALOG_NAME", 512),
            columnInfo::varchar("SCHEMA_NAME", 64),
            columnInfo::varchar("DEFAULT_CHARACTER_SET_NAME", 64),
            columnInfo::varchar("DEFAULT_COLLATION_NAME", 32),
            columnInfo::varchar("SQL_PATH", 512),
            columnInfo::varchar("TIDB_PLACEMENT_POLICY_NAME", 64),
        ],
        TableTables => vec![
            columnInfo::varchar("TABLE_CATALOG", 512),
            columnInfo::varchar("TABLE_SCHEMA", 64),
            columnInfo::varchar("TABLE_NAME", 64),
            columnInfo::varchar("TABLE_TYPE", 64),
            columnInfo::varchar("ENGINE", 64),
            columnInfo::typed("VERSION", ColumnType::Longlong, 21),
            columnInfo::varchar("ROW_FORMAT", 10),
            columnInfo::integer("TABLE_ROWS"),
            columnInfo::integer("AVG_ROW_LENGTH"),
            columnInfo::integer("DATA_LENGTH"),
            columnInfo::integer("MAX_DATA_LENGTH"),
            columnInfo::integer("INDEX_LENGTH"),
            columnInfo::integer("DATA_FREE"),
            columnInfo::integer("AUTO_INCREMENT"),
            columnInfo::typed("CREATE_TIME", ColumnType::Datetime, 19),
            columnInfo::typed("UPDATE_TIME", ColumnType::Datetime, 19),
            columnInfo::typed("CHECK_TIME", ColumnType::Datetime, 19),
            columnInfo::varchar("TABLE_COLLATION", 32).with_default("utf8mb4_bin"),
            columnInfo::integer("CHECKSUM"),
            columnInfo::varchar("CREATE_OPTIONS", 255),
            columnInfo::varchar("TABLE_COMMENT", 2048),
            columnInfo::integer("TIDB_TABLE_ID"),
            columnInfo::varchar("TIDB_ROW_ID_SHARDING_INFO", 255),
            columnInfo::varchar("TIDB_PK_TYPE", 64),
            columnInfo::varchar("TIDB_PLACEMENT_POLICY_NAME", 64),
            columnInfo::varchar("TIDB_TABLE_MODE", 16),
            columnInfo::varchar("TIDB_AFFINITY", 128),
            columnInfo::varchar("TIDB_STORAGE_CLASS", 32),
        ],
        TableColumns => vec![
            columnInfo::varchar("TABLE_CATALOG", 64),
            columnInfo::varchar("TABLE_SCHEMA", 64),
            columnInfo::varchar("TABLE_NAME", 64),
            columnInfo::varchar("COLUMN_NAME", 64),
            columnInfo::typed("ORDINAL_POSITION", ColumnType::Long, 0).unsigned(),
            columnInfo::typed("COLUMN_DEFAULT", ColumnType::Blob, 0),
            columnInfo::varchar("IS_NULLABLE", 3),
            columnInfo::typed("DATA_TYPE", ColumnType::LongBlob, 0),
            columnInfo::typed("CHARACTER_MAXIMUM_LENGTH", ColumnType::Longlong, 0),
            columnInfo::typed("CHARACTER_OCTET_LENGTH", ColumnType::Longlong, 0),
            columnInfo::typed("NUMERIC_PRECISION", ColumnType::Longlong, 0).unsigned(),
            columnInfo::typed("NUMERIC_SCALE", ColumnType::Longlong, 0).unsigned(),
            columnInfo::typed("DATETIME_PRECISION", ColumnType::Long, 0).unsigned(),
            columnInfo::varchar("CHARACTER_SET_NAME", 64),
            columnInfo::varchar("COLLATION_NAME", 64),
            columnInfo::typed("COLUMN_TYPE", ColumnType::MediumBlob, 0),
            columnInfo::varchar("COLUMN_KEY", 3),
            columnInfo::varchar("EXTRA", 256),
            columnInfo::varchar("PRIVILEGES", 154),
            columnInfo::typed("COLUMN_COMMENT", ColumnType::Blob, 0),
            columnInfo::typed("GENERATION_EXPRESSION", ColumnType::LongBlob, 0).not_null(),
            columnInfo::typed("SRS_ID", ColumnType::Long, 0).unsigned(),
        ],
        TableCharacterSets => vec![
            columnInfo::varchar("CHARACTER_SET_NAME", 32),
            columnInfo::varchar("DEFAULT_COLLATE_NAME", 32),
            columnInfo::varchar("DESCRIPTION", 60),
            columnInfo::typed("MAXLEN", ColumnType::Longlong, 3),
        ],
        TableCollations => vec![
            columnInfo::varchar("COLLATION_NAME", 32),
            columnInfo::varchar("CHARACTER_SET_NAME", 32),
            columnInfo::typed("ID", ColumnType::Longlong, 11),
            columnInfo::varchar("IS_DEFAULT", 3),
            columnInfo::varchar("IS_COMPILED", 3),
            columnInfo::typed("SORTLEN", ColumnType::Longlong, 3),
            columnInfo::varchar("PAD_ATTRIBUTE", 9),
        ],
        TableCollationCharacterSetApplicability => vec![
            columnInfo::varchar("COLLATION_NAME", 32),
            columnInfo::varchar("CHARACTER_SET_NAME", 32),
        ],
        TableStatistics => vec![
            columnInfo::varchar("TABLE_CATALOG", 512),
            columnInfo::varchar("TABLE_SCHEMA", 64),
            columnInfo::varchar("TABLE_NAME", 64),
            columnInfo::varchar("NON_UNIQUE", 1),
            columnInfo::varchar("INDEX_SCHEMA", 64),
            columnInfo::varchar("INDEX_NAME", 64),
            columnInfo::typed("SEQ_IN_INDEX", ColumnType::Longlong, 2),
            columnInfo::varchar("COLUMN_NAME", 21),
            columnInfo::varchar("COLLATION", 1),
            columnInfo::integer("CARDINALITY"),
            columnInfo::typed("SUB_PART", ColumnType::Longlong, 3),
            columnInfo::varchar("PACKED", 10),
            columnInfo::varchar("NULLABLE", 3),
            columnInfo::varchar("INDEX_TYPE", 16),
            columnInfo::varchar("COMMENT", 16),
            columnInfo::varchar("INDEX_COMMENT", 1024),
            columnInfo::varchar("IS_VISIBLE", 3),
            columnInfo::varchar("EXPRESSION", 64),
        ],
        TablePartitions => vec![
            columnInfo::varchar("TABLE_CATALOG", 512),
            columnInfo::varchar("TABLE_SCHEMA", 64),
            columnInfo::varchar("TABLE_NAME", 64),
            columnInfo::varchar("PARTITION_NAME", 64),
            columnInfo::varchar("SUBPARTITION_NAME", 64),
            columnInfo::integer("PARTITION_ORDINAL_POSITION"),
            columnInfo::integer("SUBPARTITION_ORDINAL_POSITION"),
            columnInfo::varchar("PARTITION_METHOD", 18),
            columnInfo::varchar("SUBPARTITION_METHOD", 12),
            columnInfo::typed("PARTITION_EXPRESSION", ColumnType::LongBlob, 0),
            columnInfo::typed("SUBPARTITION_EXPRESSION", ColumnType::LongBlob, 0),
            columnInfo::typed("PARTITION_DESCRIPTION", ColumnType::LongBlob, 0),
            columnInfo::integer("TABLE_ROWS"),
            columnInfo::integer("AVG_ROW_LENGTH"),
            columnInfo::integer("DATA_LENGTH"),
            columnInfo::integer("MAX_DATA_LENGTH"),
            columnInfo::integer("INDEX_LENGTH"),
            columnInfo::integer("DATA_FREE"),
            columnInfo::typed("CREATE_TIME", ColumnType::Datetime, 19),
            columnInfo::typed("UPDATE_TIME", ColumnType::Datetime, 19),
            columnInfo::typed("CHECK_TIME", ColumnType::Datetime, 19),
            columnInfo::integer("CHECKSUM"),
            columnInfo::varchar("PARTITION_COMMENT", 80),
            columnInfo::varchar("NODEGROUP", 12),
            columnInfo::varchar("TABLESPACE_NAME", 64),
            columnInfo::integer("TIDB_PARTITION_ID"),
            columnInfo::varchar("TIDB_PLACEMENT_POLICY_NAME", 64),
            columnInfo::varchar("TIDB_AFFINITY", 128),
            columnInfo::varchar("TIDB_STORAGE_CLASS", 32),
        ],
        TableKeyColumn => vec![
            columnInfo::varchar("CONSTRAINT_CATALOG", 512).not_null(),
            columnInfo::varchar("CONSTRAINT_SCHEMA", 64).not_null(),
            columnInfo::varchar("CONSTRAINT_NAME", 64).not_null(),
            columnInfo::varchar("TABLE_CATALOG", 512).not_null(),
            columnInfo::varchar("TABLE_SCHEMA", 64).not_null(),
            columnInfo::varchar("TABLE_NAME", 64).not_null(),
            columnInfo::varchar("COLUMN_NAME", 64).not_null(),
            columnInfo::typed("ORDINAL_POSITION", ColumnType::Longlong, 10).not_null(),
            columnInfo::typed("POSITION_IN_UNIQUE_CONSTRAINT", ColumnType::Longlong, 10),
            columnInfo::varchar("REFERENCED_TABLE_SCHEMA", 64),
            columnInfo::varchar("REFERENCED_TABLE_NAME", 64),
            columnInfo::varchar("REFERENCED_COLUMN_NAME", 64),
        ],
        TableConstraints => vec![
            columnInfo::varchar("CONSTRAINT_CATALOG", 512),
            columnInfo::varchar("CONSTRAINT_SCHEMA", 64),
            columnInfo::varchar("CONSTRAINT_NAME", 64),
            columnInfo::varchar("TABLE_SCHEMA", 64),
            columnInfo::varchar("TABLE_NAME", 64),
            columnInfo::varchar("CONSTRAINT_TYPE", 64),
        ],
        TableViews => vec![
            columnInfo::varchar("TABLE_CATALOG", 512).not_null(),
            columnInfo::varchar("TABLE_SCHEMA", 64).not_null(),
            columnInfo::varchar("TABLE_NAME", 64).not_null(),
            columnInfo::typed("VIEW_DEFINITION", ColumnType::LongBlob, 0).not_null(),
            columnInfo::varchar("CHECK_OPTION", 8).not_null(),
            columnInfo::varchar("IS_UPDATABLE", 3).not_null(),
            columnInfo::varchar("DEFINER", 77).not_null(),
            columnInfo::varchar("SECURITY_TYPE", 7).not_null(),
            columnInfo::varchar("CHARACTER_SET_CLIENT", 32).not_null(),
            columnInfo::varchar("COLLATION_CONNECTION", 32).not_null(),
        ],
        TableClusterConfig => vec![
            columnInfo::varchar("TYPE", 64),
            columnInfo::varchar("INSTANCE", 64),
            columnInfo::varchar("KEY", 256),
            columnInfo::typed("VALUE", ColumnType::LongBlob, 0),
        ],
        TableUserPrivileges => vec![
            columnInfo::varchar("GRANTEE", 81),
            columnInfo::varchar("TABLE_CATALOG", 512),
            columnInfo::varchar("PRIVILEGE_TYPE", 64),
            columnInfo::varchar("IS_GRANTABLE", 3),
        ],
        TableSchemaPrivileges => vec![
            columnInfo::varchar("GRANTEE", 81).not_null(),
            columnInfo::varchar("TABLE_CATALOG", 512).not_null(),
            columnInfo::varchar("TABLE_SCHEMA", 64).not_null(),
            columnInfo::varchar("PRIVILEGE_TYPE", 64).not_null(),
            columnInfo::varchar("IS_GRANTABLE", 3).not_null(),
        ],
        TableTablePrivileges => vec![
            columnInfo::varchar("GRANTEE", 81).not_null(),
            columnInfo::varchar("TABLE_CATALOG", 512).not_null(),
            columnInfo::varchar("TABLE_SCHEMA", 64).not_null(),
            columnInfo::varchar("TABLE_NAME", 64).not_null(),
            columnInfo::varchar("PRIVILEGE_TYPE", 64).not_null(),
            columnInfo::varchar("IS_GRANTABLE", 3).not_null(),
        ],
        TableColumnPrivileges => vec![
            columnInfo::varchar("GRANTEE", 81).not_null(),
            columnInfo::varchar("TABLE_CATALOG", 512).not_null(),
            columnInfo::varchar("TABLE_SCHEMA", 64).not_null(),
            columnInfo::varchar("TABLE_NAME", 64).not_null(),
            columnInfo::varchar("COLUMN_NAME", 64).not_null(),
            columnInfo::varchar("PRIVILEGE_TYPE", 64).not_null(),
            columnInfo::varchar("IS_GRANTABLE", 3).not_null(),
        ],
        TableTiDBIndexUsage | ClusterTableTiDBIndexUsage => vec![
            columnInfo::varchar("TABLE_SCHEMA", 64),
            columnInfo::varchar("TABLE_NAME", 64),
            columnInfo::varchar("INDEX_NAME", 64),
            columnInfo::integer("QUERY_TOTAL"),
            columnInfo::integer("KV_REQ_TOTAL"),
            columnInfo::integer("ROWS_ACCESS_TOTAL"),
            columnInfo::integer("PERCENTAGE_ACCESS_0"),
            columnInfo::integer("PERCENTAGE_ACCESS_0_1"),
            columnInfo::integer("PERCENTAGE_ACCESS_1_10"),
            columnInfo::integer("PERCENTAGE_ACCESS_10_20"),
            columnInfo::integer("PERCENTAGE_ACCESS_20_50"),
            columnInfo::integer("PERCENTAGE_ACCESS_50_100"),
            columnInfo::integer("PERCENTAGE_ACCESS_100"),
            columnInfo::typed("LAST_ACCESS_TIME", ColumnType::Datetime, 21),
        ],
        TableStorageClassTransitions => vec![
            columnInfo::varchar("TABLE_SCHEMA", 64),
            columnInfo::varchar("TABLE_NAME", 64),
            columnInfo::integer("TABLE_ID"),
            columnInfo::varchar("PARTITION_NAME", 64),
            columnInfo::integer("PARTITION_ID"),
            columnInfo::varchar("DIRECTION", 16),
            columnInfo::integer("TOTAL_REPLICAS").unsigned(),
            columnInfo::integer("COMPLETED_REPLICAS").unsigned(),
            columnInfo::typed("PROGRESS", ColumnType::Double, 22),
            columnInfo {
                decimal: Some(6),
                ..columnInfo::typed("START_TIME", ColumnType::Datetime, 26)
            },
            columnInfo::integer("DURATION").unsigned(),
            columnInfo {
                decimal: Some(6),
                ..columnInfo::typed("LAST_UPDATE_TIME", ColumnType::Datetime, 26)
            },
        ],
        TableCheckConstraints => vec![
            columnInfo::varchar("CONSTRAINT_CATALOG", 64).not_null(),
            columnInfo::varchar("CONSTRAINT_SCHEMA", 64).not_null(),
            columnInfo::varchar("CONSTRAINT_NAME", 64).not_null(),
            columnInfo::typed("CHECK_CLAUSE", ColumnType::LongBlob, 0).not_null(),
        ],
        TableTiDBCheckConstraints => vec![
            columnInfo::varchar("CONSTRAINT_CATALOG", 64).not_null(),
            columnInfo::varchar("CONSTRAINT_SCHEMA", 64).not_null(),
            columnInfo::varchar("CONSTRAINT_NAME", 64).not_null(),
            columnInfo::typed("CHECK_CLAUSE", ColumnType::LongBlob, 0).not_null(),
            columnInfo::varchar("TABLE_NAME", 64),
            columnInfo::integer("TABLE_ID"),
        ],
        TableProcesslist => vec![
            columnInfo::integer("ID"),
            columnInfo::varchar("USER", 32),
            columnInfo::varchar("HOST", 64),
            columnInfo::varchar("DB", 64),
            columnInfo::varchar("COMMAND", 16),
            columnInfo::integer("TIME"),
            columnInfo::varchar("STATE", 64),
            columnInfo::varchar("INFO", 1024),
        ],
        TableSlowQuery => catalog_columns_go45::slow_query_columns(),
        TableStatementsSummary | TableStatementsSummaryHistory => {
            catalog_columns_go45::statements_summary_columns()
        }
        _ => fallback_columns(),
    }
}

fn fallback_columns() -> Vec<columnInfo> {
    vec![
        columnInfo::varchar("INSTANCE", 64),
        columnInfo::varchar("NAME", 128),
        columnInfo::varchar("VALUE", 1024),
    ]
}

// Match the stable offsets in Go tableIDMap; these IDs are persisted in plans.
fn table_id_offset(name: &str) -> i64 {
    match name {
        TableSchemata => 1,
        TableTables => 2,
        TableColumns => 3,
        TableStatistics => 5,
        TableCharacterSets => 6,
        TableCollations => 7,
        TableProfiling => 10,
        TablePartitions => 11,
        TableKeyColumn => 12,
        TableReferConst => 13,
        TableConstraints => 16,
        TableTriggers => 17,
        TableUserPrivileges => 18,
        TableSchemaPrivileges => 19,
        TableTablePrivileges => 20,
        TableColumnPrivileges => 21,
        TableEngines => 22,
        TableViews => 23,
        TableRoutines => 24,
        TableParameters => 25,
        TableEvents => 26,
        TableCollationCharacterSetApplicability => 32,
        TableProcesslist => 33,
        TableTiDBIndexes => 34,
        TableTiDBHotRegions => 36,
        TableTiDBHotRegionsHistory => 78,
        TableTiKVStoreStatus => 37,
        TableAnalyzeStatus => 38,
        TableTiKVRegionStatus => 39,
        TableTiKVRegionPeers => 40,
        TableTiDBServersInfo => 41,
        TableSlowQuery => 35,
        TableClusterInfo => 42,
        TableClusterConfig => 43,
        TableClusterLog => 48,
        TableClusterLoad => 44,
        TableClusterHardware => 49,
        TableClusterSystemInfo => 50,
        TableTiFlashReplica => 45,
        TableStorageClassTransitions => 102,
        TableInspectionResult => 51,
        TableMetricTables => 54,
        TableMetricSummary => 52,
        TableMetricSummaryByLabel => 53,
        TableInspectionSummary => 55,
        TableInspectionRules => 56,
        TableDDLJobs => 57,
        TableSequences => 58,
        TableStatementsSummary => 59,
        TableStatementsSummaryHistory => 60,
        TableStatementsSummaryEvicted => 75,
        TableTiDBStatementsStats => 98,
        TableStorageStats => 63,
        TableTiFlashTables => 64,
        TableTiFlashSegments => 65,
        TableTiFlashIndexes => 95,
        TableClientErrorsSummaryGlobal => 67,
        TableClientErrorsSummaryByUser => 68,
        TableClientErrorsSummaryByHost => 69,
        TableTiDBTrx => 70,
        TableDeadlocks => 72,
        TableDataLockWaits => 74,
        TableAttributes => 77,
        TablePlacementPolicies => 79,
        TableTrxSummary => 80,
        TableVariablesInfo => 82,
        TableUserAttributes => 83,
        TableMemoryUsage => 84,
        TableMemoryUsageOpsHistory => 85,
        TableResourceGroups => 88,
        TableRunawayWatches => 89,
        TableCheckConstraints => 90,
        TableTiDBCheckConstraints => 91,
        TableKeywords => 92,
        TableTiDBIndexUsage => 93,
        TableTiDBPlanCache => 96,
        TableKeyspaceMeta => 100,
        TableSchemataExtensions => 101,
        ClusterTableTiDBIndexUsage => 94,
        _ => panic!("unregistered information_schema table: {name}"),
    }
}

// 懒初始化的表名 → VirtualTableMeta 全局注册表。
static TABLE_REGISTRY: OnceLock<HashMap<&'static str, VirtualTableMeta>> = OnceLock::new();
/// 获取 INFORMATION_SCHEMA 虚拟表注册表（只初始化一次）。
pub fn table_registry() -> &'static HashMap<&'static str, VirtualTableMeta> {
    TABLE_REGISTRY.get_or_init(|| {
        TABLE_NAMES
            .iter()
            .map(|name| {
                (
                    *name,
                    VirtualTableMeta {
                        id: astersql_meta_autoid::INFORMATION_SCHEMA_DB_ID + table_id_offset(*name),
                        name: *name,
                        columns: default_columns(name),
                    },
                )
            })
            .collect()
    })
}

/// 将简化 columnInfo 转为 infoschema::ColumnInfo。
pub fn buildColumnInfo(column_id: i64, column: &columnInfo) -> ColumnInfo {
    ColumnInfo {
        id: column_id,
        name: CiString::new(column.name),
        auto_increment: false,
    }
}
/// 由列定义构建虚拟表 TableInfo（id 来自注册表）。
pub fn buildTableMeta(table_name: &str, columns: &[columnInfo]) -> TableInfo {
    let id = table_registry().get(table_name).map_or(0, |table| table.id);
    let model_columns = columns
        .iter()
        .enumerate()
        .map(|(offset, column)| {
            let mut info = model_dependency::ColumnInfo::New(
                offset as i64,
                astersql_parser_ast::NewCIStr(column.name),
            );
            let tp = match column.column_type {
                ColumnType::Varchar => mysql::r#type::TypeVarchar,
                ColumnType::Tiny => mysql::r#type::TypeTiny,
                ColumnType::Long => mysql::r#type::TypeLong,
                ColumnType::Longlong => mysql::r#type::TypeLonglong,
                ColumnType::Double => mysql::r#type::TypeDouble,
                ColumnType::Blob => mysql::r#type::TypeBlob,
                ColumnType::MediumBlob => mysql::r#type::TypeMediumBlob,
                ColumnType::LongBlob => mysql::r#type::TypeLongBlob,
                ColumnType::Timestamp => mysql::r#type::TypeTimestamp,
                ColumnType::Datetime => mysql::r#type::TypeDatetime,
                ColumnType::Decimal => mysql::r#type::TypeNewDecimal,
                ColumnType::Json => mysql::r#type::TypeJSON,
            };
            info.FieldType.SetType(tp);
            let string_type = matches!(
                column.column_type,
                ColumnType::Varchar
                    | ColumnType::Blob
                    | ColumnType::MediumBlob
                    | ColumnType::LongBlob
            );
            info.FieldType.SetCharset(
                if string_type {
                    charset::CharsetUTF8MB4
                } else {
                    charset::CharsetBin
                }
                .to_owned(),
            );
            info.FieldType.SetCollate(
                if string_type {
                    charset::charset::CollationUTF8MB4
                } else {
                    charset::charset::CollationBin
                }
                .to_owned(),
            );
            let flen = match column.column_type {
                ColumnType::Blob => 1 << 16,
                ColumnType::MediumBlob => 1 << 24,
                ColumnType::LongBlob => 1 << 32,
                _ => column.size as isize,
            };
            info.FieldType.SetFlen(flen);
            info.FieldType
                .SetDecimal(column.decimal.unwrap_or(0) as isize);
            let mut flags = 0;
            if column.unsigned {
                flags |= mysql::r#type::UnsignedFlag;
            }
            if column.not_null {
                flags |= mysql::r#type::NotNullFlag;
            }
            if column.primary_key {
                flags |= mysql::r#type::PriKeyFlag;
            }
            if column.binary {
                flags |= mysql::r#type::BinaryFlag;
            }
            info.FieldType.SetFlag(flags);
            info.Offset = offset as isize;
            info.State = model_dependency::StatePublic;
            info.Comment = column.comment.to_owned();
            info.DefaultValue = column
                .default_value
                .map(|value| model_dependency::DefaultValue::String(value.as_bytes().to_vec()));
            info
        })
        .collect();
    let model = model_dependency::TableInfo {
        ID: id,
        DBID: astersql_meta_autoid::INFORMATION_SCHEMA_DB_ID,
        Name: astersql_parser_ast::NewCIStr(table_name),
        State: model_dependency::StatePublic,
        Charset: mysql::charset::DefaultCharset.to_owned(),
        Collate: mysql::charset::DefaultCollationName.to_owned(),
        Columns: model_columns,
        ..Default::default()
    };
    TableInfo {
        id,
        db_id: astersql_meta_autoid::INFORMATION_SCHEMA_DB_ID,
        name: CiString::new(table_name),
        columns: columns
            .iter()
            .enumerate()
            .map(|(index, column)| buildColumnInfo(index as i64 + 1, column))
            .collect(),
        model_meta: Some(Arc::new(model)),
        ..TableInfo::default()
    }
}
/// Return fresh column metadata for TIKV_STORAGE_CLASS_TRANSITIONS.
pub fn GetStorageClassTransitionsTableColumns() -> Vec<ColumnInfo> {
    let columns = &table_registry()
        .get(TableStorageClassTransitions)
        .expect("storage class transitions table is registered")
        .columns;
    buildTableMeta(TableStorageClassTransitions, columns).columns
}
/// 构造名为 INFORMATION_SCHEMA 的 DBInfo，包含全部已注册虚拟表。
pub fn information_schema_db() -> DBInfo {
    information_schema_db_with_storage_class(
        astersql_config::get_global_config().enable_storage_class,
    )
}

static INFORMATION_SCHEMA_DB: OnceLock<DBInfo> = OnceLock::new();

/// Build a schema snapshot using the storage-class setting for this instance.
pub fn information_schema_db_with_storage_class(enable_storage_class: bool) -> DBInfo {
    let mut db = INFORMATION_SCHEMA_DB
        .get_or_init(|| DBInfo {
            id: astersql_meta_autoid::INFORMATION_SCHEMA_DB_ID,
            name: CiString::new("INFORMATION_SCHEMA"),
            tables: table_registry()
                .values()
                .map(|definition| Arc::new(buildTableMeta(definition.name, &definition.columns)))
                .collect(),
            table_name_2_id: Default::default(),
        })
        .clone();
    if !enable_storage_class {
        db.tables
            .retain(|table| table.name.original != TableStorageClassTransitions);
    }
    db
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 集群节点描述：类型、地址、状态地址、版本等。
pub struct ServerInfo {
    pub server_type: String,
    pub address: String,
    pub status_address: String,
    pub version: String,
    pub git_hash: String,
    pub start_timestamp: i64,
    pub server_id: u64,
    pub engine_role: String,
}

impl ServerInfo {
    /// 判断地址是否为回环或未指定地址（含字面 `localhost`）。
    /// Mirrors Go's `net.ResolveTCPAddr("", addr)` + `IsUnspecified`/
    /// `IsLoopback`: a bare numeric loopback/unspecified IP resolves
    /// directly, and the well-known `"localhost"` hostname always resolves
    /// to a loopback address on every platform Go targets, so it is treated
    /// as loopback here too without performing real DNS resolution.
    fn isLoopBackOrUnspecifiedAddr(&self, address: &str) -> bool {
        let Some(host) = address.split(':').next() else {
            return false;
        };
        let host = host.trim_matches(['[', ']']);
        if host.eq_ignore_ascii_case("localhost") {
            return true;
        }
        host.parse::<IpAddr>()
            .is_ok_and(|ip| ip.is_loopback() || ip.is_unspecified())
    }
    /// 当 address/status_address 恰有一侧为回环时，用另一侧主机名替换该侧主机（保留端口）。
    /// Mirrors Go's `ResolveLoopBackAddr`: when exactly one of
    /// `address`/`status_address` is loopback/unspecified, that side's host
    /// is replaced with the other side's host (its own port is preserved).
    /// If both or neither side is loopback, nothing changes.
    pub fn ResolveLoopBackAddr(&mut self) {
        let address_is_loopback = self.isLoopBackOrUnspecifiedAddr(&self.address);
        let status_is_loopback = self.isLoopBackOrUnspecifiedAddr(&self.status_address);
        if address_is_loopback && !status_is_loopback {
            if let (Some(status_host), Some(port)) = (
                self.status_address.rsplit_once(':').map(|parts| parts.0),
                self.address.rsplit_once(':').map(|parts| parts.1),
            ) {
                self.address = format!("{status_host}:{port}");
            }
        } else if !address_is_loopback && status_is_loopback {
            if let (Some(address_host), Some(port)) = (
                self.address.rsplit_once(':').map(|parts| parts.0),
                self.status_address.rsplit_once(':').map(|parts| parts.1),
            ) {
                self.status_address = format!("{address_host}:{port}");
            }
        }
    }
}

/// 集群拓扑发现：按组件类型拉取 ServerInfo 列表。
pub trait ServerDiscovery {
    fn tidb_servers(&self) -> Result<Vec<ServerInfo>, String> {
        Ok(Vec::new())
    }
    fn pd_servers(&self) -> Result<Vec<ServerInfo>, String> {
        Ok(Vec::new())
    }
    fn tso_servers(&self) -> Result<Vec<ServerInfo>, String> {
        Ok(Vec::new())
    }
    fn scheduling_servers(&self) -> Result<Vec<ServerInfo>, String> {
        Ok(Vec::new())
    }
    fn store_servers(&self) -> Result<Vec<ServerInfo>, String> {
        Ok(Vec::new())
    }
    fn tiflash_servers(&self) -> Result<Vec<ServerInfo>, String> {
        Ok(Vec::new())
    }
    fn tiproxy_servers(&self) -> Result<Vec<ServerInfo>, String> {
        Ok(Vec::new())
    }
    fn ticdc_servers(&self) -> Result<Vec<ServerInfo>, String> {
        Ok(Vec::new())
    }
}

/// 汇总各组件节点并解析回环地址后返回。
pub fn GetClusterServerInfo(discovery: &dyn ServerDiscovery) -> Result<Vec<ServerInfo>, String> {
    let mut result = Vec::new();
    result.extend(discovery.tidb_servers()?);
    result.extend(discovery.pd_servers()?);
    result.extend(discovery.store_servers()?);
    result.extend(discovery.tiflash_servers()?);
    result.extend(discovery.tiproxy_servers()?);
    result.extend(discovery.ticdc_servers()?);
    result.extend(discovery.tso_servers()?);
    result.extend(discovery.scheduling_servers()?);
    for server in &mut result {
        server.ResolveLoopBackAddr();
    }
    Ok(result)
}
/// 仅返回 TiDB 节点信息。
pub fn GetTiDBServerInfo(discovery: &dyn ServerDiscovery) -> Result<Vec<ServerInfo>, String> {
    discovery.tidb_servers()
}
/// 仅返回 PD 节点信息。
pub fn GetPDServerInfo(discovery: &dyn ServerDiscovery) -> Result<Vec<ServerInfo>, String> {
    discovery.pd_servers()
}
/// 仅返回 TSO 服务节点信息。
pub fn GetTSOServerInfo(discovery: &dyn ServerDiscovery) -> Result<Vec<ServerInfo>, String> {
    discovery.tso_servers()
}
/// 仅返回 Scheduling 服务节点信息。
pub fn GetSchedulingServerInfo(discovery: &dyn ServerDiscovery) -> Result<Vec<ServerInfo>, String> {
    discovery.scheduling_servers()
}
/// 仅返回 TiKV Store 节点信息。
pub fn GetStoreServerInfo(discovery: &dyn ServerDiscovery) -> Result<Vec<ServerInfo>, String> {
    discovery.store_servers()
}
/// 仅返回 TiFlash 节点信息。
pub fn GetTiFlashServerInfo(discovery: &dyn ServerDiscovery) -> Result<Vec<ServerInfo>, String> {
    discovery.tiflash_servers()
}
/// 仅返回 TiProxy 节点信息。
pub fn GetTiProxyServerInfo(discovery: &dyn ServerDiscovery) -> Result<Vec<ServerInfo>, String> {
    discovery.tiproxy_servers()
}
/// 仅返回 TiCDC 节点信息。
pub fn GetTiCDCServerInfo(discovery: &dyn ServerDiscovery) -> Result<Vec<ServerInfo>, String> {
    discovery.ticdc_servers()
}

/// 格式化 TiDB 版本：默认版本剥离 MySQL 兼容前缀，自定义配置原样保留。
pub fn FormatTiDBVersion(version: &str, is_default: bool) -> String {
    if !is_default {
        // The user has already set the config 'ServerVersion', just use it as-is.
        return version.to_owned();
    }
    // The default TiDBVersion is "5.7.25-TiDB-${TiDBReleaseVersion}"; strip the
    // "5.7.25-TiDB-" prefix and an optional leading "v" from the release part.
    // (`strings.SplitN(nodeVersion, "-", 2)` followed by rejoining with "-" in the
    // Go source is a no-op, so it is intentionally not reproduced here.)
    let node_version = match version.find("TiDB-") {
        Some(idx) => &version[idx + "TiDB-".len()..],
        None => "",
    };
    node_version
        .strip_prefix('v')
        .unwrap_or(node_version)
        .to_owned()
}
/// 格式化 Store（TiKV/TiFlash）版本：去掉可选的前导 `v`。
pub fn FormatStoreServerVersion(version: &str) -> String {
    version.strip_prefix('v').unwrap_or(version).to_owned()
}

/// Store 标签键值对，用于识别 TiFlash 引擎与写角色。
/// Store label pair used by TiFlash engine / role detection helpers.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StoreLabel {
    pub key: String,
    pub value: String,
}

/// Store 描述符精简版：仅含标签列表（对应 Go `*metapb.Store` 的标签部分）。
/// Minimal store descriptor for `isTiFlashStore` / `isTiFlashWriteNode`.
///
/// Go takes `*metapb.Store`; the production Rust port only needs the label
/// list those helpers inspect.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StoreInfo {
    pub labels: Vec<StoreLabel>,
}

// 与 placement 包对齐的引擎/角色标签常量，避免引入 ddl 依赖。
// Match pkg/ddl/placement constants without pulling that crate in.
const ENGINE_LABEL_KEY: &str = "engine";
const ENGINE_LABEL_TIFLASH: &str = "tiflash";
const ENGINE_ROLE_LABEL_KEY: &str = "engine_role";
const ENGINE_ROLE_LABEL_WRITE: &str = "write";

/// 若存在 `engine=tiflash` 标签则判定为 TiFlash Store。
/// Go `isTiFlashStore`: true when any label is `engine=tiflash`.
pub fn isTiFlashStore(store: &StoreInfo) -> bool {
    store
        .labels
        .iter()
        .any(|label| label.key == ENGINE_LABEL_KEY && label.value == ENGINE_LABEL_TIFLASH)
}

/// 若存在 `engine_role=write` 标签则判定为 TiFlash 写节点。
/// Go `isTiFlashWriteNode`: true when any label is `engine_role=write`.
pub fn isTiFlashWriteNode(store: &StoreInfo) -> bool {
    store
        .labels
        .iter()
        .any(|label| label.key == ENGINE_ROLE_LABEL_KEY && label.value == ENGINE_ROLE_LABEL_WRITE)
}

// Snake_case aliases used by Rust tests.
// snake_case 别名，供 Rust 测试调用。
pub fn is_tiflash_store(store: &StoreInfo) -> bool {
    isTiFlashStore(store)
}
pub fn is_tiflash_write_node(store: &StoreInfo) -> bool {
    isTiFlashWriteNode(store)
}
/// 按节点类型与地址集合过滤集群 ServerInfo。
pub fn FilterClusterServerInfo(
    servers: Vec<ServerInfo>,
    node_types: &HashSet<String>,
    addresses: &HashSet<String>,
) -> Vec<ServerInfo> {
    servers
        .into_iter()
        .filter(|server| {
            (node_types.is_empty() || node_types.contains(&server.server_type))
                && (addresses.is_empty() || addresses.contains(&server.address))
        })
        .collect()
}

#[derive(Clone, Debug)]
/// 内存虚拟表：持有 TableInfo 与行数据，拒绝增删改。
pub struct infoschemaTable {
    meta: Arc<TableInfo>,
    rows: Arc<Vec<Vec<Datum>>>,
}
impl infoschemaTable {
    /// 构造带固定行集的虚拟表。
    pub fn new(meta: TableInfo, rows: Vec<Vec<Datum>>) -> Self {
        Self {
            meta: Arc::new(meta),
            rows: Arc::new(rows),
        }
    }
    /// 迭代行；访问回调返回 false 时提前停止。
    pub fn IterRecords(&self, mut visit: impl FnMut(&[Datum]) -> bool) {
        for row in self.rows.iter() {
            if !visit(row) {
                break;
            }
        }
    }
    /// 返回全部列。
    pub fn Cols(&self) -> &[ColumnInfo] {
        &self.meta.columns
    }
    /// 可见列（虚拟表无隐藏列，等同 Cols）。
    pub fn VisibleCols(&self) -> &[ColumnInfo] {
        self.Cols()
    }
    /// 隐藏列（虚拟表恒为空）。
    pub fn HiddenCols(&self) -> &[ColumnInfo] {
        &[]
    }
    /// 返回表元数据。
    pub fn Meta(&self) -> &TableInfo {
        &self.meta
    }
    /// 物理表 ID（虚拟表即 meta.id）。
    pub fn GetPhysicalID(&self) -> i64 {
        self.meta.id
    }
    /// 拒绝写入：虚拟表不支持 AddRecord。
    pub fn AddRecord(&self) -> Result<(), &'static str> {
        Err("unsupported operation on virtual table")
    }
    /// 拒绝删除：虚拟表不支持 RemoveRecord。
    pub fn RemoveRecord(&self) -> Result<(), &'static str> {
        Err("unsupported operation on virtual table")
    }
    /// 拒绝更新：虚拟表不支持 UpdateRecord。
    pub fn UpdateRecord(&self) -> Result<(), &'static str> {
        Err("unsupported operation on virtual table")
    }
}

/// 由 TableInfo 创建 infoschema Table 包装。
pub fn createInfoSchemaTable(meta: TableInfo) -> Table {
    Table::new(meta)
}
#[derive(Default)]
/// 虚拟表标记类型（无状态）。
pub struct VirtualTable;
/// 返回与 Go `GetShardingInfo` 一致的行 ID 分片信息。
pub fn GetShardingInfo(db: &CiString, table: &TableInfo) -> Option<String> {
    if table.is_view
        || matches!(
            db.lower.as_str(),
            "information_schema"
                | "performance_schema"
                | "metrics_schema"
                | "mysql"
                | "sys"
                | "workload_schema"
        )
    {
        return None;
    }
    let Some(meta) = table.model_meta.as_deref() else {
        return Some("NOT_SHARDED".to_string());
    };
    if meta.ContainsAutoRandomBits() {
        let mut result = format!("PK_AUTO_RANDOM_BITS={}", meta.AutoRandomBits);
        if meta.AutoRandomRangeBits != 0 && meta.AutoRandomRangeBits != 64 {
            result.push_str(&format!(", RANGE BITS={}", meta.AutoRandomRangeBits));
        }
        Some(result)
    } else if meta.ShardRowIDBits > 0 {
        Some(format!("SHARD_BITS={}", meta.ShardRowIDBits))
    } else if meta.PKIsHandle {
        Some("NOT_SHARDED(PK_IS_HANDLE)".to_string())
    } else {
        Some("NOT_SHARDED".to_string())
    }
}

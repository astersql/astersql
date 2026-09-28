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
// 前半 `/* Mechanical draft */` 归档完整 Go 列定义与数据填充逻辑；
// 后半为可编译的生产端口（表注册表、ServerInfo、虚拟表读写拒绝等）。
// InfoSchema 是库表元数据的版本化快照；INFORMATION_SCHEMA 以虚拟表形式对外查询。

// Static column defs, metadata construction, and in-memory virtual table interfaces.
// 文件较大，下面按 Go 声明顺序保留函数体和表定义；Go 指针、map、slice、defer、goroutine 和外部依赖均以接线占位形式保留。

/* Mechanical draft retained for migration history.
// 以下为 Go tables.go 的机械迁移草稿归档：表名/列定义、集群信息采集与
// 各虚拟表行数据填充逻辑。不参与编译；生产实现见块外后半部分。

#![allow(dead_code, non_camel_case_types, non_snake_case, non_upper_case_globals)]

// Go imports（仅作迁移索引，未在这里中逐项改写为 Rust crate）：
// - "cmp"
// - "context"
// - "encoding/json"
// - "fmt"
// - "net"
// - "net/http"
// - "runtime"
// - "slices"
// - "sort"
// - "strconv"
// - "strings"
// - "sync"
// - "time"
// - "github.com/ngaut/pools"
// - "github.com/pingcap/errors"
// - "github.com/pingcap/failpoint"
// - "github.com/pingcap/kvproto/pkg/diagnosticspb"
// - "github.com/pingcap/kvproto/pkg/metapb"
// - "github.com/pingcap/log"
// - "github.com/pingcap/tidb/pkg/config"
// - "github.com/pingcap/tidb/pkg/ddl/placement"
// - "github.com/pingcap/tidb/pkg/ddl/resourcegroup"
// - "github.com/pingcap/tidb/pkg/domain/infosync"
// - "github.com/pingcap/tidb/pkg/kv"
// - "github.com/pingcap/tidb/pkg/meta/autoid"
// - "github.com/pingcap/tidb/pkg/meta/metadef"
// - "github.com/pingcap/tidb/pkg/meta/model"
// - "github.com/pingcap/tidb/pkg/parser/ast"
// - "github.com/pingcap/tidb/pkg/parser/auth"
// - "github.com/pingcap/tidb/pkg/parser/charset"
// - "github.com/pingcap/tidb/pkg/parser/mysql"
// - "github.com/pingcap/tidb/pkg/parser/terror"
// - "github.com/pingcap/tidb/pkg/privilege"
// - "github.com/pingcap/tidb/pkg/session/txninfo"
// - "github.com/pingcap/tidb/pkg/sessionctx"
// - "github.com/pingcap/tidb/pkg/sessionctx/variable"
// - "github.com/pingcap/tidb/pkg/table"
// - "github.com/pingcap/tidb/pkg/types"
// - "github.com/pingcap/tidb/pkg/util"
// - "github.com/pingcap/tidb/pkg/util/collate"
// - "github.com/pingcap/tidb/pkg/util/deadlockhistory"
// - "github.com/pingcap/tidb/pkg/util/execdetails"
// - "github.com/pingcap/tidb/pkg/util/logutil"
// - sem "github.com/pingcap/tidb/pkg/util/sem/compat"
// - "github.com/pingcap/tidb/pkg/util/set"
// - "github.com/pingcap/tidb/pkg/util/stmtsummary"
// - "github.com/tikv/client-go/v2/tikv"
// - pd "github.com/tikv/pd/client/http"
// - "go.uber.org/zap"
// - "google.golang.org/grpc"
// - "google.golang.org/grpc/credentials"
// - "google.golang.org/grpc/credentials/insecure"

// Go const 块迁移：保留 INFORMATION_SCHEMA 表名、列名和微服务名称常量的声明顺序。
pub const (
    // TableSchemata is the string constant of infoschema table.
    TableSchemata = "SCHEMATA"
    // TableTables is the string constant of infoschema table.
    TableTables = "TABLES"
    // TableColumns is the string constant of infoschema table
    TableColumns          = "COLUMNS"
    tableColumnStatistics = "COLUMN_STATISTICS"
    // TableStatistics is the string constant of infoschema table
    TableStatistics = "STATISTICS"
    // TableCharacterSets is the string constant of infoschema charactersets memory table
    TableCharacterSets = "CHARACTER_SETS"
    // TableCollations is the string constant of infoschema collations memory table.
    TableCollations = "COLLATIONS"
    tableFiles      = "FILES"
    // CatalogVal is the string constant of TABLE_CATALOG.
    CatalogVal = "def"
    // TableProfiling is the string constant of infoschema table.
    TableProfiling = "PROFILING"
    // TablePartitions is the string constant of infoschema table.
    TablePartitions = "PARTITIONS"
    // TableKeyColumn is the string constant of KEY_COLUMN_USAGE.
    TableKeyColumn = "KEY_COLUMN_USAGE"
    // TableReferConst is the string constant of REFERENTIAL_CONSTRAINTS.
    TableReferConst = "REFERENTIAL_CONSTRAINTS"
    tablePlugins    = "PLUGINS"
    // TableConstraints is the string constant of TABLE_CONSTRAINTS.
    TableConstraints = "TABLE_CONSTRAINTS"
    tableTriggers    = "TRIGGERS"
    // TableUserPrivileges is the string constant of infoschema user privilege table.
    TableUserPrivileges   = "USER_PRIVILEGES"
    tableSchemaPrivileges = "SCHEMA_PRIVILEGES"
    tableTablePrivileges  = "TABLE_PRIVILEGES"
    tableColumnPrivileges = "COLUMN_PRIVILEGES"
    // TableEngines is the string constant of infoschema table.
    TableEngines = "ENGINES"
    // TableViews is the string constant of infoschema table.
    TableViews          = "VIEWS"
    tableRoutines       = "ROUTINES"
    tableParameters     = "PARAMETERS"
    tableEvents         = "EVENTS"
    tableOptimizerTrace = "OPTIMIZER_TRACE"
    tableTableSpaces    = "TABLESPACES"
    // TableCollationCharacterSetApplicability is the string constant of infoschema memory table.
    TableCollationCharacterSetApplicability = "COLLATION_CHARACTER_SET_APPLICABILITY"
    // TableProcesslist is the string constant of infoschema table.
    TableProcesslist = "PROCESSLIST"
    // TableTiDBIndexes is the string constant of infoschema table
    TableTiDBIndexes = "TIDB_INDEXES"
    // TableTiDBHotRegions is the string constant of infoschema table
    TableTiDBHotRegions = "TIDB_HOT_REGIONS"
    // TableTiDBHotRegionsHistory is the string constant of infoschema table
    TableTiDBHotRegionsHistory = "TIDB_HOT_REGIONS_HISTORY"
    // TableTiKVStoreStatus is the string constant of infoschema table
    TableTiKVStoreStatus = "TIKV_STORE_STATUS"
    // TableAnalyzeStatus is the string constant of Analyze Status
    TableAnalyzeStatus = "ANALYZE_STATUS"
    // TableTiKVRegionStatus is the string constant of infoschema table
    TableTiKVRegionStatus = "TIKV_REGION_STATUS"
    // TableTiKVRegionPeers is the string constant of infoschema table
    TableTiKVRegionPeers = "TIKV_REGION_PEERS"
    // TableTiDBServersInfo is the string constant of TiDB server information table.
    TableTiDBServersInfo = "TIDB_SERVERS_INFO"
    // TableSlowQuery is the string constant of slow query memory table.
    TableSlowQuery = "SLOW_QUERY"
    // TableClusterInfo is the string constant of cluster info memory table.
    TableClusterInfo = "CLUSTER_INFO"
    // TableClusterConfig is the string constant of cluster configuration memory table.
    TableClusterConfig = "CLUSTER_CONFIG"
    // TableClusterLog is the string constant of cluster log memory table.
    TableClusterLog = "CLUSTER_LOG"
    // TableClusterLoad is the string constant of cluster load memory table.
    TableClusterLoad = "CLUSTER_LOAD"
    // TableClusterHardware is the string constant of cluster hardware table.
    TableClusterHardware = "CLUSTER_HARDWARE"
    // TableClusterSystemInfo is the string constant of cluster system info table.
    TableClusterSystemInfo = "CLUSTER_SYSTEMINFO"
    // TableTiFlashReplica is the string constant of tiflash replica table.
    TableTiFlashReplica = "TIFLASH_REPLICA"
    // TableInspectionResult is the string constant of inspection result table.
    TableInspectionResult = "INSPECTION_RESULT"
    // TableMetricTables is a table that contains all metrics table definition.
    TableMetricTables = "METRICS_TABLES"
    // TableMetricSummary is a summary table that contains all metrics.
    TableMetricSummary = "METRICS_SUMMARY"
    // TableMetricSummaryByLabel is a metric table that contains all metrics that group by label info.
    TableMetricSummaryByLabel = "METRICS_SUMMARY_BY_LABEL"
    // TableInspectionSummary is the string constant of inspection summary table.
    TableInspectionSummary = "INSPECTION_SUMMARY"
    // TableInspectionRules is the string constant of currently implemented inspection and summary rules.
    TableInspectionRules = "INSPECTION_RULES"
    // TableDDLJobs is the string constant of DDL job table.
    TableDDLJobs = "DDL_JOBS"
    // TableSequences is the string constant of all sequences created by user.
    TableSequences = "SEQUENCES"
    // TableStatementsSummary is the string constant of statement summary table.
    TableStatementsSummary = "STATEMENTS_SUMMARY"
    // TableStatementsSummaryHistory is the string constant of statements summary history table.
    TableStatementsSummaryHistory = "STATEMENTS_SUMMARY_HISTORY"
    // TableStatementsSummaryEvicted is the string constant of statements summary evicted table.
    TableStatementsSummaryEvicted = "STATEMENTS_SUMMARY_EVICTED"
    // TableTiDBStatementsStats is the string constant of the TiDB statement stats table.
    TableTiDBStatementsStats = "TIDB_STATEMENTS_STATS"
    // TableStorageStats is a table that contains all tables disk usage
    TableStorageStats = "TABLE_STORAGE_STATS"
    // TableTiFlashTables is the string constant of tiflash tables table.
    TableTiFlashTables = "TIFLASH_TABLES"
    // TableTiFlashSegments is the string constant of tiflash segments table.
    TableTiFlashSegments = "TIFLASH_SEGMENTS"
    // TableTiFlashIndexes is the string constant of tiflash indexes table.
    TableTiFlashIndexes = "TIFLASH_INDEXES"
    // TableClientErrorsSummaryGlobal is the string constant of client errors table.
    TableClientErrorsSummaryGlobal = "CLIENT_ERRORS_SUMMARY_GLOBAL"
    // TableClientErrorsSummaryByUser is the string constant of client errors table.
    TableClientErrorsSummaryByUser = "CLIENT_ERRORS_SUMMARY_BY_USER"
    // TableClientErrorsSummaryByHost is the string constant of client errors table.
    TableClientErrorsSummaryByHost = "CLIENT_ERRORS_SUMMARY_BY_HOST"
    // TableTiDBTrx is current running transaction status table.
    TableTiDBTrx = "TIDB_TRX"
    // TableDeadlocks is the string constant of deadlock table.
    TableDeadlocks = "DEADLOCKS"
    // TableDataLockWaits is current lock waiting status table.
    TableDataLockWaits = "DATA_LOCK_WAITS"
    // TableAttributes is the string constant of attributes table.
    TableAttributes = "ATTRIBUTES"
    // TablePlacementPolicies is the string constant of placement policies table.
    TablePlacementPolicies = "PLACEMENT_POLICIES"
    // TableTrxSummary is the string constant of transaction summary table.
    TableTrxSummary = "TRX_SUMMARY"
    // TableVariablesInfo is the string constant of variables_info table.
    TableVariablesInfo = "VARIABLES_INFO"
    // TableUserAttributes is the string constant of user_attributes view.
    TableUserAttributes = "USER_ATTRIBUTES"
    // TableMemoryUsage is the memory usage status of tidb instance.
    TableMemoryUsage = "MEMORY_USAGE"
    // TableMemoryUsageOpsHistory is the memory control operators history.
    TableMemoryUsageOpsHistory = "MEMORY_USAGE_OPS_HISTORY"
    // TableResourceGroups is the metadata of resource groups.
    TableResourceGroups = "RESOURCE_GROUPS"
    // TableRunawayWatches is the query list of runaway watch.
    TableRunawayWatches = "RUNAWAY_WATCHES"
    // TableCheckConstraints is the list of CHECK constraints.
    TableCheckConstraints = "CHECK_CONSTRAINTS"
    // TableTiDBCheckConstraints is the list of CHECK constraints, with non-standard TiDB extensions.
    TableTiDBCheckConstraints = "TIDB_CHECK_CONSTRAINTS"
    // TableKeywords is the list of keywords.
    TableKeywords = "KEYWORDS"
    // TableTiDBIndexUsage is a table to show the usage stats of indexes in the current instance.
    TableTiDBIndexUsage = "TIDB_INDEX_USAGE"
    // TableTiDBPlanCache is the plan cache table.
    TableTiDBPlanCache = "TIDB_PLAN_CACHE"
    // TableKeyspaceMeta is the table to show the keyspace meta.
    TableKeyspaceMeta = "KEYSPACE_META"
    // TableSchemataExtensions is the table to show read only status of database.
    TableSchemataExtensions = "SCHEMATA_EXTENSIONS"
)

// Go const 块迁移：保留 INFORMATION_SCHEMA 表名、列名和微服务名称常量的声明顺序。
pub const (
    // DataLockWaitsColumnKey is the name of the KEY column of the DATA_LOCK_WAITS table.
    DataLockWaitsColumnKey = "KEY"
    // DataLockWaitsColumnKeyInfo is the name of the KEY_INFO column of the DATA_LOCK_WAITS table.
    DataLockWaitsColumnKeyInfo = "KEY_INFO"
    // DataLockWaitsColumnTrxID is the name of the TRX_ID column of the DATA_LOCK_WAITS table.
    DataLockWaitsColumnTrxID = "TRX_ID"
    // DataLockWaitsColumnCurrentHoldingTrxID is the name of the CURRENT_HOLDING_TRX_ID column of the DATA_LOCK_WAITS table.
    DataLockWaitsColumnCurrentHoldingTrxID = "CURRENT_HOLDING_TRX_ID"
    // DataLockWaitsColumnSQLDigest is the name of the SQL_DIGEST column of the DATA_LOCK_WAITS table.
    DataLockWaitsColumnSQLDigest = "SQL_DIGEST"
    // DataLockWaitsColumnSQLDigestText is the name of the SQL_DIGEST_TEXT column of the DATA_LOCK_WAITS table.
    DataLockWaitsColumnSQLDigestText = "SQL_DIGEST_TEXT"
)

// The following variables will only be used when PD in the microservice mode.
// Go const 块迁移：保留 INFORMATION_SCHEMA 表名、列名和微服务名称常量的声明顺序。
pub const (
    // tsoServiceName is the name of TSO service.
    tsoServiceName = "tso"
    // schedulingServiceName is the name of scheduling service.
    schedulingServiceName = "scheduling"
)

// tableIDMap 对应 Go 系统表 ID 映射；ID 必须与原表名顺序稳定对应。
// 表名到稳定虚拟表 ID 的映射，供系统 schema 分配使用。
pub var tableIDMap = map[string]int64{
    TableSchemata:         autoid.InformationSchemaDBID + 1,
    TableTables:           autoid.InformationSchemaDBID + 2,
    TableColumns:          autoid.InformationSchemaDBID + 3,
    tableColumnStatistics: autoid.InformationSchemaDBID + 4,
    TableStatistics:       autoid.InformationSchemaDBID + 5,
    TableCharacterSets:    autoid.InformationSchemaDBID + 6,
    TableCollations:       autoid.InformationSchemaDBID + 7,
    tableFiles:            autoid.InformationSchemaDBID + 8,
    CatalogVal:            autoid.InformationSchemaDBID + 9,
    TableProfiling:        autoid.InformationSchemaDBID + 10,
    TablePartitions:       autoid.InformationSchemaDBID + 11,
    TableKeyColumn:        autoid.InformationSchemaDBID + 12,
    TableReferConst:       autoid.InformationSchemaDBID + 13,
    // Removed, see https://github.com/pingcap/tidb/issues/9154
    // TableSessionVar: autoid.InformationSchemaDBID + 14,
    tablePlugins:          autoid.InformationSchemaDBID + 15,
    TableConstraints:      autoid.InformationSchemaDBID + 16,
    tableTriggers:         autoid.InformationSchemaDBID + 17,
    TableUserPrivileges:   autoid.InformationSchemaDBID + 18,
    tableSchemaPrivileges: autoid.InformationSchemaDBID + 19,
    tableTablePrivileges:  autoid.InformationSchemaDBID + 20,
    tableColumnPrivileges: autoid.InformationSchemaDBID + 21,
    TableEngines:          autoid.InformationSchemaDBID + 22,
    TableViews:            autoid.InformationSchemaDBID + 23,
    tableRoutines:         autoid.InformationSchemaDBID + 24,
    tableParameters:       autoid.InformationSchemaDBID + 25,
    tableEvents:           autoid.InformationSchemaDBID + 26,
    // Removed, see https://github.com/pingcap/tidb/issues/9154
    // tableGlobalStatus: autoid.InformationSchemaDBID + 27,
    // tableGlobalVariables: autoid.InformationSchemaDBID + 28,
    // tableSessionStatus: autoid.InformationSchemaDBID + 29,
    tableOptimizerTrace:                     autoid.InformationSchemaDBID + 30,
    tableTableSpaces:                        autoid.InformationSchemaDBID + 31,
    TableCollationCharacterSetApplicability: autoid.InformationSchemaDBID + 32,
    TableProcesslist:                        autoid.InformationSchemaDBID + 33,
    TableTiDBIndexes:                        autoid.InformationSchemaDBID + 34,
    TableSlowQuery:                          autoid.InformationSchemaDBID + 35,
    TableTiDBHotRegions:                     autoid.InformationSchemaDBID + 36,
    TableTiKVStoreStatus:                    autoid.InformationSchemaDBID + 37,
    TableAnalyzeStatus:                      autoid.InformationSchemaDBID + 38,
    TableTiKVRegionStatus:                   autoid.InformationSchemaDBID + 39,
    TableTiKVRegionPeers:                    autoid.InformationSchemaDBID + 40,
    TableTiDBServersInfo:                    autoid.InformationSchemaDBID + 41,
    TableClusterInfo:                        autoid.InformationSchemaDBID + 42,
    TableClusterConfig:                      autoid.InformationSchemaDBID + 43,
    TableClusterLoad:                        autoid.InformationSchemaDBID + 44,
    TableTiFlashReplica:                     autoid.InformationSchemaDBID + 45,
    ClusterTableSlowLog:                     autoid.InformationSchemaDBID + 46,
    ClusterTableProcesslist:                 autoid.InformationSchemaDBID + 47,
    TableClusterLog:                         autoid.InformationSchemaDBID + 48,
    TableClusterHardware:                    autoid.InformationSchemaDBID + 49,
    TableClusterSystemInfo:                  autoid.InformationSchemaDBID + 50,
    TableInspectionResult:                   autoid.InformationSchemaDBID + 51,
    TableMetricSummary:                      autoid.InformationSchemaDBID + 52,
    TableMetricSummaryByLabel:               autoid.InformationSchemaDBID + 53,
    TableMetricTables:                       autoid.InformationSchemaDBID + 54,
    TableInspectionSummary:                  autoid.InformationSchemaDBID + 55,
    TableInspectionRules:                    autoid.InformationSchemaDBID + 56,
    TableDDLJobs:                            autoid.InformationSchemaDBID + 57,
    TableSequences:                          autoid.InformationSchemaDBID + 58,
    TableStatementsSummary:                  autoid.InformationSchemaDBID + 59,
    TableStatementsSummaryHistory:           autoid.InformationSchemaDBID + 60,
    ClusterTableStatementsSummary:           autoid.InformationSchemaDBID + 61,
    ClusterTableStatementsSummaryHistory:    autoid.InformationSchemaDBID + 62,
    TableStorageStats:                       autoid.InformationSchemaDBID + 63,
    TableTiFlashTables:                      autoid.InformationSchemaDBID + 64,
    TableTiFlashSegments:                    autoid.InformationSchemaDBID + 65,
    // Removed, see https://github.com/pingcap/tidb/issues/28890
    // TablePlacementPolicy: autoid.InformationSchemaDBID + 66,
    TableClientErrorsSummaryGlobal:       autoid.InformationSchemaDBID + 67,
    TableClientErrorsSummaryByUser:       autoid.InformationSchemaDBID + 68,
    TableClientErrorsSummaryByHost:       autoid.InformationSchemaDBID + 69,
    TableTiDBTrx:                         autoid.InformationSchemaDBID + 70,
    ClusterTableTiDBTrx:                  autoid.InformationSchemaDBID + 71,
    TableDeadlocks:                       autoid.InformationSchemaDBID + 72,
    ClusterTableDeadlocks:                autoid.InformationSchemaDBID + 73,
    TableDataLockWaits:                   autoid.InformationSchemaDBID + 74,
    TableStatementsSummaryEvicted:        autoid.InformationSchemaDBID + 75,
    ClusterTableStatementsSummaryEvicted: autoid.InformationSchemaDBID + 76,
    TableAttributes:                      autoid.InformationSchemaDBID + 77,
    TableTiDBHotRegionsHistory:           autoid.InformationSchemaDBID + 78,
    TablePlacementPolicies:               autoid.InformationSchemaDBID + 79,
    TableTrxSummary:                      autoid.InformationSchemaDBID + 80,
    ClusterTableTrxSummary:               autoid.InformationSchemaDBID + 81,
    TableVariablesInfo:                   autoid.InformationSchemaDBID + 82,
    TableUserAttributes:                  autoid.InformationSchemaDBID + 83,
    TableMemoryUsage:                     autoid.InformationSchemaDBID + 84,
    TableMemoryUsageOpsHistory:           autoid.InformationSchemaDBID + 85,
    ClusterTableMemoryUsage:              autoid.InformationSchemaDBID + 86,
    ClusterTableMemoryUsageOpsHistory:    autoid.InformationSchemaDBID + 87,
    TableResourceGroups:                  autoid.InformationSchemaDBID + 88,
    TableRunawayWatches:                  autoid.InformationSchemaDBID + 89,
    TableCheckConstraints:                autoid.InformationSchemaDBID + 90,
    TableTiDBCheckConstraints:            autoid.InformationSchemaDBID + 91,
    TableKeywords:                        autoid.InformationSchemaDBID + 92,
    TableTiDBIndexUsage:                  autoid.InformationSchemaDBID + 93,
    ClusterTableTiDBIndexUsage:           autoid.InformationSchemaDBID + 94,
    TableTiFlashIndexes:                  autoid.InformationSchemaDBID + 95,
    TableTiDBPlanCache:                   autoid.InformationSchemaDBID + 96,
    ClusterTableTiDBPlanCache:            autoid.InformationSchemaDBID + 97,
    TableTiDBStatementsStats:             autoid.InformationSchemaDBID + 98,
    ClusterTableTiDBStatementsStats:      autoid.InformationSchemaDBID + 99,
    TableKeyspaceMeta:                    autoid.InformationSchemaDBID + 100,
    TableSchemataExtensions:              autoid.InformationSchemaDBID + 101,
}

// columnInfo represents the basic column information of all kinds of INFORMATION_SCHEMA tables
// columnInfo 对应 Go 的列元数据临时结构，buildColumnInfo 会把它转换成 model.ColumnInfo。
/// 列描述：名称、类型、长度/精度、无符号、非空、默认值与注释。
pub struct columnInfo {
    // name of column
    name string
    // tp is column type
    tp byte
    // represent size of bytes of the column
    size int
    // represent decimal length of the column
    decimal int
    // flag represent NotNull, Unsigned, PriKey flags etc.
    flag uint
    // deflt is default value
    deflt any
    // comment for the column
    comment string
    // enumElems represent all possible literal string values of an enum column
    enumElems []string
}

// buildColumnInfo 对应 Go 元数据构造函数，保留字符集、列长度、索引和表状态的设置顺序。
// 由 columnInfo 构造 model.ColumnInfo，补齐类型标志与默认值。
pub fn buildColumnInfo(colID int64, col columnInfo) *model.ColumnInfo {
    mCharset := charset.CharsetBin
    mCollation := charset.CharsetBin
    if col.tp == mysql.TypeVarchar || col.tp == mysql.TypeMediumBlob || col.tp == mysql.TypeBlob || col.tp == mysql.TypeLongBlob || col.tp == mysql.TypeEnum {
        mCharset = charset.CharsetUTF8MB4
        mCollation = charset.CollationUTF8MB4
    }
    fieldType := types.FieldType{}
    fieldType.SetType(col.tp)
    fieldType.SetCharset(mCharset)
    fieldType.SetCollate(mCollation)
    switch col.tp {
    case mysql.TypeBlob:
        fieldType.SetFlen(1 << 16)
    case mysql.TypeMediumBlob:
        fieldType.SetFlen(1 << 24)
    case mysql.TypeLongBlob:
        fieldType.SetFlen(1 << 32)
    default:
        fieldType.SetFlen(col.size)
    }
    fieldType.SetDecimal(col.decimal)
    fieldType.SetFlag(col.flag)
    fieldType.SetElems(col.enumElems)
    return &model.ColumnInfo{
        ID:           colID,
        Name:         ast.NewCIStr(col.name),
        FieldType:    fieldType,
        State:        model.StatePublic,
        DefaultValue: col.deflt,
        Comment:      col.comment,
    }
}

// buildTableMeta 对应 Go 元数据构造函数，保留字符集、列长度、索引和表状态的设置顺序。
// 由列切片构造 INFORMATION_SCHEMA 虚拟表的 TableInfo。
pub fn buildTableMeta(tableName string, cs []columnInfo) *model.TableInfo {
    cols := make([]*model.ColumnInfo, 0, len(cs))
    primaryIndices := make([]*model.IndexInfo, 0, 1)
    tblInfo := &model.TableInfo{
        Name:    ast.NewCIStr(tableName),
        State:   model.StatePublic,
        Charset: mysql.DefaultCharset,
        Collate: mysql.DefaultCollationName,
    }
    for offset, c := range cs {
        if tblInfo.Name.O == ClusterTableSlowLog && mysql.HasPriKeyFlag(c.flag) {
            switch c.tp {
            case mysql.TypeLong, mysql.TypeLonglong,
                mysql.TypeTiny, mysql.TypeShort, mysql.TypeInt24:
                tblInfo.PKIsHandle = true
            default:
                tblInfo.IsCommonHandle = true
                tblInfo.CommonHandleVersion = 1
                index := &model.IndexInfo{
                    Name:    ast.NewCIStr("primary"),
                    State:   model.StatePublic,
                    Primary: true,
                    Unique:  true,
                    Columns: []*model.IndexColumn{
                        {Name: ast.NewCIStr(c.name), Offset: offset, Length: types.UnspecifiedLength}},
                }
                primaryIndices = append(primaryIndices, index)
                tblInfo.Indices = primaryIndices
            }
        }
        cols = append(cols, buildColumnInfo(int64(offset), c))
    }
    for i, col := range cols {
        col.Offset = i
    }
    tblInfo.Columns = cols
    return tblInfo
}

// schemataCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
// SCHEMATA 列：库目录、schema 名、默认字符集/排序规则。
pub var schemataCols = []columnInfo{
    {name: "CATALOG_NAME", tp: mysql.TypeVarchar, size: 512},
    {name: "SCHEMA_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "DEFAULT_CHARACTER_SET_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "DEFAULT_COLLATION_NAME", tp: mysql.TypeVarchar, size: 32},
    {name: "SQL_PATH", tp: mysql.TypeVarchar, size: 512},
    {name: "TIDB_PLACEMENT_POLICY_NAME", tp: mysql.TypeVarchar, size: 64},
}

// tablesCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
// TABLES 列：表基本属性（引擎、行数、创建选项等）。
pub var tablesCols = []columnInfo{
    {name: "TABLE_CATALOG", tp: mysql.TypeVarchar, size: 512},
    {name: "TABLE_SCHEMA", tp: mysql.TypeVarchar, size: 64},
    {name: "TABLE_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "TABLE_TYPE", tp: mysql.TypeVarchar, size: 64},
    {name: "ENGINE", tp: mysql.TypeVarchar, size: 64},
    {name: "VERSION", tp: mysql.TypeLonglong, size: 21},
    {name: "ROW_FORMAT", tp: mysql.TypeVarchar, size: 10},
    {name: "TABLE_ROWS", tp: mysql.TypeLonglong, size: 21},
    {name: "AVG_ROW_LENGTH", tp: mysql.TypeLonglong, size: 21},
    {name: "DATA_LENGTH", tp: mysql.TypeLonglong, size: 21},
    {name: "MAX_DATA_LENGTH", tp: mysql.TypeLonglong, size: 21},
    {name: "INDEX_LENGTH", tp: mysql.TypeLonglong, size: 21},
    {name: "DATA_FREE", tp: mysql.TypeLonglong, size: 21},
    {name: "AUTO_INCREMENT", tp: mysql.TypeLonglong, size: 21},
    {name: "CREATE_TIME", tp: mysql.TypeDatetime, size: 19},
    {name: "UPDATE_TIME", tp: mysql.TypeDatetime, size: 19},
    {name: "CHECK_TIME", tp: mysql.TypeDatetime, size: 19},
    {name: "TABLE_COLLATION", tp: mysql.TypeVarchar, size: 32, deflt: mysql.DefaultCollationName},
    {name: "CHECKSUM", tp: mysql.TypeLonglong, size: 21},
    {name: "CREATE_OPTIONS", tp: mysql.TypeVarchar, size: 255},
    {name: "TABLE_COMMENT", tp: mysql.TypeVarchar, size: 2048},
    {name: "TIDB_TABLE_ID", tp: mysql.TypeLonglong, size: 21},
    {name: "TIDB_ROW_ID_SHARDING_INFO", tp: mysql.TypeVarchar, size: 255},
    {name: "TIDB_PK_TYPE", tp: mysql.TypeVarchar, size: 64},
    {name: "TIDB_PLACEMENT_POLICY_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "TIDB_TABLE_MODE", tp: mysql.TypeVarchar, size: 16},
    {name: "TIDB_AFFINITY", tp: mysql.TypeVarchar, size: 128},
}

// See: http://dev.mysql.com/doc/refman/5.7/en/information-schema-columns-table.html
// columnsCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
// COLUMNS 列：列类型、可空、默认值、额外信息等。
pub var columnsCols = []columnInfo{
    {name: "TABLE_CATALOG", tp: mysql.TypeVarchar, size: 64},
    {name: "TABLE_SCHEMA", tp: mysql.TypeVarchar, size: 64},
    {name: "TABLE_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "COLUMN_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "ORDINAL_POSITION", tp: mysql.TypeLong, flag: mysql.UnsignedFlag},
    {name: "COLUMN_DEFAULT", tp: mysql.TypeBlob},
    {name: "IS_NULLABLE", tp: mysql.TypeVarchar, size: 3},
    {name: "DATA_TYPE", tp: mysql.TypeLongBlob},
    {name: "CHARACTER_MAXIMUM_LENGTH", tp: mysql.TypeLonglong},
    {name: "CHARACTER_OCTET_LENGTH", tp: mysql.TypeLonglong},
    {name: "NUMERIC_PRECISION", tp: mysql.TypeLonglong, flag: mysql.UnsignedFlag},
    {name: "NUMERIC_SCALE", tp: mysql.TypeLonglong, flag: mysql.UnsignedFlag},
    {name: "DATETIME_PRECISION", tp: mysql.TypeLong, flag: mysql.UnsignedFlag},
    {name: "CHARACTER_SET_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "COLLATION_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "COLUMN_TYPE", tp: mysql.TypeMediumBlob},
    {name: "COLUMN_KEY", tp: mysql.TypeVarchar, size: 3},
    {name: "EXTRA", tp: mysql.TypeVarchar, size: 256},
    {name: "PRIVILEGES", tp: mysql.TypeVarchar, size: 154},
    {name: "COLUMN_COMMENT", tp: mysql.TypeBlob},
    {name: "GENERATION_EXPRESSION", tp: mysql.TypeLongBlob, flag: mysql.NotNullFlag},
    {name: "SRS_ID", tp: mysql.TypeLong, flag: mysql.UnsignedFlag},
}

// columnStatisticsCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var columnStatisticsCols = []columnInfo{
    {name: "SCHEMA_NAME", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag},
    {name: "TABLE_NAME", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag},
    {name: "COLUMN_NAME", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag},
    {name: "HISTOGRAM", tp: mysql.TypeJSON, size: 51},
}

// statisticsCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var statisticsCols = []columnInfo{
    {name: "TABLE_CATALOG", tp: mysql.TypeVarchar, size: 512},
    {name: "TABLE_SCHEMA", tp: mysql.TypeVarchar, size: 64},
    {name: "TABLE_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "NON_UNIQUE", tp: mysql.TypeVarchar, size: 1},
    {name: "INDEX_SCHEMA", tp: mysql.TypeVarchar, size: 64},
    {name: "INDEX_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "SEQ_IN_INDEX", tp: mysql.TypeLonglong, size: 2},
    {name: "COLUMN_NAME", tp: mysql.TypeVarchar, size: 21},
    {name: "COLLATION", tp: mysql.TypeVarchar, size: 1},
    {name: "CARDINALITY", tp: mysql.TypeLonglong, size: 21},
    {name: "SUB_PART", tp: mysql.TypeLonglong, size: 3},
    {name: "PACKED", tp: mysql.TypeVarchar, size: 10},
    {name: "NULLABLE", tp: mysql.TypeVarchar, size: 3},
    {name: "INDEX_TYPE", tp: mysql.TypeVarchar, size: 16},
    {name: "COMMENT", tp: mysql.TypeVarchar, size: 16},
    {name: "INDEX_COMMENT", tp: mysql.TypeVarchar, size: 1024},
    {name: "IS_VISIBLE", tp: mysql.TypeVarchar, size: 3},
    {name: "Expression", tp: mysql.TypeVarchar, size: 64},
}

// profilingCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var profilingCols = []columnInfo{
    {name: "QUERY_ID", tp: mysql.TypeLong, size: 20},
    {name: "SEQ", tp: mysql.TypeLong, size: 20},
    {name: "STATE", tp: mysql.TypeVarchar, size: 30},
    {name: "DURATION", tp: mysql.TypeNewDecimal, size: 9},
    {name: "CPU_USER", tp: mysql.TypeNewDecimal, size: 9},
    {name: "CPU_SYSTEM", tp: mysql.TypeNewDecimal, size: 9},
    {name: "CONTEXT_VOLUNTARY", tp: mysql.TypeLong, size: 20},
    {name: "CONTEXT_INVOLUNTARY", tp: mysql.TypeLong, size: 20},
    {name: "BLOCK_OPS_IN", tp: mysql.TypeLong, size: 20},
    {name: "BLOCK_OPS_OUT", tp: mysql.TypeLong, size: 20},
    {name: "MESSAGES_SENT", tp: mysql.TypeLong, size: 20},
    {name: "MESSAGES_RECEIVED", tp: mysql.TypeLong, size: 20},
    {name: "PAGE_FAULTS_MAJOR", tp: mysql.TypeLong, size: 20},
    {name: "PAGE_FAULTS_MINOR", tp: mysql.TypeLong, size: 20},
    {name: "SWAPS", tp: mysql.TypeLong, size: 20},
    {name: "SOURCE_FUNCTION", tp: mysql.TypeVarchar, size: 30},
    {name: "SOURCE_FILE", tp: mysql.TypeVarchar, size: 20},
    {name: "SOURCE_LINE", tp: mysql.TypeLong, size: 20},
}

// charsetCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var charsetCols = []columnInfo{
    {name: "CHARACTER_SET_NAME", tp: mysql.TypeVarchar, size: 32},
    {name: "DEFAULT_COLLATE_NAME", tp: mysql.TypeVarchar, size: 32},
    {name: "DESCRIPTION", tp: mysql.TypeVarchar, size: 60},
    {name: "MAXLEN", tp: mysql.TypeLonglong, size: 3},
}

// collationsCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var collationsCols = []columnInfo{
    {name: "COLLATION_NAME", tp: mysql.TypeVarchar, size: 32},
    {name: "CHARACTER_SET_NAME", tp: mysql.TypeVarchar, size: 32},
    {name: "ID", tp: mysql.TypeLonglong, size: 11},
    {name: "IS_DEFAULT", tp: mysql.TypeVarchar, size: 3},
    {name: "IS_COMPILED", tp: mysql.TypeVarchar, size: 3},
    {name: "SORTLEN", tp: mysql.TypeLonglong, size: 3},
    {name: "PAD_ATTRIBUTE", tp: mysql.TypeVarchar, size: 9},
}

// keyColumnUsageCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var keyColumnUsageCols = []columnInfo{
    {name: "CONSTRAINT_CATALOG", tp: mysql.TypeVarchar, size: 512, flag: mysql.NotNullFlag},
    {name: "CONSTRAINT_SCHEMA", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag},
    {name: "CONSTRAINT_NAME", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag},
    {name: "TABLE_CATALOG", tp: mysql.TypeVarchar, size: 512, flag: mysql.NotNullFlag},
    {name: "TABLE_SCHEMA", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag},
    {name: "TABLE_NAME", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag},
    {name: "COLUMN_NAME", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag},
    {name: "ORDINAL_POSITION", tp: mysql.TypeLonglong, size: 10, flag: mysql.NotNullFlag},
    {name: "POSITION_IN_UNIQUE_CONSTRAINT", tp: mysql.TypeLonglong, size: 10},
    {name: "REFERENCED_TABLE_SCHEMA", tp: mysql.TypeVarchar, size: 64},
    {name: "REFERENCED_TABLE_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "REFERENCED_COLUMN_NAME", tp: mysql.TypeVarchar, size: 64},
}

// See http://dev.mysql.com/doc/refman/5.7/en/information-schema-referential-constraints-table.html
// referConstCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var referConstCols = []columnInfo{
    {name: "CONSTRAINT_CATALOG", tp: mysql.TypeVarchar, size: 512, flag: mysql.NotNullFlag},
    {name: "CONSTRAINT_SCHEMA", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag},
    {name: "CONSTRAINT_NAME", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag},
    {name: "UNIQUE_CONSTRAINT_CATALOG", tp: mysql.TypeVarchar, size: 512, flag: mysql.NotNullFlag},
    {name: "UNIQUE_CONSTRAINT_SCHEMA", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag},
    {name: "UNIQUE_CONSTRAINT_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "MATCH_OPTION", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag},
    {name: "UPDATE_RULE", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag},
    {name: "DELETE_RULE", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag},
    {name: "TABLE_NAME", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag},
    {name: "REFERENCED_TABLE_NAME", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag},
}

// See https://dev.mysql.com/doc/refman/5.7/en/information-schema-plugins-table.html
// pluginsCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var pluginsCols = []columnInfo{
    {name: "PLUGIN_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "PLUGIN_VERSION", tp: mysql.TypeVarchar, size: 20},
    {name: "PLUGIN_STATUS", tp: mysql.TypeVarchar, size: 10},
    {name: "PLUGIN_TYPE", tp: mysql.TypeVarchar, size: 80},
    {name: "PLUGIN_TYPE_VERSION", tp: mysql.TypeVarchar, size: 20},
    {name: "PLUGIN_LIBRARY", tp: mysql.TypeVarchar, size: 64},
    {name: "PLUGIN_LIBRARY_VERSION", tp: mysql.TypeVarchar, size: 20},
    {name: "PLUGIN_AUTHOR", tp: mysql.TypeVarchar, size: 64},
    {name: "PLUGIN_DESCRIPTION", tp: mysql.TypeLongBlob, size: types.UnspecifiedLength},
    {name: "PLUGIN_LICENSE", tp: mysql.TypeVarchar, size: 80},
    {name: "LOAD_OPTION", tp: mysql.TypeVarchar, size: 64},
}

// See https://dev.mysql.com/doc/refman/5.7/en/information-schema-partitions-table.html
// partitionsCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var partitionsCols = []columnInfo{
    {name: "TABLE_CATALOG", tp: mysql.TypeVarchar, size: 512},
    {name: "TABLE_SCHEMA", tp: mysql.TypeVarchar, size: 64},
    {name: "TABLE_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "PARTITION_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "SUBPARTITION_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "PARTITION_ORDINAL_POSITION", tp: mysql.TypeLonglong, size: 21},
    {name: "SUBPARTITION_ORDINAL_POSITION", tp: mysql.TypeLonglong, size: 21},
    {name: "PARTITION_METHOD", tp: mysql.TypeVarchar, size: 18},
    {name: "SUBPARTITION_METHOD", tp: mysql.TypeVarchar, size: 12},
    {name: "PARTITION_EXPRESSION", tp: mysql.TypeLongBlob, size: types.UnspecifiedLength},
    {name: "SUBPARTITION_EXPRESSION", tp: mysql.TypeLongBlob, size: types.UnspecifiedLength},
    {name: "PARTITION_DESCRIPTION", tp: mysql.TypeLongBlob, size: types.UnspecifiedLength},
    {name: "TABLE_ROWS", tp: mysql.TypeLonglong, size: 21},
    {name: "AVG_ROW_LENGTH", tp: mysql.TypeLonglong, size: 21},
    {name: "DATA_LENGTH", tp: mysql.TypeLonglong, size: 21},
    {name: "MAX_DATA_LENGTH", tp: mysql.TypeLonglong, size: 21},
    {name: "INDEX_LENGTH", tp: mysql.TypeLonglong, size: 21},
    {name: "DATA_FREE", tp: mysql.TypeLonglong, size: 21},
    {name: "CREATE_TIME", tp: mysql.TypeDatetime},
    {name: "UPDATE_TIME", tp: mysql.TypeDatetime},
    {name: "CHECK_TIME", tp: mysql.TypeDatetime},
    {name: "CHECKSUM", tp: mysql.TypeLonglong, size: 21},
    {name: "PARTITION_COMMENT", tp: mysql.TypeVarchar, size: 80},
    {name: "NODEGROUP", tp: mysql.TypeVarchar, size: 12},
    {name: "TABLESPACE_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "TIDB_PARTITION_ID", tp: mysql.TypeLonglong, size: 21},
    {name: "TIDB_PLACEMENT_POLICY_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "TIDB_AFFINITY", tp: mysql.TypeVarchar, size: 128},
}

// tableConstraintsCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableConstraintsCols = []columnInfo{
    {name: "CONSTRAINT_CATALOG", tp: mysql.TypeVarchar, size: 512},
    {name: "CONSTRAINT_SCHEMA", tp: mysql.TypeVarchar, size: 64},
    {name: "CONSTRAINT_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "TABLE_SCHEMA", tp: mysql.TypeVarchar, size: 64},
    {name: "TABLE_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "CONSTRAINT_TYPE", tp: mysql.TypeVarchar, size: 64},
}

// tableTriggersCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableTriggersCols = []columnInfo{
    {name: "TRIGGER_CATALOG", tp: mysql.TypeVarchar, size: 512},
    {name: "TRIGGER_SCHEMA", tp: mysql.TypeVarchar, size: 64},
    {name: "TRIGGER_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "EVENT_MANIPULATION", tp: mysql.TypeVarchar, size: 6},
    {name: "EVENT_OBJECT_CATALOG", tp: mysql.TypeVarchar, size: 512},
    {name: "EVENT_OBJECT_SCHEMA", tp: mysql.TypeVarchar, size: 64},
    {name: "EVENT_OBJECT_TABLE", tp: mysql.TypeVarchar, size: 64},
    {name: "ACTION_ORDER", tp: mysql.TypeLonglong, size: 4},
    {name: "ACTION_CONDITION", tp: mysql.TypeBlob, size: -1},
    {name: "ACTION_STATEMENT", tp: mysql.TypeBlob, size: -1},
    {name: "ACTION_ORIENTATION", tp: mysql.TypeVarchar, size: 9},
    {name: "ACTION_TIMING", tp: mysql.TypeVarchar, size: 6},
    {name: "ACTION_REFERENCE_OLD_TABLE", tp: mysql.TypeVarchar, size: 64},
    {name: "ACTION_REFERENCE_NEW_TABLE", tp: mysql.TypeVarchar, size: 64},
    {name: "ACTION_REFERENCE_OLD_ROW", tp: mysql.TypeVarchar, size: 3},
    {name: "ACTION_REFERENCE_NEW_ROW", tp: mysql.TypeVarchar, size: 3},
    {name: "CREATED", tp: mysql.TypeDatetime, size: 2},
    {name: "SQL_MODE", tp: mysql.TypeVarchar, size: 8192},
    {name: "DEFINER", tp: mysql.TypeVarchar, size: 77},
    {name: "CHARACTER_SET_CLIENT", tp: mysql.TypeVarchar, size: 32},
    {name: "COLLATION_CONNECTION", tp: mysql.TypeVarchar, size: 32},
    {name: "DATABASE_COLLATION", tp: mysql.TypeVarchar, size: 32},
}

// tableUserPrivilegesCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableUserPrivilegesCols = []columnInfo{
    {name: "GRANTEE", tp: mysql.TypeVarchar, size: 81},
    {name: "TABLE_CATALOG", tp: mysql.TypeVarchar, size: 512},
    {name: "PRIVILEGE_TYPE", tp: mysql.TypeVarchar, size: 64},
    {name: "IS_GRANTABLE", tp: mysql.TypeVarchar, size: 3},
}

// tableSchemaPrivilegesCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableSchemaPrivilegesCols = []columnInfo{
    {name: "GRANTEE", tp: mysql.TypeVarchar, size: 81, flag: mysql.NotNullFlag},
    {name: "TABLE_CATALOG", tp: mysql.TypeVarchar, size: 512, flag: mysql.NotNullFlag},
    {name: "TABLE_SCHEMA", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag},
    {name: "PRIVILEGE_TYPE", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag},
    {name: "IS_GRANTABLE", tp: mysql.TypeVarchar, size: 3, flag: mysql.NotNullFlag},
}

// tableTablePrivilegesCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableTablePrivilegesCols = []columnInfo{
    {name: "GRANTEE", tp: mysql.TypeVarchar, size: 81, flag: mysql.NotNullFlag},
    {name: "TABLE_CATALOG", tp: mysql.TypeVarchar, size: 512, flag: mysql.NotNullFlag},
    {name: "TABLE_SCHEMA", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag},
    {name: "TABLE_NAME", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag},
    {name: "PRIVILEGE_TYPE", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag},
    {name: "IS_GRANTABLE", tp: mysql.TypeVarchar, size: 3, flag: mysql.NotNullFlag},
}

// tableColumnPrivilegesCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableColumnPrivilegesCols = []columnInfo{
    {name: "GRANTEE", tp: mysql.TypeVarchar, size: 81, flag: mysql.NotNullFlag},
    {name: "TABLE_CATALOG", tp: mysql.TypeVarchar, size: 512, flag: mysql.NotNullFlag},
    {name: "TABLE_SCHEMA", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag},
    {name: "TABLE_NAME", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag},
    {name: "COLUMN_NAME", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag},
    {name: "PRIVILEGE_TYPE", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag},
    {name: "IS_GRANTABLE", tp: mysql.TypeVarchar, size: 3, flag: mysql.NotNullFlag},
}

// tableEnginesCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableEnginesCols = []columnInfo{
    {name: "ENGINE", tp: mysql.TypeVarchar, size: 64},
    {name: "SUPPORT", tp: mysql.TypeVarchar, size: 8},
    {name: "COMMENT", tp: mysql.TypeVarchar, size: 80},
    {name: "TRANSACTIONS", tp: mysql.TypeVarchar, size: 3},
    {name: "XA", tp: mysql.TypeVarchar, size: 3},
    {name: "SAVEPOINTS", tp: mysql.TypeVarchar, size: 3},
}

// tableViewsCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableViewsCols = []columnInfo{
    {name: "TABLE_CATALOG", tp: mysql.TypeVarchar, size: 512, flag: mysql.NotNullFlag},
    {name: "TABLE_SCHEMA", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag},
    {name: "TABLE_NAME", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag},
    {name: "VIEW_DEFINITION", tp: mysql.TypeLongBlob, flag: mysql.NotNullFlag},
    {name: "CHECK_OPTION", tp: mysql.TypeVarchar, size: 8, flag: mysql.NotNullFlag},
    {name: "IS_UPDATABLE", tp: mysql.TypeVarchar, size: 3, flag: mysql.NotNullFlag},
    {name: "DEFINER", tp: mysql.TypeVarchar, size: 77, flag: mysql.NotNullFlag},
    {name: "SECURITY_TYPE", tp: mysql.TypeVarchar, size: 7, flag: mysql.NotNullFlag},
    {name: "CHARACTER_SET_CLIENT", tp: mysql.TypeVarchar, size: 32, flag: mysql.NotNullFlag},
    {name: "COLLATION_CONNECTION", tp: mysql.TypeVarchar, size: 32, flag: mysql.NotNullFlag},
}

// tableRoutinesCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableRoutinesCols = []columnInfo{
    {name: "SPECIFIC_NAME", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag},
    {name: "ROUTINE_CATALOG", tp: mysql.TypeVarchar, size: 512, flag: mysql.NotNullFlag},
    {name: "ROUTINE_SCHEMA", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag},
    {name: "ROUTINE_NAME", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag},
    {name: "ROUTINE_TYPE", tp: mysql.TypeVarchar, size: 9, flag: mysql.NotNullFlag},
    {name: "DATA_TYPE", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag},
    {name: "CHARACTER_MAXIMUM_LENGTH", tp: mysql.TypeLong, size: 21},
    {name: "CHARACTER_OCTET_LENGTH", tp: mysql.TypeLong, size: 21},
    {name: "NUMERIC_PRECISION", tp: mysql.TypeLonglong, size: 21},
    {name: "NUMERIC_SCALE", tp: mysql.TypeLong, size: 21},
    {name: "DATETIME_PRECISION", tp: mysql.TypeLonglong, size: 21},
    {name: "CHARACTER_SET_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "COLLATION_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "DTD_IDENTIFIER", tp: mysql.TypeLongBlob},
    {name: "ROUTINE_BODY", tp: mysql.TypeVarchar, size: 8, flag: mysql.NotNullFlag},
    {name: "ROUTINE_DEFINITION", tp: mysql.TypeLongBlob},
    {name: "EXTERNAL_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "EXTERNAL_LANGUAGE", tp: mysql.TypeVarchar, size: 64},
    {name: "PARAMETER_STYLE", tp: mysql.TypeVarchar, size: 8, flag: mysql.NotNullFlag},
    {name: "IS_DETERMINISTIC", tp: mysql.TypeVarchar, size: 3, flag: mysql.NotNullFlag},
    {name: "SQL_DATA_ACCESS", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag},
    {name: "SQL_PATH", tp: mysql.TypeVarchar, size: 64},
    {name: "SECURITY_TYPE", tp: mysql.TypeVarchar, size: 7, flag: mysql.NotNullFlag},
    {name: "CREATED", tp: mysql.TypeDatetime, flag: mysql.NotNullFlag, deflt: "0000-00-00 00:00:00"},
    {name: "LAST_ALTERED", tp: mysql.TypeDatetime, flag: mysql.NotNullFlag, deflt: "0000-00-00 00:00:00"},
    {name: "SQL_MODE", tp: mysql.TypeVarchar, size: 8192, flag: mysql.NotNullFlag},
    {name: "ROUTINE_COMMENT", tp: mysql.TypeLongBlob},
    {name: "DEFINER", tp: mysql.TypeVarchar, size: 77, flag: mysql.NotNullFlag},
    {name: "CHARACTER_SET_CLIENT", tp: mysql.TypeVarchar, size: 32, flag: mysql.NotNullFlag},
    {name: "COLLATION_CONNECTION", tp: mysql.TypeVarchar, size: 32, flag: mysql.NotNullFlag},
    {name: "DATABASE_COLLATION", tp: mysql.TypeVarchar, size: 32, flag: mysql.NotNullFlag},
}

// tableParametersCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableParametersCols = []columnInfo{
    {name: "SPECIFIC_CATALOG", tp: mysql.TypeVarchar, size: 512, flag: mysql.NotNullFlag},
    {name: "SPECIFIC_SCHEMA", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag},
    {name: "SPECIFIC_NAME", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag},
    {name: "ORDINAL_POSITION", tp: mysql.TypeVarchar, size: 21, flag: mysql.NotNullFlag},
    {name: "PARAMETER_MODE", tp: mysql.TypeVarchar, size: 5},
    {name: "PARAMETER_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "DATA_TYPE", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag},
    {name: "CHARACTER_MAXIMUM_LENGTH", tp: mysql.TypeVarchar, size: 21},
    {name: "CHARACTER_OCTET_LENGTH", tp: mysql.TypeVarchar, size: 21},
    {name: "NUMERIC_PRECISION", tp: mysql.TypeVarchar, size: 21},
    {name: "NUMERIC_SCALE", tp: mysql.TypeVarchar, size: 21},
    {name: "DATETIME_PRECISION", tp: mysql.TypeVarchar, size: 21},
    {name: "CHARACTER_SET_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "COLLATION_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "DTD_IDENTIFIER", tp: mysql.TypeLongBlob, flag: mysql.NotNullFlag},
    {name: "ROUTINE_TYPE", tp: mysql.TypeVarchar, size: 9, flag: mysql.NotNullFlag},
}

// tableEventsCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableEventsCols = []columnInfo{
    {name: "EVENT_CATALOG", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag},
    {name: "EVENT_SCHEMA", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag},
    {name: "EVENT_NAME", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag},
    {name: "DEFINER", tp: mysql.TypeVarchar, size: 77, flag: mysql.NotNullFlag},
    {name: "TIME_ZONE", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag},
    {name: "EVENT_BODY", tp: mysql.TypeVarchar, size: 8, flag: mysql.NotNullFlag},
    {name: "EVENT_DEFINITION", tp: mysql.TypeLongBlob},
    {name: "EVENT_TYPE", tp: mysql.TypeVarchar, size: 9, flag: mysql.NotNullFlag},
    {name: "EXECUTE_AT", tp: mysql.TypeDatetime},
    {name: "INTERVAL_VALUE", tp: mysql.TypeVarchar, size: 256},
    {name: "INTERVAL_FIELD", tp: mysql.TypeVarchar, size: 18},
    {name: "SQL_MODE", tp: mysql.TypeVarchar, size: 8192, flag: mysql.NotNullFlag},
    {name: "STARTS", tp: mysql.TypeDatetime},
    {name: "ENDS", tp: mysql.TypeDatetime},
    {name: "STATUS", tp: mysql.TypeVarchar, size: 18, flag: mysql.NotNullFlag},
    {name: "ON_COMPLETION", tp: mysql.TypeVarchar, size: 12, flag: mysql.NotNullFlag},
    {name: "CREATED", tp: mysql.TypeDatetime, flag: mysql.NotNullFlag, deflt: "0000-00-00 00:00:00"},
    {name: "LAST_ALTERED", tp: mysql.TypeDatetime, flag: mysql.NotNullFlag, deflt: "0000-00-00 00:00:00"},
    {name: "LAST_EXECUTED", tp: mysql.TypeDatetime},
    {name: "EVENT_COMMENT", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag},
    {name: "ORIGINATOR", tp: mysql.TypeLong, size: 10, flag: mysql.NotNullFlag, deflt: 0},
    {name: "CHARACTER_SET_CLIENT", tp: mysql.TypeVarchar, size: 32, flag: mysql.NotNullFlag},
    {name: "COLLATION_CONNECTION", tp: mysql.TypeVarchar, size: 32, flag: mysql.NotNullFlag},
    {name: "DATABASE_COLLATION", tp: mysql.TypeVarchar, size: 32, flag: mysql.NotNullFlag},
}

// tableOptimizerTraceCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableOptimizerTraceCols = []columnInfo{
    {name: "QUERY", tp: mysql.TypeLongBlob, flag: mysql.NotNullFlag, deflt: ""},
    {name: "TRACE", tp: mysql.TypeLongBlob, flag: mysql.NotNullFlag, deflt: ""},
    {name: "MISSING_BYTES_BEYOND_MAX_MEM_SIZE", tp: mysql.TypeShort, size: 20, flag: mysql.NotNullFlag, deflt: 0},
    {name: "INSUFFICIENT_PRIVILEGES", tp: mysql.TypeTiny, size: 1, flag: mysql.NotNullFlag, deflt: 0},
}

// tableTableSpacesCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableTableSpacesCols = []columnInfo{
    {name: "TABLESPACE_NAME", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag, deflt: ""},
    {name: "ENGINE", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag, deflt: ""},
    {name: "TABLESPACE_TYPE", tp: mysql.TypeVarchar, size: 64},
    {name: "LOGFILE_GROUP_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "EXTENT_SIZE", tp: mysql.TypeLonglong, size: 21},
    {name: "AUTOEXTEND_SIZE", tp: mysql.TypeLonglong, size: 21},
    {name: "MAXIMUM_SIZE", tp: mysql.TypeLonglong, size: 21},
    {name: "NODEGROUP_ID", tp: mysql.TypeLonglong, size: 21},
    {name: "TABLESPACE_COMMENT", tp: mysql.TypeVarchar, size: 2048},
}

// tableCollationCharacterSetApplicabilityCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableCollationCharacterSetApplicabilityCols = []columnInfo{
    {name: "COLLATION_NAME", tp: mysql.TypeVarchar, size: 32, flag: mysql.NotNullFlag},
    {name: "CHARACTER_SET_NAME", tp: mysql.TypeVarchar, size: 32, flag: mysql.NotNullFlag},
}

// tableProcesslistCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableProcesslistCols = []columnInfo{
    {name: "ID", tp: mysql.TypeLonglong, size: 21, flag: mysql.NotNullFlag | mysql.UnsignedFlag, deflt: 0},
    {name: "USER", tp: mysql.TypeVarchar, size: 16, flag: mysql.NotNullFlag, deflt: ""},
    {name: "HOST", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag, deflt: ""},
    {name: "DB", tp: mysql.TypeVarchar, size: 64},
    {name: "COMMAND", tp: mysql.TypeVarchar, size: 16, flag: mysql.NotNullFlag, deflt: ""},
    {name: "TIME", tp: mysql.TypeLong, size: 7, flag: mysql.NotNullFlag, deflt: 0},
    {name: "STATE", tp: mysql.TypeVarchar, size: 7},
    {name: "INFO", tp: mysql.TypeLongBlob, size: types.UnspecifiedLength},
    {name: "DIGEST", tp: mysql.TypeVarchar, size: 64, deflt: ""},
    {name: "MEM", tp: mysql.TypeLonglong, size: 21, flag: mysql.UnsignedFlag},
    {name: "MEM_ARBITRATION", tp: mysql.TypeDouble, size: 22},
    {name: "MEM_WAIT_ARBITRATE_START", tp: mysql.TypeVarchar, size: 32},
    {name: "MEM_WAIT_ARBITRATE_BYTES", tp: mysql.TypeLonglong, size: 21},
    {name: "DISK", tp: mysql.TypeLonglong, size: 21, flag: mysql.UnsignedFlag},
    {name: "TxnStart", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag, deflt: ""},
    {name: "RESOURCE_GROUP", tp: mysql.TypeVarchar, size: resourcegroup.MaxGroupNameLength, flag: mysql.NotNullFlag, deflt: ""},
    {name: "SESSION_ALIAS", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag, deflt: ""},
    {name: "ROWS_AFFECTED", tp: mysql.TypeLonglong, size: 21, flag: mysql.UnsignedFlag},
    {name: "TIDB_CPU", tp: mysql.TypeLonglong, size: 21, flag: mysql.NotNullFlag, deflt: 0},
    {name: "TIKV_CPU", tp: mysql.TypeLonglong, size: 21, flag: mysql.NotNullFlag, deflt: 0},
}

// tableTiDBIndexesCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableTiDBIndexesCols = []columnInfo{
    {name: "TABLE_SCHEMA", tp: mysql.TypeVarchar, size: 64},
    {name: "TABLE_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "NON_UNIQUE", tp: mysql.TypeLonglong, size: 21},
    {name: "KEY_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "SEQ_IN_INDEX", tp: mysql.TypeLonglong, size: 21},
    {name: "COLUMN_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "SUB_PART", tp: mysql.TypeLonglong, size: 21},
    {name: "INDEX_COMMENT", tp: mysql.TypeVarchar, size: 1024},
    {name: "Expression", tp: mysql.TypeVarchar, size: 64},
    {name: "INDEX_ID", tp: mysql.TypeLonglong, size: 21},
    {name: "IS_VISIBLE", tp: mysql.TypeVarchar, size: 64},
    {name: "CLUSTERED", tp: mysql.TypeVarchar, size: 64},
    {name: "IS_GLOBAL", tp: mysql.TypeLonglong, size: 21},
    {name: "PREDICATE", tp: mysql.TypeVarchar, size: 1024},
}

// slowQueryCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
// SLOW_QUERY 列：慢查询日志解析后的诊断字段。
pub var slowQueryCols = []columnInfo{
    {name: variable.SlowLogTimeStr, tp: mysql.TypeTimestamp, size: 26, decimal: 6, flag: mysql.PriKeyFlag | mysql.NotNullFlag | mysql.BinaryFlag},
    {name: variable.SlowLogTxnStartTSStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.UnsignedFlag},
    {name: variable.SlowLogUserStr, tp: mysql.TypeVarchar, size: 64},
    {name: variable.SlowLogHostStr, tp: mysql.TypeVarchar, size: 64},
    {name: variable.SlowLogConnIDStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.UnsignedFlag},
    {name: variable.SlowLogSessAliasStr, tp: mysql.TypeVarchar, size: 64},
    {name: variable.SlowLogExecRetryCount, tp: mysql.TypeLonglong, size: 20, flag: mysql.UnsignedFlag},
    {name: variable.SlowLogExecRetryTime, tp: mysql.TypeDouble, size: 22},
    {name: variable.SlowLogQueryTimeStr, tp: mysql.TypeDouble, size: 22},
    {name: variable.SlowLogParseTimeStr, tp: mysql.TypeDouble, size: 22},
    {name: variable.SlowLogCompileTimeStr, tp: mysql.TypeDouble, size: 22},
    {name: variable.SlowLogRewriteTimeStr, tp: mysql.TypeDouble, size: 22},
    {name: variable.SlowLogPreprocSubQueriesStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.UnsignedFlag},
    {name: variable.SlowLogPreProcSubQueryTimeStr, tp: mysql.TypeDouble, size: 22},
    {name: variable.SlowLogOptimizeTimeStr, tp: mysql.TypeDouble, size: 22},
    {name: variable.SlowLogWaitTSTimeStr, tp: mysql.TypeDouble, size: 22},
    {name: execdetails.PreWriteTimeStr, tp: mysql.TypeDouble, size: 22},
    {name: execdetails.WaitPrewriteBinlogTimeStr, tp: mysql.TypeDouble, size: 22},
    {name: execdetails.CommitTimeStr, tp: mysql.TypeDouble, size: 22},
    {name: execdetails.GetCommitTSTimeStr, tp: mysql.TypeDouble, size: 22},
    {name: execdetails.CommitBackoffTimeStr, tp: mysql.TypeDouble, size: 22},
    {name: execdetails.BackoffTypesStr, tp: mysql.TypeVarchar, size: 64},
    {name: execdetails.ResolveLockTimeStr, tp: mysql.TypeDouble, size: 22},
    {name: execdetails.LocalLatchWaitTimeStr, tp: mysql.TypeDouble, size: 22},
    {name: execdetails.WriteKeysStr, tp: mysql.TypeLonglong, size: 22},
    {name: execdetails.WriteSizeStr, tp: mysql.TypeLonglong, size: 22},
    {name: execdetails.PrewriteRegionStr, tp: mysql.TypeLonglong, size: 22},
    {name: execdetails.TxnRetryStr, tp: mysql.TypeLonglong, size: 22},
    {name: execdetails.CopTimeStr, tp: mysql.TypeDouble, size: 22},
    {name: execdetails.ProcessTimeStr, tp: mysql.TypeDouble, size: 22},
    {name: execdetails.WaitTimeStr, tp: mysql.TypeDouble, size: 22},
    {name: execdetails.BackoffTimeStr, tp: mysql.TypeDouble, size: 22},
    {name: execdetails.LockKeysTimeStr, tp: mysql.TypeDouble, size: 22},
    {name: execdetails.RequestCountStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.UnsignedFlag},
    {name: execdetails.TotalKeysStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.UnsignedFlag},
    {name: execdetails.ProcessKeysStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.UnsignedFlag},
    {name: execdetails.RocksdbDeleteSkippedCountStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.UnsignedFlag},
    {name: execdetails.RocksdbKeySkippedCountStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.UnsignedFlag},
    {name: execdetails.RocksdbBlockCacheHitCountStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.UnsignedFlag},
    {name: execdetails.RocksdbBlockReadCountStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.UnsignedFlag},
    {name: execdetails.RocksdbBlockReadByteStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.UnsignedFlag},
    {name: variable.SlowLogDBStr, tp: mysql.TypeVarchar, size: 64},
    {name: variable.SlowLogIndexNamesStr, tp: mysql.TypeVarchar, size: 100},
    {name: variable.SlowLogIsInternalStr, tp: mysql.TypeTiny, size: 1},
    {name: variable.SlowLogDigestStr, tp: mysql.TypeVarchar, size: 64},
    {name: variable.SlowLogStatsInfoStr, tp: mysql.TypeVarchar, size: 512},
    {name: variable.SlowLogCopProcAvg, tp: mysql.TypeDouble, size: 22},
    {name: variable.SlowLogCopProcP90, tp: mysql.TypeDouble, size: 22},
    {name: variable.SlowLogCopProcMax, tp: mysql.TypeDouble, size: 22},
    {name: variable.SlowLogCopProcAddr, tp: mysql.TypeVarchar, size: 64},
    {name: variable.SlowLogCopWaitAvg, tp: mysql.TypeDouble, size: 22},
    {name: variable.SlowLogCopWaitP90, tp: mysql.TypeDouble, size: 22},
    {name: variable.SlowLogCopWaitMax, tp: mysql.TypeDouble, size: 22},
    {name: variable.SlowLogCopWaitAddr, tp: mysql.TypeVarchar, size: 64},
    {name: variable.SlowLogMemMax, tp: mysql.TypeLonglong, size: 20},
    {name: variable.SlowLogMemArbitration, tp: mysql.TypeDouble, size: 22},
    {name: variable.SlowLogDiskMax, tp: mysql.TypeLonglong, size: 20},
    {name: variable.SlowLogKVTotal, tp: mysql.TypeDouble, size: 22},
    {name: variable.SlowLogPDTotal, tp: mysql.TypeDouble, size: 22},
    {name: variable.SlowLogBackoffTotal, tp: mysql.TypeDouble, size: 22},
    {name: variable.SlowLogUnpackedBytesSentTiKVTotal, tp: mysql.TypeLonglong, size: 20},
    {name: variable.SlowLogUnpackedBytesReceivedTiKVTotal, tp: mysql.TypeLonglong, size: 20},
    {name: variable.SlowLogUnpackedBytesSentTiKVCrossZone, tp: mysql.TypeLonglong, size: 20},
    {name: variable.SlowLogUnpackedBytesReceivedTiKVCrossZone, tp: mysql.TypeLonglong, size: 20},
    {name: variable.SlowLogUnpackedBytesSentTiFlashTotal, tp: mysql.TypeLonglong, size: 20},
    {name: variable.SlowLogUnpackedBytesReceivedTiFlashTotal, tp: mysql.TypeLonglong, size: 20},
    {name: variable.SlowLogUnpackedBytesSentTiFlashCrossZone, tp: mysql.TypeLonglong, size: 20},
    {name: variable.SlowLogUnpackedBytesReceivedTiFlashCrossZone, tp: mysql.TypeLonglong, size: 20},
    {name: variable.SlowLogWriteSQLRespTotal, tp: mysql.TypeDouble, size: 22},
    {name: variable.SlowLogResultRows, tp: mysql.TypeLonglong, size: 22},
    {name: variable.SlowLogWarnings, tp: mysql.TypeLongBlob, size: types.UnspecifiedLength},
    {name: variable.SlowLogBackoffDetail, tp: mysql.TypeVarchar, size: 4096},
    {name: variable.SlowLogPrepared, tp: mysql.TypeTiny, size: 1},
    {name: variable.SlowLogSucc, tp: mysql.TypeTiny, size: 1},
    {name: variable.SlowLogIsExplicitTxn, tp: mysql.TypeTiny, size: 1},
    {name: variable.SlowLogIsWriteCacheTable, tp: mysql.TypeTiny, size: 1},
    {name: variable.SlowLogPlanFromCache, tp: mysql.TypeTiny, size: 1},
    {name: variable.SlowLogPlanFromBinding, tp: mysql.TypeTiny, size: 1},
    {name: variable.SlowLogHasMoreResults, tp: mysql.TypeTiny, size: 1},
    {name: variable.SlowLogResourceGroup, tp: mysql.TypeVarchar, size: 64},
    {name: variable.SlowLogRRU, tp: mysql.TypeDouble, size: 22},
    {name: variable.SlowLogWRU, tp: mysql.TypeDouble, size: 22},
    {name: variable.SlowLogWaitRUDuration, tp: mysql.TypeDouble, size: 22},
    {name: variable.SlowLogTidbCPUUsageDuration, tp: mysql.TypeDouble, size: 22},
    {name: variable.SlowLogTikvCPUUsageDuration, tp: mysql.TypeDouble, size: 22},
    {name: variable.SlowLogStorageFromKV, tp: mysql.TypeTiny, size: 1},
    {name: variable.SlowLogStorageFromMPP, tp: mysql.TypeTiny, size: 1},
    {name: variable.SlowLogRequestUnitV2, tp: mysql.TypeDouble, size: 22},
    {name: variable.SlowLogRequestUnitV2Detail, tp: mysql.TypeLongBlob, size: types.UnspecifiedLength},
    {name: variable.SlowLogPlan, tp: mysql.TypeLongBlob, size: types.UnspecifiedLength},
    {name: variable.SlowLogPlanDigest, tp: mysql.TypeVarchar, size: 128},
    {name: variable.SlowLogBinaryPlan, tp: mysql.TypeLongBlob, size: types.UnspecifiedLength},
    {name: variable.SlowLogPrevStmt, tp: mysql.TypeLongBlob, size: types.UnspecifiedLength},
    {name: variable.SlowLogSessionConnectAttrs, tp: mysql.TypeJSON, size: types.UnspecifiedLength},
    {name: variable.SlowLogQuerySQLStr, tp: mysql.TypeLongBlob, size: types.UnspecifiedLength},
}

// TableTiDBHotRegionsCols is TiDB hot region mem table columns.
// TableTiDBHotRegionsCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var TableTiDBHotRegionsCols = []columnInfo{
    {name: "TABLE_ID", tp: mysql.TypeLonglong, size: 21},
    {name: "INDEX_ID", tp: mysql.TypeLonglong, size: 21},
    {name: "DB_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "TABLE_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "INDEX_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "REGION_ID", tp: mysql.TypeLonglong, size: 21},
    {name: "TYPE", tp: mysql.TypeVarchar, size: 64},
    {name: "MAX_HOT_DEGREE", tp: mysql.TypeLonglong, size: 21},
    {name: "REGION_COUNT", tp: mysql.TypeLonglong, size: 21},
    {name: "FLOW_BYTES", tp: mysql.TypeLonglong, size: 21},
}

// TableTiDBHotRegionsHistoryCols is TiDB hot region history mem table columns.
// TableTiDBHotRegionsHistoryCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var TableTiDBHotRegionsHistoryCols = []columnInfo{
    {name: "UPDATE_TIME", tp: mysql.TypeTimestamp, size: 26, decimal: 6},
    {name: "DB_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "TABLE_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "TABLE_ID", tp: mysql.TypeLonglong, size: 21},
    {name: "INDEX_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "INDEX_ID", tp: mysql.TypeLonglong, size: 21},
    {name: "REGION_ID", tp: mysql.TypeLonglong, size: 21},
    {name: "STORE_ID", tp: mysql.TypeLonglong, size: 21},
    {name: "PEER_ID", tp: mysql.TypeLonglong, size: 21},
    {name: "IS_LEARNER", tp: mysql.TypeTiny, size: 1, flag: mysql.NotNullFlag, deflt: 0},
    {name: "IS_LEADER", tp: mysql.TypeTiny, size: 1, flag: mysql.NotNullFlag, deflt: 0},
    {name: "TYPE", tp: mysql.TypeVarchar, size: 64},
    {name: "HOT_DEGREE", tp: mysql.TypeLonglong, size: 21},
    {name: "FLOW_BYTES", tp: mysql.TypeDouble, size: 22},
    {name: "KEY_RATE", tp: mysql.TypeDouble, size: 22},
    {name: "QUERY_RATE", tp: mysql.TypeDouble, size: 22},
}

// GetTableTiDBHotRegionsHistoryCols is to get TableTiDBHotRegionsHistoryCols.
// It is an optimization because Go does’t support const arrays. The solution is to use initialization functions.
// It is useful in the BCE optimization.
// https://go101.org/article/bounds-check-elimination.html
// GetTableTiDBHotRegionsHistoryCols 对应 Go 同名函数或方法，保留参数、返回值、关键分支和外部调用形状。
pub fn GetTableTiDBHotRegionsHistoryCols() []columnInfo {
    return TableTiDBHotRegionsHistoryCols
}

// TableTiKVStoreStatusCols is TiDB kv store status columns.
// TableTiKVStoreStatusCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var TableTiKVStoreStatusCols = []columnInfo{
    {name: "STORE_ID", tp: mysql.TypeLonglong, size: 21},
    {name: "ADDRESS", tp: mysql.TypeVarchar, size: 64},
    {name: "STORE_STATE", tp: mysql.TypeLonglong, size: 21},
    {name: "STORE_STATE_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "LABEL", tp: mysql.TypeJSON, size: 51},
    {name: "VERSION", tp: mysql.TypeVarchar, size: 64},
    {name: "CAPACITY", tp: mysql.TypeVarchar, size: 64},
    {name: "AVAILABLE", tp: mysql.TypeVarchar, size: 64},
    {name: "LEADER_COUNT", tp: mysql.TypeLonglong, size: 21},
    {name: "LEADER_WEIGHT", tp: mysql.TypeDouble, size: 22},
    {name: "LEADER_SCORE", tp: mysql.TypeDouble, size: 22},
    {name: "LEADER_SIZE", tp: mysql.TypeLonglong, size: 21},
    {name: "REGION_COUNT", tp: mysql.TypeLonglong, size: 21},
    {name: "REGION_WEIGHT", tp: mysql.TypeDouble, size: 22},
    {name: "REGION_SCORE", tp: mysql.TypeDouble, size: 22},
    {name: "REGION_SIZE", tp: mysql.TypeLonglong, size: 21},
    {name: "START_TS", tp: mysql.TypeDatetime},
    {name: "LAST_HEARTBEAT_TS", tp: mysql.TypeDatetime},
    {name: "UPTIME", tp: mysql.TypeVarchar, size: 64},
}

// tableAnalyzeStatusCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableAnalyzeStatusCols = []columnInfo{
    {name: "TABLE_SCHEMA", tp: mysql.TypeVarchar, size: 64},
    {name: "TABLE_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "PARTITION_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "JOB_INFO", tp: mysql.TypeLongBlob, size: types.UnspecifiedLength},
    {name: "PROCESSED_ROWS", tp: mysql.TypeLonglong, size: 21, flag: mysql.UnsignedFlag},
    {name: "START_TIME", tp: mysql.TypeDatetime},
    {name: "END_TIME", tp: mysql.TypeDatetime},
    {name: "STATE", tp: mysql.TypeVarchar, size: 64},
    {name: "FAIL_REASON", tp: mysql.TypeLongBlob, size: types.UnspecifiedLength},
    {name: "INSTANCE", tp: mysql.TypeVarchar, size: 512},
    {name: "PROCESS_ID", tp: mysql.TypeLonglong, size: 21, flag: mysql.UnsignedFlag},
    {name: "REMAINING_SECONDS", tp: mysql.TypeVarchar, size: 512},
    {name: "PROGRESS", tp: mysql.TypeDouble, size: 22, decimal: 6},
    {name: "ESTIMATED_TOTAL_ROWS", tp: mysql.TypeLonglong, size: 21, flag: mysql.UnsignedFlag},
}

// TableTiKVRegionStatusCols is TiKV region status mem table columns.
// TableTiKVRegionStatusCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var TableTiKVRegionStatusCols = []columnInfo{
    {name: "REGION_ID", tp: mysql.TypeLonglong, size: 21},
    {name: "START_KEY", tp: mysql.TypeBlob, size: types.UnspecifiedLength},
    {name: "END_KEY", tp: mysql.TypeBlob, size: types.UnspecifiedLength},
    {name: "TABLE_ID", tp: mysql.TypeLonglong, size: 21},
    {name: "DB_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "TABLE_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "IS_INDEX", tp: mysql.TypeTiny, size: 1, flag: mysql.NotNullFlag, deflt: 0},
    {name: "INDEX_ID", tp: mysql.TypeLonglong, size: 21},
    {name: "INDEX_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "IS_PARTITION", tp: mysql.TypeTiny, size: 1, flag: mysql.NotNullFlag, deflt: 0},
    {name: "PARTITION_ID", tp: mysql.TypeLonglong, size: 21},
    {name: "PARTITION_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "EPOCH_CONF_VER", tp: mysql.TypeLonglong, size: 21},
    {name: "EPOCH_VERSION", tp: mysql.TypeLonglong, size: 21},
    {name: "WRITTEN_BYTES", tp: mysql.TypeLonglong, size: 21},
    {name: "READ_BYTES", tp: mysql.TypeLonglong, size: 21},
    {name: "APPROXIMATE_SIZE", tp: mysql.TypeLonglong, size: 21},
    {name: "APPROXIMATE_KEYS", tp: mysql.TypeLonglong, size: 21},
    {name: "REPLICATIONSTATUS_STATE", tp: mysql.TypeVarchar, size: 64},
    {name: "REPLICATIONSTATUS_STATEID", tp: mysql.TypeLonglong, size: 21},
}

// TableTiKVRegionPeersCols is TiKV region peers mem table columns.
// TableTiKVRegionPeersCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var TableTiKVRegionPeersCols = []columnInfo{
    {name: "REGION_ID", tp: mysql.TypeLonglong, size: 21},
    {name: "PEER_ID", tp: mysql.TypeLonglong, size: 21},
    {name: "STORE_ID", tp: mysql.TypeLonglong, size: 21},
    {name: "IS_LEARNER", tp: mysql.TypeTiny, size: 1, flag: mysql.NotNullFlag, deflt: 0},
    {name: "IS_LEADER", tp: mysql.TypeTiny, size: 1, flag: mysql.NotNullFlag, deflt: 0},
    {name: "STATUS", tp: mysql.TypeVarchar, size: 10, deflt: 0},
    {name: "DOWN_SECONDS", tp: mysql.TypeLonglong, size: 21, deflt: 0},
}

// GetTableTiKVRegionPeersCols is to get TableTiKVRegionPeersCols.
// It is an optimization because Go does’t support const arrays. The solution is to use initialization functions.
// It is useful in the BCE optimization.
// https://go101.org/article/bounds-check-elimination.html
// GetTableTiKVRegionPeersCols 对应 Go 同名函数或方法，保留参数、返回值、关键分支和外部调用形状。
pub fn GetTableTiKVRegionPeersCols() []columnInfo {
    return TableTiKVRegionPeersCols
}

// tableTiDBServersInfoCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableTiDBServersInfoCols = []columnInfo{
    {name: "DDL_ID", tp: mysql.TypeVarchar, size: 64},
    {name: "IP", tp: mysql.TypeVarchar, size: 64},
    {name: "PORT", tp: mysql.TypeLonglong, size: 21},
    {name: "STATUS_PORT", tp: mysql.TypeLonglong, size: 21},
    {name: "LEASE", tp: mysql.TypeVarchar, size: 64},
    {name: "VERSION", tp: mysql.TypeVarchar, size: 64},
    {name: "GIT_HASH", tp: mysql.TypeVarchar, size: 64},
    {name: "LABELS", tp: mysql.TypeVarchar, size: 128},
}

// tableClusterConfigCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableClusterConfigCols = []columnInfo{
    {name: "TYPE", tp: mysql.TypeVarchar, size: 64},
    {name: "INSTANCE", tp: mysql.TypeVarchar, size: 64},
    {name: "KEY", tp: mysql.TypeVarchar, size: 256},
    {name: "VALUE", tp: mysql.TypeLongBlob, size: types.UnspecifiedLength},
}

// tableClusterLogCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableClusterLogCols = []columnInfo{
    {name: "TIME", tp: mysql.TypeVarchar, size: 32},
    {name: "TYPE", tp: mysql.TypeVarchar, size: 64},
    {name: "INSTANCE", tp: mysql.TypeVarchar, size: 64},
    {name: "LEVEL", tp: mysql.TypeVarchar, size: 8},
    {name: "MESSAGE", tp: mysql.TypeLongBlob, size: types.UnspecifiedLength},
}

// tableClusterLoadCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableClusterLoadCols = []columnInfo{
    {name: "TYPE", tp: mysql.TypeVarchar, size: 64},
    {name: "INSTANCE", tp: mysql.TypeVarchar, size: 64},
    {name: "DEVICE_TYPE", tp: mysql.TypeVarchar, size: 64},
    {name: "DEVICE_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "NAME", tp: mysql.TypeVarchar, size: 256},
    {name: "VALUE", tp: mysql.TypeVarchar, size: 128},
}

// tableClusterHardwareCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableClusterHardwareCols = []columnInfo{
    {name: "TYPE", tp: mysql.TypeVarchar, size: 64},
    {name: "INSTANCE", tp: mysql.TypeVarchar, size: 64},
    {name: "DEVICE_TYPE", tp: mysql.TypeVarchar, size: 64},
    {name: "DEVICE_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "NAME", tp: mysql.TypeVarchar, size: 256},
    {name: "VALUE", tp: mysql.TypeVarchar, size: 128},
}

// tableClusterSystemInfoCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableClusterSystemInfoCols = []columnInfo{
    {name: "TYPE", tp: mysql.TypeVarchar, size: 64},
    {name: "INSTANCE", tp: mysql.TypeVarchar, size: 64},
    {name: "SYSTEM_TYPE", tp: mysql.TypeVarchar, size: 64},
    {name: "SYSTEM_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "NAME", tp: mysql.TypeVarchar, size: 256},
    {name: "VALUE", tp: mysql.TypeVarchar, size: 128},
}

// filesCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var filesCols = []columnInfo{
    {name: "FILE_ID", tp: mysql.TypeLonglong, size: 4},
    {name: "FILE_NAME", tp: mysql.TypeVarchar, size: 4000},
    {name: "FILE_TYPE", tp: mysql.TypeVarchar, size: 20},
    {name: "TABLESPACE_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "TABLE_CATALOG", tp: mysql.TypeVarchar, size: 64},
    {name: "TABLE_SCHEMA", tp: mysql.TypeVarchar, size: 64},
    {name: "TABLE_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "LOGFILE_GROUP_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "LOGFILE_GROUP_NUMBER", tp: mysql.TypeLonglong, size: 32},
    {name: "ENGINE", tp: mysql.TypeVarchar, size: 64},
    {name: "FULLTEXT_KEYS", tp: mysql.TypeVarchar, size: 64},
    {name: "DELETED_ROWS", tp: mysql.TypeLonglong, size: 4},
    {name: "UPDATE_COUNT", tp: mysql.TypeLonglong, size: 4},
    {name: "FREE_EXTENTS", tp: mysql.TypeLonglong, size: 4},
    {name: "TOTAL_EXTENTS", tp: mysql.TypeLonglong, size: 4},
    {name: "EXTENT_SIZE", tp: mysql.TypeLonglong, size: 4},
    {name: "INITIAL_SIZE", tp: mysql.TypeLonglong, size: 21},
    {name: "MAXIMUM_SIZE", tp: mysql.TypeLonglong, size: 21},
    {name: "AUTOEXTEND_SIZE", tp: mysql.TypeLonglong, size: 21},
    {name: "CREATION_TIME", tp: mysql.TypeDatetime, size: -1},
    {name: "LAST_UPDATE_TIME", tp: mysql.TypeDatetime, size: -1},
    {name: "LAST_ACCESS_TIME", tp: mysql.TypeDatetime, size: -1},
    {name: "RECOVER_TIME", tp: mysql.TypeLonglong, size: 4},
    {name: "TRANSACTION_COUNTER", tp: mysql.TypeLonglong, size: 4},
    {name: "VERSION", tp: mysql.TypeLonglong, size: 21},
    {name: "ROW_FORMAT", tp: mysql.TypeVarchar, size: 10},
    {name: "TABLE_ROWS", tp: mysql.TypeLonglong, size: 21},
    {name: "AVG_ROW_LENGTH", tp: mysql.TypeLonglong, size: 21},
    {name: "DATA_LENGTH", tp: mysql.TypeLonglong, size: 21},
    {name: "MAX_DATA_LENGTH", tp: mysql.TypeLonglong, size: 21},
    {name: "INDEX_LENGTH", tp: mysql.TypeLonglong, size: 21},
    {name: "DATA_FREE", tp: mysql.TypeLonglong, size: 21},
    {name: "CREATE_TIME", tp: mysql.TypeDatetime, size: -1},
    {name: "UPDATE_TIME", tp: mysql.TypeDatetime, size: -1},
    {name: "CHECK_TIME", tp: mysql.TypeDatetime, size: -1},
    {name: "CHECKSUM", tp: mysql.TypeLonglong, size: 21},
    {name: "STATUS", tp: mysql.TypeVarchar, size: 20},
    {name: "EXTRA", tp: mysql.TypeVarchar, size: 255},
}

// tableClusterInfoCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableClusterInfoCols = []columnInfo{
    {name: "TYPE", tp: mysql.TypeVarchar, size: 64},
    {name: "INSTANCE", tp: mysql.TypeVarchar, size: 64},
    {name: "STATUS_ADDRESS", tp: mysql.TypeVarchar, size: 64},
    {name: "VERSION", tp: mysql.TypeVarchar, size: 64},
    {name: "GIT_HASH", tp: mysql.TypeVarchar, size: 64},
    {name: "START_TIME", tp: mysql.TypeDatetime, size: 19},
    {name: "UPTIME", tp: mysql.TypeVarchar, size: 32},
    {name: "SERVER_ID", tp: mysql.TypeLonglong, size: 21, comment: "invalid if the configuration item `enable-global-kill` is set to FALSE"},
}

// tableTableTiFlashReplicaCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableTableTiFlashReplicaCols = []columnInfo{
    {name: "TABLE_SCHEMA", tp: mysql.TypeVarchar, size: 64},
    {name: "TABLE_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "TABLE_ID", tp: mysql.TypeLonglong, size: 21},
    {name: "REPLICA_COUNT", tp: mysql.TypeLonglong, size: 21},
    {name: "LOCATION_LABELS", tp: mysql.TypeVarchar, size: 64},
    {name: "AVAILABLE", tp: mysql.TypeTiny, size: 1},
    {name: "PROGRESS", tp: mysql.TypeDouble, size: 22},
}

// tableInspectionResultCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableInspectionResultCols = []columnInfo{
    {name: "RULE", tp: mysql.TypeVarchar, size: 64},
    {name: "ITEM", tp: mysql.TypeVarchar, size: 64},
    {name: "TYPE", tp: mysql.TypeVarchar, size: 64},
    {name: "INSTANCE", tp: mysql.TypeVarchar, size: 64},
    {name: "STATUS_ADDRESS", tp: mysql.TypeVarchar, size: 64},
    {name: "VALUE", tp: mysql.TypeVarchar, size: 64},
    {name: "REFERENCE", tp: mysql.TypeVarchar, size: 64},
    {name: "SEVERITY", tp: mysql.TypeVarchar, size: 64},
    {name: "DETAILS", tp: mysql.TypeVarchar, size: 256},
}

// tableInspectionSummaryCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableInspectionSummaryCols = []columnInfo{
    {name: "RULE", tp: mysql.TypeVarchar, size: 64},
    {name: "INSTANCE", tp: mysql.TypeVarchar, size: 64},
    {name: "METRICS_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "LABEL", tp: mysql.TypeVarchar, size: 64},
    {name: "QUANTILE", tp: mysql.TypeDouble, size: 22},
    {name: "AVG_VALUE", tp: mysql.TypeDouble, size: 22, decimal: 6},
    {name: "MIN_VALUE", tp: mysql.TypeDouble, size: 22, decimal: 6},
    {name: "MAX_VALUE", tp: mysql.TypeDouble, size: 22, decimal: 6},
    {name: "COMMENT", tp: mysql.TypeVarchar, size: 256},
}

// tableInspectionRulesCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableInspectionRulesCols = []columnInfo{
    {name: "NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "TYPE", tp: mysql.TypeVarchar, size: 64},
    {name: "COMMENT", tp: mysql.TypeVarchar, size: 256},
}

// tableMetricTablesCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableMetricTablesCols = []columnInfo{
    {name: "TABLE_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "PROMQL", tp: mysql.TypeVarchar, size: 64},
    {name: "LABELS", tp: mysql.TypeVarchar, size: 64},
    {name: "QUANTILE", tp: mysql.TypeDouble, size: 22},
    {name: "COMMENT", tp: mysql.TypeVarchar, size: 256},
}

// tableMetricSummaryCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableMetricSummaryCols = []columnInfo{
    {name: "METRICS_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "QUANTILE", tp: mysql.TypeDouble, size: 22},
    {name: "SUM_VALUE", tp: mysql.TypeDouble, size: 22, decimal: 6},
    {name: "AVG_VALUE", tp: mysql.TypeDouble, size: 22, decimal: 6},
    {name: "MIN_VALUE", tp: mysql.TypeDouble, size: 22, decimal: 6},
    {name: "MAX_VALUE", tp: mysql.TypeDouble, size: 22, decimal: 6},
    {name: "COMMENT", tp: mysql.TypeVarchar, size: 256},
}

// tableMetricSummaryByLabelCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableMetricSummaryByLabelCols = []columnInfo{
    {name: "INSTANCE", tp: mysql.TypeVarchar, size: 64},
    {name: "METRICS_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "LABEL", tp: mysql.TypeVarchar, size: 64},
    {name: "QUANTILE", tp: mysql.TypeDouble, size: 22},
    {name: "SUM_VALUE", tp: mysql.TypeDouble, size: 22, decimal: 6},
    {name: "AVG_VALUE", tp: mysql.TypeDouble, size: 22, decimal: 6},
    {name: "MIN_VALUE", tp: mysql.TypeDouble, size: 22, decimal: 6},
    {name: "MAX_VALUE", tp: mysql.TypeDouble, size: 22, decimal: 6},
    {name: "COMMENT", tp: mysql.TypeVarchar, size: 256},
}

// tableDDLJobsCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableDDLJobsCols = []columnInfo{
    {name: "JOB_ID", tp: mysql.TypeLonglong, size: 21},
    {name: "DB_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "TABLE_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "JOB_TYPE", tp: mysql.TypeVarchar, size: 64},
    {name: "SCHEMA_STATE", tp: mysql.TypeVarchar, size: 64},
    {name: "SCHEMA_ID", tp: mysql.TypeLonglong, size: 21},
    {name: "TABLE_ID", tp: mysql.TypeLonglong, size: 21},
    {name: "ROW_COUNT", tp: mysql.TypeLonglong, size: 21},
    {name: "CREATE_TIME", tp: mysql.TypeDatetime, size: 26, decimal: 6},
    {name: "START_TIME", tp: mysql.TypeDatetime, size: 26, decimal: 6},
    {name: "END_TIME", tp: mysql.TypeDatetime, size: 26, decimal: 6},
    {name: "STATE", tp: mysql.TypeVarchar, size: 64},
    {name: "QUERY", tp: mysql.TypeBlob, size: types.UnspecifiedLength},
}

// tableSequencesCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableSequencesCols = []columnInfo{
    {name: "TABLE_CATALOG", tp: mysql.TypeVarchar, size: 512, flag: mysql.NotNullFlag},
    {name: "SEQUENCE_SCHEMA", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag},
    {name: "SEQUENCE_NAME", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag},
    {name: "CACHE", tp: mysql.TypeTiny, flag: mysql.NotNullFlag},
    {name: "CACHE_VALUE", tp: mysql.TypeLonglong, size: 21},
    {name: "CYCLE", tp: mysql.TypeTiny, flag: mysql.NotNullFlag},
    {name: "INCREMENT", tp: mysql.TypeLonglong, size: 21, flag: mysql.NotNullFlag},
    {name: "MAX_VALUE", tp: mysql.TypeLonglong, size: 21},
    {name: "MIN_VALUE", tp: mysql.TypeLonglong, size: 21},
    {name: "START", tp: mysql.TypeLonglong, size: 21},
    {name: "COMMENT", tp: mysql.TypeVarchar, size: 64},
}

// tableStatementsSummaryCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
// STATEMENTS_SUMMARY 列：按 digest 聚合的语句执行统计。
pub var tableStatementsSummaryCols = []columnInfo{
    {name: stmtsummary.SummaryBeginTimeStr, tp: mysql.TypeTimestamp, size: 26, flag: mysql.NotNullFlag, comment: "Begin time of this summary"},
    {name: stmtsummary.SummaryEndTimeStr, tp: mysql.TypeTimestamp, size: 26, flag: mysql.NotNullFlag, comment: "End time of this summary"},
    {name: stmtsummary.StmtTypeStr, tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag, comment: "Statement type"},
    {name: stmtsummary.SchemaNameStr, tp: mysql.TypeVarchar, size: 64, comment: "Current schema"},
    {name: stmtsummary.DigestStr, tp: mysql.TypeVarchar, size: 64},
    {name: stmtsummary.DigestTextStr, tp: mysql.TypeBlob, size: types.UnspecifiedLength, flag: mysql.NotNullFlag, comment: "Normalized statement"},
    {name: stmtsummary.TableNamesStr, tp: mysql.TypeBlob, size: types.UnspecifiedLength, comment: "Involved tables"},
    {name: stmtsummary.IndexNamesStr, tp: mysql.TypeBlob, size: types.UnspecifiedLength, comment: "Used indices"},
    {name: stmtsummary.SampleUserStr, tp: mysql.TypeVarchar, size: 64, comment: "Sampled user who executed these statements"},
    {name: stmtsummary.ExecCountStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Count of executions"},
    {name: stmtsummary.SumErrorsStr, tp: mysql.TypeLong, size: 11, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Sum of errors"},
    {name: stmtsummary.SumWarningsStr, tp: mysql.TypeLong, size: 11, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Sum of warnings"},
    {name: stmtsummary.SumLatencyStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Sum latency of these statements"},
    {name: stmtsummary.MaxLatencyStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Max latency of these statements"},
    {name: stmtsummary.MinLatencyStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Min latency of these statements"},
    {name: stmtsummary.AvgLatencyStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Average latency of these statements"},
    {name: stmtsummary.AvgParseLatencyStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Average latency of parsing"},
    {name: stmtsummary.MaxParseLatencyStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Max latency of parsing"},
    {name: stmtsummary.AvgCompileLatencyStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Average latency of compiling"},
    {name: stmtsummary.MaxCompileLatencyStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Max latency of compiling"},
    {name: stmtsummary.SumCopTaskNumStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Total number of CopTasks"},
    {name: stmtsummary.MaxCopProcessTimeStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Max processing time of CopTasks"},
    {name: stmtsummary.MaxCopProcessAddressStr, tp: mysql.TypeVarchar, size: 256, comment: "Address of the CopTask with max processing time"},
    {name: stmtsummary.MaxCopWaitTimeStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Max waiting time of CopTasks"},
    {name: stmtsummary.MaxCopWaitAddressStr, tp: mysql.TypeVarchar, size: 256, comment: "Address of the CopTask with max waiting time"},
    {name: stmtsummary.AvgProcessTimeStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Average processing time in TiKV"},
    {name: stmtsummary.MaxProcessTimeStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Max processing time in TiKV"},
    {name: stmtsummary.AvgWaitTimeStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Average waiting time in TiKV"},
    {name: stmtsummary.MaxWaitTimeStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Max waiting time in TiKV"},
    {name: stmtsummary.AvgBackoffTimeStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Average waiting time before retry"},
    {name: stmtsummary.MaxBackoffTimeStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Max waiting time before retry"},
    {name: stmtsummary.AvgTotalKeysStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Average number of scanned keys"},
    {name: stmtsummary.MaxTotalKeysStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Max number of scanned keys"},
    {name: stmtsummary.AvgProcessedKeysStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Average number of processed keys"},
    {name: stmtsummary.MaxProcessedKeysStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Max number of processed keys"},
    {name: stmtsummary.AvgRocksdbDeleteSkippedCountStr, tp: mysql.TypeDouble, size: 22, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Average number of rocksdb delete skipped count"},
    {name: stmtsummary.MaxRocksdbDeleteSkippedCountStr, tp: mysql.TypeLong, size: 11, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Max number of rocksdb delete skipped count"},
    {name: stmtsummary.AvgRocksdbKeySkippedCountStr, tp: mysql.TypeDouble, size: 22, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Average number of rocksdb key skipped count"},
    {name: stmtsummary.MaxRocksdbKeySkippedCountStr, tp: mysql.TypeLong, size: 11, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Max number of rocksdb key skipped count"},
    {name: stmtsummary.AvgRocksdbBlockCacheHitCountStr, tp: mysql.TypeDouble, size: 22, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Average number of rocksdb block cache hit count"},
    {name: stmtsummary.MaxRocksdbBlockCacheHitCountStr, tp: mysql.TypeLong, size: 11, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Max number of rocksdb block cache hit count"},
    {name: stmtsummary.AvgRocksdbBlockReadCountStr, tp: mysql.TypeDouble, size: 22, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Average number of rocksdb block read count"},
    {name: stmtsummary.MaxRocksdbBlockReadCountStr, tp: mysql.TypeLong, size: 11, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Max number of rocksdb block read count"},
    {name: stmtsummary.AvgRocksdbBlockReadByteStr, tp: mysql.TypeDouble, size: 22, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Average number of rocksdb block read byte"},
    {name: stmtsummary.MaxRocksdbBlockReadByteStr, tp: mysql.TypeLong, size: 11, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Max number of rocksdb block read byte"},
    {name: stmtsummary.AvgPrewriteTimeStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Average time of prewrite phase"},
    {name: stmtsummary.MaxPrewriteTimeStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Max time of prewrite phase"},
    {name: stmtsummary.AvgCommitTimeStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Average time of commit phase"},
    {name: stmtsummary.MaxCommitTimeStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Max time of commit phase"},
    {name: stmtsummary.AvgGetCommitTsTimeStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Average time of getting commit_ts"},
    {name: stmtsummary.MaxGetCommitTsTimeStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Max time of getting commit_ts"},
    {name: stmtsummary.AvgCommitBackoffTimeStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Average time before retry during commit phase"},
    {name: stmtsummary.MaxCommitBackoffTimeStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Max time before retry during commit phase"},
    {name: stmtsummary.AvgResolveLockTimeStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Average time for resolving locks"},
    {name: stmtsummary.MaxResolveLockTimeStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Max time for resolving locks"},
    {name: stmtsummary.AvgLocalLatchWaitTimeStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Average waiting time of local transaction"},
    {name: stmtsummary.MaxLocalLatchWaitTimeStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Max waiting time of local transaction"},
    {name: stmtsummary.AvgWriteKeysStr, tp: mysql.TypeDouble, size: 22, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Average count of written keys"},
    {name: stmtsummary.MaxWriteKeysStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Max count of written keys"},
    {name: stmtsummary.AvgWriteSizeStr, tp: mysql.TypeDouble, size: 22, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Average amount of written bytes"},
    {name: stmtsummary.MaxWriteSizeStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Max amount of written bytes"},
    {name: stmtsummary.AvgPrewriteRegionsStr, tp: mysql.TypeDouble, size: 22, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Average number of involved regions in prewrite phase"},
    {name: stmtsummary.MaxPrewriteRegionsStr, tp: mysql.TypeLong, size: 11, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Max number of involved regions in prewrite phase"},
    {name: stmtsummary.AvgTxnRetryStr, tp: mysql.TypeDouble, size: 22, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Average number of transaction retries"},
    {name: stmtsummary.MaxTxnRetryStr, tp: mysql.TypeLong, size: 11, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Max number of transaction retries"},
    {name: stmtsummary.SumExecRetryStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Sum number of execution retries in pessimistic transactions"},
    {name: stmtsummary.SumExecRetryTimeStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Sum time of execution retries in pessimistic transactions"},
    {name: stmtsummary.SumBackoffTimesStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Sum of retries"},
    {name: stmtsummary.BackoffTypesStr, tp: mysql.TypeVarchar, size: 1024, comment: "Types of errors and the number of retries for each type"},
    {name: stmtsummary.AvgMemStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Average memory(byte) used"},
    {name: stmtsummary.MaxMemStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Max memory(byte) used"},
    {name: stmtsummary.AvgMemArbitrationStr, tp: mysql.TypeDouble, size: 22, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Average time of memory arbitration"},
    {name: stmtsummary.MaxMemArbitrationStr, tp: mysql.TypeDouble, size: 22, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Max time of memory arbitration"},
    {name: stmtsummary.AvgDiskStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Average disk space(byte) used"},
    {name: stmtsummary.MaxDiskStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Max disk space(byte) used"},
    {name: stmtsummary.AvgKvTimeStr, tp: mysql.TypeLonglong, size: 22, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Average time of TiKV used"},
    {name: stmtsummary.AvgPdTimeStr, tp: mysql.TypeLonglong, size: 22, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Average time of PD used"},
    {name: stmtsummary.AvgBackoffTotalTimeStr, tp: mysql.TypeLonglong, size: 22, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Average time of Backoff used"},
    {name: stmtsummary.AvgWriteSQLRespTimeStr, tp: mysql.TypeLonglong, size: 22, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Average time of write sql resp used"},
    {name: stmtsummary.AvgTidbCPUTimeStr, tp: mysql.TypeLonglong, size: 22, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Average cpu time tidb used"},
    {name: stmtsummary.AvgTikvCPUTimeStr, tp: mysql.TypeLonglong, size: 22, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Average cpu time tikv used"},
    {name: stmtsummary.MaxResultRowsStr, tp: mysql.TypeLonglong, size: 22, flag: mysql.NotNullFlag, comment: "Max count of sql result rows"},
    {name: stmtsummary.MinResultRowsStr, tp: mysql.TypeLonglong, size: 22, flag: mysql.NotNullFlag, comment: "Min count of sql result rows"},
    {name: stmtsummary.AvgResultRowsStr, tp: mysql.TypeLonglong, size: 22, flag: mysql.NotNullFlag, comment: "Average count of sql result rows"},
    {name: stmtsummary.PreparedStr, tp: mysql.TypeTiny, size: 1, flag: mysql.NotNullFlag, comment: "Whether prepared"},
    {name: stmtsummary.AvgAffectedRowsStr, tp: mysql.TypeDouble, size: 22, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Average number of rows affected"},
    {name: stmtsummary.FirstSeenStr, tp: mysql.TypeTimestamp, size: 26, flag: mysql.NotNullFlag, comment: "The time these statements are seen for the first time"},
    {name: stmtsummary.LastSeenStr, tp: mysql.TypeTimestamp, size: 26, flag: mysql.NotNullFlag, comment: "The time these statements are seen for the last time"},
    {name: stmtsummary.PlanInCacheStr, tp: mysql.TypeTiny, size: 1, flag: mysql.NotNullFlag, comment: "Whether the last statement hit plan cache"},
    {name: stmtsummary.PlanCacheHitsStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag, comment: "The number of times these statements hit plan cache"},
    {name: stmtsummary.PlanInBindingStr, tp: mysql.TypeTiny, size: 1, flag: mysql.NotNullFlag, comment: "Whether the last statement is matched with the hints in the binding"},
    {name: stmtsummary.QuerySampleTextStr, tp: mysql.TypeBlob, size: types.UnspecifiedLength, comment: "Sampled original statement"},
    {name: stmtsummary.PrevSampleTextStr, tp: mysql.TypeBlob, size: types.UnspecifiedLength, comment: "The previous statement before commit"},
    {name: stmtsummary.PlanDigestStr, tp: mysql.TypeVarchar, size: 64, comment: "Digest of its execution plan"},
    {name: stmtsummary.PlanStr, tp: mysql.TypeBlob, size: types.UnspecifiedLength, comment: "Sampled execution plan"},
    {name: stmtsummary.BinaryPlan, tp: mysql.TypeBlob, size: types.UnspecifiedLength, comment: "Sampled binary plan"},
    {name: stmtsummary.BindingDigestStr, tp: mysql.TypeVarchar, size: 64, comment: "Digest of normalized statement for bindings"},
    {name: stmtsummary.BindingDigestTextStr, tp: mysql.TypeBlob, size: types.UnspecifiedLength, flag: mysql.NotNullFlag, comment: "Normalized statement for bindings"},
    {name: stmtsummary.Charset, tp: mysql.TypeVarchar, size: 64, comment: "Sampled charset"},
    {name: stmtsummary.Collation, tp: mysql.TypeVarchar, size: 64, comment: "Sampled collation"},
    {name: stmtsummary.PlanHint, tp: mysql.TypeBlob, size: types.UnspecifiedLength, comment: "Sampled plan hint"},
    {name: stmtsummary.MaxRequestUnitReadStr, tp: mysql.TypeDouble, flag: mysql.NotNullFlag | mysql.UnsignedFlag, size: 22, comment: "Max read request-unit cost of these statements"},
    {name: stmtsummary.AvgRequestUnitReadStr, tp: mysql.TypeDouble, flag: mysql.NotNullFlag | mysql.UnsignedFlag, size: 22, comment: "Average read request-unit cost of these statements"},
    {name: stmtsummary.MaxRequestUnitWriteStr, tp: mysql.TypeDouble, flag: mysql.NotNullFlag | mysql.UnsignedFlag, size: 22, comment: "Max write request-unit cost of these statements"},
    {name: stmtsummary.AvgRequestUnitWriteStr, tp: mysql.TypeDouble, flag: mysql.NotNullFlag | mysql.UnsignedFlag, size: 22, comment: "Average write request-unit cost of these statements"},
    {name: stmtsummary.MaxQueuedRcTimeStr, tp: mysql.TypeLonglong, size: 22, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Max time of waiting for available request-units"},
    {name: stmtsummary.AvgQueuedRcTimeStr, tp: mysql.TypeLonglong, size: 22, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Average time of waiting for available request-units"},
    {name: stmtsummary.MaxRequestUnitV2Str, tp: mysql.TypeDouble, flag: mysql.NotNullFlag | mysql.UnsignedFlag, size: 22, comment: "Max request-unit v2 cost of these statements"},
    {name: stmtsummary.AvgRequestUnitV2Str, tp: mysql.TypeDouble, flag: mysql.NotNullFlag | mysql.UnsignedFlag, size: 22, comment: "Average request-unit v2 cost of these statements"},
    {name: stmtsummary.ResourceGroupName, tp: mysql.TypeVarchar, size: 64, comment: "Bind resource group name"},
    {name: stmtsummary.PlanCacheUnqualifiedStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag, comment: "The number of times that these statements are not supported by the plan cache"},
    {name: stmtsummary.PlanCacheUnqualifiedLastReasonStr, tp: mysql.TypeBlob, size: types.UnspecifiedLength, comment: "The last reason why the statement is not supported by the plan cache"},
    {name: stmtsummary.SumUnpackedBytesSentTiKVTotalStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Total bytes sent to tikv"},
    {name: stmtsummary.SumUnpackedBytesReceivedTiKVTotalStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Total bytes received from tikv"},
    {name: stmtsummary.SumUnpackedBytesSentTiKVCrossZoneStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Total bytes sent to tikv cross zone"},
    {name: stmtsummary.SumUnpackedBytesReceivedTiKVCrossZoneStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Total bytes received from tikv cross zone"},
    {name: stmtsummary.SumUnpackedBytesSentTiFlashTotalStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Total bytes sent to tiflash"},
    {name: stmtsummary.SumUnpackedBytesReceivedTiFlashTotalStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Total bytes received from tiflash"},
    {name: stmtsummary.SumUnpackedBytesSentTiFlashCrossZoneStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Total bytes sent to tiflash cross zone"},
    {name: stmtsummary.SumUnpackedBytesReceiveTiFlashCrossZoneStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Total bytes received from tiflash cross zone"},
    {name: stmtsummary.StorageKVStr, tp: mysql.TypeTiny, size: 1, flag: mysql.NotNullFlag, comment: "Whether the last statement read data from TiKV"},
    {name: stmtsummary.StorageMPPStr, tp: mysql.TypeTiny, size: 1, flag: mysql.NotNullFlag, comment: "Whether the last statement read data from TiFlash"},
}

// tableTiDBStatementsStatsCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableTiDBStatementsStatsCols = []columnInfo{
    {name: stmtsummary.StmtTypeStr, tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag, comment: "Statement type"},
    {name: stmtsummary.SchemaNameStr, tp: mysql.TypeVarchar, size: 64, comment: "Current schema"},
    {name: stmtsummary.DigestStr, tp: mysql.TypeVarchar, size: 64},
    {name: stmtsummary.DigestTextStr, tp: mysql.TypeBlob, size: types.UnspecifiedLength, flag: mysql.NotNullFlag, comment: "Normalized statement"},
    {name: stmtsummary.TableNamesStr, tp: mysql.TypeBlob, size: types.UnspecifiedLength, comment: "Involved tables"},
    {name: stmtsummary.IndexNamesStr, tp: mysql.TypeBlob, size: types.UnspecifiedLength, comment: "Used indices"},
    {name: stmtsummary.SampleUserStr, tp: mysql.TypeVarchar, size: 64, comment: "Sampled user who executed these statements"},
    {name: stmtsummary.ExecCountStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Count of executions"},
    {name: stmtsummary.ErrorsStr, tp: mysql.TypeLong, size: 11, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Sum of errors"},
    {name: stmtsummary.WarningsStr, tp: mysql.TypeLong, size: 11, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Sum of warnings"},
    {name: stmtsummary.MemStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Total memory(byte) used"},
    {name: stmtsummary.MemArbitrationStr, tp: mysql.TypeDouble, size: 22, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Total time of memory arbitration"},
    {name: stmtsummary.DiskStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Total disk space(byte) used"},
    {name: stmtsummary.TotalTimeStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Sum latency of these statements"},
    {name: stmtsummary.ParseTimeStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Total latency of parsing"},
    {name: stmtsummary.CompileTimeStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Total latency of compiling"},
    {name: stmtsummary.CopTaskNumStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Total number of CopTasks"},
    {name: stmtsummary.CopProcessTimeStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Total processing time of CopTasks"},
    {name: stmtsummary.MaxCopProcessAddressStr, tp: mysql.TypeVarchar, size: 256, comment: "Address of the CopTask with max processing time"},
    {name: stmtsummary.CopWaitTimeStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Total waiting time of CopTasks"},
    {name: stmtsummary.MaxCopWaitAddressStr, tp: mysql.TypeVarchar, size: 256, comment: "Address of the CopTask with max waiting time"},
    {name: stmtsummary.PdTimeStr, tp: mysql.TypeLonglong, size: 22, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Total time of PD used"},
    {name: stmtsummary.KvTimeStr, tp: mysql.TypeLonglong, size: 22, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Total time of TiKV used"},
    {name: stmtsummary.ProcessTimeStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Total processing time in TiKV"},
    {name: stmtsummary.WaitTimeStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Total waiting time in TiKV"},
    {name: stmtsummary.BackoffTimeStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Total waiting time before retry"},
    {name: stmtsummary.TotalKeysStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Total number of scanned keys"},
    {name: stmtsummary.ProcessedKeysStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Total number of processed keys"},
    {name: stmtsummary.RocksdbDeleteSkippedCountStr, tp: mysql.TypeDouble, size: 22, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Total RocksDB delete skipped count"},
    {name: stmtsummary.RocksdbKeySkippedCountStr, tp: mysql.TypeDouble, size: 22, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Total RocksDB key skipped count"},
    {name: stmtsummary.RocksdbBlockCacheHitCountStr, tp: mysql.TypeDouble, size: 22, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Total RocksDB block cache hit count"},
    {name: stmtsummary.RocksdbBlockReadCountStr, tp: mysql.TypeDouble, size: 22, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Total RocksDB block read count"},
    {name: stmtsummary.RocksdbBlockReadByteStr, tp: mysql.TypeDouble, size: 22, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Total RocksDB block read byte"},
    {name: stmtsummary.PrewriteTimeStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Total time of prewrite phase"},
    {name: stmtsummary.CommitTimeStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Total time of commit phase"},
    {name: stmtsummary.CommitTsTimeStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Total time of getting commit_ts"},
    {name: stmtsummary.CommitBackoffTimeStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Total time before retry during commit phase"},
    {name: stmtsummary.ResolveLockTimeStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Total time for resolving locks"},
    {name: stmtsummary.LocalLatchWaitTimeStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Total waiting time of local transaction"},
    {name: stmtsummary.WriteKeysStr, tp: mysql.TypeDouble, size: 22, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Total count of written keys"},
    {name: stmtsummary.WriteSizeStr, tp: mysql.TypeDouble, size: 22, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Total amount of written bytes"},
    {name: stmtsummary.PrewriteRegionsStr, tp: mysql.TypeDouble, size: 22, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Total number of involved regions in prewrite phase"},
    {name: stmtsummary.TxnRetryStr, tp: mysql.TypeDouble, size: 22, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Total number of transaction retries"},
    {name: stmtsummary.ExecRetryStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Sum number of execution retries in pessimistic transactions"},
    {name: stmtsummary.ExecRetryTimeStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Sum time of execution retries in pessimistic transactions"},
    {name: stmtsummary.BackoffTimesStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Sum of retries"},
    {name: stmtsummary.BackoffTypesStr, tp: mysql.TypeVarchar, size: 1024, comment: "Types of errors and the number of retries for each type"},
    {name: stmtsummary.BackoffTotalTimeStr, tp: mysql.TypeLonglong, size: 22, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Total time spent in backoff and retry"},
    {name: stmtsummary.WriteSQLRespTimeStr, tp: mysql.TypeLonglong, size: 22, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Total time used to write a response to the client."},
    {name: stmtsummary.ResultRowsStr, tp: mysql.TypeLonglong, size: 22, flag: mysql.NotNullFlag, comment: "Total count of SQL result rows"},
    {name: stmtsummary.AffectedRowsStr, tp: mysql.TypeDouble, size: 22, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Total number of rows affected"},
    {name: stmtsummary.PreparedStr, tp: mysql.TypeTiny, size: 1, flag: mysql.NotNullFlag, comment: "Whether prepared"},
    {name: stmtsummary.FirstSeenStr, tp: mysql.TypeTimestamp, size: 26, flag: mysql.NotNullFlag, comment: "The time these statements are seen for the first time"},
    {name: stmtsummary.LastSeenStr, tp: mysql.TypeTimestamp, size: 26, flag: mysql.NotNullFlag, comment: "The time these statements are seen for the last time"},
    {name: stmtsummary.PlanInCacheStr, tp: mysql.TypeTiny, size: 1, flag: mysql.NotNullFlag, comment: "Whether the last statement hit the plan cache"},
    {name: stmtsummary.PlanCacheHitsStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag, comment: "The number of times these statements hit the plan cache"},
    {name: stmtsummary.PlanInBindingStr, tp: mysql.TypeTiny, size: 1, flag: mysql.NotNullFlag, comment: "Whether the last statement is matched with the hints in the binding"},
    {name: stmtsummary.QuerySampleTextStr, tp: mysql.TypeBlob, size: types.UnspecifiedLength, comment: "Sampled original statement"},
    {name: stmtsummary.PrevSampleTextStr, tp: mysql.TypeBlob, size: types.UnspecifiedLength, comment: "The previous statement before commit"},
    {name: stmtsummary.PlanDigestStr, tp: mysql.TypeVarchar, size: 64, comment: "Digest of its execution plan"},
    {name: stmtsummary.PlanStr, tp: mysql.TypeBlob, size: types.UnspecifiedLength, comment: "Sampled execution plan"},
    {name: stmtsummary.BinaryPlan, tp: mysql.TypeBlob, size: types.UnspecifiedLength, comment: "Sampled binary plan"},
    {name: stmtsummary.BindingDigestStr, tp: mysql.TypeVarchar, size: 64, comment: "Digest of normalized statement for bindings"},
    {name: stmtsummary.BindingDigestTextStr, tp: mysql.TypeBlob, size: types.UnspecifiedLength, flag: mysql.NotNullFlag, comment: "Normalized statement for bindings"},
    {name: stmtsummary.Charset, tp: mysql.TypeVarchar, size: 64, comment: "Sampled charset"},
    {name: stmtsummary.Collation, tp: mysql.TypeVarchar, size: 64, comment: "Sampled collation"},
    {name: stmtsummary.PlanHint, tp: mysql.TypeBlob, size: types.UnspecifiedLength, comment: "Sampled plan hint"},
    {name: stmtsummary.RequestUnitReadStr, tp: mysql.TypeDouble, flag: mysql.NotNullFlag | mysql.UnsignedFlag, size: 22, comment: "Total read request-unit cost of these statements"},
    {name: stmtsummary.RequestUnitWriteStr, tp: mysql.TypeDouble, flag: mysql.NotNullFlag | mysql.UnsignedFlag, size: 22, comment: "Total write request-unit cost of these statements"},
    {name: stmtsummary.QueuedRcTimeStr, tp: mysql.TypeLonglong, size: 22, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Total time waiting for available request-units"},
    {name: stmtsummary.ResourceGroupName, tp: mysql.TypeVarchar, size: 64, comment: "Bind resource group name"},
    {name: stmtsummary.UnpackedBytesSentTiKVTotalStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Total bytes sent to tikv"},
    {name: stmtsummary.UnpackedBytesReceivedTiKVTotalStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Total bytes received from tikv"},
    {name: stmtsummary.UnpackedBytesSentTiKVCrossZoneStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Total bytes sent to tikv cross zone"},
    {name: stmtsummary.UnpackedBytesReceivedTiKVCrossZoneStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Total bytes received from tikv cross zone"},
    {name: stmtsummary.UnpackedBytesSentTiFlashTotalStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Total bytes sent to tiflash"},
    {name: stmtsummary.UnpackedBytesReceivedTiFlashTotalStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Total bytes received from tiflash"},
    {name: stmtsummary.UnpackedBytesSentTiFlashCrossZoneStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Total bytes sent to tiflash cross zone"},
    {name: stmtsummary.UnpackedBytesReceiveTiFlashCrossZoneStr, tp: mysql.TypeLonglong, size: 20, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Total bytes received from tiflash cross zone"},
    {name: stmtsummary.StorageKVStr, tp: mysql.TypeTiny, size: 1, flag: mysql.NotNullFlag, comment: "Whether the last statement read data from TiKV"},
    {name: stmtsummary.StorageMPPStr, tp: mysql.TypeTiny, size: 1, flag: mysql.NotNullFlag, comment: "Whether the last statement read data from TiFlash"},
}

// tableStorageStatsCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableStorageStatsCols = []columnInfo{
    {name: "TABLE_SCHEMA", tp: mysql.TypeVarchar, size: 64},
    {name: "TABLE_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "TABLE_ID", tp: mysql.TypeLonglong, size: 21},
    {name: "PEER_COUNT", tp: mysql.TypeLonglong, size: 21},
    {name: "REGION_COUNT", tp: mysql.TypeLonglong, size: 21, comment: "The region count of single replica of the table"},
    {name: "EMPTY_REGION_COUNT", tp: mysql.TypeLonglong, size: 21, comment: "The region count of single replica of the table"},
    {name: "TABLE_SIZE", tp: mysql.TypeLonglong, size: 21, comment: "The disk usage(MB) of single replica of the table, if the table size is empty or less than 1MB, it would show 1MB "},
    {name: "TABLE_KEYS", tp: mysql.TypeLonglong, size: 21, comment: "The count of keys of single replica of the table"},
}

// tableTableTiFlashTablesCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableTableTiFlashTablesCols = []columnInfo{
    // TiFlash DB and Table Name contains the internal KeyspaceID,
    // which is not suitable for presenting to users. Commented out.
    // {name: "DATABASE", tp: mysql.TypeVarchar, size: 64},
    // {name: "TABLE", tp: mysql.TypeVarchar, size: 64},
    {name: "TIDB_DATABASE", tp: mysql.TypeVarchar, size: 64},
    {name: "TIDB_TABLE", tp: mysql.TypeVarchar, size: 64},
    {name: "TABLE_ID", tp: mysql.TypeLonglong, size: 21},
    {name: "IS_TOMBSTONE", tp: mysql.TypeLonglong, size: 21},
    {name: "COLUMN_COUNT", tp: mysql.TypeLonglong, size: 21},
    {name: "SEGMENT_COUNT", tp: mysql.TypeLonglong, size: 21},
    {name: "TOTAL_ROWS", tp: mysql.TypeLonglong, size: 21},
    {name: "TOTAL_SIZE", tp: mysql.TypeLonglong, size: 21},
    {name: "TOTAL_DELETE_RANGES", tp: mysql.TypeLonglong, size: 21},
    {name: "DELTA_RATE_ROWS", tp: mysql.TypeDouble, size: 64},
    {name: "DELTA_RATE_SEGMENTS", tp: mysql.TypeDouble, size: 64},
    {name: "DELTA_PLACED_RATE", tp: mysql.TypeDouble, size: 64},
    {name: "DELTA_CACHE_SIZE", tp: mysql.TypeLonglong, size: 21},
    {name: "DELTA_CACHE_ALLOC_SIZE", tp: mysql.TypeLonglong, size: 21},
    {name: "DELTA_CACHE_RATE", tp: mysql.TypeDouble, size: 64},
    {name: "DELTA_CACHE_WASTED_RATE", tp: mysql.TypeDouble, size: 64},
    {name: "DELTA_INDEX_SIZE", tp: mysql.TypeLonglong, size: 21},
    {name: "AVG_SEGMENT_ROWS", tp: mysql.TypeDouble, size: 64},
    {name: "AVG_SEGMENT_SIZE", tp: mysql.TypeDouble, size: 64},
    {name: "DELTA_COUNT", tp: mysql.TypeLonglong, size: 21},
    {name: "TOTAL_DELTA_ROWS", tp: mysql.TypeLonglong, size: 21},
    {name: "TOTAL_DELTA_SIZE", tp: mysql.TypeLonglong, size: 21},
    {name: "AVG_DELTA_ROWS", tp: mysql.TypeDouble, size: 64},
    {name: "AVG_DELTA_SIZE", tp: mysql.TypeDouble, size: 64},
    {name: "AVG_DELTA_DELETE_RANGES", tp: mysql.TypeDouble, size: 64},
    {name: "STABLE_COUNT", tp: mysql.TypeLonglong, size: 21},
    {name: "TOTAL_STABLE_ROWS", tp: mysql.TypeLonglong, size: 21},
    {name: "TOTAL_STABLE_SIZE", tp: mysql.TypeLonglong, size: 21},
    {name: "TOTAL_STABLE_SIZE_ON_DISK", tp: mysql.TypeLonglong, size: 21},
    {name: "AVG_STABLE_ROWS", tp: mysql.TypeDouble, size: 64},
    {name: "AVG_STABLE_SIZE", tp: mysql.TypeDouble, size: 64},
    {name: "TOTAL_PACK_COUNT_IN_DELTA", tp: mysql.TypeLonglong, size: 21},
    {name: "MAX_PACK_COUNT_IN_DELTA", tp: mysql.TypeLonglong, size: 21},
    {name: "AVG_PACK_COUNT_IN_DELTA", tp: mysql.TypeDouble, size: 64},
    {name: "AVG_PACK_ROWS_IN_DELTA", tp: mysql.TypeDouble, size: 64},
    {name: "AVG_PACK_SIZE_IN_DELTA", tp: mysql.TypeDouble, size: 64},
    {name: "TOTAL_PACK_COUNT_IN_STABLE", tp: mysql.TypeLonglong, size: 21},
    {name: "AVG_PACK_COUNT_IN_STABLE", tp: mysql.TypeDouble, size: 64},
    {name: "AVG_PACK_ROWS_IN_STABLE", tp: mysql.TypeDouble, size: 64},
    {name: "AVG_PACK_SIZE_IN_STABLE", tp: mysql.TypeDouble, size: 64},
    {name: "STORAGE_STABLE_NUM_SNAPSHOTS", tp: mysql.TypeLonglong, size: 21},
    {name: "STORAGE_STABLE_OLDEST_SNAPSHOT_LIFETIME", tp: mysql.TypeDouble, size: 64},
    {name: "STORAGE_STABLE_OLDEST_SNAPSHOT_THREAD_ID", tp: mysql.TypeLonglong, size: 21},
    {name: "STORAGE_STABLE_OLDEST_SNAPSHOT_TRACING_ID", tp: mysql.TypeVarchar, size: 128},
    {name: "STORAGE_DELTA_NUM_SNAPSHOTS", tp: mysql.TypeLonglong, size: 21},
    {name: "STORAGE_DELTA_OLDEST_SNAPSHOT_LIFETIME", tp: mysql.TypeDouble, size: 64},
    {name: "STORAGE_DELTA_OLDEST_SNAPSHOT_THREAD_ID", tp: mysql.TypeLonglong, size: 21},
    {name: "STORAGE_DELTA_OLDEST_SNAPSHOT_TRACING_ID", tp: mysql.TypeVarchar, size: 128},
    {name: "STORAGE_META_NUM_SNAPSHOTS", tp: mysql.TypeLonglong, size: 21},
    {name: "STORAGE_META_OLDEST_SNAPSHOT_LIFETIME", tp: mysql.TypeDouble, size: 64},
    {name: "STORAGE_META_OLDEST_SNAPSHOT_THREAD_ID", tp: mysql.TypeLonglong, size: 21},
    {name: "STORAGE_META_OLDEST_SNAPSHOT_TRACING_ID", tp: mysql.TypeVarchar, size: 128},
    {name: "BACKGROUND_TASKS_LENGTH", tp: mysql.TypeLonglong, size: 21},
    {name: "TIFLASH_INSTANCE", tp: mysql.TypeVarchar, size: 64},
}

// tableTableTiFlashSegmentsCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableTableTiFlashSegmentsCols = []columnInfo{
    // TiFlash DB and Table Name contains the internal KeyspaceID,
    // which is not suitable for presenting to users. Commented out.
    // {name: "DATABASE", tp: mysql.TypeVarchar, size: 64},
    // {name: "TABLE", tp: mysql.TypeVarchar, size: 64},
    {name: "TIDB_DATABASE", tp: mysql.TypeVarchar, size: 64},
    {name: "TIDB_TABLE", tp: mysql.TypeVarchar, size: 64},
    {name: "TABLE_ID", tp: mysql.TypeLonglong, size: 21},
    {name: "IS_TOMBSTONE", tp: mysql.TypeLonglong, size: 21},
    {name: "SEGMENT_ID", tp: mysql.TypeLonglong, size: 21},
    {name: "RANGE", tp: mysql.TypeVarchar, size: 64},
    {name: "EPOCH", tp: mysql.TypeLonglong, size: 21},
    {name: "ROWS", tp: mysql.TypeLonglong, size: 21},
    {name: "SIZE", tp: mysql.TypeLonglong, size: 21},
    {name: "DELTA_RATE", tp: mysql.TypeDouble, size: 64},
    {name: "DELTA_MEMTABLE_ROWS", tp: mysql.TypeLonglong, size: 21},
    {name: "DELTA_MEMTABLE_SIZE", tp: mysql.TypeLonglong, size: 21},
    {name: "DELTA_MEMTABLE_COLUMN_FILES", tp: mysql.TypeLonglong, size: 21},
    {name: "DELTA_MEMTABLE_DELETE_RANGES", tp: mysql.TypeLonglong, size: 21},
    {name: "DELTA_PERSISTED_PAGE_ID", tp: mysql.TypeLonglong, size: 21},
    {name: "DELTA_PERSISTED_ROWS", tp: mysql.TypeLonglong, size: 21},
    {name: "DELTA_PERSISTED_SIZE", tp: mysql.TypeLonglong, size: 21},
    {name: "DELTA_PERSISTED_COLUMN_FILES", tp: mysql.TypeLonglong, size: 21},
    {name: "DELTA_PERSISTED_DELETE_RANGES", tp: mysql.TypeLonglong, size: 21},
    {name: "DELTA_CACHE_SIZE", tp: mysql.TypeLonglong, size: 21},
    {name: "DELTA_CACHE_ALLOC_SIZE", tp: mysql.TypeLonglong, size: 21},
    {name: "DELTA_INDEX_SIZE", tp: mysql.TypeLonglong, size: 21},
    {name: "STABLE_PAGE_ID", tp: mysql.TypeLonglong, size: 21},
    {name: "STABLE_ROWS", tp: mysql.TypeLonglong, size: 21},
    {name: "STABLE_SIZE", tp: mysql.TypeLonglong, size: 21},
    {name: "STABLE_DMFILES", tp: mysql.TypeLonglong, size: 21},
    {name: "STABLE_DMFILES_ID_0", tp: mysql.TypeLonglong, size: 21},
    {name: "STABLE_DMFILES_ROWS", tp: mysql.TypeLonglong, size: 21},
    {name: "STABLE_DMFILES_SIZE", tp: mysql.TypeLonglong, size: 21},
    {name: "STABLE_DMFILES_SIZE_ON_DISK", tp: mysql.TypeLonglong, size: 21},
    {name: "STABLE_DMFILES_PACKS", tp: mysql.TypeLonglong, size: 21},
    {name: "TIFLASH_INSTANCE", tp: mysql.TypeVarchar, size: 64},
}

// tableTiFlashIndexesCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableTiFlashIndexesCols = []columnInfo{
    {name: "TIDB_DATABASE", tp: mysql.TypeVarchar, size: 64},
    {name: "TIDB_TABLE", tp: mysql.TypeVarchar, size: 64},
    {name: "TABLE_ID", tp: mysql.TypeLonglong, size: 21},
    {name: "COLUMN_NAME", tp: mysql.TypeVarchar, size: 64}, // Supplied by TiDB
    {name: "INDEX_NAME", tp: mysql.TypeVarchar, size: 64},  // Supplied by TiDB
    {name: "COLUMN_ID", tp: mysql.TypeLonglong, size: 64},
    {name: "INDEX_ID", tp: mysql.TypeLonglong, size: 21},
    {name: "INDEX_KIND", tp: mysql.TypeVarchar, size: 64},
    {name: "ROWS_STABLE_INDEXED", tp: mysql.TypeLonglong, size: 64},
    {name: "ROWS_STABLE_NOT_INDEXED", tp: mysql.TypeLonglong, size: 64},
    {name: "ROWS_DELTA_INDEXED", tp: mysql.TypeLonglong, size: 64},
    {name: "ROWS_DELTA_NOT_INDEXED", tp: mysql.TypeLonglong, size: 64},
    {name: "ERROR_MESSAGE", tp: mysql.TypeVarchar, size: 1024},
    {name: "TIFLASH_INSTANCE", tp: mysql.TypeVarchar, size: 64},
}

// tableClientErrorsSummaryGlobalCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableClientErrorsSummaryGlobalCols = []columnInfo{
    {name: "ERROR_NUMBER", tp: mysql.TypeLonglong, size: 21, flag: mysql.NotNullFlag},
    {name: "ERROR_MESSAGE", tp: mysql.TypeVarchar, size: 1024, flag: mysql.NotNullFlag},
    {name: "ERROR_COUNT", tp: mysql.TypeLonglong, size: 21, flag: mysql.NotNullFlag},
    {name: "WARNING_COUNT", tp: mysql.TypeLonglong, size: 21, flag: mysql.NotNullFlag},
    {name: "FIRST_SEEN", tp: mysql.TypeTimestamp, size: 26},
    {name: "LAST_SEEN", tp: mysql.TypeTimestamp, size: 26},
}

// tableClientErrorsSummaryByUserCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableClientErrorsSummaryByUserCols = []columnInfo{
    {name: "USER", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag},
    {name: "ERROR_NUMBER", tp: mysql.TypeLonglong, size: 21, flag: mysql.NotNullFlag},
    {name: "ERROR_MESSAGE", tp: mysql.TypeVarchar, size: 1024, flag: mysql.NotNullFlag},
    {name: "ERROR_COUNT", tp: mysql.TypeLonglong, size: 21, flag: mysql.NotNullFlag},
    {name: "WARNING_COUNT", tp: mysql.TypeLonglong, size: 21, flag: mysql.NotNullFlag},
    {name: "FIRST_SEEN", tp: mysql.TypeTimestamp, size: 26},
    {name: "LAST_SEEN", tp: mysql.TypeTimestamp, size: 26},
}

// tableClientErrorsSummaryByHostCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableClientErrorsSummaryByHostCols = []columnInfo{
    {name: "HOST", tp: mysql.TypeVarchar, size: 255, flag: mysql.NotNullFlag},
    {name: "ERROR_NUMBER", tp: mysql.TypeLonglong, size: 21, flag: mysql.NotNullFlag},
    {name: "ERROR_MESSAGE", tp: mysql.TypeVarchar, size: 1024, flag: mysql.NotNullFlag},
    {name: "ERROR_COUNT", tp: mysql.TypeLonglong, size: 21, flag: mysql.NotNullFlag},
    {name: "WARNING_COUNT", tp: mysql.TypeLonglong, size: 21, flag: mysql.NotNullFlag},
    {name: "FIRST_SEEN", tp: mysql.TypeTimestamp, size: 26},
    {name: "LAST_SEEN", tp: mysql.TypeTimestamp, size: 26},
}

// tableTiDBTrxCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
// TIDB_TRX 列：当前事务状态与 SQL digest 列表。
pub var tableTiDBTrxCols = []columnInfo{
    {name: txninfo.IDStr, tp: mysql.TypeLonglong, size: 21, flag: mysql.PriKeyFlag | mysql.NotNullFlag | mysql.UnsignedFlag},
    {name: txninfo.StartTimeStr, tp: mysql.TypeTimestamp, decimal: 6, size: 26, comment: "Start time of the transaction"},
    {name: txninfo.CurrentSQLDigestStr, tp: mysql.TypeVarchar, size: 64, comment: "Digest of the sql the transaction are currently running"},
    {name: txninfo.CurrentSQLDigestTextStr, tp: mysql.TypeBlob, size: types.UnspecifiedLength, comment: "The normalized sql the transaction are currently running"},
    {name: txninfo.StateStr, tp: mysql.TypeEnum, size: 16, enumElems: txninfo.TxnRunningStateStrs, comment: "Current running state of the transaction"},
    {name: txninfo.WaitingStartTimeStr, tp: mysql.TypeTimestamp, decimal: 6, size: 26, comment: "Current lock waiting's start time"},
    {name: txninfo.MemBufferKeysStr, tp: mysql.TypeLonglong, size: 21, comment: "How many entries are in MemDB"},
    {name: txninfo.MemBufferBytesStr, tp: mysql.TypeLonglong, size: 21, comment: "MemDB used memory"},
    {name: txninfo.SessionIDStr, tp: mysql.TypeLonglong, size: 21, flag: mysql.UnsignedFlag, comment: "Which session this transaction belongs to"},
    {name: txninfo.UserStr, tp: mysql.TypeVarchar, size: 16, comment: "The user who open this session"},
    {name: txninfo.DBStr, tp: mysql.TypeVarchar, size: 64, comment: "The schema this transaction works on"},
    {name: txninfo.AllSQLDigestsStr, tp: mysql.TypeBlob, size: types.UnspecifiedLength, comment: "A list of the digests of SQL statements that the transaction has executed"},
    {name: txninfo.RelatedTableIDsStr, tp: mysql.TypeBlob, size: types.UnspecifiedLength, comment: "A list of the table IDs that the transaction has accessed"},
    {name: txninfo.WaitingTimeStr, tp: mysql.TypeDouble, size: 22, comment: "Current lock waiting time"},
}

// tableDeadlocksCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
// DEADLOCKS 列：死锁历史记录字段。
pub var tableDeadlocksCols = []columnInfo{
    {name: deadlockhistory.ColDeadlockIDStr, tp: mysql.TypeLonglong, size: 21, flag: mysql.NotNullFlag, comment: "The ID to distinguish different deadlock events"},
    {name: deadlockhistory.ColOccurTimeStr, tp: mysql.TypeTimestamp, decimal: 6, size: 26, comment: "The physical time when the deadlock occurs"},
    {name: deadlockhistory.ColRetryableStr, tp: mysql.TypeTiny, size: 1, flag: mysql.NotNullFlag, comment: "Whether the deadlock is retryable. Retryable deadlocks are usually not reported to the client"},
    {name: deadlockhistory.ColTryLockTrxIDStr, tp: mysql.TypeLonglong, size: 21, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "The transaction ID (start ts) of the transaction that's trying to acquire the lock"},
    {name: deadlockhistory.ColCurrentSQLDigestStr, tp: mysql.TypeVarchar, size: 64, comment: "The digest of the SQL that's being blocked"},
    {name: deadlockhistory.ColCurrentSQLDigestTextStr, tp: mysql.TypeBlob, size: types.UnspecifiedLength, comment: "The normalized SQL that's being blocked"},
    {name: deadlockhistory.ColKeyStr, tp: mysql.TypeBlob, size: types.UnspecifiedLength, comment: "The key on which a transaction is waiting for another"},
    {name: deadlockhistory.ColKeyInfoStr, tp: mysql.TypeBlob, size: types.UnspecifiedLength, comment: "Information of the key"},
    {name: deadlockhistory.ColTrxHoldingLockStr, tp: mysql.TypeLonglong, size: 21, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "The transaction ID (start ts) of the transaction that's currently holding the lock"},
}

// tableDataLockWaitsCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableDataLockWaitsCols = []columnInfo{
    {name: DataLockWaitsColumnKey, tp: mysql.TypeBlob, size: types.UnspecifiedLength, flag: mysql.NotNullFlag, comment: "The key that's being waiting on"},
    {name: DataLockWaitsColumnKeyInfo, tp: mysql.TypeBlob, size: types.UnspecifiedLength, comment: "Information of the key"},
    {name: DataLockWaitsColumnTrxID, tp: mysql.TypeLonglong, size: 21, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "Current transaction that's waiting for the lock"},
    {name: DataLockWaitsColumnCurrentHoldingTrxID, tp: mysql.TypeLonglong, size: 21, flag: mysql.NotNullFlag | mysql.UnsignedFlag, comment: "The transaction that's holding the lock and blocks the current transaction"},
    {name: DataLockWaitsColumnSQLDigest, tp: mysql.TypeVarchar, size: 64, comment: "Digest of the SQL that's trying to acquire the lock"},
    {name: DataLockWaitsColumnSQLDigestText, tp: mysql.TypeBlob, size: types.UnspecifiedLength, comment: "Digest of the SQL that's trying to acquire the lock"},
}

// tableStatementsSummaryEvictedCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableStatementsSummaryEvictedCols = []columnInfo{
    {name: "BEGIN_TIME", tp: mysql.TypeTimestamp, size: 26},
    {name: "END_TIME", tp: mysql.TypeTimestamp, size: 26},
    {name: "EVICTED_COUNT", tp: mysql.TypeLonglong, size: 21, flag: mysql.NotNullFlag},
}

// tableAttributesCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableAttributesCols = []columnInfo{
    {name: "ID", tp: mysql.TypeVarchar, size: types.UnspecifiedLength, flag: mysql.NotNullFlag},
    {name: "TYPE", tp: mysql.TypeVarchar, size: 16, flag: mysql.NotNullFlag},
    {name: "ATTRIBUTES", tp: mysql.TypeVarchar, size: types.UnspecifiedLength},
    {name: "RANGES", tp: mysql.TypeBlob, size: types.UnspecifiedLength},
}

// tableTrxSummaryCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableTrxSummaryCols = []columnInfo{
    {name: "DIGEST", tp: mysql.TypeVarchar, size: 16, flag: mysql.NotNullFlag, comment: "Digest of a transaction"},
    {name: txninfo.AllSQLDigestsStr, tp: mysql.TypeBlob, size: types.UnspecifiedLength, comment: "A list of the digests of SQL statements that the transaction has executed"},
}

// tablePlacementPoliciesCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tablePlacementPoliciesCols = []columnInfo{
    {name: "POLICY_ID", tp: mysql.TypeLonglong, size: 21, flag: mysql.NotNullFlag},
    {name: "CATALOG_NAME", tp: mysql.TypeVarchar, size: 512, flag: mysql.NotNullFlag},
    {name: "POLICY_NAME", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag}, // Catalog wide policy
    {name: "PRIMARY_REGION", tp: mysql.TypeVarchar, size: 1024},
    {name: "REGIONS", tp: mysql.TypeVarchar, size: 1024},
    {name: "CONSTRAINTS", tp: mysql.TypeVarchar, size: 1024},
    {name: "LEADER_CONSTRAINTS", tp: mysql.TypeVarchar, size: 1024},
    {name: "FOLLOWER_CONSTRAINTS", tp: mysql.TypeVarchar, size: 1024},
    {name: "LEARNER_CONSTRAINTS", tp: mysql.TypeVarchar, size: 1024},
    {name: "SCHEDULE", tp: mysql.TypeVarchar, size: 20}, // EVEN or MAJORITY_IN_PRIMARY
    {name: "FOLLOWERS", tp: mysql.TypeLonglong, size: 21},
    {name: "LEARNERS", tp: mysql.TypeLonglong, size: 21},
}

// tableVariablesInfoCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableVariablesInfoCols = []columnInfo{
    {name: "VARIABLE_NAME", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag},
    {name: "VARIABLE_SCOPE", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag},
    {name: "DEFAULT_VALUE", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag},
    {name: "CURRENT_VALUE", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag},
    {name: "MIN_VALUE", tp: mysql.TypeLonglong, size: 21},
    {name: "MAX_VALUE", tp: mysql.TypeLonglong, size: 21, flag: mysql.UnsignedFlag},
    {name: "POSSIBLE_VALUES", tp: mysql.TypeVarchar, size: 256},
    {name: "IS_NOOP", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag},
}

// tableUserAttributesCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableUserAttributesCols = []columnInfo{
    {name: "USER", tp: mysql.TypeVarchar, size: 32, flag: mysql.NotNullFlag},
    {name: "HOST", tp: mysql.TypeVarchar, size: 255, flag: mysql.NotNullFlag},
    {name: "ATTRIBUTE", tp: mysql.TypeLongBlob, size: types.UnspecifiedLength},
}

// tableMemoryUsageCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableMemoryUsageCols = []columnInfo{
    {name: "MEMORY_TOTAL", tp: mysql.TypeLonglong, size: 21, flag: mysql.NotNullFlag},
    {name: "MEMORY_LIMIT", tp: mysql.TypeLonglong, size: 21, flag: mysql.NotNullFlag},
    {name: "MEMORY_CURRENT", tp: mysql.TypeLonglong, size: 21, flag: mysql.NotNullFlag},
    {name: "MEMORY_MAX_USED", tp: mysql.TypeLonglong, size: 21, flag: mysql.NotNullFlag},
    {name: "CURRENT_OPS", tp: mysql.TypeVarchar, size: 50},
    {name: "SESSION_KILL_LAST", tp: mysql.TypeDatetime},
    {name: "SESSION_KILL_TOTAL", tp: mysql.TypeLonglong, size: 21, flag: mysql.NotNullFlag},
    {name: "GC_LAST", tp: mysql.TypeDatetime},
    {name: "GC_TOTAL", tp: mysql.TypeLonglong, size: 21, flag: mysql.NotNullFlag},
    {name: "DISK_USAGE", tp: mysql.TypeLonglong, size: 21, flag: mysql.NotNullFlag},
    {name: "QUERY_FORCE_DISK", tp: mysql.TypeLonglong, size: 21, flag: mysql.NotNullFlag},
}

// tableMemoryUsageOpsHistoryCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableMemoryUsageOpsHistoryCols = []columnInfo{
    {name: "TIME", tp: mysql.TypeDatetime, size: 64, flag: mysql.NotNullFlag},
    {name: "OPS", tp: mysql.TypeVarchar, size: 20, flag: mysql.NotNullFlag},
    {name: "MEMORY_LIMIT", tp: mysql.TypeLonglong, size: 21, flag: mysql.NotNullFlag},
    {name: "MEMORY_CURRENT", tp: mysql.TypeLonglong, size: 21, flag: mysql.NotNullFlag},
    {name: "PROCESSID", tp: mysql.TypeLonglong, size: 21, flag: mysql.UnsignedFlag},
    {name: "MEM", tp: mysql.TypeLonglong, size: 21, flag: mysql.UnsignedFlag},
    {name: "DISK", tp: mysql.TypeLonglong, size: 21, flag: mysql.UnsignedFlag},
    {name: "CLIENT", tp: mysql.TypeVarchar, size: 64},
    {name: "DB", tp: mysql.TypeVarchar, size: 64},
    {name: "USER", tp: mysql.TypeVarchar, size: 16},
    {name: "SQL_DIGEST", tp: mysql.TypeVarchar, size: 64},
    {name: "SQL_TEXT", tp: mysql.TypeVarchar, size: 256},
}

// tableResourceGroupsCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableResourceGroupsCols = []columnInfo{
    {name: "NAME", tp: mysql.TypeVarchar, size: resourcegroup.MaxGroupNameLength, flag: mysql.NotNullFlag},
    {name: "RU_PER_SEC", tp: mysql.TypeVarchar, size: 21},
    {name: "PRIORITY", tp: mysql.TypeVarchar, size: 6},
    {name: "BURSTABLE", tp: mysql.TypeVarchar, size: 3},
    {name: "QUERY_LIMIT", tp: mysql.TypeVarchar, size: 256},
    {name: "BACKGROUND", tp: mysql.TypeVarchar, size: 256},
}

// tableRunawayWatchListCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableRunawayWatchListCols = []columnInfo{
    {name: "ID", tp: mysql.TypeLonglong, size: 21, flag: mysql.NotNullFlag},
    {name: "RESOURCE_GROUP_NAME", tp: mysql.TypeVarchar, size: resourcegroup.MaxGroupNameLength, flag: mysql.NotNullFlag},
    {name: "START_TIME", tp: mysql.TypeVarchar, size: 32, flag: mysql.NotNullFlag},
    {name: "END_TIME", tp: mysql.TypeVarchar, size: 32},
    {name: "WATCH", tp: mysql.TypeVarchar, size: 12, flag: mysql.NotNullFlag},
    {name: "WATCH_TEXT", tp: mysql.TypeBlob, size: types.UnspecifiedLength, flag: mysql.NotNullFlag},
    {name: "SOURCE", tp: mysql.TypeVarchar, size: 128, flag: mysql.NotNullFlag},
    {name: "ACTION", tp: mysql.TypeVarchar, size: 12, flag: mysql.NotNullFlag},
    {name: "RULE", tp: mysql.TypeVarchar, size: 128, flag: mysql.NotNullFlag},
}

// information_schema.CHECK_CONSTRAINTS
// tableCheckConstraintsCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableCheckConstraintsCols = []columnInfo{
    {name: "CONSTRAINT_CATALOG", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag},
    {name: "CONSTRAINT_SCHEMA", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag},
    {name: "CONSTRAINT_NAME", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag},
    {name: "CHECK_CLAUSE", tp: mysql.TypeLongBlob, size: types.UnspecifiedLength, flag: mysql.NotNullFlag},
}

// information_schema.TIDB_CHECK_CONSTRAINTS
// tableTiDBCheckConstraintsCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableTiDBCheckConstraintsCols = []columnInfo{
    {name: "CONSTRAINT_CATALOG", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag},
    {name: "CONSTRAINT_SCHEMA", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag},
    {name: "CONSTRAINT_NAME", tp: mysql.TypeVarchar, size: 64, flag: mysql.NotNullFlag},
    {name: "CHECK_CLAUSE", tp: mysql.TypeLongBlob, size: types.UnspecifiedLength, flag: mysql.NotNullFlag},
    {name: "TABLE_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "TABLE_ID", tp: mysql.TypeLonglong, size: 21},
}

// tableKeywords 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableKeywords = []columnInfo{
    {name: "WORD", tp: mysql.TypeVarchar, size: 128},
    {name: "RESERVED", tp: mysql.TypeLong, size: 11},
}

// tableTiDBIndexUsage 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableTiDBIndexUsage = []columnInfo{
    {name: "TABLE_SCHEMA", tp: mysql.TypeVarchar, size: 64},
    {name: "TABLE_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "INDEX_NAME", tp: mysql.TypeVarchar, size: 64},
    {name: "QUERY_TOTAL", tp: mysql.TypeLonglong, size: 21},
    {name: "KV_REQ_TOTAL", tp: mysql.TypeLonglong, size: 21},
    {name: "ROWS_ACCESS_TOTAL", tp: mysql.TypeLonglong, size: 21},
    {name: "PERCENTAGE_ACCESS_0", tp: mysql.TypeLonglong, size: 21},
    {name: "PERCENTAGE_ACCESS_0_1", tp: mysql.TypeLonglong, size: 21},
    {name: "PERCENTAGE_ACCESS_1_10", tp: mysql.TypeLonglong, size: 21},
    {name: "PERCENTAGE_ACCESS_10_20", tp: mysql.TypeLonglong, size: 21},
    {name: "PERCENTAGE_ACCESS_20_50", tp: mysql.TypeLonglong, size: 21},
    {name: "PERCENTAGE_ACCESS_50_100", tp: mysql.TypeLonglong, size: 21},
    {name: "PERCENTAGE_ACCESS_100", tp: mysql.TypeLonglong, size: 21},
    {name: "LAST_ACCESS_TIME", tp: mysql.TypeDatetime, size: 21},
}

// tablePlanCache 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tablePlanCache = []columnInfo{
    {name: "SQL_DIGEST", tp: mysql.TypeVarchar, size: 64},
    {name: "SQL_TEXT", tp: mysql.TypeLongBlob, size: types.UnspecifiedLength},
    {name: "STMT_TYPE", tp: mysql.TypeVarchar, size: 64},
    {name: "PARSE_USER", tp: mysql.TypeVarchar, size: 64},
    {name: "PLAN_DIGEST", tp: mysql.TypeVarchar, size: 64},
    {name: "BINARY_PLAN", tp: mysql.TypeLongBlob, size: types.UnspecifiedLength},
    {name: "BINDING", tp: mysql.TypeLongBlob, size: types.UnspecifiedLength},
    {name: "OPT_ENV", tp: mysql.TypeVarchar, size: 64},
    {name: "PARSE_VALUES", tp: mysql.TypeLongBlob, size: types.UnspecifiedLength},
    {name: "MEM_SIZE", tp: mysql.TypeLonglong, size: 21},
    {name: "EXECUTIONS", tp: mysql.TypeLonglong, size: 21},
    {name: "PROCESSED_KEYS", tp: mysql.TypeLonglong, size: 21},
    {name: "TOTAL_KEYS", tp: mysql.TypeLonglong, size: 21},
    {name: "SUM_LATENCY", tp: mysql.TypeLonglong, size: 21},
    {name: "LOAD_TIME", tp: mysql.TypeDatetime, size: 19},
    {name: "LAST_ACTIVE_TIME", tp: mysql.TypeDatetime, size: 19},
}

// tableKeyspaceMetaCols 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableKeyspaceMetaCols = []columnInfo{
    {name: "KEYSPACE_NAME", tp: mysql.TypeVarchar, size: 128},
    {name: "KEYSPACE_ID", tp: mysql.TypeVarchar, size: 64},
    {name: "CONFIG", tp: mysql.TypeJSON, size: types.UnspecifiedLength},
}

// GetShardingInfo returns a nil or description string for the sharding information of given TableInfo.
// The returned description string may be:
//   - "NOT_SHARDED": for tables that SHARD_ROW_ID_BITS is not specified.
//   - "NOT_SHARDED(PK_IS_HANDLE)": for tables of which primary key is row id.
//   - "PK_AUTO_RANDOM_BITS={bit_number}, RANGE BITS={bit_number}": for tables of which primary key is sharded row id.
//   - "SHARD_BITS={bit_number}": for tables that with SHARD_ROW_ID_BITS.
// The returned nil indicates that sharding information is not suitable for the table(for example, when the table is a View).
// This function is exported for unit test.
// GetShardingInfo 对应 Go 同名函数或方法，保留参数、返回值、关键分支和外部调用形状。
pub fn GetShardingInfo(dbInfo ast.CIStr, tableInfo *model.TableInfo) any {
    if tableInfo == nil || tableInfo.IsView() || metadef.IsMemOrSysDB(dbInfo.L) {
        return nil
    }
    shardingInfo := "NOT_SHARDED"
    if tableInfo.ContainsAutoRandomBits() {
        shardingInfo = "PK_AUTO_RANDOM_BITS=" + strconv.Itoa(int(tableInfo.AutoRandomBits))
        rangeBits := tableInfo.AutoRandomRangeBits
        if rangeBits != 0 && rangeBits != autoid.AutoRandomRangeBitsDefault {
            shardingInfo = fmt.Sprintf("%s, RANGE BITS=%d", shardingInfo, rangeBits)
        }
    } else if tableInfo.ShardRowIDBits > 0 {
        shardingInfo = "SHARD_BITS=" + strconv.Itoa(int(tableInfo.ShardRowIDBits))
    } else if tableInfo.PKIsHandle {
        shardingInfo = "NOT_SHARDED(PK_IS_HANDLE)"
    }
    return shardingInfo
}

// Go const 块迁移：保留 INFORMATION_SCHEMA 表名、列名和微服务名称常量的声明顺序。
pub const (
    // PrimaryKeyType is the string constant of PRIMARY KEY.
    PrimaryKeyType = "PRIMARY KEY"
    // PrimaryConstraint is the string constant of PRIMARY.
    PrimaryConstraint = "PRIMARY"
    // UniqueKeyType is the string constant of UNIQUE.
    UniqueKeyType = "UNIQUE"
    // ForeignKeyType is the string constant of Foreign Key.
    ForeignKeyType = "FOREIGN KEY"
)

// Go const 块迁移：保留 INFORMATION_SCHEMA 表名、列名和微服务名称常量的声明顺序。
pub const (
    // TiFlashWrite is the TiFlash write node in disaggregated mode.
    TiFlashWrite = "tiflash_write"
)

// ServerInfo represents the basic server information of single cluster component
// ServerInfo 描述集群组件地址和版本，后续函数会按 TiDB/PD/TiKV/TiFlash/TiProxy/TiCDC 分别填充。
/// 集群节点信息：类型、地址、状态地址、版本、git hash、启动时间与 server_id。
pub struct ServerInfo {
    ServerType     string
    Address        string
    StatusAddr     string
    Version        string
    GitHash        string
    StartTimestamp int64
    ServerID       uint64
    EngineRole     string
}

// isLoopBackOrUnspecifiedAddr 对应 Go 同名函数或方法，保留参数、返回值、关键分支和外部调用形状。
pub fn (s *ServerInfo) isLoopBackOrUnspecifiedAddr(addr string) bool {
    tcpAddr, err := net.ResolveTCPAddr("", addr)
    if err != nil {
        return false
    }
    ip := net.ParseIP(tcpAddr.IP.String())
    return ip != nil && (ip.IsUnspecified() || ip.IsLoopback())
}

// ResolveLoopBackAddr exports for testing.
// ResolveLoopBackAddr 对应 Go 同名函数或方法，保留参数、返回值、关键分支和外部调用形状。
pub fn (s *ServerInfo) ResolveLoopBackAddr() {
    if s.isLoopBackOrUnspecifiedAddr(s.Address) && !s.isLoopBackOrUnspecifiedAddr(s.StatusAddr) {
        addr, err1 := net.ResolveTCPAddr("", s.Address)
        statusAddr, err2 := net.ResolveTCPAddr("", s.StatusAddr)
        if err1 == nil && err2 == nil {
            addr.IP = statusAddr.IP
            s.Address = addr.String()
        }
    } else if !s.isLoopBackOrUnspecifiedAddr(s.Address) && s.isLoopBackOrUnspecifiedAddr(s.StatusAddr) {
        addr, err1 := net.ResolveTCPAddr("", s.Address)
        statusAddr, err2 := net.ResolveTCPAddr("", s.StatusAddr)
        if err1 == nil && err2 == nil {
            statusAddr.IP = addr.IP
            s.StatusAddr = statusAddr.String()
        }
    }
}

// GetClusterServerInfo returns all components information of cluster
// 汇总 TiDB/PD/TiKV/TiFlash/TiProxy/TiCDC/TSO/Scheduling 等节点信息。
pub fn GetClusterServerInfo(ctx sessionctx.Context) ([]ServerInfo, error) {
// failpoint 注入点只服务测试；生产路径应保持原控制流。
    failpoint.Inject("mockClusterInfo", func(val failpoint.Value) {
        // The cluster topology is injected by `failpoint` expression and
        // there is no extra checks for it. (let the test fail if the expression invalid)
        if s := val.(string); len(s) > 0 {
            pub var servers []ServerInfo
            for _, server := range strings.Split(s, ";") {
                parts := strings.Split(server, ",")
                serverID, err := strconv.ParseUint(parts[5], 10, 64)
                if err != nil {
                    panic("convert parts[5] to uint64 failed")
                }
                servers = append(servers, ServerInfo{
                    ServerType: parts[0],
                    Address:    parts[1],
                    StatusAddr: parts[2],
                    Version:    parts[3],
                    GitHash:    parts[4],
                    ServerID:   serverID,
                })
            }
            failpoint.Return(servers, nil)
        }
    })

    type retriever func(ctx sessionctx.Context) ([]ServerInfo, error)
    retrievers := []retriever{GetTiDBServerInfo, GetPDServerInfo, func(ctx sessionctx.Context) ([]ServerInfo, error) {
        return GetStoreServerInfo(ctx.GetStore())
    }, GetTiProxyServerInfo, GetTiCDCServerInfo, GetTSOServerInfo, GetSchedulingServerInfo}
    //nolint: prealloc
    pub var servers []ServerInfo
    for _, r := range retrievers {
        nodes, err := r(ctx)
        if err != nil {
            return nil, err
        }

        // Create an error group with Panic recovery and concurrency limit
        resolveGroup := util.NewErrorGroupWithRecover()
        resolveGroup.SetLimit(runtime.GOMAXPROCS(0)) //Limit concurrency to number of CPU cores

        // Resolve loopback addresses concurrently for each node
        for i := range nodes {
            resolveGroup.Go(func() error {
                nodes[i].ResolveLoopBackAddr()
                return nil
            })
        }

        // Wait for all address resolutions to complete and check for errors
        if err := resolveGroup.Wait(); err != nil {
            return nil, err
        }
        servers = append(servers, nodes...)
    }
    return servers, nil
}

// GetTiDBServerInfo returns all TiDB nodes information of cluster
// 仅返回本集群 TiDB 节点信息。
pub fn GetTiDBServerInfo(ctx sessionctx.Context) ([]ServerInfo, error) {
    // Get TiDB servers info.
    tidbNodes, err := infosync.GetAllServerInfo(context.Background())
    if err != nil {
        return nil, errors.Trace(err)
    }
    pub var isDefaultVersion bool
    if len(config.GetGlobalConfig().ServerVersion) == 0 {
        isDefaultVersion = true
    }
    pub var servers = make([]ServerInfo, 0, len(tidbNodes))
    for _, node := range tidbNodes {
        servers = append(servers, ServerInfo{
            ServerType:     "tidb",
            Address:        net.JoinHostPort(node.IP, strconv.Itoa(int(node.Port))),
            StatusAddr:     net.JoinHostPort(node.IP, strconv.Itoa(int(node.StatusPort))),
            Version:        FormatTiDBVersion(node.Version, isDefaultVersion),
            GitHash:        node.GitHash,
            StartTimestamp: node.StartTimestamp,
            ServerID:       node.ServerIDGetter(),
        })
    }
    return servers, nil
}

// FormatTiDBVersion make TiDBVersion consistent to TiKV and PD.
// The default TiDBVersion is 5.7.25-TiDB-${TiDBReleaseVersion}.
// FormatTiDBVersion 对应 Go 同名函数或方法，保留参数、返回值、关键分支和外部调用形状。
// 格式化 TiDB 版本串：默认版本去掉 MySQL 兼容前缀，自定义版本原样返回。
pub fn FormatTiDBVersion(TiDBVersion string, isDefaultVersion bool) string {
    pub var version, nodeVersion string

    // The user hasn't set the config 'ServerVersion'.
    if isDefaultVersion {
        nodeVersion = TiDBVersion[strings.Index(TiDBVersion, "TiDB-")+len("TiDB-"):]
        if len(nodeVersion) > 0 && nodeVersion[0] == 'v' {
            nodeVersion = nodeVersion[1:]
        }
        nodeVersions := strings.SplitN(nodeVersion, "-", 2)
        if len(nodeVersions) == 1 {
            version = nodeVersions[0]
        } else if len(nodeVersions) >= 2 {
            version = fmt.Sprintf("%s-%s", nodeVersions[0], nodeVersions[1])
        }
    } else { // The user has already set the config 'ServerVersion',it would be a complex scene, so just use the 'ServerVersion' as version.
        version = TiDBVersion
    }

    return version
}

// GetPDServerInfo returns all PD nodes information of cluster
// GetPDServerInfo：对应 Go 同名函数，保留调用顺序与返回形状。
pub fn GetPDServerInfo(ctx sessionctx.Context) ([]ServerInfo, error) {
    // Get PD servers info.
    members, err := getEtcdMembers(ctx)
    if err != nil {
        return nil, err
    }
    // TODO: maybe we should unify the PD API request interface.
    pub var (
        memberNum = len(members)
        servers   = make([]ServerInfo, 0, memberNum)
        errs      = make([]error, 0, memberNum)
    )
    if memberNum == 0 {
        return servers, nil
    }
    // Try on each member until one succeeds or all fail.
    for _, addr := range members {
        // Get PD version, git_hash
        url := fmt.Sprintf("%s://%s%s", util.InternalHTTPSchema(), addr, pd.Status)
// 外部网络调用保持 Go 形状，这里不会真正发起请求。
        req, err := http.NewRequest(http.MethodGet, url, nil)
        if err != nil {
            ctx.GetSessionVars().StmtCtx.AppendWarning(err)
            logutil.BgLogger().Warn("create pd server info request error", zap.String("url", url), zap.Error(err))
            errs = append(errs, err)
            continue
        }
        req.Header.Add("PD-Allow-follower-handle", "true")
        resp, err := util.InternalHTTPClient().Do(req)
        if err != nil {
            ctx.GetSessionVars().StmtCtx.AppendWarning(err)
            logutil.BgLogger().Warn("request pd server info error", zap.String("url", url), zap.Error(err))
            errs = append(errs, err)
            continue
        }
        pub var content = struct {
            Version        string `json:"version"`
            GitHash        string `json:"git_hash"`
            StartTimestamp int64  `json:"start_timestamp"`
        }{}
        err = json.NewDecoder(resp.Body).Decode(&content)
        terror.Log(resp.Body.Close())
        if err != nil {
            ctx.GetSessionVars().StmtCtx.AppendWarning(err)
            logutil.BgLogger().Warn("close pd server info request error", zap.String("url", url), zap.Error(err))
            errs = append(errs, err)
            continue
        }
        if len(content.Version) > 0 && content.Version[0] == 'v' {
            content.Version = content.Version[1:]
        }

        servers = append(servers, ServerInfo{
            ServerType:     "pd",
            Address:        addr,
            StatusAddr:     addr,
            Version:        content.Version,
            GitHash:        content.GitHash,
            StartTimestamp: content.StartTimestamp,
        })
    }
    // Return the errors if all members' requests fail.
    if len(errs) == memberNum {
        errorMsg := ""
        for idx, err := range errs {
            errorMsg += err.Error()
            if idx < memberNum-1 {
                errorMsg += "; "
            }
        }
        return nil, errors.Trace(fmt.Errorf("%s", errorMsg))
    }
    return servers, nil
}

// GetTSOServerInfo returns all TSO nodes information of cluster
// GetTSOServerInfo 对应 Go 同名函数或方法，保留参数、返回值、关键分支和外部调用形状。
pub fn GetTSOServerInfo(ctx sessionctx.Context) ([]ServerInfo, error) {
    return getMicroServiceServerInfo(ctx, tsoServiceName)
}

// GetSchedulingServerInfo returns all scheduling nodes information of cluster
// GetSchedulingServerInfo 对应 Go 同名函数或方法，保留参数、返回值、关键分支和外部调用形状。
pub fn GetSchedulingServerInfo(ctx sessionctx.Context) ([]ServerInfo, error) {
    return getMicroServiceServerInfo(ctx, schedulingServiceName)
}

// getMicroServiceServerInfo 对应 Go 同名函数或方法，保留参数、返回值、关键分支和外部调用形状。
pub fn getMicroServiceServerInfo(ctx sessionctx.Context, serviceName string) ([]ServerInfo, error) {
    members, err := getEtcdMembers(ctx)
    if err != nil {
        return nil, err
    }
    // TODO: maybe we should unify the PD API request interface.
    pub var servers []ServerInfo

    if len(members) == 0 {
        return servers, nil
    }
    // Try on each member until one succeeds or all fail.
    for _, addr := range members {
        // Get members
        url := fmt.Sprintf("%s://%s%s/%s", util.InternalHTTPSchema(), addr, "/pd/api/v2/ms/members", serviceName)
// 外部网络调用保持 Go 形状，这里不会真正发起请求。
        req, err := http.NewRequest(http.MethodGet, url, nil)
        if err != nil {
            ctx.GetSessionVars().StmtCtx.AppendWarning(err)
            logutil.BgLogger().Warn("create microservice server info request error", zap.String("service", serviceName), zap.String("url", url), zap.Error(err))
            continue
        }
        req.Header.Add("PD-Allow-follower-handle", "true")
        resp, err := util.InternalHTTPClient().Do(req)
        if err != nil {
            ctx.GetSessionVars().StmtCtx.AppendWarning(err)
            logutil.BgLogger().Warn("request microservice server info error", zap.String("service", serviceName), zap.String("url", url), zap.Error(err))
            continue
        }
// 外部网络调用保持 Go 形状，这里不会真正发起请求。
        if resp.StatusCode != http.StatusOK {
            terror.Log(resp.Body.Close())
            continue
        }
        pub var content = []struct {
            ServiceAddr    string `json:"service-addr"`
            Version        string `json:"version"`
            GitHash        string `json:"git-hash"`
            DeployPath     string `json:"deploy-path"`
            StartTimestamp int64  `json:"start-timestamp"`
        }{}
        err = json.NewDecoder(resp.Body).Decode(&content)
        terror.Log(resp.Body.Close())
        if err != nil {
            ctx.GetSessionVars().StmtCtx.AppendWarning(err)
            logutil.BgLogger().Warn("close microservice server info request error", zap.String("service", serviceName), zap.String("url", url), zap.Error(err))
            continue
        }

        for _, c := range content {
            addr := strings.TrimPrefix(c.ServiceAddr, "http://")
            addr = strings.TrimPrefix(addr, "https://")
            if len(c.Version) > 0 && c.Version[0] == 'v' {
                c.Version = c.Version[1:]
            }
            servers = append(servers, ServerInfo{
                ServerType:     serviceName,
                Address:        addr,
                StatusAddr:     addr,
                Version:        c.Version,
                GitHash:        c.GitHash,
                StartTimestamp: c.StartTimestamp,
            })
        }
        return servers, nil
    }
    return servers, nil
}

// getEtcdMembers 对应 Go 同名函数或方法，保留参数、返回值、关键分支和外部调用形状。
pub fn getEtcdMembers(ctx sessionctx.Context) ([]string, error) {
    store := ctx.GetStore()
    etcd, ok := store.(kv.EtcdBackend)
    if !ok {
        return nil, errors.Errorf("%T not an etcd backend", store)
    }
    members, err := etcd.GetPDAddrs()
    if err != nil {
        return nil, errors.Trace(err)
    }
    return members, nil
}

// isTiFlashStore 对应 Go 同名函数或方法，保留参数、返回值、关键分支和外部调用形状。
pub fn isTiFlashStore(store *metapb.Store) bool {
    return slices.ContainsFunc(store.Labels, func(label *metapb.StoreLabel) bool {
        return label.GetKey() == placement.EngineLabelKey && label.GetValue() == placement.EngineLabelTiFlash
    })
}

// isTiFlashWriteNode 对应 Go 同名函数或方法，保留参数、返回值、关键分支和外部调用形状。
pub fn isTiFlashWriteNode(store *metapb.Store) bool {
    return slices.ContainsFunc(store.Labels, func(label *metapb.StoreLabel) bool {
        return label.GetKey() == placement.EngineRoleLabelKey && label.GetValue() == placement.EngineRoleLabelWrite
    })
}

// GetStoreServerInfo returns all store nodes(TiKV or TiFlash) cluster information
// GetStoreServerInfo：对应 Go 同名函数，保留调用顺序与返回形状。
pub fn GetStoreServerInfo(store kv.Storage) ([]ServerInfo, error) {
// failpoint 注入点只服务测试；生产路径应保持原控制流。
    failpoint.Inject("mockStoreServerInfo", func(val failpoint.Value) {
        if s := val.(string); len(s) > 0 {
            pub var servers []ServerInfo
            for _, server := range strings.Split(s, ";") {
                parts := strings.Split(server, ",")
                servers = append(servers, ServerInfo{
                    ServerType:     parts[0],
                    Address:        parts[1],
                    StatusAddr:     parts[2],
                    Version:        parts[3],
                    GitHash:        parts[4],
                    StartTimestamp: 0,
                })
            }
            failpoint.Return(servers, nil)
        }
    })

    // Get TiKV servers info.
    tikvStore, ok := store.(tikv.Storage)
    if !ok {
        return nil, errors.Errorf("%T is not an TiKV or TiFlash store instance", store)
    }
    pdClient := tikvStore.GetRegionCache().PDClient()
    if pdClient == nil {
        return nil, errors.New("pd unavailable")
    }
    stores, err := pdClient.GetAllStores(context.Background())
    if err != nil {
        return nil, errors.Trace(err)
    }
    servers := make([]ServerInfo, 0, len(stores))
    for _, store := range stores {
// failpoint 注入点只服务测试；生产路径应保持原控制流。
        failpoint.Inject("mockStoreTombstone", func(val failpoint.Value) {
            if val.(bool) {
                store.State = metapb.StoreState_Tombstone
            }
        })

        if store.GetState() == metapb.StoreState_Tombstone {
            continue
        }
        pub var tp string
        if isTiFlashStore(store) {
            tp = kv.TiFlash.Name()
        } else {
            tp = tikv.GetStoreTypeByMeta(store).Name()
        }
        pub var engineRole string
        if isTiFlashWriteNode(store) {
            engineRole = placement.EngineRoleLabelWrite
        }
        servers = append(servers, ServerInfo{
            ServerType:     tp,
            Address:        store.Address,
            StatusAddr:     store.StatusAddress,
            Version:        FormatStoreServerVersion(store.Version),
            GitHash:        store.GitHash,
            StartTimestamp: store.StartTimestamp,
            EngineRole:     engineRole,
        })
    }
    return servers, nil
}

// FormatStoreServerVersion format version of store servers(Tikv or TiFlash)
// FormatStoreServerVersion 对应 Go 同名函数或方法，保留参数、返回值、关键分支和外部调用形状。
pub fn FormatStoreServerVersion(version string) string {
    if len(version) >= 1 && version[0] == 'v' {
        version = version[1:]
    }
    return version
}

// GetTiFlashStoreCount returns the count of tiflash server.
// GetTiFlashStoreCount 对应 Go 同名函数或方法，保留参数、返回值、关键分支和外部调用形状。
pub fn GetTiFlashStoreCount(store kv.Storage) (cnt uint64, err error) {
// failpoint 注入点只服务测试；生产路径应保持原控制流。
    failpoint.Inject("mockTiFlashStoreCount", func(val failpoint.Value) {
        if val.(bool) {
            failpoint.Return(uint64(10), nil)
        }
    })

    stores, err := GetStoreServerInfo(store)
    if err != nil {
        return cnt, err
    }
    for _, store := range stores {
        if store.ServerType == kv.TiFlash.Name() {
            cnt++
        }
    }
    return cnt, nil
}

// GetTiProxyServerInfo gets server info of TiProxy from PD.
// GetTiProxyServerInfo：对应 Go 同名函数，保留调用顺序与返回形状。
pub fn GetTiProxyServerInfo(ctx sessionctx.Context) ([]ServerInfo, error) {
    tiproxyNodes, err := infosync.GetTiProxyServerInfo(context.Background())
    if err != nil {
        return nil, errors.Trace(err)
    }
    pub var servers = make([]ServerInfo, 0, len(tiproxyNodes))
    for _, node := range tiproxyNodes {
        servers = append(servers, ServerInfo{
            ServerType:     "tiproxy",
            Address:        net.JoinHostPort(node.IP, node.Port),
            StatusAddr:     net.JoinHostPort(node.IP, node.StatusPort),
            Version:        node.Version,
            GitHash:        node.GitHash,
            StartTimestamp: node.StartTimestamp,
        })
    }
    return servers, nil
}

// GetTiCDCServerInfo gets server info of TiCDC from PD.
// GetTiCDCServerInfo：对应 Go 同名函数，保留调用顺序与返回形状。
pub fn GetTiCDCServerInfo(ctx sessionctx.Context) ([]ServerInfo, error) {
    ticdcNodes, err := infosync.GetTiCDCServerInfo(context.Background())
    if err != nil {
        return nil, errors.Trace(err)
    }
    pub var servers = make([]ServerInfo, 0, len(ticdcNodes))
    for _, node := range ticdcNodes {
        servers = append(servers, ServerInfo{
            ServerType:     "ticdc",
            Address:        node.Address,
            StatusAddr:     node.Address,
            Version:        node.Version,
            GitHash:        node.GitHash,
            StartTimestamp: node.StartTimestamp,
        })
    }
    return servers, nil
}

// SysVarHiddenForSem checks if a given sysvar is hidden according to SEM and privileges.
// SysVarHiddenForSem 对应 Go 同名函数或方法，保留参数、返回值、关键分支和外部调用形状。
pub fn SysVarHiddenForSem(ctx sessionctx.Context, sysVarNameInLower string) bool {
    if !sem.IsEnabled() || !sem.IsInvisibleSysVar(sysVarNameInLower) {
        return false
    }
    checker := privilege.GetPrivilegeManager(ctx)
    if checker == nil || checker.RequestDynamicVerification(ctx.GetSessionVars().ActiveRoles, "RESTRICTED_VARIABLES_ADMIN", false) {
        return false
    }
    return true
}

// GetDataFromSessionVariables return the [name, value] of all session variables
// GetDataFromSessionVariables 对应会话内存数据读取路径；这里不访问真实连接，只保留过滤和行拼装语义。
pub fn GetDataFromSessionVariables(ctx context.Context, sctx sessionctx.Context) ([][]types.Datum, error) {
    sessionVars := sctx.GetSessionVars()
    sysVars := variable.GetSysVars()
    rows := make([][]types.Datum, 0, len(sysVars))
    for _, v := range sysVars {
        if SysVarHiddenForSem(sctx, v.Name) {
            continue
        }
        pub var value string
        value, err := sessionVars.GetSessionOrGlobalSystemVar(ctx, v.Name)
        if err != nil {
            return nil, err
        }
        row := types.MakeDatums(v.Name, value)
        rows = append(rows, row)
    }
    return rows, nil
}

// GetDataFromSessionConnectAttrs produces the rows for the session_connect_attrs table.
// GetDataFromSessionConnectAttrs 对应会话内存数据读取路径；这里不访问真实连接，只保留过滤和行拼装语义。
pub fn GetDataFromSessionConnectAttrs(sctx sessionctx.Context, sameAccount bool) ([][]types.Datum, error) {
    sm := sctx.GetSessionManager()
    if sm == nil {
        return nil, nil
    }
    pub var user *auth.UserIdentity
    if sameAccount {
        user = sctx.GetSessionVars().User
    }
    allAttrs := sm.GetConAttrs(user)
    rows := make([][]types.Datum, 0, len(allAttrs)*10) // 10 Attributes per connection
    for pid, attrs := range allAttrs {                 // Note: PID is not ordered.
        // Sorts the attributes by key and gives ORDINAL_POSITION based on this. This is needed as we didn't store the
        // ORDINAL_POSITION and a map doesn't have a guaranteed sort order. This is needed to keep the ORDINAL_POSITION
        // stable over multiple queries.
        attrnames := make([]string, 0, len(attrs))
        for attrname := range attrs {
            attrnames = append(attrnames, attrname)
        }
        sort.Strings(attrnames)

        for ord, attrkey := range attrnames {
            row := types.MakeDatums(
                pid,
                attrkey,
                attrs[attrkey],
                ord,
            )
            rows = append(rows, row)
        }
    }
    return rows, nil
}

// tableNameToColumns 对应 Go 的列定义数组，字段顺序就是 INFORMATION_SCHEMA 暴露的列顺序。
pub var tableNameToColumns = map[string][]columnInfo{
    TableSchemata:                           schemataCols,
    TableTables:                             tablesCols,
    TableColumns:                            columnsCols,
    tableColumnStatistics:                   columnStatisticsCols,
    TableStatistics:                         statisticsCols,
    TableCharacterSets:                      charsetCols,
    TableCollations:                         collationsCols,
    tableFiles:                              filesCols,
    TableProfiling:                          profilingCols,
    TablePartitions:                         partitionsCols,
    TableKeyColumn:                          keyColumnUsageCols,
    TableReferConst:                         referConstCols,
    tablePlugins:                            pluginsCols,
    TableConstraints:                        tableConstraintsCols,
    tableTriggers:                           tableTriggersCols,
    TableUserPrivileges:                     tableUserPrivilegesCols,
    tableSchemaPrivileges:                   tableSchemaPrivilegesCols,
    tableTablePrivileges:                    tableTablePrivilegesCols,
    tableColumnPrivileges:                   tableColumnPrivilegesCols,
    TableEngines:                            tableEnginesCols,
    TableViews:                              tableViewsCols,
    tableRoutines:                           tableRoutinesCols,
    tableParameters:                         tableParametersCols,
    tableEvents:                             tableEventsCols,
    tableOptimizerTrace:                     tableOptimizerTraceCols,
    tableTableSpaces:                        tableTableSpacesCols,
    TableCollationCharacterSetApplicability: tableCollationCharacterSetApplicabilityCols,
    TableProcesslist:                        tableProcesslistCols,
    TableTiDBIndexes:                        tableTiDBIndexesCols,
    TableSlowQuery:                          slowQueryCols,
    TableTiDBHotRegions:                     TableTiDBHotRegionsCols,
    TableTiDBHotRegionsHistory:              TableTiDBHotRegionsHistoryCols,
    TableTiKVStoreStatus:                    TableTiKVStoreStatusCols,
    TableAnalyzeStatus:                      tableAnalyzeStatusCols,
    TableTiKVRegionStatus:                   TableTiKVRegionStatusCols,
    TableTiKVRegionPeers:                    TableTiKVRegionPeersCols,
    TableTiDBServersInfo:                    tableTiDBServersInfoCols,
    TableClusterInfo:                        tableClusterInfoCols,
    TableClusterConfig:                      tableClusterConfigCols,
    TableClusterLog:                         tableClusterLogCols,
    TableClusterLoad:                        tableClusterLoadCols,
    TableTiFlashReplica:                     tableTableTiFlashReplicaCols,
    TableClusterHardware:                    tableClusterHardwareCols,
    TableClusterSystemInfo:                  tableClusterSystemInfoCols,
    TableInspectionResult:                   tableInspectionResultCols,
    TableMetricSummary:                      tableMetricSummaryCols,
    TableMetricSummaryByLabel:               tableMetricSummaryByLabelCols,
    TableMetricTables:                       tableMetricTablesCols,
    TableInspectionSummary:                  tableInspectionSummaryCols,
    TableInspectionRules:                    tableInspectionRulesCols,
    TableDDLJobs:                            tableDDLJobsCols,
    TableSequences:                          tableSequencesCols,
    TableStatementsSummary:                  tableStatementsSummaryCols,
    TableStatementsSummaryHistory:           tableStatementsSummaryCols,
    TableStatementsSummaryEvicted:           tableStatementsSummaryEvictedCols,
    TableStorageStats:                       tableStorageStatsCols,
    TableTiDBStatementsStats:                tableTiDBStatementsStatsCols,
    TableTiFlashTables:                      tableTableTiFlashTablesCols,
    TableTiFlashSegments:                    tableTableTiFlashSegmentsCols,
    TableTiFlashIndexes:                     tableTiFlashIndexesCols,
    TableClientErrorsSummaryGlobal:          tableClientErrorsSummaryGlobalCols,
    TableClientErrorsSummaryByUser:          tableClientErrorsSummaryByUserCols,
    TableClientErrorsSummaryByHost:          tableClientErrorsSummaryByHostCols,
    TableTiDBTrx:                            tableTiDBTrxCols,
    TableDeadlocks:                          tableDeadlocksCols,
    TableDataLockWaits:                      tableDataLockWaitsCols,
    TableAttributes:                         tableAttributesCols,
    TablePlacementPolicies:                  tablePlacementPoliciesCols,
    TableTrxSummary:                         tableTrxSummaryCols,
    TableVariablesInfo:                      tableVariablesInfoCols,
    TableUserAttributes:                     tableUserAttributesCols,
    TableMemoryUsage:                        tableMemoryUsageCols,
    TableMemoryUsageOpsHistory:              tableMemoryUsageOpsHistoryCols,
    TableResourceGroups:                     tableResourceGroupsCols,
    TableRunawayWatches:                     tableRunawayWatchListCols,
    TableCheckConstraints:                   tableCheckConstraintsCols,
    TableTiDBCheckConstraints:               tableTiDBCheckConstraintsCols,
    TableKeywords:                           tableKeywords,
    TableTiDBIndexUsage:                     tableTiDBIndexUsage,
    TableTiDBPlanCache:                      tablePlanCache,
    TableKeyspaceMeta:                       tableKeyspaceMetaCols,
}

// createInfoSchemaTable 对应 Go 元数据构造函数，保留字符集、列长度、索引和表状态的设置顺序。
pub fn createInfoSchemaTable(_ autoid.Allocators, _ func() (pools.Resource, error), meta *model.TableInfo) (table.Table, error) {
    columns := make([]*table.Column, len(meta.Columns))
    for i, col := range meta.Columns {
        columns[i] = table.ToColumn(col)
    }
    tp := table.VirtualTable
    if IsClusterTableByName(metadef.InformationSchemaName.L, meta.Name.L) {
        tp = table.ClusterTable
    }
    return &infoschemaTable{meta: meta, cols: columns, tp: tp}, nil
}

// 虚拟表类型只暴露 table.Table 接口形状，写入/删除/更新路径保留只读表错误语义。
pub struct infoschemaTable {
    meta *model.TableInfo
    cols []*table.Column
    tp   table.Type
}

// IterRecords implements table.Table IterRecords interface.
// IterRecords 对应 Go 同名函数或方法，保留参数、返回值、关键分支和外部调用形状。
pub fn (*infoschemaTable) IterRecords(ctx context.Context, sctx sessionctx.Context, cols []*table.Column, fn table.RecordIterFunc) error {
    return nil
}

// Cols implements table.Table Cols interface.
// Cols 对应 Go 同名函数或方法，保留参数、返回值、关键分支和外部调用形状。
pub fn (it *infoschemaTable) Cols() []*table.Column {
    return it.cols
}

// VisibleCols implements table.Table VisibleCols interface.
// VisibleCols 对应 Go 同名函数或方法，保留参数、返回值、关键分支和外部调用形状。
pub fn (it *infoschemaTable) VisibleCols() []*table.Column {
    return it.cols
}

// HiddenCols implements table.Table HiddenCols interface.
// HiddenCols 对应 Go 同名函数或方法，保留参数、返回值、关键分支和外部调用形状。
pub fn (it *infoschemaTable) HiddenCols() []*table.Column {
    return nil
}

// WritableCols implements table.Table WritableCols interface.
// WritableCols 对应 Go 同名函数或方法，保留参数、返回值、关键分支和外部调用形状。
pub fn (it *infoschemaTable) WritableCols() []*table.Column {
    return it.cols
}

// DeletableCols implements table.Table WritableCols interface.
// DeletableCols 对应 Go 同名函数或方法，保留参数、返回值、关键分支和外部调用形状。
pub fn (it *infoschemaTable) DeletableCols() []*table.Column {
    return it.cols
}

// FullHiddenColsAndVisibleCols implements table FullHiddenColsAndVisibleCols interface.
// FullHiddenColsAndVisibleCols 对应 Go 同名函数或方法，保留参数、返回值、关键分支和外部调用形状。
pub fn (it *infoschemaTable) FullHiddenColsAndVisibleCols() []*table.Column {
    return it.cols
}

// Indices implements table.Table Indices interface.
// Indices 对应 Go 同名函数或方法，保留参数、返回值、关键分支和外部调用形状。
pub fn (it *infoschemaTable) Indices() []table.Index {
    return nil
}

// DeletableIndices implements table.Table DeletableIndices interface.
// DeletableIndices 对应 Go 同名函数或方法，保留参数、返回值、关键分支和外部调用形状。
pub fn (it *infoschemaTable) DeletableIndices() []table.Index {
    return nil
}

// WritableConstraint implements table.Table WritableConstraint interface.
// WritableConstraint 对应 Go 同名函数或方法，保留参数、返回值、关键分支和外部调用形状。
pub fn (it *infoschemaTable) WritableConstraint() []*table.Constraint {
    return nil
}

// RecordPrefix implements table.Table RecordPrefix interface.
// RecordPrefix 对应 Go 同名函数或方法，保留参数、返回值、关键分支和外部调用形状。
pub fn (it *infoschemaTable) RecordPrefix() kv.Key {
    return nil
}

// IndexPrefix implements table.Table IndexPrefix interface.
// IndexPrefix 对应 Go 同名函数或方法，保留参数、返回值、关键分支和外部调用形状。
pub fn (it *infoschemaTable) IndexPrefix() kv.Key {
    return nil
}

// AddRecord implements table.Table AddRecord interface.
// AddRecord 保留 INFORMATION_SCHEMA 只读表写路径报错语义。
pub fn (it *infoschemaTable) AddRecord(ctx table.MutateContext, txn kv.Transaction, r []types.Datum, opts ...table.AddRecordOption) (recordID kv.Handle, err error) {
    return nil, table.ErrUnsupportedOp
}

// RemoveRecord implements table.Table RemoveRecord interface.
// RemoveRecord 保留 INFORMATION_SCHEMA 只读表写路径报错语义。
pub fn (it *infoschemaTable) RemoveRecord(ctx table.MutateContext, txn kv.Transaction, h kv.Handle, r []types.Datum, opts ...table.RemoveRecordOption) error {
    return table.ErrUnsupportedOp
}

// UpdateRecord implements table.Table UpdateRecord interface.
// UpdateRecord 保留 INFORMATION_SCHEMA 只读表写路径报错语义。
pub fn (it *infoschemaTable) UpdateRecord(ctx table.MutateContext, txn kv.Transaction, h kv.Handle, oldData, newData []types.Datum, touched []bool, opts ...table.UpdateRecordOption) error {
    return table.ErrUnsupportedOp
}

// Allocators implements table.Table Allocators interface.
// Allocators 对应 Go 同名函数或方法，保留参数、返回值、关键分支和外部调用形状。
pub fn (it *infoschemaTable) Allocators(_ table.AllocatorContext) autoid.Allocators {
    return autoid.Allocators{}
}

// Meta implements table.Table Meta interface.
// Meta 对应 Go 同名函数或方法，保留参数、返回值、关键分支和外部调用形状。
pub fn (it *infoschemaTable) Meta() *model.TableInfo {
    return it.meta
}

// UseNewCollate implements table.Table UseNewCollate interface. Info schema
// tables are not persisted user tables, so they use the current process default.
// UseNewCollate 对应 Go 同名函数或方法，保留参数、返回值、关键分支和外部调用形状。
pub fn (it *infoschemaTable) UseNewCollate() bool {
    return collate.NewCollationEnabled()
}

// GetPhysicalID implements table.Table GetPhysicalID interface.
// GetPhysicalID 对应 Go 同名函数或方法，保留参数、返回值、关键分支和外部调用形状。
pub fn (it *infoschemaTable) GetPhysicalID() int64 {
    return it.meta.ID
}

// Type implements table.Table Type interface.
// Type 对应 Go 同名函数或方法，保留参数、返回值、关键分支和外部调用形状。
pub fn (it *infoschemaTable) Type() table.Type {
    return it.tp
}

// GetPartitionedTable implements table.Table GetPartitionedTable interface.
// GetPartitionedTable 对应 Go 同名函数或方法，保留参数、返回值、关键分支和外部调用形状。
pub fn (it *infoschemaTable) GetPartitionedTable() table.PartitionedTable {
    return nil
}

// VirtualTable is a dummy table.Table implementation.
// 虚拟表类型只暴露 table.Table 接口形状，写入/删除/更新路径保留只读表错误语义。
type VirtualTable struct{}

// Cols implements table.Table Cols interface.
// Cols 对应 Go 同名函数或方法，保留参数、返回值、关键分支和外部调用形状。
pub fn (vt *VirtualTable) Cols() []*table.Column {
    return nil
}

// VisibleCols implements table.Table VisibleCols interface.
// VisibleCols 对应 Go 同名函数或方法，保留参数、返回值、关键分支和外部调用形状。
pub fn (vt *VirtualTable) VisibleCols() []*table.Column {
    return nil
}

// HiddenCols implements table.Table HiddenCols interface.
// HiddenCols 对应 Go 同名函数或方法，保留参数、返回值、关键分支和外部调用形状。
pub fn (vt *VirtualTable) HiddenCols() []*table.Column {
    return nil
}

// WritableCols implements table.Table WritableCols interface.
// WritableCols 对应 Go 同名函数或方法，保留参数、返回值、关键分支和外部调用形状。
pub fn (vt *VirtualTable) WritableCols() []*table.Column {
    return nil
}

// DeletableCols implements table.Table WritableCols interface.
// DeletableCols 对应 Go 同名函数或方法，保留参数、返回值、关键分支和外部调用形状。
pub fn (vt *VirtualTable) DeletableCols() []*table.Column {
    return nil
}

// FullHiddenColsAndVisibleCols implements table FullHiddenColsAndVisibleCols interface.
// FullHiddenColsAndVisibleCols 对应 Go 同名函数或方法，保留参数、返回值、关键分支和外部调用形状。
pub fn (vt *VirtualTable) FullHiddenColsAndVisibleCols() []*table.Column {
    return nil
}

// Indices implements table.Table Indices interface.
// Indices 对应 Go 同名函数或方法，保留参数、返回值、关键分支和外部调用形状。
pub fn (vt *VirtualTable) Indices() []table.Index {
    return nil
}

// DeletableIndices implements table.Table DeletableIndices interface.
// DeletableIndices 对应 Go 同名函数或方法，保留参数、返回值、关键分支和外部调用形状。
pub fn (vt *VirtualTable) DeletableIndices() []table.Index {
    return nil
}

// WritableConstraint implements table.Table WritableConstraint interface.
// WritableConstraint 对应 Go 同名函数或方法，保留参数、返回值、关键分支和外部调用形状。
pub fn (vt *VirtualTable) WritableConstraint() []*table.Constraint {
    return nil
}

// RecordPrefix implements table.Table RecordPrefix interface.
// RecordPrefix 对应 Go 同名函数或方法，保留参数、返回值、关键分支和外部调用形状。
pub fn (vt *VirtualTable) RecordPrefix() kv.Key {
    return nil
}

// IndexPrefix implements table.Table IndexPrefix interface.
// IndexPrefix 对应 Go 同名函数或方法，保留参数、返回值、关键分支和外部调用形状。
pub fn (vt *VirtualTable) IndexPrefix() kv.Key {
    return nil
}

// AddRecord implements table.Table AddRecord interface.
// AddRecord 保留 INFORMATION_SCHEMA 只读表写路径报错语义。
pub fn (vt *VirtualTable) AddRecord(ctx table.MutateContext, txn kv.Transaction, r []types.Datum, opts ...table.AddRecordOption) (recordID kv.Handle, err error) {
    return nil, table.ErrUnsupportedOp
}

// RemoveRecord implements table.Table RemoveRecord interface.
// RemoveRecord 保留 INFORMATION_SCHEMA 只读表写路径报错语义。
pub fn (vt *VirtualTable) RemoveRecord(ctx table.MutateContext, txn kv.Transaction, h kv.Handle, r []types.Datum, opts ...table.RemoveRecordOption) error {
    return table.ErrUnsupportedOp
}

// UpdateRecord implements table.Table UpdateRecord interface.
// UpdateRecord 保留 INFORMATION_SCHEMA 只读表写路径报错语义。
pub fn (vt *VirtualTable) UpdateRecord(ctx table.MutateContext, txn kv.Transaction, h kv.Handle, oldData, newData []types.Datum, touched []bool, opts ...table.UpdateRecordOption) error {
    return table.ErrUnsupportedOp
}

// Allocators implements table.Table Allocators interface.
// Allocators 对应 Go 同名函数或方法，保留参数、返回值、关键分支和外部调用形状。
pub fn (vt *VirtualTable) Allocators(_ table.AllocatorContext) autoid.Allocators {
    return autoid.Allocators{}
}

// Meta implements table.Table Meta interface.
// Meta 对应 Go 同名函数或方法，保留参数、返回值、关键分支和外部调用形状。
pub fn (vt *VirtualTable) Meta() *model.TableInfo {
    return nil
}

// UseNewCollate implements table.Table UseNewCollate interface. Virtual tables
// are not persisted user tables, so they use the current process default.
// UseNewCollate 对应 Go 同名函数或方法，保留参数、返回值、关键分支和外部调用形状。
pub fn (vt *VirtualTable) UseNewCollate() bool {
    return collate.NewCollationEnabled()
}

// GetPhysicalID implements table.Table GetPhysicalID interface.
// GetPhysicalID 对应 Go 同名函数或方法，保留参数、返回值、关键分支和外部调用形状。
pub fn (vt *VirtualTable) GetPhysicalID() int64 {
    return 0
}

// Type implements table.Table Type interface.
// Type 对应 Go 同名函数或方法，保留参数、返回值、关键分支和外部调用形状。
pub fn (vt *VirtualTable) Type() table.Type {
    return table.VirtualTable
}

// GetTiFlashServerInfo returns all TiFlash server infos
// GetTiFlashServerInfo：对应 Go 同名函数，保留调用顺序与返回形状。
pub fn GetTiFlashServerInfo(store kv.Storage) ([]ServerInfo, error) {
    if config.GetGlobalConfig().DisaggregatedTiFlash {
        return nil, table.ErrUnsupportedOp
    }
    serversInfo, err := GetStoreServerInfo(store)
    if err != nil {
        return nil, err
    }
    serversInfo = FilterClusterServerInfo(serversInfo, set.NewStringSet(kv.TiFlash.Name()), set.NewStringSet())
    return serversInfo, nil
}

// ServerInfoResult contains server info results
// ServerInfoResult 对应远端 diagnostics gRPC 返回后的扁平化结果。
pub struct ServerInfoResult {
    Idx  int
    Rows [][]types.Datum
    Err  error
}

// FetchClusterServerInfoWithoutPrivilegeCheck fetches cluster server information
pub fn FetchClusterServerInfoWithoutPrivilegeCheck(ctx context.Context, vars *variable.SessionVars, serversInfo []ServerInfo, serverInfoType diagnosticspb.ServerInfoType, recordWarningInStmtCtx bool) []ServerInfoResult {
    wg := sync.WaitGroup{}
    ch := make(chan ServerInfoResult, len(serversInfo))
    infoTp := serverInfoType
    for i, srv := range serversInfo {
        address := srv.Address
        remote := address
        if srv.ServerType == "tidb" || srv.ServerType == "tiproxy" {
            remote = srv.StatusAddr
        }
        wg.Add(1)
// Go goroutine 并发分支：Rust 实现需要改为任务/线程并保持错误通过 channel 聚合。
        go func(index int, remote, address, serverTP string) {
            util.WithRecovery(func() {
// Go defer 负责资源收尾或解锁；Rust 接线时必须用作用域守卫覆盖所有提前返回路径。
                defer wg.Done()
                items, err := getServerInfoByGRPC(ctx, remote, infoTp)
                if err != nil {
                    ch <- ServerInfoResult{Idx: index, Err: err}
                    return
                }
                partRows := serverInfoItemToRows(items, serverTP, address)
                ch <- ServerInfoResult{Idx: index, Rows: partRows}
            }, nil)
        }(i, remote, address, srv.ServerType)
    }
    wg.Wait()
    close(ch)
    // Keep the original order to make the result more stable
    pub var results []ServerInfoResult //nolint: prealloc
    for result := range ch {
        if result.Err != nil {
            if recordWarningInStmtCtx {
                vars.StmtCtx.AppendWarning(result.Err)
            } else {
                log.Warn(result.Err.Error())
            }
            continue
        }
        results = append(results, result)
    }
    slices.SortFunc(results, func(i, j ServerInfoResult) int { return cmp.Compare(i.Idx, j.Idx) })
    return results
}

// serverInfoItemToRows 对应 Go 同名函数或方法，保留参数、返回值、关键分支和外部调用形状。
pub fn serverInfoItemToRows(items []*diagnosticspb.ServerInfoItem, tp, addr string) [][]types.Datum {
    rows := make([][]types.Datum, 0, len(items))
    for _, v := range items {
        for _, item := range v.Pairs {
            row := types.MakeDatums(
                tp,
                addr,
                v.Tp,
                v.Name,
                item.Key,
                item.Value,
            )
            rows = append(rows, row)
        }
    }
    return rows
}

pub fn getServerInfoByGRPC(ctx context.Context, address string, tp diagnosticspb.ServerInfoType) ([]*diagnosticspb.ServerInfoItem, error) {
// 外部网络调用保持 Go 形状，这里不会真正发起请求。
    opt := grpc.WithTransportCredentials(insecure.NewCredentials())
    security := config.GetGlobalConfig().Security
    if len(security.ClusterSSLCA) != 0 {
        clusterSecurity := security.ClusterSecurity()
        tlsConfig, err := clusterSecurity.ToTLSConfig()
        if err != nil {
            return nil, errors.Trace(err)
        }
// 外部网络调用保持 Go 形状，这里不会真正发起请求。
        opt = grpc.WithTransportCredentials(credentials.NewTLS(tlsConfig))
    }
// 外部网络调用保持 Go 形状，这里不会真正发起请求。
    conn, err := grpc.Dial(address, opt)
    if err != nil {
        return nil, err
    }
// Go defer 负责资源收尾或解锁；Rust 接线时必须用作用域守卫覆盖所有提前返回路径。
    defer func() {
        err := conn.Close()
        if err != nil {
            log.Error("close grpc connection error", zap.Error(err))
        }
    }()

    cli := diagnosticspb.NewDiagnosticsClient(conn)
    ctx, cancel := context.WithTimeout(ctx, time.Second*10)
// Go defer 负责资源收尾或解锁；Rust 接线时必须用作用域守卫覆盖所有提前返回路径。
    defer cancel()
    r, err := cli.ServerInfo(ctx, &diagnosticspb.ServerInfoRequest{Tp: tp})
    if err != nil {
        return nil, err
    }
    return r.Items, nil
}

// FilterClusterServerInfo filters serversInfo by nodeTypes and addresses
// FilterClusterServerInfo 对应 Go 同名函数或方法，保留参数、返回值、关键分支和外部调用形状。
pub fn FilterClusterServerInfo(serversInfo []ServerInfo, nodeTypes, addresses set.StringSet) []ServerInfo {
    if len(nodeTypes) == 0 && len(addresses) == 0 {
        return serversInfo
    }

    filterServers := make([]ServerInfo, 0, len(serversInfo))
    for _, srv := range serversInfo {
        // Skip some node type which has been filtered in WHERE clause
        // e.g: SELECT * FROM cluster_config WHERE type='tikv'
        if len(nodeTypes) > 0 && !nodeTypes.Exist(srv.ServerType) {
            continue
        }
        // Skip some node address which has been filtered in WHERE clause
        // e.g: SELECT * FROM cluster_config WHERE address='192.16.8.12:2379'
        if len(addresses) > 0 && !addresses.Exist(srv.Address) {
            continue
        }
        filterServers = append(filterServers, srv)
    }
    return filterServers
}

// GetDataFromStatusByConn is getting the per-connection status for `performance_schema.status_by_connection`
// GetDataFromStatusByConn 对应会话内存数据读取路径；这里不访问真实连接，只保留过滤和行拼装语义。
pub fn GetDataFromStatusByConn(sctx sessionctx.Context) ([][]types.Datum, error) {
    sm := sctx.GetSessionManager()
    if sm == nil {
        return nil, nil
    }
    statusVars := sm.GetStatusVars()
    rows := make([][]types.Datum, 0, 2*len(statusVars))
    for pid, svar := range statusVars {
        for varkey, varval := range svar {
            row := types.MakeDatums(
                pid,
                varkey,
                varval,
            )
            rows = append(rows, row)
        }
    }
    return rows, nil
}
*/

// ---- 生产端口：可编译的 INFORMATION_SCHEMA 表注册与集群辅助 ----

#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::sync::{Arc, OnceLock};

use crate::cluster::{ClusterTableTiDBIndexUsage, Datum};
use crate::infoschema::{CiString, ColumnInfo, DBInfo, Table, TableInfo};

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
        _ => vec![
            columnInfo::varchar("INSTANCE", 64),
            columnInfo::varchar("NAME", 128),
            columnInfo::varchar("VALUE", 1024),
        ],
    }
}

// 懒初始化的表名 → VirtualTableMeta 全局注册表。
static TABLE_REGISTRY: OnceLock<HashMap<&'static str, VirtualTableMeta>> = OnceLock::new();
/// 获取 INFORMATION_SCHEMA 虚拟表注册表（只初始化一次）。
pub fn table_registry() -> &'static HashMap<&'static str, VirtualTableMeta> {
    TABLE_REGISTRY.get_or_init(|| {
        TABLE_NAMES
            .iter()
            .enumerate()
            .map(|(index, name)| {
                (
                    *name,
                    VirtualTableMeta {
                        id: index as i64 + 1,
                        name,
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
    TableInfo {
        id,
        name: CiString::new(table_name),
        columns: columns
            .iter()
            .enumerate()
            .map(|(index, column)| buildColumnInfo(index as i64 + 1, column))
            .collect(),
        ..TableInfo::default()
    }
}
/// 构造名为 INFORMATION_SCHEMA 的 DBInfo，包含全部已注册虚拟表。
pub fn information_schema_db() -> DBInfo {
    DBInfo {
        id: -1,
        name: CiString::new("INFORMATION_SCHEMA"),
        tables: table_registry()
            .values()
            .map(|definition| Arc::new(buildTableMeta(definition.name, &definition.columns)))
            .collect(),
        table_name_2_id: Default::default(),
    }
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

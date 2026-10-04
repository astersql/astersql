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

// Information Schema 内存表读取与各类系统表检索器。
//
// 对应 Go 的 infoschema reader：按表名分发到 SCHEMATA/TABLES/COLUMNS 等
// INFORMATION_SCHEMA（信息模式）表，通过 `InfoSchemaDataSource` 拉取行数据；
// 支持事务内快照（snapshot）、谓词下推、分批物化与内存追踪。
// Region 为键空间分片单元；Label Rule 描述分区/表的放置标签。

#![allow(dead_code, non_camel_case_types, non_snake_case)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// 内存表一次物化/返回的批大小。
const BATCH_SIZE: usize = 1024;
/// 查看进程/事务等敏感系统表所需的 PROCESS 权限名。
const PROCESS_PRIVILEGE: &str = "PROCESS";

/// Information Schema 操作的统一 Result 别名。
pub type InfoResult<T = ()> = Result<T, InfoSchemaError>;
/// 一行系统表数据：Datum 单元格序列。
pub type Row = Vec<Datum>;

#[derive(Clone, Debug, PartialEq)]
/// 系统表单元格值的简化代数类型。
pub enum Datum {
    Null,
    Bool(bool),
    Int(i64),
    Uint(u64),
    Float(f64),
    Text(String),
    Bytes(Vec<u8>),
    Time(SystemTime),
    Json(String),
}

impl Datum {
    /// 估算该 Datum 占用的字节数，供内存追踪累计。
    fn estimated_size(&self) -> i64 {
        match self {
            Self::Null => 0,
            Self::Bool(_) => 1,
            Self::Int(_) | Self::Uint(_) | Self::Float(_) | Self::Time(_) => 8,
            Self::Text(value) | Self::Json(value) => value.len() as i64,
            Self::Bytes(value) => value.len() as i64,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Information Schema 读取错误。
pub struct InfoSchemaError(pub String);

impl InfoSchemaError {
    /// 由消息构造错误。
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for InfoSchemaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for InfoSchemaError {}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 输出列元信息：列名与在全行列中的偏移。
pub struct ColumnInfo {
    pub name: String,
    pub offset: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 逻辑表元信息（含分区 ID 列表）。
pub struct TableInfo {
    pub id: i64,
    pub schema: String,
    pub name: String,
    pub columns: Vec<ColumnInfo>,
    pub partition_ids: Vec<i64>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 谓词提取结果：可跳过请求、过滤 schema/表/列及 TiFlash 目标。
pub struct PredicateExtractor {
    pub skip_request: bool,
    pub schemas: BTreeSet<String>,
    pub tables: BTreeSet<String>,
    pub columns: BTreeSet<String>,
    pub predicates: BTreeMap<String, BTreeSet<String>>,
    pub tiflash_instances: BTreeSet<String>,
    pub tiflash_databases: String,
    pub tiflash_tables: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Information Schema 快照版本（对应某时刻的元数据视图）。
pub struct InfoSchemaSnapshot {
    pub version: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 会话事务与用户状态，用于选择快照时间戳。
pub struct SessionState {
    pub in_transaction: bool,
    pub transaction_start_ts: u64,
    pub snapshot_ts: u64,
    pub current_user: String,
    pub current_host: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 字段类型种类，用于数值显示精度换算。
pub enum FieldKind {
    Tiny,
    Short,
    Int24,
    Long,
    LongLong,
    Bit,
    Float,
    Double,
    Decimal,
    Other,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 字段类型描述（种类 + 是否无符号）。
pub struct FieldType {
    pub kind: FieldKind,
    pub unsigned: bool,
}

#[derive(Clone, Debug, PartialEq)]
/// Region（键空间分片）状态行：ID 与字段映射。
pub struct RegionInfo {
    pub id: u64,
    pub fields: BTreeMap<String, Datum>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 放置/标签规则：ID、类型、labels 与 key-range 数据。
pub struct LabelRule {
    pub id: String,
    pub rule_type: String,
    pub labels: Vec<(String, String)>,
    pub data: Vec<BTreeMap<String, String>>,
    pub keyspace_mode: bool,
}

#[derive(Clone, Debug, PartialEq)]
/// 死锁记录：等待链上的各行。
pub struct DeadlockRecord {
    pub wait_chain: Vec<Row>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// TiFlash 实例地址信息。
pub struct TiFlashInstance {
    pub address: String,
    pub status_address: String,
}

#[derive(Clone, Debug, PartialEq)]
/// 向数据源发起的具体拉取请求变体。
pub enum DataRequest {
    VariablesInfo,
    UserAttributes,
    Schemata,
    Statistics,
    StatisticsInTable {
        schema: String,
        table_id: i64,
    },
    ReferentialConstraints,
    TableRow {
        schema: String,
        table_id: i64,
    },
    Tables,
    CheckConstraints {
        tidb_extended: bool,
    },
    Columns {
        schema_cursor: usize,
        table_cursor: usize,
        batch: usize,
    },
    ColumnsForTable {
        schema: String,
        table_id: i64,
    },
    ColumnRows {
        schema: String,
        table_id: i64,
        privilege_mask: u64,
    },
    Partitions,
    Indexes,
    Index {
        schema: String,
        table_id: i64,
    },
    Views,
    TiKvStoreStatus,
    Engines,
    CharacterSets,
    Collations,
    CollationCharacterSetApplicability,
    ClusterInfo,
    KeyColumnUsage,
    ProcessList {
        cluster: bool,
    },
    UserPrivileges,
    MetricTables,
    KeyColumnUsageInTable {
        schema: String,
        table_id: i64,
    },
    TiKvRegionStatus,
    RegionsForTable {
        table_id: i64,
    },
    RegionsForSingleTable {
        table_id: i64,
    },
    RegionStatusColumn {
        region_id: u64,
    },
    TiDbHotRegions,
    HotRegionsByMetrics {
        metric_type: String,
    },
    TableConstraints,
    TableStorageInitial,
    TableStorageStats {
        table_cursor: usize,
        batch: usize,
    },
    AnalyzeStatus,
    AnalyzeRemaining {
        database: String,
        table: String,
        partition: String,
        processed_rows: i64,
    },
    Profiling,
    ServersInfo,
    Sequences,
    TiFlashReplica,
    ClientErrorsSummary {
        table: String,
    },
    TransactionSummary {
        cluster: bool,
    },
    MemoryUsage {
        cluster: bool,
        history: bool,
    },
    Transactions {
        cursor: usize,
        batch: usize,
    },
    DataLockWaits {
        cursor: usize,
        batch: usize,
    },
    Deadlocks {
        record: usize,
        wait_chain: usize,
        batch: usize,
    },
    TiFlashSystemTable {
        instance: String,
        row_offset: usize,
        limit: usize,
        databases: String,
        tables: String,
    },
    Attributes,
    PlacementPolicies,
    RunawayWatches,
    ResourceGroups,
    Keywords,
    IndexUsage {
        cluster: bool,
    },
    PlanCache {
        cluster: bool,
    },
    KeyspaceMeta,
}

/// 内存消耗追踪接口。
pub trait MemoryTracker: Send + Sync {
    fn consume(&self, bytes: i64);
}

/// Information Schema 数据源：会话、快照、行加载与权限等。
pub trait InfoSchemaDataSource: Send + Sync {
    /// The concrete MySQL manager and active roles bound to this session.
    /// None also represents a foreign manager, for which Go leaves rows visible.
    fn user_attributes_privileges(
        &self,
    ) -> Option<(
        astersql_privilege_privileges::UserPrivileges,
        Vec<astersql_privilege_privileges::RoleIdentity>,
    )> {
        None
    }

    fn session_state(&self) -> InfoResult<SessionState>;
    fn snapshot_info_schema(&self, timestamp: u64) -> InfoResult<InfoSchemaSnapshot>;
    fn latest_info_schema(&self) -> InfoResult<InfoSchemaSnapshot>;
    fn transaction_info_schema(&self) -> InfoResult<InfoSchemaSnapshot>;
    fn load_rows(
        &self,
        request: DataRequest,
        snapshot: Option<&InfoSchemaSnapshot>,
        extractor: Option<&PredicateExtractor>,
    ) -> InfoResult<Vec<Row>>;
    fn privilege_verification(
        &self,
        privilege: &str,
        schema: &str,
        table: &str,
    ) -> InfoResult<Option<bool>>;
    fn auto_increment_id(
        &self,
        snapshot: &InfoSchemaSnapshot,
        table_id: i64,
    ) -> InfoResult<Option<i64>>;
    fn update_stats_cache(&self, table_ids: &[i64]) -> InfoResult;
    fn ddl_jobs_open(&self, snapshot: &InfoSchemaSnapshot) -> InfoResult<u64>;
    fn ddl_jobs_next(&self, token: u64, capacity: usize) -> InfoResult<Vec<Row>>;
    fn ddl_jobs_close(&self, token: u64) -> InfoResult;
    fn initial_tables(&self, extractor: &PredicateExtractor) -> InfoResult<Vec<initialTable>>;
    fn transaction_row_count(&self) -> InfoResult<usize>;
    fn data_lock_wait_count(&self) -> InfoResult<usize>;
    fn deadlock_records(&self) -> InfoResult<Vec<DeadlockRecord>>;
    fn tiflash_instances(&self, selected: &BTreeSet<String>) -> InfoResult<Vec<TiFlashInstance>>;
    fn analyze_total_count(&self, database: &str, table: &str, partition: &str) -> InfoResult<f64>;
    fn decode_table_id_from_start_key(&self, key: &[u8]) -> InfoResult<i64>;
    fn table_matches_id(
        &self,
        database: &str,
        table: &str,
        partition: &str,
        table_id: i64,
    ) -> InfoResult<bool>;
}

/// 通用内存表检索器：按表名分发并分批返回行。
pub struct memtableRetriever {
    pub table: TableInfo,
    pub columns: Vec<ColumnInfo>,
    pub rows: Vec<Row>,
    pub row_idx: usize,
    pub retrieved: bool,
    pub initialized: bool,
    pub extractor: PredicateExtractor,
    pub info_schema: Option<InfoSchemaSnapshot>,
    pub mem_tracker: Option<Arc<dyn MemoryTracker>>,
    pub accumulated_memory_per_batch: i64,
    pub accumulated_memory_record_count: usize,
    pub source: Arc<dyn InfoSchemaDataSource>,
}

impl memtableRetriever {
    /// 初始化快照并分发装载后，按批返回行并做列投影。
    pub fn retrieve(&mut self) -> InfoResult<Vec<Row>> {
        // CLUSTER_INFO 需要 PROCESS 权限
        if self.table.name.eq_ignore_ascii_case("CLUSTER_INFO")
            && !hasPriv(self.source.as_ref(), PROCESS_PRIVILEGE)?
        {
            return Err(InfoSchemaError::new("PROCESS privilege is required"));
        }
        if self.retrieved {
            return Ok(Vec::new());
        }
        if !self.initialized {
            let state = self.source.session_state()?;
            // 事务内用 snapshot_ts 或 start_ts 取元数据快照；否则用最新
            self.info_schema = Some(if state.in_transaction {
                let timestamp = if state.snapshot_ts != 0 {
                    state.snapshot_ts
                } else {
                    state.transaction_start_ts
                };
                self.source.snapshot_info_schema(timestamp)?
            } else {
                self.source.latest_info_schema()?
            });
            self.dispatch_table()?;
            self.initialized = true;
            self.flush_memory();
        }
        // 按 BATCH_SIZE 切片返回，耗尽后标记 retrieved
        let end = self.rows.len().min(self.row_idx.saturating_add(BATCH_SIZE));
        let result = self.rows[self.row_idx..end].to_vec();
        self.row_idx = end;
        self.retrieved = end == self.rows.len();
        adjustColumns(&result, &self.columns, &self.table)
    }

    /// 按 INFORMATION_SCHEMA 表名分发到对应 setData 方法。
    fn dispatch_table(&mut self) -> InfoResult {
        match self.table.name.to_ascii_uppercase().as_str() {
            "SCHEMATA" => self.setDataFromSchemata(),
            "STATISTICS" => self.setDataForStatistics(),
            "TABLES" => self.setDataFromTables(),
            "REFERENTIAL_CONSTRAINTS" => self.setDataFromReferConst(),
            "SEQUENCES" => self.setDataFromSequences(),
            "PARTITIONS" => self.setDataFromPartitions(),
            "CLUSTER_INFO" => self.dataForTiDBClusterInfo(),
            "ANALYZE_STATUS" => self.setDataForAnalyzeStatus(),
            "TIDB_INDEXES" => self.setDataFromIndexes(),
            "VIEWS" => self.setDataFromViews(),
            "ENGINES" => self.setDataFromEngines(),
            "CHARACTER_SETS" => self.setDataFromCharacterSets(),
            "COLLATIONS" => self.setDataFromCollations(),
            "KEY_COLUMN_USAGE" => self.setDataFromKeyColumnUsage(),
            "METRICS_TABLES" => self.setDataForMetricTables(),
            "PROFILING" => self.setDataForPseudoProfiling(),
            "COLLATION_CHARACTER_SET_APPLICABILITY" => {
                self.dataForCollationCharacterSetApplicability()
            }
            "PROCESSLIST" => self.setDataForProcessList(),
            "CLUSTER_PROCESSLIST" => self.setDataForClusterProcessList(),
            "USER_PRIVILEGES" => self.setDataFromUserPrivileges(),
            "TIKV_REGION_STATUS" => self.setDataForTiKVRegionStatus(),
            "TIDB_HOT_REGIONS" => self.setDataForTiDBHotRegions(),
            "TABLE_CONSTRAINTS" => self.setDataFromTableConstraints(),
            "TIDB_SERVERS_INFO" => self.setDataForServersInfo(),
            "TIFLASH_REPLICA" => self.dataForTableTiFlashReplica(),
            "TIKV_STORE_STATUS" => self.dataForTiKVStoreStatus(),
            "CLIENT_ERRORS_SUMMARY_GLOBAL"
            | "CLIENT_ERRORS_SUMMARY_BY_USER"
            | "CLIENT_ERRORS_SUMMARY_BY_HOST" => {
                let table = self.table.name.clone();
                self.setDataForClientErrorsSummary(&table)
            }
            "ATTRIBUTES" => self.setDataForAttributes(),
            "PLACEMENT_POLICIES" => self.setDataFromPlacementPolicies(),
            "TRX_SUMMARY" => self.setDataForTrxSummary(),
            "CLUSTER_TRX_SUMMARY" => self.setDataForClusterTrxSummary(),
            "VARIABLES_INFO" => self.setDataForVariablesInfo(),
            "USER_ATTRIBUTES" => self.setDataForUserAttributes(),
            "MEMORY_USAGE" => self.setDataForMemoryUsage(),
            "CLUSTER_MEMORY_USAGE" => self.setDataForClusterMemoryUsage(),
            "MEMORY_USAGE_OPS_HISTORY" => self.setDataForMemoryUsageOpsHistory(),
            "CLUSTER_MEMORY_USAGE_OPS_HISTORY" => self.setDataForClusterMemoryUsageOpsHistory(),
            "RESOURCE_GROUPS" => self.setDataFromResourceGroups(),
            "RUNAWAY_WATCHES" => self.setDataFromRunawayWatches(),
            "CHECK_CONSTRAINTS" => self.setDataFromCheckConstraints(),
            "TIDB_CHECK_CONSTRAINTS" => self.setDataFromTiDBCheckConstraints(),
            "KEYWORDS" => self.setDataFromKeywords(),
            "TIDB_INDEX_USAGE" => self.setDataFromIndexUsage(),
            "CLUSTER_TIDB_INDEX_USAGE" => self.setDataFromClusterIndexUsage(),
            "TIDB_PLAN_CACHE" => self.setDataFromPlanCache(false),
            "CLUSTER_TIDB_PLAN_CACHE" => self.setDataFromPlanCache(true),
            "KEYSPACE_META" => self.setDataForKeyspaceMeta(),
            _ => Ok(()),
        }
    }

    /// 累计一行内存占用，达批大小时刷到 tracker。
    pub fn recordMemoryConsume(&mut self, data: &[Datum]) {
        if self.mem_tracker.is_none() {
            return;
        }
        self.accumulated_memory_per_batch += data.iter().map(Datum::estimated_size).sum::<i64>();
        self.accumulated_memory_record_count += 1;
        if self.accumulated_memory_record_count >= BATCH_SIZE {
            self.flush_memory();
        }
    }

    /// 将累计内存消耗提交给 MemoryTracker 并清零。
    fn flush_memory(&mut self) {
        if self.accumulated_memory_record_count == 0 {
            return;
        }
        if let Some(tracker) = &self.mem_tracker {
            tracker.consume(self.accumulated_memory_per_batch);
        }
        self.accumulated_memory_per_batch = 0;
        self.accumulated_memory_record_count = 0;
    }

    /// 用一次 load_rows 结果整体替换当前行缓冲。
    fn replace_rows(&mut self, request: DataRequest) -> InfoResult {
        let rows =
            self.source
                .load_rows(request, self.info_schema.as_ref(), Some(&self.extractor))?;
        for row in &rows {
            self.recordMemoryConsume(row);
        }
        self.rows = rows;
        Ok(())
    }

    /// 追加 load_rows 结果到当前行缓冲。
    fn append_rows(&mut self, request: DataRequest) -> InfoResult {
        let rows =
            self.source
                .load_rows(request, self.info_schema.as_ref(), Some(&self.extractor))?;
        for row in &rows {
            self.recordMemoryConsume(row);
        }
        self.rows.extend(rows);
        Ok(())
    }

    /// 加载 VARIABLES_INFO 行。
    pub fn setDataForVariablesInfo(&mut self) -> InfoResult {
        self.replace_rows(DataRequest::VariablesInfo)
    }
    /// 加载 USER_ATTRIBUTES 行。
    pub fn setDataForUserAttributes(&mut self) -> InfoResult {
        let rows = self.source.load_rows(
            DataRequest::UserAttributes,
            self.info_schema.as_ref(),
            Some(&self.extractor),
        )?;
        if rows.is_empty() {
            return Ok(());
        }
        let viewer = self.source.session_state()?;
        let privileges = self.source.user_attributes_privileges();
        let (manager, roles) = match privileges.as_ref() {
            Some((manager, roles)) => (Some(manager), roles.as_slice()),
            None => (None, &[][..]),
        };
        let filter = astersql_privilege_privileges::NewUserAttrFilter(
            roles,
            &viewer.current_user,
            &viewer.current_host,
            manager,
        );
        let mut visible_rows = Vec::with_capacity(rows.len());
        for mut row in rows {
            if row.len() != 3 {
                continue;
            }
            let (Datum::Text(user), Datum::Text(host)) = (&row[0], &row[1]) else {
                continue;
            };
            if !filter.Visible(user, host) {
                continue;
            }
            if matches!(&row[2], Datum::Text(attribute) if attribute.is_empty()) {
                row[2] = Datum::Null;
            }
            self.recordMemoryConsume(&row);
            visible_rows.push(row);
        }
        self.rows = visible_rows;
        Ok(())
    }
    /// 加载 SCHEMATA 行。
    pub fn setDataFromSchemata(&mut self) -> InfoResult {
        self.replace_rows(DataRequest::Schemata)
    }
    /// 加载 STATISTICS 行。
    pub fn setDataForStatistics(&mut self) -> InfoResult {
        self.replace_rows(DataRequest::Statistics)
    }
    /// 追加指定表的 STATISTICS 行。
    pub fn setDataForStatisticsInTable(&mut self, schema: &str, table_id: i64) -> InfoResult {
        self.append_rows(DataRequest::StatisticsInTable {
            schema: schema.to_owned(),
            table_id,
        })
    }
    /// 加载 REFERENTIAL_CONSTRAINTS 行。
    pub fn setDataFromReferConst(&mut self) -> InfoResult {
        self.replace_rows(DataRequest::ReferentialConstraints)
    }
    /// 若输出列依赖统计信息则刷新 stats cache。
    pub fn updateStatsCacheIfNeed(&self, tables: &[TableInfo]) -> InfoResult {
        let needs = self.columns.iter().any(|column| {
            matches!(
                column.name.to_ascii_uppercase().as_str(),
                "AVG_ROW_LENGTH" | "DATA_LENGTH" | "INDEX_LENGTH" | "TABLE_ROWS"
            )
        });
        if !needs {
            return Ok(());
        }
        let mut ids = Vec::new();
        for table in tables {
            ids.extend_from_slice(&table.partition_ids);
            ids.push(table.id);
        }
        self.source.update_stats_cache(&ids)
    }
    /// 追加单表 TABLES 行。
    pub fn setDataFromOneTable(&mut self, schema: &str, table_id: i64) -> InfoResult {
        self.append_rows(DataRequest::TableRow {
            schema: schema.to_owned(),
            table_id,
        })
    }
    /// 加载 TABLES 行。
    pub fn setDataFromTables(&mut self) -> InfoResult {
        self.replace_rows(DataRequest::Tables)
    }
    /// 加载标准 CHECK_CONSTRAINTS。
    pub fn setDataFromCheckConstraints(&mut self) -> InfoResult {
        self.replace_rows(DataRequest::CheckConstraints {
            tidb_extended: false,
        })
    }
    /// 加载 TiDB 扩展 CHECK_CONSTRAINTS。
    pub fn setDataFromTiDBCheckConstraints(&mut self) -> InfoResult {
        self.replace_rows(DataRequest::CheckConstraints {
            tidb_extended: true,
        })
    }
    /// 加载 PARTITIONS 行。
    pub fn setDataFromPartitions(&mut self) -> InfoResult {
        self.replace_rows(DataRequest::Partitions)
    }
    /// 加载 TIDB_INDEXES 行。
    pub fn setDataFromIndexes(&mut self) -> InfoResult {
        self.replace_rows(DataRequest::Indexes)
    }
    /// 追加指定表的索引行。
    pub fn setDataFromIndex(&mut self, schema: &str, table_id: i64) -> InfoResult {
        self.append_rows(DataRequest::Index {
            schema: schema.to_owned(),
            table_id,
        })
    }
    /// 加载 VIEWS 行。
    pub fn setDataFromViews(&mut self) -> InfoResult {
        self.replace_rows(DataRequest::Views)
    }
    /// 加载 TIKV_STORE_STATUS 行。
    pub fn dataForTiKVStoreStatus(&mut self) -> InfoResult {
        self.replace_rows(DataRequest::TiKvStoreStatus)
    }
    /// 加载 ENGINES 行。
    pub fn setDataFromEngines(&mut self) -> InfoResult {
        self.replace_rows(DataRequest::Engines)
    }
    /// 加载 CHARACTER_SETS 行。
    pub fn setDataFromCharacterSets(&mut self) -> InfoResult {
        self.replace_rows(DataRequest::CharacterSets)
    }
    /// 加载 COLLATIONS 行。
    pub fn setDataFromCollations(&mut self) -> InfoResult {
        self.replace_rows(DataRequest::Collations)
    }
    /// 加载 COLLATION_CHARACTER_SET_APPLICABILITY。
    pub fn dataForCollationCharacterSetApplicability(&mut self) -> InfoResult {
        self.replace_rows(DataRequest::CollationCharacterSetApplicability)
    }
    /// 加载 CLUSTER_INFO 行。
    pub fn dataForTiDBClusterInfo(&mut self) -> InfoResult {
        self.replace_rows(DataRequest::ClusterInfo)
    }
    /// 加载 KEY_COLUMN_USAGE 行。
    pub fn setDataFromKeyColumnUsage(&mut self) -> InfoResult {
        self.replace_rows(DataRequest::KeyColumnUsage)
    }
    /// 加载集群 PROCESSLIST。
    pub fn setDataForClusterProcessList(&mut self) -> InfoResult {
        self.replace_rows(DataRequest::ProcessList { cluster: true })
    }
    /// 加载本节点 PROCESSLIST。
    pub fn setDataForProcessList(&mut self) -> InfoResult {
        self.replace_rows(DataRequest::ProcessList { cluster: false })
    }
    /// 加载 USER_PRIVILEGES 行。
    pub fn setDataFromUserPrivileges(&mut self) -> InfoResult {
        self.replace_rows(DataRequest::UserPrivileges)
    }
    /// 加载 METRICS_TABLES 行。
    pub fn setDataForMetricTables(&mut self) -> InfoResult {
        self.replace_rows(DataRequest::MetricTables)
    }
    /// 查询指定表的 KEY_COLUMN_USAGE 行。
    pub fn keyColumnUsageInTable(&mut self, schema: &str, table_id: i64) -> InfoResult<Vec<Row>> {
        self.source.load_rows(
            DataRequest::KeyColumnUsageInTable {
                schema: schema.to_owned(),
                table_id,
            },
            self.info_schema.as_ref(),
            Some(&self.extractor),
        )
    }
    /// 加载 TIKV_REGION_STATUS 行。
    pub fn setDataForTiKVRegionStatus(&mut self) -> InfoResult {
        self.replace_rows(DataRequest::TiKvRegionStatus)
    }
    /// 获取表相关 Region 信息行。
    pub fn getRegionsInfoForTable(&self, table_id: i64) -> InfoResult<Vec<Row>> {
        self.source.load_rows(
            DataRequest::RegionsForTable { table_id },
            self.info_schema.as_ref(),
            Some(&self.extractor),
        )
    }
    /// 获取单表 Region 信息行。
    pub fn getRegionsInfoForSingleTable(&self, table_id: i64) -> InfoResult<Vec<Row>> {
        self.source.load_rows(
            DataRequest::RegionsForSingleTable { table_id },
            self.info_schema.as_ref(),
            Some(&self.extractor),
        )
    }
    /// 追加单个 Region 的状态列。
    pub fn setNewTiKVRegionStatusCol(&mut self, region: &RegionInfo) -> InfoResult {
        self.append_rows(DataRequest::RegionStatusColumn {
            region_id: region.id,
        })
    }
    /// 加载 TIDB_HOT_REGIONS 行。
    pub fn setDataForTiDBHotRegions(&mut self) -> InfoResult {
        self.replace_rows(DataRequest::TiDbHotRegions)
    }
    /// 按指标类型追加热点 Region 行。
    pub fn setDataForHotRegionByMetrics(&mut self, metric_type: &str) -> InfoResult {
        self.append_rows(DataRequest::HotRegionsByMetrics {
            metric_type: metric_type.to_owned(),
        })
    }
    /// 加载 TABLE_CONSTRAINTS 行。
    pub fn setDataFromTableConstraints(&mut self) -> InfoResult {
        self.replace_rows(DataRequest::TableConstraints)
    }
    /// 通过 helper 加载 ANALYZE_STATUS 并记内存。
    pub fn setDataForAnalyzeStatus(&mut self) -> InfoResult {
        let source = Arc::clone(&self.source);
        self.rows = dataForAnalyzeStatusHelper(source.as_ref(), Some(self))?;
        Ok(())
    }
    /// 加载伪 PROFILING 行。
    pub fn setDataForPseudoProfiling(&mut self) -> InfoResult {
        self.replace_rows(DataRequest::Profiling)
    }
    /// 加载 TIDB_SERVERS_INFO 行。
    pub fn setDataForServersInfo(&mut self) -> InfoResult {
        self.replace_rows(DataRequest::ServersInfo)
    }
    /// 加载 SEQUENCES 行。
    pub fn setDataFromSequences(&mut self) -> InfoResult {
        self.replace_rows(DataRequest::Sequences)
    }
    /// 加载 TIFLASH_REPLICA 行。
    pub fn dataForTableTiFlashReplica(&mut self) -> InfoResult {
        self.replace_rows(DataRequest::TiFlashReplica)
    }
    /// 加载客户端错误汇总表。
    pub fn setDataForClientErrorsSummary(&mut self, table: &str) -> InfoResult {
        self.replace_rows(DataRequest::ClientErrorsSummary {
            table: table.to_owned(),
        })
    }
    /// 加载本节点 TRX_SUMMARY。
    pub fn setDataForTrxSummary(&mut self) -> InfoResult {
        self.replace_rows(DataRequest::TransactionSummary { cluster: false })
    }
    /// 加载集群 TRX_SUMMARY。
    pub fn setDataForClusterTrxSummary(&mut self) -> InfoResult {
        self.replace_rows(DataRequest::TransactionSummary { cluster: true })
    }
    /// 加载本节点 MEMORY_USAGE。
    pub fn setDataForMemoryUsage(&mut self) -> InfoResult {
        self.replace_rows(DataRequest::MemoryUsage {
            cluster: false,
            history: false,
        })
    }
    /// 加载集群 MEMORY_USAGE。
    pub fn setDataForClusterMemoryUsage(&mut self) -> InfoResult {
        self.replace_rows(DataRequest::MemoryUsage {
            cluster: true,
            history: false,
        })
    }
    /// 加载本节点内存操作历史。
    pub fn setDataForMemoryUsageOpsHistory(&mut self) -> InfoResult {
        self.replace_rows(DataRequest::MemoryUsage {
            cluster: false,
            history: true,
        })
    }
    /// 加载集群内存操作历史。
    pub fn setDataForClusterMemoryUsageOpsHistory(&mut self) -> InfoResult {
        self.replace_rows(DataRequest::MemoryUsage {
            cluster: true,
            history: true,
        })
    }
    /// 加载 ATTRIBUTES 行。
    pub fn setDataForAttributes(&mut self) -> InfoResult {
        self.replace_rows(DataRequest::Attributes)
    }
    /// 加载 PLACEMENT_POLICIES 行。
    pub fn setDataFromPlacementPolicies(&mut self) -> InfoResult {
        self.replace_rows(DataRequest::PlacementPolicies)
    }
    /// 加载 RUNAWAY_WATCHES 行。
    pub fn setDataFromRunawayWatches(&mut self) -> InfoResult {
        self.replace_rows(DataRequest::RunawayWatches)
    }
    /// 加载 RESOURCE_GROUPS 行。
    pub fn setDataFromResourceGroups(&mut self) -> InfoResult {
        self.replace_rows(DataRequest::ResourceGroups)
    }
    /// 加载 KEYWORDS 行。
    pub fn setDataFromKeywords(&mut self) -> InfoResult {
        self.replace_rows(DataRequest::Keywords)
    }
    /// 加载本节点 TIDB_INDEX_USAGE。
    pub fn setDataFromIndexUsage(&mut self) -> InfoResult {
        self.replace_rows(DataRequest::IndexUsage { cluster: false })
    }
    /// 加载集群 TIDB_INDEX_USAGE。
    pub fn setDataFromClusterIndexUsage(&mut self) -> InfoResult {
        self.replace_rows(DataRequest::IndexUsage { cluster: true })
    }
    /// 加载计划缓存表（可含集群视图）。
    pub fn setDataFromPlanCache(&mut self, cluster: bool) -> InfoResult {
        self.replace_rows(DataRequest::PlanCache { cluster })
    }
    /// 加载 KEYSPACE_META 行。
    pub fn setDataForKeyspaceMeta(&mut self) -> InfoResult {
        self.replace_rows(DataRequest::KeyspaceMeta)
    }
}

/// 读取表自增 ID；失败或缺失时返回 0。
pub fn getAutoIncrementID(
    source: &dyn InfoSchemaDataSource,
    snapshot: &InfoSchemaSnapshot,
    table_id: i64,
) -> i64 {
    source
        .auto_increment_id(snapshot, table_id)
        .ok()
        .flatten()
        .unwrap_or(0)
}

/// 检查会话是否具备指定权限（缺省视为有权限）。
pub fn hasPriv(source: &dyn InfoSchemaDataSource, privilege: &str) -> InfoResult<bool> {
    Ok(source
        .privilege_verification(privilege, "", "")?
        .unwrap_or(true))
}

/// 输出列是否仅含 schema/表/catalog 标识列。
pub fn onlySchemaOrTableColumns(columns: &[ColumnInfo]) -> bool {
    columns.len() <= 3
        && columns.iter().all(|column| {
            matches!(
                column.name.to_ascii_lowercase().as_str(),
                "table_schema" | "table_name" | "table_catalog"
            )
        })
}

/// 谓词是否仅作用于 schema/表/catalog 列。
pub fn onlySchemaOrTableColPredicates(predicates: &BTreeMap<String, BTreeSet<String>>) -> bool {
    predicates.keys().all(|name| {
        matches!(
            name.to_ascii_lowercase().as_str(),
            "table_schema" | "table_name" | "table_catalog"
        )
    })
}

/// 大表（如 COLUMNS）分批游标检索器。
pub struct hugeMemTableRetriever {
    pub extractor: PredicateExtractor,
    pub table: TableInfo,
    pub columns: Vec<ColumnInfo>,
    pub retrieved: bool,
    pub initialized: bool,
    pub rows: Vec<Row>,
    pub database_index: usize,
    pub table_index: usize,
    pub batch: usize,
    pub info_schema: Option<InfoSchemaSnapshot>,
    pub source: Arc<dyn InfoSchemaDataSource>,
}

impl hugeMemTableRetriever {
    /// 分批加载 COLUMNS；skip 或耗尽时返回空。
    pub fn retrieve(&mut self) -> InfoResult<Vec<Row>> {
        if self.extractor.skip_request || self.retrieved {
            return Ok(Vec::new());
        }
        if !self.initialized {
            self.info_schema = Some(self.source.transaction_info_schema()?);
            self.initialized = true;
            self.batch = BATCH_SIZE;
        }
        self.setDataForColumns()?;
        self.retrieved = self.rows.is_empty();
        adjustColumns(&self.rows, &self.columns, &self.table)
    }

    /// 按库/表游标分批加载 COLUMNS。
    pub fn setDataForColumns(&mut self) -> InfoResult {
        self.rows = self.source.load_rows(
            DataRequest::Columns {
                schema_cursor: self.database_index,
                table_cursor: self.table_index,
                batch: self.batch,
            },
            self.info_schema.as_ref(),
            Some(&self.extractor),
        )?;
        if self.rows.len() < self.batch {
            self.retrieved = true;
        } else {
            self.table_index += self.rows.len();
        }
        Ok(())
    }

    /// 追加单表列定义；满批返回 true。
    pub fn setDataForColumnsWithOneTable(
        &mut self,
        schema: &str,
        table_id: i64,
    ) -> InfoResult<bool> {
        let rows = self.source.load_rows(
            DataRequest::ColumnsForTable {
                schema: schema.to_owned(),
                table_id,
            },
            self.info_schema.as_ref(),
            Some(&self.extractor),
        )?;
        self.rows.extend(rows);
        Ok(self.rows.len() >= self.batch)
    }

    /// 按权限掩码追加表内列行。
    pub fn dataForColumnsInTable(
        &mut self,
        schema: &str,
        table_id: i64,
        privilege_mask: u64,
    ) -> InfoResult {
        let rows = self.source.load_rows(
            DataRequest::ColumnRows {
                schema: schema.to_owned(),
                table_id,
                privilege_mask,
            },
            self.info_schema.as_ref(),
            Some(&self.extractor),
        )?;
        self.rows.extend(rows);
        Ok(())
    }
}

/// 按字段种类返回 MySQL 风格数值显示精度。
pub fn getNumericPrecision(field_type: &FieldType, column_length: i32) -> i32 {
    match field_type.kind {
        FieldKind::Tiny => 3,
        FieldKind::Short => 5,
        FieldKind::Int24 if field_type.unsigned => 8,
        FieldKind::Int24 => 7,
        FieldKind::Long => 10,
        FieldKind::LongLong if field_type.unsigned => 20,
        FieldKind::LongLong => 19,
        FieldKind::Bit | FieldKind::Float | FieldKind::Double | FieldKind::Decimal => column_length,
        FieldKind::Other => 0,
    }
}

/// 按字符集最大字节数将字符长度换算为八位组长度。
pub fn calcCharOctLength(character_length: i32, charset: &str) -> i32 {
    let maximum_length = match charset.to_ascii_lowercase().as_str() {
        "utf8mb4" => 4,
        "utf8" | "utf8mb3" => 3,
        "gbk" | "ucs2" => 2,
        "utf16" | "utf16le" => 4,
        "utf32" => 4,
        _ => 1,
    };
    maximum_length * character_length
}

/// DDL 任务列表流式读取执行器（Open/Next/Close）。
pub struct DDLJobsReaderExec {
    pub rows: Vec<Row>,
    pub cursor: usize,
    pub maximum_chunk_size: usize,
    pub session_token: Option<u64>,
    pub info_schema: InfoSchemaSnapshot,
    pub source: Arc<dyn InfoSchemaDataSource>,
}

impl DDLJobsReaderExec {
    /// 打开 DDL jobs 会话令牌。
    pub fn Open(&mut self) -> InfoResult {
        self.session_token = Some(self.source.ddl_jobs_open(&self.info_schema)?);
        Ok(())
    }
    /// 拉取下一批 DDL jobs 行。
    pub fn Next(&mut self) -> InfoResult<Vec<Row>> {
        let token = self
            .session_token
            .ok_or_else(|| InfoSchemaError::new("DDL jobs reader is not open"))?;
        let rows = self.source.ddl_jobs_next(token, self.maximum_chunk_size)?;
        self.cursor += rows.len();
        Ok(rows)
    }
    /// 关闭并释放 DDL jobs 会话令牌。
    pub fn Close(&mut self) -> InfoResult {
        if let Some(token) = self.session_token.take() {
            self.source.ddl_jobs_close(token)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 表存储统计初始化用的库表对。
pub struct initialTable {
    pub database: String,
    pub table: TableInfo,
}

/// TABLE_STORAGE_STATS 分批检索器。
pub struct tableStorageStatsRetriever {
    pub table: TableInfo,
    pub output_columns: Vec<ColumnInfo>,
    pub retrieved: bool,
    pub initialized: bool,
    pub extractor: PredicateExtractor,
    pub initial_tables: Vec<initialTable>,
    pub current_table: usize,
    pub source: Arc<dyn InfoSchemaDataSource>,
}

impl tableStorageStatsRetriever {
    /// 初始化后按表游标分批返回存储统计行。
    pub fn retrieve(&mut self) -> InfoResult<Vec<Row>> {
        if self.retrieved {
            return Ok(Vec::new());
        }
        if !self.initialized {
            self.initialize()?;
        }
        if self.current_table >= self.initial_tables.len() {
            self.retrieved = true;
            return Ok(Vec::new());
        }
        let rows = self.setDataForTableStorageStats()?;
        adjustColumns(&rows, &self.output_columns, &self.table)
    }
    /// 要求 TABLE_SCHEMA 谓词并加载初始表列表。
    pub fn initialize(&mut self) -> InfoResult {
        if self.extractor.schemas.is_empty() {
            return Err(InfoSchemaError::new("TABLE_SCHEMA predicate is required"));
        }
        self.initial_tables = self.source.initial_tables(&self.extractor)?;
        self.initialized = true;
        Ok(())
    }
    /// 按当前表游标加载一批存储统计行。
    pub fn setDataForTableStorageStats(&mut self) -> InfoResult<Vec<Row>> {
        let rows = self.source.load_rows(
            DataRequest::TableStorageStats {
                table_cursor: self.current_table,
                batch: BATCH_SIZE,
            },
            None,
            Some(&self.extractor),
        )?;
        self.current_table = self
            .initial_tables
            .len()
            .min(self.current_table + BATCH_SIZE);
        Ok(rows)
    }
}

/// 加载 ANALYZE_STATUS；可选地记入 memtable 内存消耗。
pub fn dataForAnalyzeStatusHelper(
    source: &dyn InfoSchemaDataSource,
    mut retriever: Option<&mut memtableRetriever>,
) -> InfoResult<Vec<Row>> {
    let rows = source.load_rows(DataRequest::AnalyzeStatus, None, None)?;
    if let Some(retriever) = retriever.as_mut() {
        for row in &rows {
            retriever.recordMemoryConsume(row);
        }
    }
    Ok(rows)
}

/// 计算 ANALYZE 剩余时间、进度百分比与总行数。
pub fn getRemainDurationForAnalyzeStatusHelper(
    source: &dyn InfoSchemaDataSource,
    start_time: SystemTime,
    database: &str,
    table: &str,
    partition: &str,
    processed_rows: i64,
) -> InfoResult<(Duration, f64, f64)> {
    let duration = SystemTime::now()
        .duration_since(start_time)
        .unwrap_or_default();
    let total = source.analyze_total_count(database, table, partition)?;
    let (remaining, percentage) =
        calRemainInfoForAnalyzeStatus(total as i64, processed_rows, duration);
    Ok((remaining, percentage, total))
}

/// 由总量、已处理行与已用时长估算剩余 Duration 与进度比。
pub fn calRemainInfoForAnalyzeStatus(
    total_count: i64,
    processed_rows: i64,
    duration: Duration,
) -> (Duration, f64) {
    if total_count == 0 {
        return (Duration::ZERO, 100.0);
    }
    let remaining_lines = total_count - processed_rows;
    let processed = processed_rows.max(1);
    // Go only substitutes one second for an exactly-zero duration. Preserve a
    // non-zero subsecond sample and truncate the final estimate to whole seconds.
    let seconds = if duration.is_zero() {
        1.0
    } else {
        duration.as_secs_f64()
    };
    let remaining = (remaining_lines as f64 * seconds / processed as f64).max(0.0);
    (
        Duration::from_secs(remaining as u64),
        processed_rows as f64 / total_count as f64,
    )
}

/// TIDB_TRX 事务列表分批检索器（需 PROCESS 权限）。
pub struct tidbTrxTableRetriever {
    pub table: TableInfo,
    pub columns: Vec<ColumnInfo>,
    pub cursor: usize,
    pub total_rows: usize,
    pub initialized: bool,
    pub retrieved: bool,
    pub source: Arc<dyn InfoSchemaDataSource>,
}

impl tidbTrxTableRetriever {
    /// 校验权限后分批返回事务行。
    pub fn retrieve(&mut self) -> InfoResult<Vec<Row>> {
        if self.retrieved {
            return Ok(Vec::new());
        }
        if !self.initialized {
            if !hasPriv(self.source.as_ref(), PROCESS_PRIVILEGE)? {
                return Err(InfoSchemaError::new("PROCESS privilege is required"));
            }
            self.total_rows = self.source.transaction_row_count()?;
            self.initialized = true;
        }
        let rows = self.source.load_rows(
            DataRequest::Transactions {
                cursor: self.cursor,
                batch: BATCH_SIZE,
            },
            None,
            None,
        )?;
        self.cursor += rows.len();
        self.retrieved = self.cursor >= self.total_rows;
        adjustColumns(&rows, &self.columns, &self.table)
    }
}

/// DATA_LOCK_WAITS 锁等待分批检索器（需 PROCESS 权限）。
pub struct dataLockWaitsTableRetriever {
    pub table: TableInfo,
    pub columns: Vec<ColumnInfo>,
    pub cursor: usize,
    pub total_rows: usize,
    pub initialized: bool,
    pub retrieved: bool,
    pub source: Arc<dyn InfoSchemaDataSource>,
}

impl dataLockWaitsTableRetriever {
    /// 校验权限后分批返回锁等待行。
    pub fn retrieve(&mut self) -> InfoResult<Vec<Row>> {
        if self.retrieved {
            return Ok(Vec::new());
        }
        if !self.initialized {
            if !hasPriv(self.source.as_ref(), PROCESS_PRIVILEGE)? {
                return Err(InfoSchemaError::new("PROCESS privilege is required"));
            }
            self.total_rows = self.source.data_lock_wait_count()?;
            self.initialized = true;
        }
        let rows = self.source.load_rows(
            DataRequest::DataLockWaits {
                cursor: self.cursor,
                batch: BATCH_SIZE,
            },
            None,
            None,
        )?;
        self.cursor += rows.len();
        self.retrieved = self.cursor >= self.total_rows;
        adjustColumns(&rows, &self.columns, &self.table)
    }
}

/// DEADLOCKS 死锁等待链检索器（需 PROCESS 权限）。
pub struct deadlocksTableRetriever {
    pub current_index: usize,
    pub current_wait_chain_index: usize,
    pub table: TableInfo,
    pub columns: Vec<ColumnInfo>,
    pub deadlocks: Vec<DeadlockRecord>,
    pub initialized: bool,
    pub retrieved: bool,
    pub source: Arc<dyn InfoSchemaDataSource>,
}

impl deadlocksTableRetriever {
    /// 推进到下一条等待链条目的 (记录下标, 链内下标)。
    pub fn nextIndexPair(&self, mut index: usize, mut wait_chain_index: usize) -> (usize, usize) {
        wait_chain_index += 1;
        if index < self.deadlocks.len()
            && wait_chain_index >= self.deadlocks[index].wait_chain.len()
        {
            wait_chain_index = 0;
            index += 1;
            while index < self.deadlocks.len() && self.deadlocks[index].wait_chain.is_empty() {
                index += 1;
            }
        }
        (index, wait_chain_index)
    }
    /// 校验权限后按等待链推进返回死锁行。
    pub fn retrieve(&mut self) -> InfoResult<Vec<Row>> {
        if self.retrieved {
            return Ok(Vec::new());
        }
        if !self.initialized {
            if !hasPriv(self.source.as_ref(), PROCESS_PRIVILEGE)? {
                return Err(InfoSchemaError::new("PROCESS privilege is required"));
            }
            self.deadlocks = self.source.deadlock_records()?;
            while self.current_index < self.deadlocks.len()
                && self.deadlocks[self.current_index].wait_chain.is_empty()
            {
                self.current_index += 1;
            }
            self.initialized = true;
        }
        let rows = self.source.load_rows(
            DataRequest::Deadlocks {
                record: self.current_index,
                wait_chain: self.current_wait_chain_index,
                batch: BATCH_SIZE,
            },
            None,
            None,
        )?;
        for _ in 0..rows.len() {
            (self.current_index, self.current_wait_chain_index) =
                self.nextIndexPair(self.current_index, self.current_wait_chain_index);
        }
        self.retrieved = self.current_index >= self.deadlocks.len();
        adjustColumns(&rows, &self.columns, &self.table)
    }
}

/// 按输出列偏移从全行列投影子集。
pub fn adjustColumns(
    input: &[Row],
    output_columns: &[ColumnInfo],
    table: &TableInfo,
) -> InfoResult<Vec<Row>> {
    if output_columns.len() == table.columns.len() {
        return Ok(input.to_vec());
    }
    input
        .iter()
        .map(|full_row| {
            output_columns
                .iter()
                .map(|column| {
                    full_row
                        .get(column.offset)
                        .cloned()
                        .ok_or_else(|| InfoSchemaError::new("output column offset is out of range"))
                })
                .collect()
        })
        .collect()
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// TiFlash 系统查询响应的元数据列。
pub struct tiFlashSQLExecuteResponseMetaColumn {
    pub name: String,
    pub column_type: String,
}

#[derive(Clone, Debug, PartialEq)]
/// TiFlash 系统查询响应：元数据 + 数据行。
pub struct tiFlashSQLExecuteResponse {
    pub meta: Vec<tiFlashSQLExecuteResponseMetaColumn>,
    pub data: Vec<Row>,
}

/// TiFlash 系统表跨实例分批检索器。
pub struct TiFlashSystemTableRetriever {
    pub table: TableInfo,
    pub output_columns: Vec<ColumnInfo>,
    pub instance_count: usize,
    pub instance_index: usize,
    pub instance_ids: Vec<String>,
    pub row_index: usize,
    pub retrieved: bool,
    pub initialized: bool,
    pub extractor: PredicateExtractor,
    pub source: Arc<dyn InfoSchemaDataSource>,
}

impl TiFlashSystemTableRetriever {
    /// 跨 TiFlash 实例拉取系统表行直至有数据或耗尽。
    pub fn retrieve(&mut self) -> InfoResult<Vec<Row>> {
        if self.extractor.skip_request || self.retrieved {
            return Ok(Vec::new());
        }
        if !self.initialized {
            self.initialize()?;
        }
        while self.instance_index < self.instance_count {
            let rows = self.dataForTiFlashSystemTables(
                &self.extractor.tiflash_databases.clone(),
                &self.extractor.tiflash_tables.clone(),
            )?;
            if !rows.is_empty() {
                return Ok(rows);
            }
        }
        self.retrieved = true;
        Ok(Vec::new())
    }
    /// 解析谓词选中的 TiFlash 实例列表。
    pub fn initialize(&mut self) -> InfoResult {
        let instances = self
            .source
            .tiflash_instances(&self.extractor.tiflash_instances)?;
        self.instance_ids = instances
            .into_iter()
            .map(|instance| instance.address)
            .collect();
        self.instance_count = self.instance_ids.len();
        self.initialized = true;
        Ok(())
    }
    /// 从当前实例拉取一批系统表行，耗尽后切下一实例。
    pub fn dataForTiFlashSystemTables(
        &mut self,
        databases: &str,
        tables: &str,
    ) -> InfoResult<Vec<Row>> {
        if self.instance_index >= self.instance_ids.len() {
            return Ok(Vec::new());
        }
        let rows = self.source.load_rows(
            DataRequest::TiFlashSystemTable {
                instance: self.instance_ids[self.instance_index].clone(),
                row_offset: self.row_index,
                limit: BATCH_SIZE,
                databases: databases.to_owned(),
                tables: tables.to_owned(),
            },
            None,
            Some(&self.extractor),
        )?;
        self.row_index += rows.len();
        if rows.len() < BATCH_SIZE {
            self.instance_index += 1;
            self.row_index = 0;
        }
        adjustColumns(&rows, &self.output_columns, &self.table)
    }
}

/// 解析 Label Rule ID，返回 (schema, table, partition)。
pub fn checkRule(rule: &LabelRule) -> InfoResult<(String, String, String)> {
    let parts = rule.id.split('/').collect::<Vec<_>>();
    if parts.len() < 3 {
        return Err(InfoSchemaError::new(format!(
            "invalid label rule ID: {}",
            rule.id
        )));
    }
    if rule.rule_type.is_empty() {
        return Err(InfoSchemaError::new("empty label rule type"));
    }
    if rule.labels.is_empty() {
        return Err(InfoSchemaError::new("the label rule has no label"));
    }
    if rule.data.is_empty() {
        return Err(InfoSchemaError::new("the label rule has no data"));
    }
    // keyspace 模式下 ID 形如 keyspace/<id>/schema/<db>/<tbl>/...
    let offset = if rule.keyspace_mode && parts.first() == Some(&"keyspace") {
        if parts.len() < 5 {
            return Err(InfoSchemaError::new(format!(
                "invalid keyspace label rule ID: {}",
                rule.id
            )));
        }
        2
    } else {
        0
    };
    if parts.get(offset) != Some(&"schema") || parts.len() < offset + 3 {
        return Err(InfoSchemaError::new(format!(
            "invalid label rule ID: {}",
            rule.id
        )));
    }
    Ok((
        parts[offset + 1].to_owned(),
        parts[offset + 2].to_owned(),
        parts
            .get(offset + 3)
            .copied()
            .unwrap_or_default()
            .to_owned(),
    ))
}

/// 从规则 start_key 十六进制解码表 ID。
pub fn decodeTableIDFromRule(
    rule: &LabelRule,
    source: &dyn InfoSchemaDataSource,
) -> InfoResult<i64> {
    let data = rule
        .data
        .first()
        .ok_or_else(|| InfoSchemaError::new(format!("there is no data in rule {}", rule.id)))?;
    let encoded = data
        .get("start_key")
        .ok_or_else(|| InfoSchemaError::new(format!("start_key is missing in rule {}", rule.id)))?;
    let key = decode_hex(encoded).map_err(|_| {
        InfoSchemaError::new(format!(
            "decode key from start_key {encoded} in rule {} failed",
            rule.id
        ))
    })?;
    let table_id = source.decode_table_id_from_start_key(&key)?;
    if table_id == 0 {
        return Err(InfoSchemaError::new(format!(
            "decode tableID from key in rule {} failed",
            rule.id
        )));
    }
    Ok(table_id)
}

/// 判断库/表/分区是否与给定 table_id 不匹配（即不存在）。
pub fn tableOrPartitionNotExist(
    source: &dyn InfoSchemaDataSource,
    database: &str,
    table: &str,
    partition: &str,
    table_id: i64,
) -> InfoResult<bool> {
    Ok(!source.table_matches_id(database, table, partition, table_id)?)
}

/// 将偶数长度十六进制字符串解码为字节。
fn decode_hex(value: &str) -> Result<Vec<u8>, ()> {
    if !value.len().is_multiple_of(2) {
        return Err(());
    }
    (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&value[index..index + 2], 16).map_err(|_| ()))
        .collect()
}

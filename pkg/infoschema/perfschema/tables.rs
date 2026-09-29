// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// PERFORMANCE_SCHEMA 虚拟表实现：元数据、行数据源与远端 pprof 拉取。
//
// 将静态 TableMeta 包装为可迭代的虚拟表；本地 TiDB profile、会话变量/连接属性，
// 以及 TiKV/PD 远端 HTTP pprof 结果均在此汇聚。pprof：性能剖析接口；虚拟表无物理存储。

#![allow(non_upper_case_globals)]

use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::thread;

/// PERFORMANCE_SCHEMA 库的固定 ID（高位系统库区间）。
pub const PERFORMANCE_SCHEMA_DB_ID: i64 = (1_i64 << 62) | 10_000;

/// 以下常量为 performance_schema 表名，与 Go 字符串及顺序一致。
pub const tableNameGlobalStatus: &str = "global_status";
pub const tableNameGlobalVariables: &str = "global_variables";
pub const tableNameSessionStatus: &str = "session_status";
pub const tableNameSetupActors: &str = "setup_actors";
pub const tableNameSetupObjects: &str = "setup_objects";
pub const tableNameSetupInstruments: &str = "setup_instruments";
pub const tableNameSetupConsumers: &str = "setup_consumers";
pub const tableNameEventsStatementsCurrent: &str = "events_statements_current";
pub const tableNameEventsStatementsHistory: &str = "events_statements_history";
pub const tableNameEventsStatementsHistoryLong: &str = "events_statements_history_long";
pub const tableNamePreparedStatementsInstances: &str = "prepared_statements_instances";
pub const tableNameEventsTransactionsCurrent: &str = "events_transactions_current";
pub const tableNameEventsTransactionsHistory: &str = "events_transactions_history";
pub const tableNameEventsTransactionsHistoryLong: &str = "events_transactions_history_long";
pub const tableNameEventsStagesCurrent: &str = "events_stages_current";
pub const tableNameEventsStagesHistory: &str = "events_stages_history";
pub const tableNameEventsStagesHistoryLong: &str = "events_stages_history_long";
pub const tableNameEventsStatementsSummaryByDigest: &str = "events_statements_summary_by_digest";
pub const tableNameTiDBProfileCPU: &str = "tidb_profile_cpu";
pub const tableNameTiDBProfileMemory: &str = "tidb_profile_memory";
pub const tableNameTiDBProfileMutex: &str = "tidb_profile_mutex";
pub const tableNameTiDBProfileAllocs: &str = "tidb_profile_allocs";
pub const tableNameTiDBProfileBlock: &str = "tidb_profile_block";
pub const tableNameTiDBProfileGoroutines: &str = "tidb_profile_goroutines";
pub const tableNameTiKVProfileCPU: &str = "tikv_profile_cpu";
pub const tableNamePDProfileCPU: &str = "pd_profile_cpu";
pub const tableNamePDProfileMemory: &str = "pd_profile_memory";
pub const tableNamePDProfileMutex: &str = "pd_profile_mutex";
pub const tableNamePDProfileAllocs: &str = "pd_profile_allocs";
pub const tableNamePDProfileBlock: &str = "pd_profile_block";
pub const tableNamePDProfileGoroutines: &str = "pd_profile_goroutines";
pub const tableNameSessionAccountConnectAttrs: &str = "session_account_connect_attrs";
pub const tableNameSessionConnectAttrs: &str = "session_connect_attrs";
pub const tableNameSessionVariables: &str = "session_variables";
pub const tableNameStatusByConnection: &str = "status_by_connection";
pub const tableNameCondInstances: &str = "cond_instances";
pub const tableNameEventsWaitsCurrent: &str = "events_waits_current";
pub const tableNameEventsWaitsHistory: &str = "events_waits_history";
pub const tableNameEventsWaitsHistoryLong: &str = "events_waits_history_long";
pub const tableNameAccounts: &str = "accounts";
pub const tableNameHosts: &str = "hosts";
pub const tableNameUsers: &str = "users";
pub const tableNameBinaryLogTransactionCompressionStats: &str =
    "binary_log_transaction_compression_stats";
pub const tableNameEventsTransactionsSummaryByUserByEventName: &str =
    "events_transactions_summary_by_user_by_event_name";

/// 表名 → 稳定表 ID 映射；ID = DB_ID + 序号。
pub static TABLE_ID_MAP: LazyLock<BTreeMap<&'static str, i64>> = LazyLock::new(|| {
    let names = [
        tableNameGlobalStatus,
        tableNameSessionStatus,
        tableNameSetupActors,
        tableNameSetupObjects,
        tableNameSetupInstruments,
        tableNameSetupConsumers,
        tableNameEventsStatementsCurrent,
        tableNameEventsStatementsHistory,
        tableNameEventsStatementsHistoryLong,
        tableNamePreparedStatementsInstances,
        tableNameEventsTransactionsCurrent,
        tableNameEventsTransactionsHistory,
        tableNameEventsTransactionsHistoryLong,
        tableNameEventsStagesCurrent,
        tableNameEventsStagesHistory,
        tableNameEventsStagesHistoryLong,
        tableNameEventsStatementsSummaryByDigest,
        tableNameTiDBProfileCPU,
        tableNameTiDBProfileMemory,
        tableNameTiDBProfileMutex,
        tableNameTiDBProfileAllocs,
        tableNameTiDBProfileBlock,
        tableNameTiDBProfileGoroutines,
        tableNameTiKVProfileCPU,
        tableNamePDProfileCPU,
        tableNamePDProfileMemory,
        tableNamePDProfileMutex,
        tableNamePDProfileAllocs,
        tableNamePDProfileBlock,
        tableNamePDProfileGoroutines,
        tableNameSessionVariables,
        tableNameSessionConnectAttrs,
        tableNameSessionAccountConnectAttrs,
        tableNameGlobalVariables,
        tableNameStatusByConnection,
        tableNameCondInstances,
        tableNameEventsWaitsCurrent,
        tableNameEventsWaitsHistory,
        tableNameEventsWaitsHistoryLong,
        tableNameAccounts,
        tableNameHosts,
        tableNameUsers,
        tableNameBinaryLogTransactionCompressionStats,
        tableNameEventsTransactionsSummaryByUserByEventName,
    ];
    names
        .into_iter()
        .enumerate()
        .map(|(offset, name)| (name, PERFORMANCE_SCHEMA_DB_ID + offset as i64 + 1))
        .collect()
});

/// 返回表 ID 映射的静态引用。
pub fn table_id_map() -> &'static BTreeMap<&'static str, i64> {
    &TABLE_ID_MAP
}
/// 按小写表名判断是否为预定义 performance_schema 表。
pub fn is_predefined_table(table_name: &str) -> bool {
    TABLE_ID_MAP.contains_key(table_name.to_ascii_lowercase().as_str())
}

/// Go-style alias for `is_predefined_table`.
#[allow(non_snake_case)]
/// Go 风格别名：`IsPredefinedTable`。
pub fn IsPredefinedTable(table_name: &str) -> bool {
    is_predefined_table(table_name)
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 虚拟表单元格取值（空、整型、无符号、字符串、字节）。
pub enum Datum {
    Null,
    Integer(i64),
    Unsigned(u64),
    String(String),
    Bytes(Vec<u8>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 列元数据：ID、名、偏移与是否隐藏。
pub struct ColumnInfo {
    pub id: i64,
    pub name: String,
    pub offset: usize,
    pub hidden: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 索引元数据：ID、名、列偏移列表与 public 状态。
pub struct IndexInfo {
    pub id: i64,
    pub name: String,
    pub columns: Vec<usize>,
    pub public: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 虚拟表元数据，含建表 SQL 原文。
pub struct TableMeta {
    pub id: i64,
    pub database_id: i64,
    pub name: String,
    pub columns: Vec<ColumnInfo>,
    pub indices: Vec<IndexInfo>,
    pub public: bool,
    pub create_sql: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 虚拟表读写与插件路径上的错误类型。
pub enum PerfSchemaError {
    UnknownTable(String),
    InvalidIndexState(String),
    InvalidProjection,
    UnsupportedNodeType(String),
    Transport(String),
    Profile(String),
    Plugin(String),
}

impl std::fmt::Display for PerfSchemaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for PerfSchemaError {}

/// 本地行数据源：profile、会话变量、连接属性、按连接状态。
pub trait RowSource: Send + Sync {
    fn local_profile(&self, profile: &str) -> Result<Vec<Vec<Datum>>, PerfSchemaError>;
    fn session_variables(&self) -> Result<Vec<Vec<Datum>>, PerfSchemaError>;
    fn session_connect_attrs(&self, account: bool) -> Result<Vec<Vec<Datum>>, PerfSchemaError>;
    fn status_by_connection(&self) -> Result<Vec<Vec<Datum>>, PerfSchemaError>;

    /// 记录一次本地 profile 表读取；生产默认记录表名，测试可观察调用。
    fn on_profile_request(&self, table: &str) {
        tracing::info!(table = table, "profiling request received");
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 集群节点拓扑信息（类型、地址、status 地址）。
pub struct ServerInfo {
    pub server_type: String,
    pub address: String,
    pub status_address: String,
}

/// 远端 pprof 客户端：列举节点、HTTP 拉取与解析。
pub trait RemoteProfileClient: Send + Sync {
    fn servers(&self, node_type: &str) -> Result<Vec<ServerInfo>, PerfSchemaError>;
    fn fetch(&self, url: &str, allow_follower: bool) -> Result<Vec<u8>, PerfSchemaError>;
    fn parse_profile(
        &self,
        body: &[u8],
        goroutines: bool,
    ) -> Result<Vec<Vec<Datum>>, PerfSchemaError>;
    fn internal_http_scheme(&self) -> &str {
        "http"
    }
}

/// 可按表名覆盖默认虚拟表创建逻辑的插件接口。
pub trait VirtualTablePlugin: Send + Sync {
    fn create(&self, meta: &TableMeta) -> Result<PerfSchemaTable, PerfSchemaError>;
}

/// 插件表注册表（小写表名 → 工厂）。
static PLUGIN_TABLES: LazyLock<Mutex<BTreeMap<String, Arc<dyn VirtualTablePlugin>>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));

/// 注册可覆盖默认创建路径的虚拟表插件。
pub fn register_plugin_table(name: &str, plugin: Arc<dyn VirtualTablePlugin>) {
    PLUGIN_TABLES
        .lock()
        .expect("performance schema plugin lock poisoned")
        .insert(name.to_ascii_lowercase(), plugin);
}

/// 注销已注册的虚拟表插件。
pub fn unregister_plugin_table(name: &str) {
    PLUGIN_TABLES
        .lock()
        .expect("performance schema plugin lock poisoned")
        .remove(&name.to_ascii_lowercase());
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// performance_schema 虚拟表包装：元数据、列与索引。
pub struct PerfSchemaTable {
    pub meta: TableMeta,
    pub columns: Vec<ColumnInfo>,
    pub indices: Vec<IndexInfo>,
}

/// 工厂：优先插件，否则创建默认 PerfSchemaTable。
pub fn table_from_meta(meta: &TableMeta) -> Result<PerfSchemaTable, PerfSchemaError> {
    if let Some(plugin) = PLUGIN_TABLES
        .lock()
        .expect("performance schema plugin lock poisoned")
        .get(&meta.name.to_ascii_lowercase())
        .cloned()
    {
        return plugin.create(meta);
    }
    create_perf_schema_table(meta)
}

/// 由 TableMeta 创建虚拟表并初始化索引。
pub fn create_perf_schema_table(meta: &TableMeta) -> Result<PerfSchemaTable, PerfSchemaError> {
    let mut table = PerfSchemaTable {
        meta: meta.clone(),
        columns: meta.columns.clone(),
        indices: Vec::new(),
    };
    init_table_indices(&mut table)?;
    Ok(table)
}

/// 复制 public 索引；非 public 时返回 InvalidIndexState。
pub fn init_table_indices(table: &mut PerfSchemaTable) -> Result<(), PerfSchemaError> {
    for index in &table.meta.indices {
        if !index.public {
            return Err(PerfSchemaError::InvalidIndexState(index.name.clone()));
        }
        table.indices.push(index.clone());
    }
    Ok(())
}

impl PerfSchemaTable {
    /// 返回全部列（含隐藏列）。
    pub fn columns(&self) -> &[ColumnInfo] {
        &self.columns
    }
    /// Go 的 performance_schema 虚拟表将全部列视为可见列。
    pub fn visible_columns(&self) -> Vec<&ColumnInfo> {
        self.columns.iter().collect()
    }
    /// Go 的 performance_schema 虚拟表没有单独的隐藏列集合。
    pub fn hidden_columns(&self) -> Vec<&ColumnInfo> {
        Vec::new()
    }
    /// 可写列（虚拟表等同全部列）。
    pub fn writable_columns(&self) -> &[ColumnInfo] {
        &self.columns
    }
    /// 可删列（虚拟表等同全部列）。
    pub fn deletable_columns(&self) -> &[ColumnInfo] {
        &self.columns
    }
    /// 物理/虚拟表 ID。
    pub fn physical_id(&self) -> i64 {
        self.meta.id
    }
    /// 已初始化索引列表。
    pub fn indices(&self) -> &[IndexInfo] {
        &self.indices
    }
    /// 可删索引；虚拟表恒为空。
    pub fn deletable_indices(&self) -> &[IndexInfo] {
        &[]
    }

    /// 按表名分派到本地/远端数据源，并按请求列做投影。
    pub fn get_rows(
        &self,
        requested_columns: &[ColumnInfo],
        source: &dyn RowSource,
        remote: &dyn RemoteProfileClient,
        warnings: &mut Vec<String>,
    ) -> Result<Vec<Vec<Datum>>, PerfSchemaError> {
        if matches!(
            self.meta.name.as_str(),
            tableNameTiDBProfileCPU
                | tableNameTiDBProfileMemory
                | tableNameTiDBProfileMutex
                | tableNameTiDBProfileAllocs
                | tableNameTiDBProfileBlock
                | tableNameTiDBProfileGoroutines
        ) {
            source.on_profile_request(&format!("performance_schema.{}", self.meta.name));
        }
        // 按表名路由到本地 profile、远端 pprof 或会话元数据。
        let full_rows = match self.meta.name.as_str() {
            tableNameTiDBProfileCPU => source.local_profile("cpu")?,
            tableNameTiDBProfileMemory => source.local_profile("heap")?,
            tableNameTiDBProfileMutex => source.local_profile("mutex")?,
            tableNameTiDBProfileAllocs => source.local_profile("allocs")?,
            tableNameTiDBProfileBlock => source.local_profile("block")?,
            tableNameTiDBProfileGoroutines => source.local_profile("goroutine")?,
            tableNameTiKVProfileCPU => data_for_remote_profile(
                remote,
                "tikv",
                "/debug/pprof/profile?seconds=30",
                false,
                warnings,
            )?,
            tableNamePDProfileCPU => data_for_remote_profile(
                remote,
                "pd",
                "/pd/api/v1/debug/pprof/profile?seconds=30",
                false,
                warnings,
            )?,
            tableNamePDProfileMemory => data_for_remote_profile(
                remote,
                "pd",
                "/pd/api/v1/debug/pprof/heap",
                false,
                warnings,
            )?,
            tableNamePDProfileMutex => data_for_remote_profile(
                remote,
                "pd",
                "/pd/api/v1/debug/pprof/mutex",
                false,
                warnings,
            )?,
            tableNamePDProfileAllocs => data_for_remote_profile(
                remote,
                "pd",
                "/pd/api/v1/debug/pprof/allocs",
                false,
                warnings,
            )?,
            tableNamePDProfileBlock => data_for_remote_profile(
                remote,
                "pd",
                "/pd/api/v1/debug/pprof/block",
                false,
                warnings,
            )?,
            tableNamePDProfileGoroutines => data_for_remote_profile(
                remote,
                "pd",
                "/pd/api/v1/debug/pprof/goroutine?debug=2",
                true,
                warnings,
            )?,
            tableNameSessionVariables => source.session_variables()?,
            tableNameSessionConnectAttrs => source.session_connect_attrs(false)?,
            tableNameSessionAccountConnectAttrs => source.session_connect_attrs(true)?,
            tableNameStatusByConnection => source.status_by_connection()?,
            _ => Vec::new(),
        };
        // 请求列与全列一致且 offset 对齐时直接返回，避免投影拷贝。
        if requested_columns.len() == self.columns.len() {
            return Ok(full_rows);
        }
        full_rows
            .into_iter()
            .map(|row| {
                requested_columns
                    .iter()
                    .map(|column| {
                        row.get(column.offset)
                            .cloned()
                            .ok_or(PerfSchemaError::InvalidProjection)
                    })
                    .collect()
            })
            .collect()
    }

    /// 逐行回调；visitor 返回 false 时提前停止。
    pub fn iter_records<F>(
        &self,
        columns: &[ColumnInfo],
        source: &dyn RowSource,
        remote: &dyn RemoteProfileClient,
        warnings: &mut Vec<String>,
        mut visitor: F,
    ) -> Result<(), PerfSchemaError>
    where
        F: FnMut(i64, Vec<Datum>) -> Result<bool, PerfSchemaError>,
    {
        for (handle, row) in self
            .get_rows(columns, source, remote, warnings)?
            .into_iter()
            .enumerate()
        {
            if !visitor(handle as i64, row)? {
                break;
            }
        }
        Ok(())
    }
}

/// 从 TiKV/PD status 地址拉取 pprof，按地址排序后拼成行（首列地址）。
pub fn data_for_remote_profile(
    client: &dyn RemoteProfileClient,
    node_type: &str,
    uri: &str,
    goroutines: bool,
    warnings: &mut Vec<String>,
) -> Result<Vec<Vec<Datum>>, PerfSchemaError> {
    // 仅支持 tikv / pd；其它节点类型与 Go 一样报错。
    if !matches!(node_type, "tikv" | "pd") {
        return Err(PerfSchemaError::UnsupportedNodeType(node_type.to_string()));
    }
    let mut servers = client.servers(node_type)?;
    let mut work = Vec::with_capacity(servers.len());
    for server in servers.drain(..) {
        if server.status_address.is_empty() {
            // Go uses this historical warning text for both TiKV and PD.
            warnings.push(format!(
                "TiKV node {} does not contain status address",
                server.address
            ));
            continue;
        }
        work.push(server);
    }

    let results = thread::scope(|scope| {
        let handles = work.into_iter().map(|server| {
            scope.spawn(move || {
                let url = format!(
                    "{}://{}{}",
                    client.internal_http_scheme(),
                    server.status_address,
                    uri
                );
                let result = client
                    .fetch(&url, true)
                    .and_then(|body| client.parse_profile(&body, goroutines));
                (server.status_address, result)
            })
        });
        handles
            .map(|handle| {
                handle.join().unwrap_or_else(|_| {
                    (
                        String::new(),
                        Err(PerfSchemaError::Transport(
                            "remote profile worker panicked".to_string(),
                        )),
                    )
                })
            })
            .collect::<Vec<_>>()
    });

    let mut successful_results = Vec::new();
    for (address, result) in results {
        match result {
            Ok(rows) => successful_results.push((address, rows)),
            Err(error) => warnings.push(error.to_string()),
        }
    }
    // 按 status 地址排序，保证结果稳定。
    successful_results.sort_by(|left, right| left.0.cmp(&right.0));
    let mut rows = Vec::new();
    for (address, profile_rows) in successful_results {
        for row in profile_rows {
            let mut full = vec![Datum::String(address.clone())];
            full.extend(row);
            rows.push(full);
        }
    }
    Ok(rows)
}

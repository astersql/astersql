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

/*
// 远端 HTTP、goroutine、failpoint、sessionctx 等外部依赖均保留调用形状，后续接线时再实现异步与错误传播。

#![allow(dead_code, non_camel_case_types, non_snake_case, non_upper_case_globals)]

use std::collections::HashMap;

// 这些常量对应 Go 文件开头的 performance_schema 表名清单，顺序和字符串保持一致。
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

// tableIDMap 对应 Go 包级 map，使用构造函数表达 autoid 常量参与的初始化。
pub fn tableIDMap() -> HashMap<&'static str, i64> {
    HashMap::from([
        (tableNameGlobalStatus, autoid::PerformanceSchemaDBID + 1),
        (tableNameSessionStatus, autoid::PerformanceSchemaDBID + 2),
        (tableNameSetupActors, autoid::PerformanceSchemaDBID + 3),
        (tableNameSetupObjects, autoid::PerformanceSchemaDBID + 4),
        (tableNameSetupInstruments, autoid::PerformanceSchemaDBID + 5),
        (tableNameSetupConsumers, autoid::PerformanceSchemaDBID + 6),
        (tableNameEventsStatementsCurrent, autoid::PerformanceSchemaDBID + 7),
        (tableNameEventsStatementsHistory, autoid::PerformanceSchemaDBID + 8),
        (tableNameEventsStatementsHistoryLong, autoid::PerformanceSchemaDBID + 9),
        (tableNamePreparedStatementsInstances, autoid::PerformanceSchemaDBID + 10),
        (tableNameEventsTransactionsCurrent, autoid::PerformanceSchemaDBID + 11),
        (tableNameEventsTransactionsHistory, autoid::PerformanceSchemaDBID + 12),
        (tableNameEventsTransactionsHistoryLong, autoid::PerformanceSchemaDBID + 13),
        (tableNameEventsStagesCurrent, autoid::PerformanceSchemaDBID + 14),
        (tableNameEventsStagesHistory, autoid::PerformanceSchemaDBID + 15),
        (tableNameEventsStagesHistoryLong, autoid::PerformanceSchemaDBID + 16),
        (tableNameEventsStatementsSummaryByDigest, autoid::PerformanceSchemaDBID + 17),
        (tableNameTiDBProfileCPU, autoid::PerformanceSchemaDBID + 18),
        (tableNameTiDBProfileMemory, autoid::PerformanceSchemaDBID + 19),
        (tableNameTiDBProfileMutex, autoid::PerformanceSchemaDBID + 20),
        (tableNameTiDBProfileAllocs, autoid::PerformanceSchemaDBID + 21),
        (tableNameTiDBProfileBlock, autoid::PerformanceSchemaDBID + 22),
        (tableNameTiDBProfileGoroutines, autoid::PerformanceSchemaDBID + 23),
        (tableNameTiKVProfileCPU, autoid::PerformanceSchemaDBID + 24),
        (tableNamePDProfileCPU, autoid::PerformanceSchemaDBID + 25),
        (tableNamePDProfileMemory, autoid::PerformanceSchemaDBID + 26),
        (tableNamePDProfileMutex, autoid::PerformanceSchemaDBID + 27),
        (tableNamePDProfileAllocs, autoid::PerformanceSchemaDBID + 28),
        (tableNamePDProfileBlock, autoid::PerformanceSchemaDBID + 29),
        (tableNamePDProfileGoroutines, autoid::PerformanceSchemaDBID + 30),
        (tableNameSessionVariables, autoid::PerformanceSchemaDBID + 31),
        (tableNameSessionConnectAttrs, autoid::PerformanceSchemaDBID + 32),
        (tableNameSessionAccountConnectAttrs, autoid::PerformanceSchemaDBID + 33),
        (tableNameGlobalVariables, autoid::PerformanceSchemaDBID + 34),
        (tableNameStatusByConnection, autoid::PerformanceSchemaDBID + 35),
    ])
}

// perfSchemaTable stands for the fake table all its data is in the memory.
// perfSchemaTable 对应 Go 虚拟表包装，保存元数据、列、类型和索引；不持有物理存储句柄。
pub struct perfSchemaTable {
    pub VirtualTable: infoschema::VirtualTable,
    pub meta: model::TableInfo,
    pub cols: Vec<table::Column>,
    pub tp: table::Type,
    pub indices: Vec<table::Index>,
}

// pluginTable 对应 Go 可扩展注册表，插件可按表名覆盖默认 createPerfSchemaTable。
pub fn pluginTable() -> HashMap<String, fn(autoid::Allocators, &model::TableInfo) -> Result<Box<dyn table::Table>, errors::Error>> { HashMap::new() }

// IsPredefinedTable judges whether this table is predefined.
// IsPredefinedTable 按小写表名查 tableIDMap，保留 Go 对大小写不敏感的判断。
pub fn IsPredefinedTable(tableName: &str) -> bool { tableIDMap().contains_key(tableName.to_lowercase().as_str()) }

// tableFromMeta 对应 Go 工厂函数：优先走插件表，否则创建默认 performance_schema 虚拟表。
pub fn tableFromMeta(allocs: autoid::Allocators, _factory: fn() -> Result<pools::Resource, errors::Error>, meta: &model::TableInfo) -> Result<Box<dyn table::Table>, errors::Error> {
    if let Some(f) = pluginTable().get(&meta.Name.L) { return f(allocs, meta); }
    createPerfSchemaTable(meta).map(|t| Box::new(t) as Box<dyn table::Table>)
}

// createPerfSchemaTable creates all perfSchemaTables
// createPerfSchemaTable 将 TableInfo.Columns 转成 table.Column，并初始化索引。
pub fn createPerfSchemaTable(meta: &model::TableInfo) -> Result<perfSchemaTable, errors::Error> {
    let mut columns = Vec::with_capacity(meta.Columns.len());
    for colInfo in &meta.Columns { columns.push(table::ToColumn(colInfo)); }
    let mut t = perfSchemaTable { VirtualTable: infoschema::VirtualTable{}, meta: meta.clone(), cols: columns, tp: table::VirtualTable, indices: Vec::new() };
    initTableIndices(&mut t)?;
    Ok(t)
}

impl perfSchemaTable {
    // 以下访问器逐一对应 table.Table 接口，均只返回内存字段，不做 IO。
    pub fn Cols(&self) -> Vec<table::Column> { self.cols.clone() }
    pub fn VisibleCols(&self) -> Vec<table::Column> { self.cols.clone() }
    pub fn HiddenCols(&self) -> Vec<table::Column> { Vec::new() }
    pub fn WritableCols(&self) -> Vec<table::Column> { self.cols.clone() }
    pub fn DeletableCols(&self) -> Vec<table::Column> { self.cols.clone() }
    pub fn FullHiddenColsAndVisibleCols(&self) -> Vec<table::Column> { self.cols.clone() }
    pub fn GetPhysicalID(&self) -> i64 { self.meta.ID }
    pub fn Meta(&self) -> &model::TableInfo { &self.meta }
    pub fn Type(&self) -> table::Type { self.tp }
    pub fn Indices(&self) -> Vec<table::Index> { self.indices.clone() }
    pub fn DeletableIndices(&self) -> Vec<table::Index> { Vec::new() }
    pub fn GetPartitionedTable(&self) -> Option<table::PartitionedTable> { None }

    // getRows 按表名分派到本地 profile、远端 profile 或 infoschema 连接属性数据源。
    pub fn getRows(&self, ctx: context::Context, sctx: sessionctx::Context, cols: &[table::Column]) -> Result<Vec<Vec<types::Datum>>, errors::Error> {
        let mut fullRows = match self.meta.Name.O.as_str() {
            tableNameTiDBProfileCPU => profile::Collector{}.ProfileGraph("cpu")?,
            tableNameTiDBProfileMemory => profile::Collector{}.ProfileGraph("heap")?,
            tableNameTiDBProfileMutex => profile::Collector{}.ProfileGraph("mutex")?,
            tableNameTiDBProfileAllocs => profile::Collector{}.ProfileGraph("allocs")?,
            tableNameTiDBProfileBlock => profile::Collector{}.ProfileGraph("block")?,
            tableNameTiDBProfileGoroutines => profile::Collector{}.ProfileGraph("goroutine")?,
            tableNameTiKVProfileCPU => dataForRemoteProfile(sctx, "tikv", &format!("/debug/pprof/profile?seconds={}", profile::CPUProfileInterval / time::Second), false)?,
            tableNamePDProfileCPU => dataForRemoteProfile(sctx, "pd", &pd::PProfProfileAPIWithInterval(profile::CPUProfileInterval), false)?,
            tableNamePDProfileMemory => dataForRemoteProfile(sctx, "pd", pd::PProfHeap, false)?,
            tableNamePDProfileMutex => dataForRemoteProfile(sctx, "pd", pd::PProfMutex, false)?,
            tableNamePDProfileAllocs => dataForRemoteProfile(sctx, "pd", pd::PProfAllocs, false)?,
            tableNamePDProfileBlock => dataForRemoteProfile(sctx, "pd", pd::PProfBlock, false)?,
            tableNamePDProfileGoroutines => dataForRemoteProfile(sctx, "pd", &pd::PProfGoroutineWithDebugLevel(2), true)?,
            tableNameSessionVariables => infoschema::GetDataFromSessionVariables(ctx, sctx)?,
            tableNameSessionConnectAttrs => infoschema::GetDataFromSessionConnectAttrs(sctx, false)?,
            tableNameSessionAccountConnectAttrs => infoschema::GetDataFromSessionConnectAttrs(sctx, true)?,
            tableNameStatusByConnection => infoschema::GetDataFromStatusByConn(sctx)?,
            _ => Vec::new(),
        };
        if cols.len() == self.cols.len() { return Ok(fullRows); }
        // Go 在投影列少于全列时按 Column.Offset 重新组装行，保持列裁剪语义。
        let rows = fullRows.drain(..).map(|fullRow| cols.iter().map(|col| fullRow[col.Offset].clone()).collect()).collect();
        Ok(rows)
    }

    // IterRecords implements table.Table IterRecords interface.
    // IterRecords 将每一行交给回调；回调返回 more=false 时提前停止。
    pub fn IterRecords(&self, ctx: context::Context, sctx: sessionctx::Context, cols: &[table::Column], mut fn_: table::RecordIterFunc) -> Result<(), errors::Error> {
        let rows = self.getRows(ctx, sctx, cols)?;
        for (i, row) in rows.into_iter().enumerate() {
            let more = fn_(kv::IntHandle(i as i64), row, cols)?;
            if !more { break; }
        }
        Ok(())
    }
}

// initTableIndices initializes the indices of the perfSchemaTable.
// initTableIndices 保留 Go 的 StateNone 检查和 NewIndex 错误传播顺序。
pub fn initTableIndices(t: &mut perfSchemaTable) -> Result<(), errors::Error> {
    let tblInfo = &t.meta;
    for idxInfo in &tblInfo.Indices {
        if idxInfo.State == model::StateNone { return Err(table::ErrIndexStateCantNone.GenWithStackByArgs(idxInfo.Name.clone())); }
        let idx = tables::NewIndex(t.meta.ID, tblInfo, idxInfo)?;
        t.indices.push(idx);
    }
    Ok(())
}

// dataForRemoteProfile 从 TiKV 或 PD status address 拉取 pprof 数据并转换成 INFORMATION_SCHEMA 行。
// Go 版本使用 goroutine、WaitGroup 和 channel；这里顺序表达同一流程，并用注释标出并发/资源收尾语义。
pub fn dataForRemoteProfile(ctx: sessionctx::Context, nodeType: &str, uri: &str, isGoroutine: bool) -> Result<Vec<Vec<types::Datum>>, errors::Error> {
    let mut servers = match nodeType {
        "tikv" => infoschema::GetStoreServerInfo(ctx.GetStore())?,
        "pd" => infoschema::GetPDServerInfo(ctx)?,
        _ => return Err(errors::Errorf(format!("{} does not support profile remote component", nodeType))),
    };
    // failpoint mockRemoteNodeStatusAddress 会覆写拓扑列表；表达式无额外校验，保持 Go 测试注入语义。
    failpoint::Inject("mockRemoteNodeStatusAddress", |val| {
        if let Some(s) = val.as_string().filter(|s| !s.is_empty()) {
            servers.clear();
            for server in s.split(';') {
                let parts: Vec<_> = server.split(',').collect();
                if parts[0] != nodeType { continue; }
                servers.push(infoschema::ServerInfo{ ServerType: parts[0].to_owned(), Address: parts[1].to_owned(), StatusAddr: parts[2].to_owned(), ..Default::default() });
            }
        }
    });

    struct result { addr: String, rows: Vec<Vec<types::Datum>>, err: Option<errors::Error> }
    let mut results = Vec::new();
    for server in servers {
        let statusAddr = server.StatusAddr;
        if statusAddr.is_empty() {
            ctx.GetSessionVars().StmtCtx.AppendWarning(errors::NewNoStackErrorf(format!("TiKV node {} does not contain status address", server.Address)));
            continue;
        }
        // Go 在 goroutine 中用 util.WithRecovery 包裹，HTTP body 通过 defer terror.Log(Close) 收尾。
        let url = format!("{}://{}{}", util::InternalHTTPSchema(), statusAddr, uri);
        let mut req = http::NewRequest(http::MethodGet, &url, None).map_err(errors::Trace)?;
        req.Header.Add("PD-Allow-follower-handle", "true");
        req.Header.Add("Content-Type", "application/protobuf");
        let resp = util::InternalHTTPClient().Do(req).map_err(errors::Trace)?;
        if resp.StatusCode != http::StatusOK { results.push(result { addr: String::new(), rows: Vec::new(), err: Some(errors::Errorf(format!("request {} failed: {}", url, resp.Status))) }); continue; }
        let collector = profile::Collector{};
        let rows = if isGoroutine { collector.ParseGoroutines(resp.Body)? } else { collector.ProfileReaderToDatums(resp.Body)? };
        results.push(result { addr: statusAddr, rows, err: None });
    }

    // Keep the original order to make the result more stable
    // Go 先收集 channel 结果再按地址排序，失败项只追加 warning 不中断整个表扫描。
    let mut ok_results = Vec::new();
    for r in results {
        if let Some(err) = r.err { ctx.GetSessionVars().StmtCtx.AppendWarning(err); continue; }
        ok_results.push(r);
    }
    ok_results.sort_by(|i, j| i.addr.cmp(&j.addr));
    let mut finalRows = Vec::new();
    for r in ok_results {
        let addr = types::NewStringDatum(r.addr);
        for row in r.rows {
            let mut full = vec![addr.clone()];
            full.extend(row);
            finalRows.push(full);
        }
    }
    Ok(finalRows)
}
*/

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

// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// TiKV Status HTTP handlers：schema、Region、MVCC、DDL、TiFlash、settings 等运维接口。
//
// 对齐 Go `tikvhandler` 包；Region 是 TiKV 数据分片，MVCC 为多版本并发控制。
// 具体存储/PD/解码副作用经 `TikvRuntime` / `TikvHandlerTool` 注入。

use std::any::Any;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

/// 请求默认超时：10 秒。
pub const REQUEST_DEFAULT_TIMEOUT: Duration = Duration::seconds(10);

/// SettingsHandler 对应 Go 中列出或更新 TiDB server settings 的 handler。
pub struct SettingsHandler {
    pub tool: TikvHandlerTool,
}
/// 构造 SettingsHandler。
pub fn NewSettingsHandler(tool: TikvHandlerTool) -> SettingsHandler {
    SettingsHandler { tool }
}

/// SchemaHandler 对应库表 schema 查询接口。
pub struct SchemaHandler {
    pub tool: TikvHandlerTool,
}
/// 构造 SchemaHandler。
pub fn NewSchemaHandler(tool: TikvHandlerTool) -> SchemaHandler {
    SchemaHandler { tool }
}

/// SchemaStorageHandler 对应 INFORMATION_SCHEMA.TABLES 存储统计接口。
pub struct SchemaStorageHandler {
    pub tool: TikvHandlerTool,
}
/// 构造 SchemaStorageHandler。
pub fn NewSchemaStorageHandler(tool: TikvHandlerTool) -> SchemaStorageHandler {
    SchemaStorageHandler { tool }
}

/// DBTableHandler 通过 tableID 返回 database/table 信息。
pub struct DBTableHandler {
    pub tool: TikvHandlerTool,
}
/// 构造 DBTableHandler。
pub fn NewDBTableHandler(tool: TikvHandlerTool) -> DBTableHandler {
    DBTableHandler { tool }
}

/// FlashReplicaHandler 对应 TiFlash replica 查询和状态上报。
pub struct FlashReplicaHandler {
    pub tool: TikvHandlerTool,
}
/// 构造 FlashReplicaHandler。
pub fn NewFlashReplicaHandler(tool: TikvHandlerTool) -> FlashReplicaHandler {
    FlashReplicaHandler { tool }
}

/// RegionHandler 持有通用 TiKV 工具，用于 region 元信息和 hot region 查询。
pub struct RegionHandler {
    pub tool: TikvHandlerTool,
}
/// 构造 RegionHandler。
pub fn NewRegionHandler(tool: TikvHandlerTool) -> RegionHandler {
    RegionHandler { tool }
}

/// TableHandler 额外保存 PD client 和 op，按 op 分发 regions/ranges/disk/scatter。
pub struct TableHandler {
    pub tool: TikvHandlerTool,
    pub pd_client: PdClient,
    pub op: String,
}
/// 构造 TableHandler；从 region cache 取带 caller 组件标记的 PD client。
pub fn NewTableHandler(tool: TikvHandlerTool, op: String) -> TableHandler {
    let pd_client = tool
        .region_cache
        .pd_client
        .with_caller_component("tikv-handler");
    TableHandler {
        tool,
        pd_client,
        op,
    }
}

/// DDL 历史 job 查询 handler。
pub struct DDLHistoryJobHandler {
    pub tool: TikvHandlerTool,
}
/// 构造 DDLHistoryJobHandler。
pub fn NewDDLHistoryJobHandler(tool: TikvHandlerTool) -> DDLHistoryJobHandler {
    DDLHistoryJobHandler { tool }
}

/// 辞去 DDL owner 的 handler。
pub struct DDLResignOwnerHandler {
    pub store: Storage,
}
/// admin check index 诊断 handler。
pub struct DDLCheckHandler {
    pub tool: TikvHandlerTool,
}
/// 构造 DDLResignOwnerHandler。
pub fn NewDDLResignOwnerHandler(store: Storage) -> DDLResignOwnerHandler {
    DDLResignOwnerHandler { store }
}
/// 构造 DDLCheckHandler。
pub fn NewDDLCheckHandler(tool: TikvHandlerTool) -> DDLCheckHandler {
    DDLCheckHandler { tool }
}

/// 本机 server 信息 handler。
pub struct ServerInfoHandler {
    pub tool: TikvHandlerTool,
}
/// 构造 ServerInfoHandler。
pub fn NewServerInfoHandler(tool: TikvHandlerTool) -> ServerInfoHandler {
    ServerInfoHandler { tool }
}

/// 集群全部 server 信息 handler。
pub struct AllServerInfoHandler {
    pub tool: TikvHandlerTool,
}
/// 构造 AllServerInfoHandler。
pub fn NewAllServerInfoHandler(tool: TikvHandlerTool) -> AllServerInfoHandler {
    AllServerInfoHandler { tool }
}

/// executor profile 导出 handler。
pub struct ProfileHandler {
    pub tool: TikvHandlerTool,
}
/// 构造 ProfileHandler。
pub fn NewProfileHandler(tool: TikvHandlerTool) -> ProfileHandler {
    ProfileHandler { tool }
}

/// DDLHookHandler/ValueHandler/LabelHandler 对应无字段 Go handler。
pub struct DDLHookHandler {
    pub runtime: Arc<dyn TikvRuntime>,
}
/// 单列 datum 解码写出 handler。
pub struct ValueHandler {
    pub runtime: Arc<dyn TikvRuntime>,
}
/// 标签相关空字段 handler。
pub struct LabelHandler {
    pub runtime: Arc<dyn TikvRuntime>,
}

/// 表操作：查询 regions。
pub const OP_TABLE_REGIONS: &str = "regions";
/// 表操作：查询 key ranges。
pub const OP_TABLE_RANGES: &str = "ranges";
/// 表操作：磁盘占用。
pub const OP_TABLE_DISK_USAGE: &str = "disk-usage";
/// 表操作：打散（scatter）表数据。
pub const OP_TABLE_SCATTER: &str = "scatter-table";
/// 表操作：停止 scatter。
pub const OP_STOP_TABLE_SCATTER: &str = "stop-scatter-table";

/// MVCC（多版本并发控制）查询 handler，按 op 分发。
pub struct MvccTxnHandler {
    pub tool: TikvHandlerTool,
    pub op: String,
}
/// 构造 MvccTxnHandler。
pub fn NewMvccTxnHandler(tool: TikvHandlerTool, op: String) -> MvccTxnHandler {
    MvccTxnHandler { tool, op }
}

/// MVCC 按十六进制 key 查询。
pub const OP_MVCC_GET_BY_HEX: &str = "hex";
/// MVCC 按表行 key/handle 查询。
pub const OP_MVCC_GET_BY_KEY: &str = "key";
/// MVCC 按索引值查询。
pub const OP_MVCC_GET_BY_IDX: &str = "idx";
/// MVCC 按事务 startTS 查询。
pub const OP_MVCC_GET_BY_TXN: &str = "txn";

/// ValueHandler.ServeHTTP 解析列元信息和 rowBin，解码单列 datum 后写出字符串。
pub fn serve_value_http(h: &ValueHandler, w: &mut ResponseWriter, req: &Request) {
    let params = req.path_vars();
    let col_id = match parse_i64(params.get(COLUMN_ID)) {
        Ok(v) => v,
        Err(err) => {
            write_error(w, err);
            return;
        }
    };
    let col_tp = match parse_i64(params.get(COLUMN_TP)) {
        Ok(v) => v,
        Err(err) => {
            write_error(w, err);
            return;
        }
    };
    let col_flag = match parse_u64(params.get(COLUMN_FLAG)) {
        Ok(v) => v,
        Err(err) => {
            write_error(w, err);
            return;
        }
    };
    let col_len = match parse_i64(params.get(COLUMN_LEN)) {
        Ok(v) => v,
        Err(err) => {
            write_error(w, err);
            return;
        }
    };
    let values = match parseQuery(req.raw_query(), false) {
        Ok(v) => v,
        Err(err) => {
            write_error(w, err);
            return;
        }
    };
    if values.count(ROW_BIN) != 1 {
        write_error(w, Error::bad_request("Invalid Query"));
        return;
    }
    // Go 使用 base64 解码原始 row bytes，再按 FieldType 解出 datum。
    let bin = match base64_decode(&values.first(ROW_BIN)) {
        Ok(v) => v,
        Err(err) => {
            write_error(w, err);
            return;
        }
    };
    let field_type = FieldType::new(col_tp as u8, col_flag, col_len, 6);
    match h.runtime.decode_row(&bin, col_id, field_type) {
        Ok(value) => write_data(w, value),
        Err(err) => write_error(w, err),
    }
}

/// TableRegions 对应表 record/index region 列表响应。
pub struct TableRegions {
    pub table_name: String,
    pub table_id: i64,
    pub record_regions: Vec<RegionMeta>,
    pub indices: Vec<IndexRegions>,
}
/// 键区间详情：原始字节与 hex 表示。
pub struct RangeDetail {
    pub start_key: Vec<u8>,
    pub end_key: Vec<u8>,
    pub start_key_hex: String,
    pub end_key_hex: String,
}
/// 由起止 key 构造 RangeDetail，并生成 hex。
pub fn createRangeDetail(start: Vec<u8>, end: Vec<u8>) -> RangeDetail {
    RangeDetail {
        start_key_hex: hex_encode(&start),
        end_key_hex: hex_encode(&end),
        start_key: start,
        end_key: end,
    }
}
/// 表级 key 区间：整表、record、index 与各索引区间。
pub struct TableRanges {
    pub table_name: String,
    pub table_id: i64,
    pub range: RangeDetail,
    pub record: RangeDetail,
    pub index: RangeDetail,
    pub indices: Map<RangeDetail>,
}
/// 单个索引覆盖的 Region 列表。
pub struct IndexRegions {
    pub name: String,
    pub id: i64,
    pub regions: Vec<RegionMeta>,
}
/// Region 详情：区间、ID 与 frame 列表。
pub struct RegionDetail {
    pub range_detail: RangeDetail,
    pub region_id: u64,
    pub frames: Vec<FrameItem>,
}

/// addTableInRange 保留 Go 对 partition/index/record frame 的双层扫描。
pub fn addTableInRange(
    rt: &mut RegionDetail,
    db_name: &str,
    cur_table: &TableInfo,
    range: &RegionFrameRange,
) {
    for index in &cur_table.indices {
        if index.primary && cur_table.is_common_handle {
            continue;
        }
        for frame in range.index_frames(db_name, cur_table, index) {
            rt.frames.push(frame);
        }
    }
    for frame in range.record_frames(db_name, cur_table) {
        rt.frames.push(frame);
    }
}

#[derive(Clone)]
/// Region 内某表/索引的 frame 描述。
pub struct FrameItem {
    pub db_name: String,
    pub table_name: String,
    pub table_id: i64,
    pub is_record: bool,
    pub record_id: i64,
    pub index_name: String,
    pub index_id: i64,
    pub index_values: Vec<String>,
}

/// SettingsHandler.ServeHTTP：POST 时更新表单指定的全局设置，非 POST 时返回全局配置。
pub fn serve_settings_http(h: &SettingsHandler, w: &mut ResponseWriter, req: &Request) {
    if req.method() != METHOD_POST {
        write_data(w, h.tool.store.runtime.global_config());
        return;
    }
    if let Err(err) = req.parse_form() {
        write_error(w, err);
        return;
    }
    // 这些分支对应 Go 中 log level、general log、async commit、1PC、DDL slow threshold 等参数。
    for key in [
        "log_level",
        "tidb_general_log",
        "tidb_enable_async_commit",
        "tidb_enable_1pc",
        "ddl_slow_threshold",
        "check_mb4_value_in_utf8",
        "deadlock_history_capacity",
        "deadlock_history_collect_retryable",
        "tidb_enable_mutation_checker",
        "transaction_summary_capacity",
        "transaction_id_digest_min_duration",
    ] {
        let value = req.form_value(key);
        // Go's `req.Form.Get` gates every setting branch on a non-empty value.
        // In particular, an unrelated POST must not overwrite the other
        // process-wide settings with empty strings.
        if value.is_empty() {
            continue;
        }
        if let Err(err) = validate_setting_value(key, &value) {
            write_error(w, err);
            return;
        }
        // Go accepts non-positive DDL slow thresholds but leaves the existing
        // setting unchanged rather than forwarding them to a setter.
        if key == "ddl_slow_threshold" && value.parse::<i64>().is_ok_and(|value| value <= 0) {
            continue;
        }
        if let Err(err) = h.tool.store.runtime.apply_setting(key, &value) {
            write_error(w, err);
            return;
        }
    }
}

/// Validate the settings whose parsing and range checks belong to Go's HTTP
/// handler itself, before a runtime observes any side effect.
fn validate_setting_value(key: &str, value: &str) -> Result<(), Error> {
    match key {
        "tidb_general_log"
        | "tidb_enable_async_commit"
        | "tidb_enable_1pc"
        | "check_mb4_value_in_utf8"
        | "tidb_enable_mutation_checker"
            if !matches!(value, "0" | "1") =>
        {
            Err(Error::new("illegal argument"))
        }
        "ddl_slow_threshold" if value.parse::<i64>().is_err() => {
            Err(Error::new("invalid ddl_slow_threshold"))
        }
        "deadlock_history_capacity" => match value.parse::<i64>() {
            Ok(capacity) if (0..=10_000).contains(&capacity) => Ok(()),
            Ok(_) => Err(Error::new(
                "deadlock_history_capacity out of range, should be in 0 to 10000",
            )),
            Err(_) => Err(Error::new("illegal argument")),
        },
        "deadlock_history_collect_retryable" if parse_go_bool(value).is_none() => {
            Err(Error::new("illegal argument"))
        }
        "transaction_summary_capacity" => match value.parse::<i64>() {
            Ok(capacity) if (0..=5_000).contains(&capacity) => Ok(()),
            Ok(_) => Err(Error::new(
                "transaction_summary_capacity out of range, should be in 0 to 5000",
            )),
            Err(_) => Err(Error::new("illegal argument")),
        },
        "transaction_id_digest_min_duration" => match value.parse::<i64>() {
            Ok(duration) if (0..=i64::from(i32::MAX)).contains(&duration) => Ok(()),
            Ok(_) => Err(Error::new(
                "transaction_id_digest_min_duration out of range, should be in 0 to 2147483647",
            )),
            Err(_) => Err(Error::new("illegal argument")),
        },
        _ => Ok(()),
    }
}

/// `strconv.ParseBool` accepts the documented Go spellings, including `1` and
/// `0`, which Rust's `bool::from_str` deliberately does not.
fn parse_go_bool(value: &str) -> Option<bool> {
    match value {
        "1" | "t" | "T" | "TRUE" | "true" | "True" => Some(true),
        "0" | "f" | "F" | "FALSE" | "false" | "False" => Some(false),
        _ => None,
    }
}

#[derive(Clone)]
/// TiFlash 副本信息条目。
pub struct TableFlashReplicaInfo {
    pub id: i64,
    pub replica_count: u64,
    pub location_labels: Vec<String>,
    pub available: bool,
    pub high_priority: bool,
}

/// FlashReplicaHandler.ServeHTTP：POST 处理 TiFlash 上报，否则汇总当前和已删除/截断表的 replica。
pub fn serve_flash_replica_http(h: &FlashReplicaHandler, w: &mut ResponseWriter, req: &Request) {
    if req.method() == METHOD_POST {
        handleStatusReport(h, w, req);
        return;
    }
    let schema = match h.tool.schema() {
        Ok(v) => v,
        Err(err) => {
            write_error(w, err);
            return;
        }
    };
    let mut infos = Vec::new();
    for table in schema.tables_with_special_attribute("tiflash") {
        infos = appendTiFlashReplicaInfo(infos, &table);
    }
    match getDropOrTruncateTableTiflash(h, &schema) {
        Ok(mut extra) => {
            infos.append(&mut extra);
            write_data(w, infos);
        }
        Err(err) => write_error(w, err),
    }
}

/// 若表配置了 TiFlash 副本则展开并追加到列表。
pub fn appendTiFlashReplicaInfo(
    mut infos: Vec<TableFlashReplicaInfo>,
    tbl: &TableInfo,
) -> Vec<TableFlashReplicaInfo> {
    if tbl.tiflash_replica.is_none() {
        return infos;
    }
    // Go 对普通表、已存在分区和 AddingDefinitions 分别追加记录；这里统一由 helper 展开。
    infos.extend(tbl.expand_tiflash_replica_infos());
    infos
}

/// 获取已删除/截断表上仍残留的 TiFlash 副本信息。
pub fn getDropOrTruncateTableTiflash(
    h: &FlashReplicaHandler,
    schema: &InfoSchema,
) -> Result<Vec<TableFlashReplicaInfo>, Error> {
    h.tool.store.runtime.historical_tiflash(schema)
}

/// TiFlash 上报的副本同步进度状态。
pub struct tableFlashReplicaStatus {
    pub id: i64,
    pub region_count: u64,
    pub flash_region_count: u64,
}
/// 当 flash Region 数等于总 Region 数时视为可用。
pub fn checkTableFlashReplicaAvailable(tf: &tableFlashReplicaStatus) -> bool {
    tf.flash_region_count == tf.region_count
}
/// 处理 TiFlash 状态上报，更新 DDL replica 与进度缓存。
pub fn handleStatusReport(h: &FlashReplicaHandler, w: &mut ResponseWriter, req: &Request) {
    let status = match h.tool.store.runtime.decode_flash_status(req.body()) {
        Ok(v) => v,
        Err(err) => {
            write_error(w, err);
            return;
        }
    };
    // Go 更新 DDL replica info，并按 available 删除或刷新 TiFlash progress cache。
    match h
        .tool
        .store
        .runtime
        .update_table_replica(status.id, checkTableFlashReplicaAvailable(&status))
    {
        Ok(()) => h.tool.store.runtime.log_info("handle flash replica report"),
        Err(err) => write_error(w, err),
    }
}

/// INFORMATION_SCHEMA.TABLES 存储统计一行。
pub struct SchemaTableStorage {
    pub table_schema: String,
    pub table_name: String,
    pub table_rows: i64,
    pub avg_row_length: i64,
    pub data_length: i64,
    pub max_data_length: i64,
    pub index_length: i64,
    pub data_free: i64,
}

/// getSchemaTablesStorageInfo 通过 internal SQL 查询 INFORMATION_SCHEMA.TABLES。
pub fn getSchemaTablesStorageInfo(
    h: &SchemaStorageHandler,
    schema: Option<CIStr>,
    table: Option<CIStr>,
) -> Result<Vec<SchemaTableStorage>, Error> {
    h.tool.store.runtime.schema_storage(
        schema.as_ref().map(|value| value.0.as_str()),
        table.as_ref().map(|value| value.0.as_str()),
    )
}

/// SchemaStorageHandler 入口：解析路由后查询存储统计。
pub fn serve_schema_storage_http(h: &SchemaStorageHandler, w: &mut ResponseWriter, req: &Request) {
    let (db_name, table_name, is_single) = match h.tool.store.runtime.resolve_schema_route(req) {
        Ok(v) => v,
        Err(err) => {
            write_error(w, err);
            return;
        }
    };
    match getSchemaTablesStorageInfo(h, db_name, table_name) {
        Ok(results) if is_single => write_data(w, results.into_iter().next()),
        Ok(results) => write_data(w, results),
        Err(err) => write_error(w, err),
    }
}

/// WriteDBTablesData/manualWriteJSONArray 保留 Go 手写 JSON array 的低内存写出策略。
pub fn WriteDBTablesData(w: &mut ResponseWriter, tbs: Vec<TableInfo>) {
    manualWriteJSONArray(w, tbs);
}
/// 以单个 JSON 数组响应写出表列表。
pub fn manualWriteJSONArray<T: JsonValue + Any + Send + 'static>(
    w: &mut ResponseWriter,
    array: Vec<T>,
) {
    // The Go implementation emits one JSON array, including for a non-empty
    // list. Keep the array as one response value so callers never observe a
    // sequence of independent JSON documents.
    w.header_set(HEADER_CONTENT_TYPE, CONTENT_TYPE_JSON);
    w.write_header(STATUS_OK);
    write_data(w, array);
}
/// 写出仅含表名信息的简化表列表。
pub fn writeDBSimpleTablesData(w: &mut ResponseWriter, tbs: Vec<TableNameInfo>) {
    manualWriteJSONArray(w, tbs);
}

/// SchemaHandler 入口：按路由解析并返回 schema 数据。
pub fn serve_schema_http(h: &SchemaHandler, w: &mut ResponseWriter, req: &Request) {
    let schema = match h.tool.schema() {
        Ok(v) => v,
        Err(err) => {
            write_error(w, err);
            return;
        }
    };
    // Go 按 db/table、table_id、table_ids、全部 schema 的顺序尝试。
    match h.tool.store.runtime.resolve_schema_request(&schema, req) {
        Ok(data) => write_data(w, data),
        Err(err) => write_error(w, err),
    }
}
/// 将 table_id 字符串解析为表（含分区表）。
pub fn getTableByIDStr(schema: &InfoSchema, table_id: &str) -> Result<TableInfo, Error> {
    let tid = table_id
        .parse::<i64>()
        .map_err(|_| Error::new("invalid table id"))?;
    if tid < 0 {
        return Err(Error::new("table id not exists"));
    }
    schema.table_by_id_or_partition(tid)
}

/// TableHandler 入口：按 op 分发 regions/ranges/disk/scatter。
pub fn serve_table_http(h: &TableHandler, w: &mut ResponseWriter, req: &Request) {
    // Go resolves `table(partition)` against the logical table before it
    // dispatches regions/ranges/scatter. Preserve that normalization at the
    // runtime boundary instead of making every runtime parse route syntax.
    let mut table_request = req.clone();
    let partition = table_request
        .path
        .get(TABLE_NAME)
        .map(|name| astersql_server_handler::util::ExtractTableAndPartitionName(name))
        .unwrap_or_default();
    if table_request.path.contains_key(TABLE_NAME) {
        table_request
            .path
            .insert(TABLE_NAME.into(), partition.0.clone());
    }
    let (table, runtime_partition) = match h.tool.store.runtime.resolve_table_route(&table_request)
    {
        Ok(v) => v,
        Err(err) => {
            write_error(w, err);
            return;
        }
    };
    let partition = if partition.1.is_empty() {
        runtime_partition
    } else {
        partition.1
    };
    match h.op.as_str() {
        OP_TABLE_REGIONS => handleRegionRequest(h, &table, w),
        OP_TABLE_RANGES => handleRangeRequest(h, &table, w),
        OP_TABLE_DISK_USAGE => handleDiskUsageRequest(h, &table, w),
        OP_TABLE_SCATTER => handleScatterTableRequest(h, table.partition(&partition), w),
        OP_STOP_TABLE_SCATTER => handleStopScatterTableRequest(h, table.partition(&partition), w),
        _ => write_error(w, Error::new("method not found")),
    }
}

/// DDL 历史查询入口。
pub fn serve_ddl_history_http(h: &DDLHistoryJobHandler, w: &mut ResponseWriter, req: &Request) {
    let (job_id, limit) = match parse_ddl_history_query(req) {
        Ok(v) => v,
        Err(err) => {
            write_error(w, err);
            return;
        }
    };
    match getHistoryDDL(h, job_id, limit) {
        Ok(jobs) => write_data(w, jobs),
        Err(err) => write_error(w, err),
    }
}
/// 从运行时拉取 DDL 历史 job。
pub fn getHistoryDDL(h: &DDLHistoryJobHandler, job_id: i32, limit: i32) -> Result<Vec<Job>, Error> {
    h.tool.store.runtime.history_ddl(job_id, limit)
}
/// 请求当前节点辞去 DDL owner。
pub fn resignDDLOwner(h: &DDLResignOwnerHandler) -> Result<(), Error> {
    h.store.runtime.resign_ddl_owner()
}
/// 辞去 DDL owner 的 HTTP 入口（仅 POST）。
pub fn serve_ddl_resign_owner_http(
    h: &DDLResignOwnerHandler,
    w: &mut ResponseWriter,
    req: &Request,
) {
    if req.method() != METHOD_POST {
        write_error(w, Error::new("This api only support POST method"));
        return;
    }
    match resignDDLOwner(h) {
        Ok(()) => write_data(w, "success!"),
        Err(err) => {
            h.store
                .runtime
                .log_error("failed to resign DDL owner", &err);
            write_error(w, err);
        }
    }
}

/// DDLCheckHandler.ServeHTTP 构造 admin check index SQL，并收集 RecordSet 行返回诊断结果。
pub fn serve_ddl_check_http(h: &DDLCheckHandler, w: &mut ResponseWriter, req: &Request) {
    if req.method() != METHOD_POST {
        write_error(w, Error::new("This api only support POST method"));
        return;
    }
    let params = req.path_vars();
    let (db, table, index) = (
        params.get(DB_NAME),
        params.get(TABLE_NAME),
        params.get(INDEX_NAME),
    );
    if db.is_empty() || table.is_empty() || index.is_empty() {
        write_error(w, Error::new("db, table and index are required"));
        return;
    }
    let sql = format!(
        "admin check index {} `{}`",
        quoted_table_name(&db, &table),
        index.replace('`', "``")
    );
    match h
        .tool
        .store
        .runtime
        .execute_admin_check(req.context(), &sql)
    {
        Ok(rows) => write_data(w, check_index_result(db, table, index, &sql, rows, None)),
        Err(err) => write_data(
            w,
            check_index_result(db, table, index, &sql, Vec::new(), Some(err)),
        ),
    }
}
/// 收集多个 RecordSet 的全部行；遇错关闭当前集并返回。
pub fn collectRecordSetRows(
    _: Context,
    _: Session,
    mut rss: Vec<RecordSet>,
) -> Result<Vec<Vec<String>>, Error> {
    let mut rows = Vec::new();
    for record_set in &mut rss {
        for row in record_set.rows.drain(..) {
            match row {
                Ok(row) => rows.push(row),
                Err(error) => {
                    record_set.closed = true;
                    return Err(error);
                }
            }
        }
    }
    Ok(rows)
}

/// 获取 PD 地址列表。
pub fn getPDAddr(h: &TableHandler) -> Result<Vec<String>, Error> {
    h.tool.store.pd_addrs()
}
/// 向 PD 添加 scatter-range 调度。
pub fn addScatterSchedule(
    h: &TableHandler,
    start_key: Vec<u8>,
    end_key: Vec<u8>,
    name: &str,
) -> Result<(), Error> {
    // Go 通过 PD HTTP /schedulers 添加 scatter-range，并无论成功与否都关闭 response body。
    let addresses = getPDAddr(h)?;
    if addresses.is_empty() {
        return Err(Error::new("pd unavailable"));
    }
    h.tool.store.runtime.add_scatter(start_key, end_key, name)
}
/// 删除指定名称的 scatter 调度。
pub fn deleteScatterSchedule(h: &TableHandler, name: &str) -> Result<(), Error> {
    let addresses = getPDAddr(h)?;
    if addresses.is_empty() {
        return Err(Error::new("pd unavailable"));
    }
    h.tool.store.runtime.delete_scatter(name)
}
/// 触发表 scatter。
pub fn handleScatterTableRequest(h: &TableHandler, tbl: PhysicalTable, w: &mut ResponseWriter) {
    if let Err(err) = h.tool.store.runtime.scatter_table(tbl, false) {
        write_error(w, err);
        return;
    }
    write_data(w, "success!");
}
/// 停止表 scatter。
pub fn handleStopScatterTableRequest(h: &TableHandler, tbl: PhysicalTable, w: &mut ResponseWriter) {
    if let Err(err) = h.tool.store.runtime.scatter_table(tbl, true) {
        write_error(w, err);
        return;
    }
    write_data(w, "success!");
}
/// 返回表的 Region 分布。
pub fn handleRegionRequest(h: &TableHandler, tbl: &Table, w: &mut ResponseWriter) {
    match h.tool.store.runtime.table_regions(tbl) {
        Ok(data) => write_data(w, data),
        Err(err) => write_error(w, err),
    }
}
/// 按表 ID/索引构造整表与 record/index 键区间。
pub fn createTableRanges(tbl_id: i64, tbl_name: String, indices: Vec<IndexInfo>) -> TableRanges {
    let table_end = encode_table_prefix(tbl_id.wrapping_add(1));
    let record_prefix = gen_table_record_prefix(tbl_id);
    let index_prefix = gen_table_index_prefix(tbl_id);
    TableRanges {
        table_name: tbl_name,
        table_id: tbl_id,
        range: createRangeDetail(encode_table_prefix(tbl_id), table_end.clone()),
        record: createRangeDetail(record_prefix.clone(), table_end),
        index: createRangeDetail(index_prefix, record_prefix),
        indices: index_ranges(tbl_id, indices),
    }
}
/// 返回表的 key ranges。
pub fn handleRangeRequest(_: &TableHandler, tbl: &Table, w: &mut ResponseWriter) {
    match table_ranges_response(tbl) {
        TableRangesResponse::Single(ranges) => write_data(w, ranges),
        TableRangesResponse::Multiple(ranges) => write_data(w, ranges),
    }
}
/// 按表 ID 扫描 PD Region 元信息。
pub fn getRegionsByID(
    h: &TableHandler,
    tbl: &Table,
    id: i64,
    name: &str,
) -> Result<TableRegions, Error> {
    // Go 分别扫描 record key range 和每个 index key range，并转换 PD region meta。
    h.pd_client.runtime.scan_regions(tbl, id, name)
}
/// 返回表在 PD 上的存储占用。
pub fn handleDiskUsageRequest(h: &TableHandler, tbl: &Table, w: &mut ResponseWriter) {
    match h.tool.get_pd_region_stats(tbl.meta.id, false) {
        Ok(stats) => write_data(w, stats.storage_size),
        Err(err) => write_error(w, err),
    }
}

/// RegionHandler 入口：无 ID 时走路由汇总，有 ID 时返回详情。
pub fn serve_region_http(h: &RegionHandler, w: &mut ResponseWriter, req: &Request) {
    let params = req.path_vars();
    if params.get(REGION_ID).is_empty() {
        // 无 regionID 时 Go 根据 route name 返回 RegionsMeta 或 hot read/write。
        match h.tool.store.runtime.region_route(req) {
            Ok(data) => write_data(w, data),
            Err(err) => write_error(w, err),
        }
        return;
    }
    let region_id = match params.get(REGION_ID).parse::<u64>() {
        Ok(v) => v,
        Err(_) => {
            write_error(w, Error::new("invalid region id"));
            return;
        }
    };
    match h.tool.store.runtime.region_detail(region_id) {
        Ok(detail) => write_data(w, detail),
        Err(err) => write_error(w, err),
    }
}

/// parseQuery 保留 Go 对 `?a=` 与 `?a` 的区分；shouldUnescape 控制是否 URL decode。
pub fn parseQuery(query: &str, should_unescape: bool) -> Result<UrlValues, Error> {
    let mut values = UrlValues::new();
    for raw in query.split(&['&', ';'][..]).filter(|s| !s.is_empty()) {
        if let Some((key, value)) = raw.split_once('=') {
            values.append(
                maybe_unescape(key, should_unescape)?,
                maybe_unescape(value, should_unescape)?,
            );
        } else {
            values.ensure_key(maybe_unescape(raw, should_unescape)?);
        }
    }
    Ok(values)
}

/// MVCC 查询入口：按 op 分发 hex/idx/key/txn。
pub fn serve_mvcc_txn_http(h: &MvccTxnHandler, w: &mut ResponseWriter, req: &Request) {
    let params = req.path_vars();
    let result = match h.op.as_str() {
        OP_MVCC_GET_BY_HEX => h.tool.handle_mvcc_get_by_hex(&params),
        OP_MVCC_GET_BY_IDX => {
            parseQuery(req.raw_query(), true).and_then(|v| handleMvccGetByIdx(h, params, v))
        }
        OP_MVCC_GET_BY_KEY => {
            parseQuery(req.raw_query(), true).and_then(|v| handleMvccGetByKey(h, params, v))
        }
        OP_MVCC_GET_BY_TXN => handleMvccGetByTxn(h, params),
        _ => Err(Error::new("Operation not supported.")),
    };
    match result {
        Ok(data) => write_data(w, data),
        Err(err) => write_error(w, err),
    }
}
/// 按索引名与查询参数构造 handle，再取 MVCC。
pub fn handleMvccGetByIdx(
    h: &MvccTxnHandler,
    params: PathValues,
    values: UrlValues,
) -> Result<Data, Error> {
    let table = h
        .tool
        .get_table(&params.get(DB_NAME), &params.get(TABLE_NAME))?;
    let handle = h.tool.get_handle(&table, &params, &values)?;
    h.tool
        .get_mvcc_by_index_value(&table, &params.get(INDEX_NAME), values, handle)
}
/// 按行 handle 取 MVCC；可选 decode。
pub fn handleMvccGetByKey(
    h: &MvccTxnHandler,
    params: PathValues,
    values: UrlValues,
) -> Result<Data, Error> {
    let table = h
        .tool
        .get_table(&params.get(DB_NAME), &params.get(TABLE_NAME))?;
    let handle = h.tool.get_handle(&table, &params, &values)?;
    // decode 参数存在时，Go 会尝试把短值/历史值按列类型解码，并把解码错误放入响应。
    h.tool
        .get_mvcc_by_record_key(&table, handle, values.contains("decode"))
}
/// 将 MVCC 原始字节按列类型解码为 map。
pub fn decodeMvccData(
    h: &MvccTxnHandler,
    bs: Vec<u8>,
    cols: Map<FieldType>,
    tb: &TableInfo,
) -> Result<Map<String>, Error> {
    h.tool.store.runtime.decode_mvcc(bs, cols, tb)
}
/// 按 startTS 在表 key 范围内扫描 MVCC。
pub fn handleMvccGetByTxn(h: &MvccTxnHandler, params: PathValues) -> Result<Data, Error> {
    // Go uses strconv.ParseInt(..., 0, 64), so hexadecimal and octal values
    // are accepted before the value is passed to the MVCC uint64 API.
    let start_ts = parse_start_ts(params.get(START_TS))?;
    let table_id = h
        .tool
        .get_table_id(&params.get(DB_NAME), &params.get(TABLE_NAME))?;
    h.tool.get_mvcc_by_start_ts(
        start_ts,
        encode_table_prefix(table_id),
        encode_row_key_with_max_handle(table_id),
    )
}

/// 本机 server 信息响应体。
pub struct ServerInfo {
    pub is_owner: bool,
    pub max_procs: i32,
    pub gogc: i32,
    pub server_info: NodeServerInfo,
}
/// 写出本机 server 信息。
pub fn serve_server_info_http(h: &ServerInfoHandler, w: &mut ResponseWriter) {
    match h.tool.store.runtime.server_info() {
        Ok(info) => write_data(w, info),
        Err(err) => {
            h.tool
                .store
                .runtime
                .log_error("failed to get server info", &err);
            write_error(w, err);
        }
    }
}
/// 集群 server 汇总信息。
pub struct ClusterServerInfo {
    pub servers_num: i32,
    pub owner_id: String,
    pub is_all_server_version_consistent: bool,
    pub all_servers_diff_versions: Vec<VersionInfo>,
    pub all_servers_info: Map<NodeServerInfo>,
}
/// 写出集群全部 server 信息。
pub fn serve_all_server_info_http(h: &AllServerInfoHandler, w: &mut ResponseWriter) {
    match h.tool.store.runtime.cluster_server_info() {
        Ok(info) => write_data(w, info),
        Err(err) => {
            h.tool
                .store
                .runtime
                .log_error("failed to get all server info", &err);
            write_error(w, err);
        }
    }
}
/// 由 tableID 解析出的库表与 schema 版本。
pub struct DBTableInfo {
    pub db_info: DBInfo,
    pub table_info: TableInfo,
    pub schema_version: i64,
}
/// 按 tableID 返回库表信息。
pub fn serve_db_table_http(h: &DBTableHandler, w: &mut ResponseWriter, req: &Request) {
    match h.tool.store.runtime.db_table_info(&req.path_var(TABLE_ID)) {
        Ok(info) => write_data(w, info),
        Err(err) => write_error(w, err),
    }
}
/// 导出 executor profile 二进制。
pub fn serve_profile_http(h: &ProfileHandler, w: &mut ResponseWriter, req: &Request) {
    // Go 解析 start/end/type，收集 executor profile 后直接写 binary body。
    match h.tool.store.runtime.profile(req) {
        Ok(bytes) => {
            if let Err(error) = w.write(&bytes) {
                h.tool
                    .store
                    .runtime
                    .log_error("failed to write profile", &error);
            }
        }
        Err(err) => write_error(w, err),
    }
}

/// 测试/GC 调试 handler，用原子标志防止并发 GC。
pub struct TestHandler {
    pub tool: TikvHandlerTool,
    pub gc_is_running: AtomicU32,
}
/// 构造 TestHandler。
pub fn NewTestHandler(tool: TikvHandlerTool, gc_is_running: u32) -> TestHandler {
    TestHandler {
        tool,
        gc_is_running: AtomicU32::new(gc_is_running),
    }
}
/// 按 mod 分发测试模块（如 gc）。
pub fn serve_test_http(h: &TestHandler, w: &mut ResponseWriter, req: &Request) {
    match req.path_var("mod").to_lowercase().as_str() {
        "gc" => handleGC(h, req.path_var("op").to_lowercase(), w, req),
        other => write_error(w, Error::new(format!("module({}) not supported", other))),
    }
}
/// 删除 key 成功后的响应，含 hex 编码 key。
pub struct rowKeyDeleteResponse {
    pub key: String,
}
/// 测试用删除 row/index key 的 handler。
pub struct DeleteKeyHandler {
    pub tool: TikvHandlerTool,
}
/// 构造 DeleteKeyHandler。
pub fn NewDeleteKeyHandler(tool: TikvHandlerTool) -> DeleteKeyHandler {
    DeleteKeyHandler { tool }
}
/// 删除编码后的 row/index key（仅 POST）。
pub fn serve_delete_key_http(h: &DeleteKeyHandler, w: &mut ResponseWriter, req: &Request) {
    if req.method() != METHOD_POST {
        write_error(w, Error::new("This api only support POST method"));
        return;
    }
    let values = match parseQuery(req.raw_query(), true) {
        Ok(v) => v,
        Err(err) => {
            write_error(w, err);
            return;
        }
    };
    // Go 支持删除 row key 或 index key；公共 handle 表可通过主键列 query 构造 handle。
    match h
        .tool
        .store
        .runtime
        .delete_encoded_key(req.path_vars(), values)
    {
        Ok(key) => write_data(
            w,
            rowKeyDeleteResponse {
                key: hex_upper(&key),
            },
        ),
        Err(err) => write_error(w, err),
    }
}
/// GC 测试入口；互斥执行后恢复运行标志。
pub fn handleGC(h: &TestHandler, op: String, w: &mut ResponseWriter, req: &Request) {
    if h.gc_is_running
        .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        write_error(w, Error::new("GC is running"));
        return;
    }
    match op.as_str() {
        "resolvelock" => handleGCResolveLocks(h, w, req),
        other => write_error(w, Error::new(format!("operation({}) not supported", other))),
    }
    // Go 使用 defer atomic.StoreUint32；Rust 在分支后显式恢复。
    h.gc_is_running.store(0, Ordering::Release);
}
/// 按 safepoint 解析锁（resolve locks）。
pub fn handleGCResolveLocks(h: &TestHandler, w: &mut ResponseWriter, req: &Request) {
    let safe_point = match req.form_value("safepoint").parse::<u64>() {
        Ok(v) => v,
        Err(_) => {
            write_error(w, Error::new("parse safePoint failed"));
            return;
        }
    };
    if let Err(err) = h
        .tool
        .store
        .runtime
        .resolve_locks(req.context(), safe_point)
    {
        write_error(w, err.annotate("resolveLocks failed"));
    }
}
/// 切换 DDL hook（ctc/default，仅 POST）。
pub fn serve_ddl_hook_http(h: &DDLHookHandler, w: &mut ResponseWriter, req: &Request) {
    if req.method() != METHOD_POST {
        write_error(w, Error::new("This api only support POST method"));
        return;
    }
    let result = match req.form_value("ddl_hook").as_str() {
        "ctc_hook" => h.runtime.set_ctc_ddl_hook(true),
        "default_hook" => h.runtime.set_ctc_ddl_hook(false),
        _ => Ok(()),
    };
    match result {
        Ok(()) => write_data(w, "success!"),
        Err(err) => write_error(w, err),
    }
}
/// 更新 server labels（仅 POST）。
pub fn serve_label_http(h: &LabelHandler, w: &mut ResponseWriter, req: &Request) {
    if req.method() != METHOD_POST {
        write_error(w, Error::new("This api only support POST method"));
        return;
    }
    match h.runtime.update_server_labels(req.body()) {
        Ok(labels) => write_data(w, labels),
        Err(err) => write_error(w, err),
    }
}

/// Ingest 并发参数名类型。
pub type IngestParam = &'static str;
/// 单批最大 split ranges。
pub const INGEST_PARAM_MAX_BATCH_SPLIT_RANGES: IngestParam = "max_batch_split_ranges";
/// 每秒最大 split ranges。
pub const INGEST_PARAM_MAX_SPLIT_RANGES_PER_SEC: IngestParam = "max_split_ranges_per_sec";
/// 最大 in-flight ingest 数。
pub const INGEST_PARAM_MAX_INFLIGHT: IngestParam = "max_inflight";
/// 每秒最大 ingest 数。
pub const INGEST_PARAM_MAX_PER_SECOND: IngestParam = "max_per_second";
/// 读写 ingest 并发限流参数的 handler。
pub struct IngestConcurrencyHandler {
    pub tool: TikvHandlerTool,
    pub param: IngestParam,
}
/// 构造 IngestConcurrencyHandler。
pub fn NewIngestConcurrencyHandler(
    tool: TikvHandlerTool,
    param: IngestParam,
) -> IngestConcurrencyHandler {
    IngestConcurrencyHandler { tool, param }
}
/// GET 读、POST 写指定 ingest 参数。
pub fn serve_ingest_concurrency_http(
    h: &IngestConcurrencyHandler,
    w: &mut ResponseWriter,
    req: &Request,
) {
    if !matches!(
        h.param,
        INGEST_PARAM_MAX_BATCH_SPLIT_RANGES
            | INGEST_PARAM_MAX_SPLIT_RANGES_PER_SEC
            | INGEST_PARAM_MAX_INFLIGHT
            | INGEST_PARAM_MAX_PER_SECOND
    ) {
        write_error(w, Error::new("unsupported ingest parameter"));
        return;
    }
    match req.method() {
        METHOD_GET => match h.tool.store.runtime.ingest_get(h.param) {
            Ok(v) => write_data(w, v),
            Err(err) => write_error(w, err),
        },
        METHOD_POST => {
            let payload = match h.tool.store.runtime.decode_value_payload(req.body()) {
                Ok(v) => v,
                Err(err) => {
                    write_error(w, err);
                    return;
                }
            };
            if payload.value < 0.0 {
                write_error(w, Error::new("value must be >= 0"));
                return;
            }
            match h.tool.store.runtime.ingest_set(h.param, payload.value) {
                Ok(_old) => {
                    h.tool.store.runtime.log_info("set ingest concurrency");
                    write_data(w, Map::<String>::message("success".to_owned()));
                }
                Err(err) => write_error(w, err),
            }
        }
        _ => {
            w.write_header(STATUS_METHOD_NOT_ALLOWED);
            write_error(w, Error::new("method not allowed"));
        }
    }
}

/// 事务 GC 状态查询 handler。
pub struct TxnGCStatesHandler {
    pub store: Storage,
}
/// 构造 TxnGCStatesHandler。
pub fn NewTxnGCStatesHandler(store: Storage) -> TxnGCStatesHandler {
    TxnGCStatesHandler { store }
}
/// 查询事务 GC 状态（仅 GET）。
pub fn serve_txn_gc_states_http(gc: &TxnGCStatesHandler, w: &mut ResponseWriter, req: &Request) {
    if req.method() != METHOD_GET {
        http_error(
            w,
            "This API only supports GET method",
            STATUS_METHOD_NOT_ALLOWED,
        );
        return;
    }
    match gc.store.runtime.gc_state() {
        Ok(state) => write_data(w, state),
        Err(err) => write_error(w, err),
    }
}

impl SettingsHandler {
    /// HTTP 入口：转发到对应 serve_* 函数。
    pub fn ServeHTTP(&self, w: &mut ResponseWriter, req: &Request) {
        serve_settings_http(self, w, req);
    }
}
impl SchemaHandler {
    /// HTTP 入口：转发到对应 serve_* 函数。
    pub fn ServeHTTP(&self, w: &mut ResponseWriter, req: &Request) {
        serve_schema_http(self, w, req);
    }
}
impl SchemaStorageHandler {
    /// HTTP 入口：转发到对应 serve_* 函数。
    pub fn ServeHTTP(&self, w: &mut ResponseWriter, req: &Request) {
        serve_schema_storage_http(self, w, req);
    }
}
impl DBTableHandler {
    /// HTTP 入口：转发到对应 serve_* 函数。
    pub fn ServeHTTP(&self, w: &mut ResponseWriter, req: &Request) {
        serve_db_table_http(self, w, req);
    }
}
impl FlashReplicaHandler {
    /// HTTP 入口：转发到对应 serve_* 函数。
    pub fn ServeHTTP(&self, w: &mut ResponseWriter, req: &Request) {
        serve_flash_replica_http(self, w, req);
    }
}
impl RegionHandler {
    /// HTTP 入口：转发到对应 serve_* 函数。
    pub fn ServeHTTP(&self, w: &mut ResponseWriter, req: &Request) {
        serve_region_http(self, w, req);
    }
}
impl TableHandler {
    /// HTTP 入口：转发到对应 serve_* 函数。
    pub fn ServeHTTP(&self, w: &mut ResponseWriter, req: &Request) {
        serve_table_http(self, w, req);
    }
}
impl DDLHistoryJobHandler {
    /// HTTP 入口：转发到对应 serve_* 函数。
    pub fn ServeHTTP(&self, w: &mut ResponseWriter, req: &Request) {
        serve_ddl_history_http(self, w, req);
    }
}
impl DDLResignOwnerHandler {
    /// HTTP 入口：转发到对应 serve_* 函数。
    pub fn ServeHTTP(&self, w: &mut ResponseWriter, req: &Request) {
        serve_ddl_resign_owner_http(self, w, req);
    }
}
impl DDLCheckHandler {
    /// HTTP 入口：转发到对应 serve_* 函数。
    pub fn ServeHTTP(&self, w: &mut ResponseWriter, req: &Request) {
        serve_ddl_check_http(self, w, req);
    }
}
impl ServerInfoHandler {
    /// HTTP 入口：转发到对应 serve_* 函数。
    pub fn ServeHTTP(&self, w: &mut ResponseWriter, _: &Request) {
        serve_server_info_http(self, w);
    }
}
impl AllServerInfoHandler {
    /// HTTP 入口：转发到对应 serve_* 函数。
    pub fn ServeHTTP(&self, w: &mut ResponseWriter, _: &Request) {
        serve_all_server_info_http(self, w);
    }
}
impl ProfileHandler {
    /// HTTP 入口：转发到对应 serve_* 函数。
    pub fn ServeHTTP(&self, w: &mut ResponseWriter, req: &Request) {
        serve_profile_http(self, w, req);
    }
}
impl DDLHookHandler {
    /// HTTP 入口：转发到对应 serve_* 函数。
    pub fn ServeHTTP(&self, w: &mut ResponseWriter, req: &Request) {
        serve_ddl_hook_http(self, w, req);
    }
}
impl ValueHandler {
    /// HTTP 入口：转发到对应 serve_* 函数。
    pub fn ServeHTTP(&self, w: &mut ResponseWriter, req: &Request) {
        serve_value_http(self, w, req);
    }
}
impl LabelHandler {
    /// HTTP 入口：转发到对应 serve_* 函数。
    pub fn ServeHTTP(&self, w: &mut ResponseWriter, req: &Request) {
        serve_label_http(self, w, req);
    }
}
impl MvccTxnHandler {
    /// HTTP 入口：转发到对应 serve_* 函数。
    pub fn ServeHTTP(&self, w: &mut ResponseWriter, req: &Request) {
        serve_mvcc_txn_http(self, w, req);
    }
}
impl TestHandler {
    /// HTTP 入口：转发到对应 serve_* 函数。
    pub fn ServeHTTP(&self, w: &mut ResponseWriter, req: &Request) {
        serve_test_http(self, w, req);
    }
}
impl DeleteKeyHandler {
    /// HTTP 入口：转发到对应 serve_* 函数。
    pub fn ServeHTTP(&self, w: &mut ResponseWriter, req: &Request) {
        serve_delete_key_http(self, w, req);
    }
}
impl IngestConcurrencyHandler {
    /// HTTP 入口：转发到对应 serve_* 函数。
    pub fn ServeHTTP(&self, w: &mut ResponseWriter, req: &Request) {
        serve_ingest_concurrency_http(self, w, req);
    }
}
impl TxnGCStatesHandler {
    /// HTTP 入口：转发到对应 serve_* 函数。
    pub fn ServeHTTP(&self, w: &mut ResponseWriter, req: &Request) {
        serve_txn_gc_states_http(self, w, req);
    }
}

#[derive(Clone)]
/// Handler 共用工具：存储、Region cache、schema 解析等。
pub struct TikvHandlerTool {
    pub store: Storage,
    pub region_cache: RegionCache,
}
#[derive(Clone)]
/// kv.Storage 占位，持有运行时。
pub struct Storage {
    pub runtime: Arc<dyn TikvRuntime>,
}
#[derive(Clone)]
/// Region 缓存，可取 PD client。
pub struct RegionCache {
    pub pd_client: PdClient,
}
#[derive(Clone)]
/// PD（Placement Driver）客户端占位。
pub struct PdClient {
    pub runtime: Arc<dyn TikvRuntime>,
}
#[derive(Clone, Default)]
/// HTTP 请求占位。
pub struct Request {
    pub method: String,
    pub form: HashMap<String, Vec<String>>,
    pub raw_query: String,
    pub body: Vec<u8>,
    pub context: Context,
    pub path: HashMap<String, String>,
    pub form_error: Option<Error>,
}
#[derive(Default)]
/// HTTP 响应写出器占位。
pub struct ResponseWriter {
    pub headers: HashMap<String, String>,
    pub status: Option<u16>,
    pub body: Vec<u8>,
    pub data: Vec<Box<dyn Any + Send>>,
    pub errors: Vec<(Option<u16>, Error)>,
}
#[derive(Clone, Default)]
/// 请求上下文占位。
pub struct Context {
    pub request_id: String,
}
/// Session 占位。
pub struct Session;
/// 查询结果集占位。
pub struct RecordSet {
    pub rows: Vec<Result<Vec<String>, Error>>,
    pub closed: bool,
}
/// DDL history job response.  The status adapter needs the job ID on the
/// wire so pagination and ordering assertions are observable.
pub struct Job {
    pub id: i64,
}
/// 配置序列化占位。
pub struct Config(pub String);
/// 信息模式（表元数据视图）占位。
pub struct InfoSchema {
    pub tables: Vec<TableInfo>,
}
/// 大小写不敏感标识符。
pub struct CIStr(pub String);
/// 逻辑表占位。
pub struct Table {
    pub meta: TableInfo,
}
/// 物理表（含分区）占位。
pub struct PhysicalTable {
    pub id: i64,
    pub name: String,
}
#[derive(Clone)]
/// 表元信息占位。
pub struct TableInfo {
    pub id: i64,
    pub name: String,
    pub indices: Vec<IndexInfo>,
    pub partitions: Vec<PartitionInfo>,
    pub is_common_handle: bool,
    pub tiflash_replica: Option<TiFlashReplica>,
    pub tiflash_replica_infos: Vec<TableFlashReplicaInfo>,
}
/// 仅含表名的简化信息。
pub struct TableNameInfo;
#[derive(Clone)]
/// 索引元信息占位。
pub struct IndexInfo {
    pub id: i64,
    pub name: String,
    pub primary: bool,
}
#[derive(Clone)]
/// 分区信息占位。
pub struct PartitionInfo {
    pub id: i64,
    pub name: String,
}
#[derive(Clone)]
/// TiFlash 副本配置占位。
pub struct TiFlashReplica;
/// Region 上的 frame 范围辅助。
pub struct RegionFrameRange {
    pub index_frames: Vec<FrameItem>,
    pub record_frames: Vec<FrameItem>,
}
/// Region 元数据。
pub struct RegionMeta {
    pub id: u64,
}
/// 列字段类型占位。
pub struct FieldType {
    pub column_type: u8,
    pub flag: u64,
    pub length: i64,
    pub decimal: i32,
}
/// 通用数据写出占位。
pub struct Data(pub String);
/// 数据库信息。
pub struct DBInfo {
    pub name: String,
}
/// 节点 server 信息占位。
pub struct NodeServerInfo;
/// 版本信息占位。
pub struct VersionInfo;
/// PD Region 统计占位。
pub struct Stats {
    pub storage_size: u64,
}
/// ingest 设置请求体中的 value。
pub struct ValuePayload {
    pub value: f64,
}
#[derive(Clone, Debug, Eq, PartialEq)]
/// Handler 错误占位。
pub struct Error {
    pub message: String,
}
impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}
impl std::error::Error for Error {}
/// 查询串多值 map，区分 `?a=` 与 `?a`。
pub struct UrlValues {
    inner: Vec<(String, Vec<String>)>,
}
/// 路径变量别名。
pub type PathValues = UrlValues;
/// 有序键值 map 占位。
pub struct Map<T = String>(Vec<(String, T)>);
/// 可 JSON 序列化的占位 trait。
pub trait JsonValue {}

/// TiKV handler 运行时边界：配置、schema、MVCC、PD、ingest、日志等副作用。
pub trait TikvRuntime: Send + Sync {
    /// 获取当前 InfoSchema。
    fn schema(&self) -> Result<InfoSchema, Error>;
    /// 查询表 PD region 统计。
    fn pd_region_stats(&self, table_id: i64, include_learners: bool) -> Result<Stats, Error>;
    /// 按 hex key 查 MVCC。
    fn mvcc_by_hex(&self, params: &PathValues) -> Result<Data, Error>;
    /// 按库表名取 Table。
    fn table(&self, database: &str, table: &str) -> Result<Table, Error>;
    /// 解析行 handle。
    fn handle(&self, table: &Table, params: &PathValues, values: &UrlValues)
    -> Result<Data, Error>;
    /// 按索引查 MVCC。
    fn mvcc_by_index(
        &self,
        table: &Table,
        index: &str,
        values: UrlValues,
        handle: Data,
    ) -> Result<Data, Error>;
    /// 按记录 handle 查 MVCC。
    fn mvcc_by_record(&self, table: &Table, handle: Data, decode: bool) -> Result<Data, Error>;
    /// 解析表 ID。
    fn table_id(&self, database: &str, table: &str) -> Result<i64, Error>;
    /// 按 startTS 与范围查 MVCC。
    fn mvcc_by_start_ts(&self, start_ts: u64, start: Vec<u8>, end: Vec<u8>) -> Result<Data, Error>;
    /// PD 地址列表。
    fn pd_addresses(&self) -> Result<Vec<String>, Error>;
    /// 解码行中单列值。
    fn decode_row(
        &self,
        bytes: &[u8],
        column_id: i64,
        field_type: FieldType,
    ) -> Result<String, Error>;
    /// 全局配置快照。
    fn global_config(&self) -> Config;
    /// 应用单项设置。
    fn apply_setting(&self, key: &str, value: &str) -> Result<(), Error>;
    /// 解码 TiFlash 上报状态。
    fn decode_flash_status(&self, body: &[u8]) -> Result<tableFlashReplicaStatus, Error>;
    /// 更新表 TiFlash replica 可用性。
    fn update_table_replica(&self, table_id: i64, available: bool) -> Result<(), Error>;
    /// 历史/已删表 TiFlash 信息。
    fn historical_tiflash(&self, schema: &InfoSchema) -> Result<Vec<TableFlashReplicaInfo>, Error>;
    /// 查询 INFORMATION_SCHEMA.TABLES 存储统计。
    fn schema_storage(
        &self,
        schema: Option<&str>,
        table: Option<&str>,
    ) -> Result<Vec<SchemaTableStorage>, Error>;
    /// 解析 schema 路由参数。
    fn resolve_schema_route(
        &self,
        request: &Request,
    ) -> Result<(Option<CIStr>, Option<CIStr>, bool), Error>;
    /// 解析 schema 请求并返回数据。
    fn resolve_schema_request(&self, schema: &InfoSchema, request: &Request)
    -> Result<Data, Error>;
    /// 解析表路由（含分区名）。
    fn resolve_table_route(&self, request: &Request) -> Result<(Table, String), Error>;
    /// DDL 历史任务。
    fn history_ddl(&self, job_id: i32, limit: i32) -> Result<Vec<Job>, Error>;
    /// 放弃 DDL owner。
    fn resign_ddl_owner(&self) -> Result<(), Error>;
    /// 执行 admin check SQL。
    fn execute_admin_check(&self, context: Context, sql: &str) -> Result<Vec<Vec<String>>, Error>;
    /// 添加 scatter 调度。
    fn add_scatter(&self, start: Vec<u8>, end: Vec<u8>, name: &str) -> Result<(), Error>;
    /// 删除 scatter 调度。
    fn delete_scatter(&self, name: &str) -> Result<(), Error>;
    /// 启动或停止表 scatter。
    fn scatter_table(&self, table: PhysicalTable, stop: bool) -> Result<(), Error>;
    /// 表覆盖 Region 列表。
    fn table_regions(&self, table: &Table) -> Result<Vec<TableRegions>, Error>;
    /// 扫描指定 ID 的 Region。
    fn scan_regions(&self, table: &Table, id: i64, name: &str) -> Result<TableRegions, Error>;
    /// 按路由返回 Region meta/hot。
    fn region_route(&self, request: &Request) -> Result<Data, Error>;
    /// Region 详情。
    fn region_detail(&self, region_id: u64) -> Result<RegionDetail, Error>;
    /// 解码 MVCC 字节。
    fn decode_mvcc(
        &self,
        bytes: Vec<u8>,
        columns: Map<FieldType>,
        table: &TableInfo,
    ) -> Result<Map<String>, Error>;
    /// 本节点信息。
    fn server_info(&self) -> Result<ServerInfo, Error>;
    /// 集群节点信息。
    fn cluster_server_info(&self) -> Result<ClusterServerInfo, Error>;
    /// 按 tableID 取库表信息。
    fn db_table_info(&self, table_id: &str) -> Result<DBTableInfo, Error>;
    /// 采集 profile 字节。
    fn profile(&self, request: &Request) -> Result<Vec<u8>, Error>;
    /// 删除编码后的 key。
    fn delete_encoded_key(&self, params: PathValues, values: UrlValues) -> Result<Vec<u8>, Error>;
    /// 在 safepoint 解析锁。
    fn resolve_locks(&self, context: Context, safe_point: u64) -> Result<(), Error>;
    /// 开关 CTC DDL hook。
    fn set_ctc_ddl_hook(&self, enabled: bool) -> Result<(), Error>;
    /// 更新 server labels。
    fn update_server_labels(&self, body: &[u8]) -> Result<Map<String>, Error>;
    /// 读取 ingest 参数。
    fn ingest_get(&self, param: IngestParam) -> Result<Map<String>, Error>;
    /// 设置 ingest 参数，返回旧值。
    fn ingest_set(&self, param: IngestParam, value: f64) -> Result<f64, Error>;
    /// 解码 value JSON 载荷。
    fn decode_value_payload(&self, body: &[u8]) -> Result<ValuePayload, Error>;
    /// 事务 GC 状态。
    fn gc_state(&self) -> Result<Data, Error>;
    /// 错误日志。
    fn log_error(&self, message: &str, error: &Error);
    /// 信息日志。
    fn log_info(&self, message: &str);
}
/// HTTP GET。
pub const METHOD_GET: &str = "GET";
/// HTTP POST。
pub const METHOD_POST: &str = "POST";
/// HTTP 200。
pub const STATUS_OK: u16 = 200;
/// HTTP 405。
pub const STATUS_METHOD_NOT_ALLOWED: u16 = 405;
/// Content-Type 头名。
pub const HEADER_CONTENT_TYPE: &str = "Content-Type";
/// JSON Content-Type。
pub const CONTENT_TYPE_JSON: &str = "application/json";
/// 路径变量：库名。
pub const DB_NAME: &str = "db";
/// 路径变量：表名。
pub const TABLE_NAME: &str = "table";
/// 路径变量：表 ID。
pub const TABLE_ID: &str = "tableID";
/// 路径变量：索引名。
pub const INDEX_NAME: &str = "index";
/// 路径变量：Region ID。
pub const REGION_ID: &str = "regionID";
/// 路径变量：列 ID。
pub const COLUMN_ID: &str = "colID";
/// 路径变量：列类型。
pub const COLUMN_TP: &str = "colTp";
/// 路径变量：列 flag。
pub const COLUMN_FLAG: &str = "colFlag";
/// 路径变量：列长度。
pub const COLUMN_LEN: &str = "colLen";
/// 查询参数：行二进制。
pub const ROW_BIN: &str = "rowBin";
/// 路径变量：事务 startTS。
pub const START_TS: &str = "startTS";
/// 时长占位（秒）。
pub struct Duration(i64);
impl Duration {
    /// 以秒构造 Duration。
    pub const fn seconds(v: i64) -> Duration {
        Duration(v)
    }
}
impl Error {
    /// 构造错误。
    fn new<T: Into<String>>(message: T) -> Error {
        Error {
            message: message.into(),
        }
    }
    /// 构造坏请求类错误。
    fn bad_request<T: Into<String>>(message: T) -> Error {
        Error::new(message)
    }
    /// 为错误消息添加前缀注解。
    fn annotate<T: Into<String>>(self, prefix: T) -> Error {
        Error::new(format!("{}: {}", prefix.into(), self.message))
    }
}
impl PdClient {
    /// 克隆并带上 caller component（当前透传）。
    fn with_caller_component(&self, _: &str) -> PdClient {
        self.clone()
    }
}
impl TikvHandlerTool {
    /// 获取当前 InfoSchema。
    fn schema(&self) -> Result<InfoSchema, Error> {
        self.store.runtime.schema()
    }
    /// 查询表的 PD region 统计。
    fn get_pd_region_stats(&self, table_id: i64, include_learners: bool) -> Result<Stats, Error> {
        self.store
            .runtime
            .pd_region_stats(table_id, include_learners)
    }
    /// 按 hex key 查询 MVCC。
    fn handle_mvcc_get_by_hex(&self, params: &PathValues) -> Result<Data, Error> {
        self.store.runtime.mvcc_by_hex(params)
    }
    /// 按库表名解析 Table。
    fn get_table(&self, database: &str, table: &str) -> Result<Table, Error> {
        let (table_name, _) = astersql_server_handler::util::ExtractTableAndPartitionName(table);
        self.store.runtime.table(database, &table_name)
    }
    /// 由路径/查询参数解析行 handle。
    fn get_handle(
        &self,
        table: &Table,
        params: &PathValues,
        values: &UrlValues,
    ) -> Result<Data, Error> {
        self.store.runtime.handle(table, params, values)
    }
    /// 按索引值查询 MVCC。
    fn get_mvcc_by_index_value(
        &self,
        table: &Table,
        index: &str,
        values: UrlValues,
        handle: Data,
    ) -> Result<Data, Error> {
        self.store
            .runtime
            .mvcc_by_index(table, index, values, handle)
    }
    /// 按记录 key/handle 查询 MVCC。
    fn get_mvcc_by_record_key(
        &self,
        table: &Table,
        handle: Data,
        decode: bool,
    ) -> Result<Data, Error> {
        self.store.runtime.mvcc_by_record(table, handle, decode)
    }
    /// 解析表 ID。
    fn get_table_id(&self, database: &str, table: &str) -> Result<i64, Error> {
        self.store.runtime.table_id(database, table)
    }
    /// 按 startTS 与 key 范围查询 MVCC。
    fn get_mvcc_by_start_ts(
        &self,
        start_ts: u64,
        start: Vec<u8>,
        end: Vec<u8>,
    ) -> Result<Data, Error> {
        self.store.runtime.mvcc_by_start_ts(start_ts, start, end)
    }
}
impl Storage {
    /// 返回 PD 地址。
    fn pd_addrs(&self) -> Result<Vec<String>, Error> {
        self.runtime.pd_addresses()
    }
}
impl Request {
    /// 请求方法。
    fn method(&self) -> &str {
        &self.method
    }
    /// 若表单解析失败则返回错误。
    fn parse_form(&self) -> Result<(), Error> {
        self.form_error.clone().map_or(Ok(()), Err)
    }
    /// 取表单首个值。
    fn form_value(&self, name: &str) -> String {
        self.form
            .get(name)
            .and_then(|values| values.first())
            .cloned()
            .unwrap_or_default()
    }
    /// 原始 query 串。
    fn raw_query(&self) -> &str {
        &self.raw_query
    }
    /// 请求体。
    fn body(&self) -> &[u8] {
        &self.body
    }
    /// 克隆请求上下文。
    fn context(&self) -> Context {
        self.context.clone()
    }
    /// 取单个路径变量。
    fn path_var(&self, name: &str) -> String {
        self.path.get(name).cloned().unwrap_or_default()
    }
    /// 全部路径变量。
    fn path_vars(&self) -> PathValues {
        UrlValues {
            inner: self
                .path
                .iter()
                .map(|(key, value)| (key.clone(), vec![value.clone()]))
                .collect(),
        }
    }
}
impl ResponseWriter {
    /// 设置响应头。
    fn header_set(&mut self, name: &str, value: &str) {
        self.headers.insert(name.to_owned(), value.to_owned());
    }
    /// 设置响应状态码。
    fn write_header(&mut self, status: u16) {
        self.status = Some(status);
    }
    /// 追加响应体字节。
    fn write(&mut self, data: &[u8]) -> Result<(), Error> {
        self.body.extend_from_slice(data);
        Ok(())
    }
}
impl UrlValues {
    /// 空 UrlValues。
    fn new() -> UrlValues {
        UrlValues { inner: Vec::new() }
    }
    /// 追加一对键值。
    fn append(&mut self, k: String, v: String) {
        self.inner.push((k, vec![v]));
    }
    /// 确保键存在（可无值，对齐 `?a`）。
    fn ensure_key(&mut self, k: String) {
        self.inner.push((k, Vec::new()));
    }
    /// 取首个值，缺省空串。
    /// 取首个值，缺省空串；供注入式运行时读取路由参数。
    pub fn get(&self, name: &str) -> String {
        self.inner
            .iter()
            .find(|(key, _)| key == name)
            .and_then(|(_, values)| values.first())
            .cloned()
            .unwrap_or_default()
    }
    /// 同 get。
    fn first(&self, name: &str) -> String {
        self.get(name)
    }
    /// 键出现次数合计。
    pub fn count(&self, name: &str) -> usize {
        self.inner
            .iter()
            .filter(|(key, _)| key == name)
            .map(|(_, values)| values.len())
            .sum()
    }
    /// 是否包含键。
    pub fn contains(&self, name: &str) -> bool {
        self.inner.iter().any(|(key, _)| key == name)
    }
}
impl<T> Map<T> {
    /// Constructs a one-field ordered response map.
    pub fn single(key: impl Into<String>, value: T) -> Self {
        Self(vec![(key.into(), value)])
    }

    /// Exposes response fields to the status-server wire adapter while
    /// preserving insertion order used by Go's stable JSON assertions.
    pub fn entries(&self) -> &[(String, T)] {
        &self.0
    }

    /// 构造 message 字段 map。
    fn message(message: String) -> Map<String> {
        Map::single("message", message)
    }
}
impl FieldType {
    /// 构造 FieldType。
    fn new(column_type: u8, flag: u64, length: i64, decimal: i32) -> FieldType {
        FieldType {
            column_type,
            flag,
            length,
            decimal,
        }
    }
}
impl InfoSchema {
    /// 过滤具有特殊属性的表（此处取含 TiFlash replica 的表）。
    fn tables_with_special_attribute(&self, _: &str) -> Vec<TableInfo> {
        self.tables
            .iter()
            .filter(|table| table.tiflash_replica.is_some())
            .cloned()
            .collect()
    }
    /// 按表/分区 ID 查找。
    fn table_by_id_or_partition(&self, id: i64) -> Result<TableInfo, Error> {
        if let Some(table) = self.tables.iter().find(|table| table.id == id) {
            return Ok(table.clone());
        }
        self.tables
            .iter()
            .find(|table| table.partitions.iter().any(|partition| partition.id == id))
            .cloned()
            .ok_or_else(|| Error::new("table id not exists"))
    }
}
impl TableInfo {
    /// 展开表的 TiFlash replica 信息列表。
    fn expand_tiflash_replica_infos(&self) -> Vec<TableFlashReplicaInfo> {
        self.tiflash_replica_infos.clone()
    }
}
impl RegionFrameRange {
    /// 返回索引相关 frames。
    fn index_frames(&self, _: &str, _: &TableInfo, _: &IndexInfo) -> Vec<FrameItem> {
        self.index_frames.clone()
    }
    /// 返回记录相关 frames。
    fn record_frames(&self, _: &str, _: &TableInfo) -> Vec<FrameItem> {
        self.record_frames.clone()
    }
}
impl Table {
    /// 解析分区名到 PhysicalTable；空串则用表名。
    fn partition(&self, partition: &str) -> PhysicalTable {
        PhysicalTable {
            id: self.meta.id,
            name: if partition.is_empty() {
                self.meta.name.clone()
            } else {
                partition.to_owned()
            },
        }
    }
}
impl JsonValue for TableInfo {}
impl JsonValue for TableNameInfo {}

/// 记录错误到 ResponseWriter（延迟写出）。
fn write_error(w: &mut ResponseWriter, error: Error) {
    w.errors.push((None, error));
}
/// 将成功数据挂到 ResponseWriter。
fn write_data<T: Any + Send + 'static>(w: &mut ResponseWriter, data: T) {
    w.data.push(Box::new(data));
}
/// 设置状态码并记录错误。
fn http_error(w: &mut ResponseWriter, message: &str, status: u16) {
    w.write_header(status);
    w.errors.push((Some(status), Error::new(message)));
}

/// 解析有符号整数，支持 0x/0o/前导 0 八进制。
pub(crate) fn parse_i64(value: String) -> Result<i64, Error> {
    let (negative, unsigned) = if let Some(rest) = value.strip_prefix('-') {
        (true, rest)
    } else {
        (false, value.strip_prefix('+').unwrap_or(&value))
    };
    let (radix, digits) = base_zero_digits(unsigned)
        .map_err(|error| Error::new(format!("invalid integer {value}: {error}")))?;
    let magnitude = u64::from_str_radix(&digits, radix)
        .map_err(|error| Error::new(format!("invalid integer {value}: {error}")))?;
    if negative {
        if magnitude == 1_u64 << 63 {
            Ok(i64::MIN)
        } else {
            let magnitude = i64::try_from(magnitude)
                .map_err(|error| Error::new(format!("invalid integer {value}: {error}")))?;
            Ok(-magnitude)
        }
    } else {
        i64::try_from(magnitude)
            .map_err(|error| Error::new(format!("invalid integer {value}: {error}")))
    }
}

/// Parse a transaction start timestamp with Go's base-0 integer rules.
pub(crate) fn parse_start_ts(value: String) -> Result<u64, Error> {
    parse_i64(value).map(|timestamp| timestamp as u64)
}
/// 解析无符号整数，支持多进制前缀。
fn parse_u64(value: String) -> Result<u64, Error> {
    let (radix, digits) = base_zero_digits(&value)
        .map_err(|error| Error::new(format!("invalid unsigned integer {value}: {error}")))?;
    u64::from_str_radix(&digits, radix)
        .map_err(|error| Error::new(format!("invalid unsigned integer {value}: {error}")))
}
/// 按 Go `strconv.ParseInt/ParseUint` 的 base 0 规则拆分并校验数字。
fn base_zero_digits(value: &str) -> Result<(u32, String), &'static str> {
    let (radix, digits, prefixed) = if let Some(rest) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        (16, rest, true)
    } else if let Some(rest) = value
        .strip_prefix("0b")
        .or_else(|| value.strip_prefix("0B"))
    {
        (2, rest, true)
    } else if let Some(rest) = value
        .strip_prefix("0o")
        .or_else(|| value.strip_prefix("0O"))
    {
        (8, rest, true)
    } else if value.len() > 1 && value.starts_with('0') {
        (8, value, false)
    } else {
        (10, value, false)
    };
    if digits.is_empty() {
        return Err("invalid syntax");
    }
    let bytes = digits.as_bytes();
    let mut cleaned = String::with_capacity(digits.len());
    for (index, byte) in bytes.iter().copied().enumerate() {
        if byte == b'_' {
            let after_prefix = prefixed && index == 0;
            let between_digits = index > 0 && bytes[index - 1].is_ascii_alphanumeric();
            let followed_by_digit = bytes
                .get(index + 1)
                .is_some_and(|next| next.is_ascii_alphanumeric());
            if !(followed_by_digit && (after_prefix || between_digits)) {
                return Err("invalid underscore placement");
            }
            continue;
        }
        let digit = (byte as char).to_digit(radix).ok_or("invalid digit")?;
        cleaned.push(char::from_digit(digit, radix).expect("validated digit"));
    }
    Ok((radix, cleaned))
}
/// 轻量 base64 解码（仅跳过 CR/LF，并严格校验 padding）。
pub(crate) fn base64_decode(value: &str) -> Result<Vec<u8>, Error> {
    let mut output = Vec::with_capacity(value.len() * 3 / 4);
    let mut quartet = [0_u8; 4];
    let mut count = 0;
    let mut padded = false;
    for byte in value.bytes() {
        // Go's encoding/base64 ignores only CR and LF, not arbitrary ASCII
        // whitespace, and rejects any encoded data after terminal padding.
        if matches!(byte, b'\r' | b'\n') {
            continue;
        }
        if padded {
            return Err(Error::new("invalid base64 data after padding"));
        }
        quartet[count] = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => 64,
            _ => return Err(Error::new("invalid base64 character")),
        };
        count += 1;
        if count == 4 {
            if quartet[0] == 64 || quartet[1] == 64 || (quartet[2] == 64 && quartet[3] != 64) {
                return Err(Error::new("invalid base64 padding"));
            }
            output.push((quartet[0] << 2) | (quartet[1] >> 4));
            if quartet[2] != 64 {
                output.push((quartet[1] << 4) | (quartet[2] >> 2));
            }
            if quartet[3] != 64 {
                output.push((quartet[2] << 6) | quartet[3]);
            }
            padded = quartet[2] == 64 || quartet[3] == 64;
            count = 0;
        }
    }
    if count != 0 {
        return Err(Error::new("invalid base64 length"));
    }
    Ok(output)
}
/// 字节转小写 hex。
fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    bytes
        .iter()
        .flat_map(|byte| {
            [
                HEX[(byte >> 4) as usize] as char,
                HEX[(byte & 15) as usize] as char,
            ]
        })
        .collect()
}
/// 字节转大写 hex。
fn hex_upper(bytes: &[u8]) -> String {
    hex_encode(bytes).to_ascii_uppercase()
}
/// 解析 DDL 历史查询参数 start_job_id 与 limit。
fn parse_ddl_history_query(req: &Request) -> Result<(i32, i32), Error> {
    const DEFAULT_HISTORY_LIMIT: i32 = 10;
    let job_value = req.form_value("start_job_id");
    let job_id = if job_value.is_empty() {
        0
    } else {
        let value = job_value
            .parse::<i32>()
            .map_err(|error| Error::new(error.to_string()))?;
        if value < 1 {
            return Err(Error::new(
                "ddl history start_job_id must be greater than 0",
            ));
        }
        value
    };
    let limit_value = req.form_value("limit");
    let limit = if limit_value.is_empty() {
        0
    } else {
        let value = limit_value
            .parse::<i32>()
            .map_err(|error| Error::new(error.to_string()))?;
        if !(1..=DEFAULT_HISTORY_LIMIT).contains(&value) {
            return Err(Error::new(format!(
                "ddl history limit must be greater than 0 and less than or equal to {DEFAULT_HISTORY_LIMIT}"
            )));
        }
        value
    };
    Ok((job_id, limit))
}
/// 生成反引号转义的 `db`.`table`。
fn quoted_table_name(database: &str, table: &str) -> String {
    format!(
        "`{}`.`{}`",
        database.replace('`', "``"),
        table.replace('`', "``")
    )
}

/// admin check index 诊断结果。
pub struct DDLCheckResult {
    pub db: String,
    pub table: String,
    pub index: String,
    pub check_sql: String,
    pub rows: Vec<Vec<String>>,
    pub result: String,
    pub error: Option<String>,
}
/// 组装 DDL check 结果结构。
fn check_index_result(
    db: String,
    table: String,
    index: String,
    sql: &str,
    rows: Vec<Vec<String>>,
    error: Option<Error>,
) -> DDLCheckResult {
    DDLCheckResult {
        db,
        table,
        index,
        check_sql: sql.to_owned(),
        rows,
        result: if error.is_some() { "failed" } else { "success" }.to_owned(),
        error: error.map(|value| value.to_string()),
    }
}
/// 将 i64 编码为可比较的大端字节（符号位翻转）。
fn comparable_i64(value: i64) -> [u8; 8] {
    ((value as u64) ^ (1_u64 << 63)).to_be_bytes()
}
/// 编码表前缀 key：`t` + comparable table_id。
fn encode_table_prefix(table_id: i64) -> Vec<u8> {
    let mut key = Vec::with_capacity(9);
    key.push(b't');
    key.extend_from_slice(&comparable_i64(table_id));
    key
}
/// 编码表 record 前缀：`t{id}_r`。
fn gen_table_record_prefix(table_id: i64) -> Vec<u8> {
    let mut key = encode_table_prefix(table_id);
    key.extend_from_slice(b"_r");
    key
}
/// 编码表 index 前缀：`t{id}_i`。
fn gen_table_index_prefix(table_id: i64) -> Vec<u8> {
    let mut key = encode_table_prefix(table_id);
    key.extend_from_slice(b"_i");
    key
}
/// 编码单索引前缀：`t{id}_i{index_id}`。
fn encode_table_index_prefix(table_id: i64, index_id: i64) -> Vec<u8> {
    let mut key = gen_table_index_prefix(table_id);
    key.extend_from_slice(&comparable_i64(index_id));
    key
}
/// 为每个索引构造 [start, end) RangeDetail。
fn index_ranges(table_id: i64, indices: Vec<IndexInfo>) -> Map<RangeDetail> {
    Map(indices
        .into_iter()
        .map(|index| {
            let start = encode_table_index_prefix(table_id, index.id);
            let end = encode_table_index_prefix(table_id, index.id.wrapping_add(1));
            (index.name, createRangeDetail(start, end))
        })
        .collect())
}
/// 无分区返回单条；有分区则按分区展开。
pub(crate) enum TableRangesResponse {
    Single(TableRanges),
    Multiple(Vec<TableRanges>),
}

pub(crate) fn table_ranges_response(table: &Table) -> TableRangesResponse {
    if table.meta.partitions.is_empty() {
        return TableRangesResponse::Single(createTableRanges(
            table.meta.id,
            table.meta.name.clone(),
            table.meta.indices.clone(),
        ));
    }
    TableRangesResponse::Multiple(
        table
            .meta
            .partitions
            .iter()
            .map(|partition| {
                createTableRanges(
                    partition.id,
                    partition.name.clone(),
                    table.meta.indices.clone(),
                )
            })
            .collect(),
    )
}
/// 可选 URL 解码（+→空格，%XX）。
fn maybe_unescape(value: &str, should_unescape: bool) -> Result<String, Error> {
    if !should_unescape {
        return Ok(value.to_owned());
    }
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => output.push(b' '),
            b'%' if index + 2 < bytes.len() => {
                let high = hex_digit(bytes[index + 1])?;
                let low = hex_digit(bytes[index + 2])?;
                output.push((high << 4) | low);
                index += 2;
            }
            b'%' => return Err(Error::new("incomplete URL escape")),
            byte => output.push(byte),
        }
        index += 1;
    }
    String::from_utf8(output).map_err(|error| Error::new(error.to_string()))
}
/// 解析单个 hex 字符为半字节。
fn hex_digit(byte: u8) -> Result<u8, Error> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(Error::new("invalid URL escape")),
    }
}
/// 编码表 record 范围上界：handle = i64::MAX。
fn encode_row_key_with_max_handle(table_id: i64) -> Vec<u8> {
    let mut key = gen_table_record_prefix(table_id);
    key.extend_from_slice(&comparable_i64(i64::MAX));
    key
}

// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// TikvHandlerTool 是访问 TiKV 数据的 HTTP 工具，对应 Go 的 Helper 嵌入。
// TiKV 数据访问 HTTP 工具：按 handle/索引/十六进制 key 查询 MVCC，并解析表与 Region。
//
// MVCC（多版本并发控制）保留同一 key 的多版本；Region 是 TiKV 的数据分片单位。
// 本模块对应 Go 侧 `TikvHandlerTool`/`Helper`，机械迁移基线中外部依赖以占位类型表示。

/// 访问 TiKV 数据的 HTTP 工具集；内嵌 Helper（对应 Go 的嵌入）。
pub struct TikvHandlerTool {
    pub helper: Helper,
}
/// 用 Storage 构造 TikvHandlerTool。
pub fn new_tikv_handler_tool(store: Storage) -> TikvHandlerTool {
    TikvHandlerTool {
        helper: Helper::new(store),
    }
}

/// Go 风格命名的构造函数别名。
pub fn NewTikvHandlerTool(store: Storage) -> TikvHandlerTool {
    new_tikv_handler_tool(store)
}

#[derive(Clone)]
/// 一条 MVCC（多版本并发控制）键值查询结果，含 Region ID。
pub struct MvccKv {
    pub key: String,
    pub region_id: u64,
    pub value: Option<MvccResponse>,
}
#[allow(non_camel_case_types)]
/// Go 风格类型别名。
pub type mvccKV = MvccKv;

// get_region_id_by_key 通过 RegionCache 定位 key；底层错误转换为 TiDB 错误。
/// 通过 RegionCache 定位 encoded key 所属 Region 的 ID。
pub fn get_region_id_by_key(t: &TikvHandlerTool, encoded_key: &[u8]) -> Result<u64, Error> {
    let location = t.helper.region_cache.locate_key(encoded_key)?;
    Ok(location.region.id)
}

// get_handle 优先读取显式整数 handle；公共 handle 则从 URL 中按主键列构造 datum row。
/// 解析行 handle：整型 handle 或聚簇索引（common handle）主键列构造。
pub fn get_handle(
    t: &TikvHandlerTool,
    tb: &PhysicalTable,
    params: &std::collections::HashMap<String, String>,
    values: &UrlValues,
) -> Result<Handle, Error> {
    if let Some(raw) = params.get(crate::util::HANDLE) {
        if tb.meta().is_common_handle {
            return Err(Error::bad_request(
                "For clustered index tables, please use query strings to specify the column values.",
            ));
        }
        return Ok(Handle::Int(parse_go_base_zero_int(raw)?));
    }
    let info = tb.meta();
    let pk_idx = find_primary_index(&info);
    if pk_idx.is_none() || !info.is_common_handle {
        return Err(Error::bad_request("Clustered common handle not found."));
    }
    let idx = pk_idx.unwrap();
    let pk_cols: Vec<ColumnInfo> = idx
        .columns
        .iter()
        .map(|c| info.cols[c.offset].clone())
        .collect();
    let mut sc = StatementContext::new();
    sc.set_time_zone("UTC");
    let pk_data = form_value_to_datum_row(&sc, values, &pk_cols)?;
    truncate_index_values(&info, &idx, &pk_data);
    let bytes = encode_key(sc.time_zone(), &pk_data)?;
    let handle_bytes = sc.handle_error(bytes)?;
    Handle::common(handle_bytes)
}

// get_mvcc_by_idx_value 先读取正常 index key，再转换为临时 index key 重读一次。
/// 按索引列取值查询 MVCC，并额外查询临时索引键。
pub fn get_mvcc_by_idx_value(
    t: &TikvHandlerTool,
    idx: &Index,
    values: &UrlValues,
    idx_cols: &[ColumnInfo],
    handle: Handle,
) -> Result<Vec<MvccKv>, Error> {
    let sc = StatementContext::with_time_zone("UTC");
    let row = form_value_to_datum_row(&sc, values, idx_cols)?;
    let encoded_key = idx.gen_index_key(&sc, &row, &handle)?;
    let data = t.get_mvcc_by_encoded_key(&encoded_key)?;
    let region_id = get_region_id_by_key(t, &encoded_key)?;
    let normal = MvccKv {
        key: hex_upper(&encoded_key),
        region_id,
        value: Some(data),
    };
    let temporary_key = index_key_to_temp_index_key(&encoded_key);
    let data = t.get_mvcc_by_encoded_key(&temporary_key)?;
    let region_id = get_region_id_by_key(t, &temporary_key)?;
    let temporary = MvccKv {
        key: hex_upper(&temporary_key),
        region_id,
        value: Some(data),
    };
    Ok(vec![normal, temporary])
}

// form_value_to_datum_row 将 query string 转换为列类型 Datum；每个索引列必须恰好一个值。
/// 将 URL query 转为列类型 Datum 行；每列至多一个值。
pub fn form_value_to_datum_row(
    sc: &StatementContext,
    values: &UrlValues,
    idx_cols: &[ColumnInfo],
) -> Result<Vec<Datum>, Error> {
    let mut data = vec![Datum::Null; idx_cols.len()];
    for (i, col) in idx_cols.iter().enumerate() {
        let name = col.name.clone();
        let vals = values.get(&name).ok_or_else(|| {
            Error::bad_request(format!("Missing value for index column {}.", name))
        })?;
        match vals.len() {
            0 => data[i] = Datum::Null,
            1 => data[i] = Datum::from_string(&vals[0]).convert_to(sc, col)?,
            _ => {
                return Err(Error::bad_request(format!(
                    "Invalid query form for column '{}', its values are {:?}. Column value should be unique for one index record.",
                    name, vals
                )));
            }
        }
    }
    Ok(data)
}

/// 同 form_value_to_datum_row，错误经 Helper.trace 包装。
pub fn form_value_2_datum_row(
    t: &TikvHandlerTool,
    sc: &StatementContext,
    values: &UrlValues,
    cols: &[ColumnInfo],
) -> Result<Vec<Datum>, Error> {
    form_value_to_datum_row(sc, values, cols).map_err(|e| t.helper.trace(e))
}
/// 解析库表名并返回物理表 ID。
pub fn get_table_id(t: &TikvHandlerTool, db: &str, table: &str) -> Result<i64, Error> {
    Ok(get_table(t, db, table)?.physical_id())
}

// get_table 先通过 domain schema 找表，再按 table(partition) 语法解析分区名。
/// 按库表名取物理表，支持 `table(partition)` 语法。
pub fn get_table(t: &TikvHandlerTool, db: &str, table: &str) -> Result<PhysicalTable, Error> {
    let schema = t.schema()?;
    let (table_name, partition) = extract_table_and_partition_name(table);
    let table_val = schema.table_by_name(db, &table_name)?;
    get_partition(&table_val, &partition)
}
/// 从分区表解析指定分区；非分区表不得带分区名。
pub fn get_partition(table_val: &Table, partition_name: &str) -> Result<PhysicalTable, Error> {
    if let Some(pt) = table_val.partitioned() {
        if partition_name.is_empty() {
            return Err(Error::new(
                "work on partitioned table, please specify table(partition)",
            ));
        }
        return Ok(pt.get_partition(find_partition_by_name(table_val.meta(), partition_name)?)?);
    }
    if !partition_name.is_empty() {
        return Err(Error::new("not a partitioned table"));
    }
    table_val.as_physical()
}
/// 从 Domain 获取当前 InfoSchema（表结构元数据视图）。
pub fn schema(t: &TikvHandlerTool) -> Result<InfoSchema, Error> {
    Ok(get_domain(&t.helper.store)?.info_schema())
}

// handle_mvcc_get_by_hex 解码 URL 中的十六进制 key，并同时返回区域 ID。
/// 解码十六进制 key 并返回 MVCC 与 Region ID。
pub fn handle_mvcc_get_by_hex(
    t: &TikvHandlerTool,
    params: &std::collections::HashMap<String, String>,
) -> Result<MvccKv, Error> {
    let raw = params
        .get(crate::util::HEX_KEY)
        .cloned()
        .unwrap_or_default();
    let key = hex_decode(&raw)?;
    let data = t.get_mvcc_by_encoded_key(&key)?;
    let region_id = get_region_id_by_key(t, &key)?;
    Ok(MvccKv {
        key: raw.to_uppercase(),
        value: Some(data),
        region_id,
    })
}

#[derive(Clone)]
/// Region 元信息：ID、leader、peers 与 region_epoch。
pub struct RegionMeta {
    pub id: u64,
    pub leader: Option<Peer>,
    pub peers: Vec<Peer>,
    pub region_epoch: Option<RegionEpoch>,
}
// get_regions_meta 逐个向 PD 查询 region；failpoint 可模拟 Meta 为空，必须返回明确错误。
/// 向 PD 批量查询 Region 元信息。
pub fn get_regions_meta(t: &TikvHandlerTool, region_ids: &[u64]) -> Result<Vec<RegionMeta>, Error> {
    let mut regions = Vec::with_capacity(region_ids.len());
    for id in region_ids {
        let region = t.helper.region_cache.pd_client.get_region_by_id(*id)?;
        let meta = region
            .meta
            .ok_or_else(|| Error::new(format!("region not found for regionID {:?}", id)))?;
        regions.push(RegionMeta {
            id: *id,
            leader: region.leader,
            peers: meta.peers,
            region_epoch: meta.region_epoch,
        });
    }
    Ok(regions)
}

// get_mvcc_by_encoded_key 和 extract_table_and_partition_name 对应 Go 中跨文件提供的方法/函数。
impl TikvHandlerTool {
    /// 按编码 key 读 MVCC；基线返回外部依赖错误。
    fn get_mvcc_by_encoded_key(&self, _: &[u8]) -> Result<MvccResponse, Error> {
        Err(Error::new("external TiKV dependency"))
    }
    /// 委托到模块级 schema。
    fn schema(&self) -> Result<InfoSchema, Error> {
        schema(self)
    }

    /// 方法形式的 get_region_id_by_key。
    pub fn GetRegionIDByKey(&self, encoded_key: &[u8]) -> Result<u64, Error> {
        get_region_id_by_key(self, encoded_key)
    }

    /// 方法形式的 get_handle。
    pub fn GetHandle(
        &self,
        table: &PhysicalTable,
        params: &std::collections::HashMap<String, String>,
        values: &UrlValues,
    ) -> Result<Handle, Error> {
        get_handle(self, table, params, values)
    }

    /// 方法形式的 get_mvcc_by_idx_value。
    pub fn GetMvccByIdxValue(
        &self,
        index: &Index,
        values: &UrlValues,
        columns: &[ColumnInfo],
        handle: Handle,
    ) -> Result<Vec<MvccKv>, Error> {
        get_mvcc_by_idx_value(self, index, values, columns, handle)
    }

    /// 方法形式的 form_value_2_datum_row。
    pub fn FormValue2DatumRow(
        &self,
        statement_context: &StatementContext,
        values: &UrlValues,
        columns: &[ColumnInfo],
    ) -> Result<Vec<Datum>, Error> {
        form_value_2_datum_row(self, statement_context, values, columns)
    }

    /// Go 风格 camelCase 别名，直接调用 form_value_to_datum_row。
    pub fn formValue2DatumRow(
        &self,
        statement_context: &StatementContext,
        values: &UrlValues,
        columns: &[ColumnInfo],
    ) -> Result<Vec<Datum>, Error> {
        form_value_to_datum_row(statement_context, values, columns)
    }

    /// 方法形式的 get_table_id。
    pub fn GetTableID(&self, database: &str, table: &str) -> Result<i64, Error> {
        get_table_id(self, database, table)
    }

    /// 方法形式的 get_table。
    pub fn GetTable(&self, database: &str, table: &str) -> Result<PhysicalTable, Error> {
        get_table(self, database, table)
    }

    /// 方法形式的 get_partition。
    pub fn GetPartition(&self, table: &Table, partition: &str) -> Result<PhysicalTable, Error> {
        get_partition(table, partition)
    }

    /// 方法形式的 schema。
    pub fn Schema(&self) -> Result<InfoSchema, Error> {
        schema(self)
    }

    /// 方法形式的 handle_mvcc_get_by_hex。
    pub fn HandleMvccGetByHex(
        &self,
        params: &std::collections::HashMap<String, String>,
    ) -> Result<MvccKv, Error> {
        handle_mvcc_get_by_hex(self, params)
    }

    /// 方法形式的 get_regions_meta。
    pub fn GetRegionsMeta(&self, region_ids: &[u64]) -> Result<Vec<RegionMeta>, Error> {
        get_regions_meta(self, region_ids)
    }
}

#[derive(Clone)]
/// TiKV 访问辅助：持有 Storage 与 RegionCache。
pub struct Helper {
    pub store: Storage,
    region_cache: RegionCache,
}
impl Helper {
    /// 构造 Helper 并初始化空 RegionCache。
    fn new(store: Storage) -> Helper {
        Helper {
            store,
            region_cache: RegionCache::new(),
        }
    }
    /// 错误追踪包装（基线原样返回）。
    fn trace(&self, e: Error) -> Error {
        e
    }
}
#[derive(Clone)]
/// KV 存储句柄占位类型（机械迁移基线）。
pub struct Storage;
#[derive(Clone)]
/// Region 路由缓存，内部依赖 PD Client。
pub struct RegionCache {
    pd_client: PdClient,
}
impl RegionCache {
    /// 构造空 RegionCache。
    fn new() -> RegionCache {
        RegionCache {
            pd_client: PdClient,
        }
    }
    /// 定位 key 所属 Region（外部依赖占位）。
    fn locate_key(&self, _: &[u8]) -> Result<Location, Error> {
        Err(Error::new("external region cache dependency"))
    }
}
#[derive(Clone)]
/// Placement Driver 客户端占位类型。
pub struct PdClient;
impl PdClient {
    /// 按 ID 向 PD 查 Region（外部依赖占位）。
    fn get_region_by_id(&self, _: u64) -> Result<Region, Error> {
        Err(Error::new("external PD dependency"))
    }
}
/// key 定位结果，含所属 Region 引用。
pub struct Location {
    region: RegionRef,
}
/// Region 轻量引用（仅 ID）。
pub struct RegionRef {
    id: u64,
}
/// PD 返回的 Region：meta 与 leader。
pub struct Region {
    meta: Option<RegionInfo>,
    leader: Option<Peer>,
}
/// Region 元数据正文：peers 与 epoch。
pub struct RegionInfo {
    peers: Vec<Peer>,
    region_epoch: Option<RegionEpoch>,
}
#[derive(Clone)]
/// Raft peer 占位类型。
pub struct Peer;
#[derive(Clone)]
/// Region 版本纪元占位类型。
pub struct RegionEpoch;
#[derive(Clone)]
/// MVCC 查询响应占位类型。
pub struct MvccResponse;
#[derive(Clone)]
/// 物理表（可对应分区）占位类型。
pub struct PhysicalTable;
/// 逻辑表，可含分区定义。
pub struct Table {
    info: TableInfo,
}
#[derive(Clone)]
/// 表元信息：是否 common handle、列定义等。
pub struct TableInfo {
    is_common_handle: bool,
    cols: Vec<ColumnInfo>,
}
#[derive(Clone)]
/// 列元信息。
pub struct ColumnInfo {
    name: String,
}
/// 索引定义，含列偏移。
pub struct Index {
    columns: Vec<IndexColumn>,
}
/// 索引中的一列及其在表中的 offset。
pub struct IndexColumn {
    offset: usize,
}
/// 分区表句柄占位类型。
pub struct PartitionedTable;
/// 信息系统：当前库表结构快照。
pub struct InfoSchema;
/// 语句执行上下文（时区、错误处理等）。
pub struct StatementContext;
/// HTTP query 多值映射。
pub struct UrlValues;
#[derive(Clone)]
/// TiDB Datum：类型化单元格值。
pub enum Datum {
    Null,
    String(String),
}
/// 行句柄：整型或 common handle 字节。
pub enum Handle {
    Int(i64),
    Common(Vec<u8>),
}
/// 本模块错误类型；保留 Go handler 的错误文案与 BadRequest 分类。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Error {
    message: String,
    bad_request: bool,
}
impl PhysicalTable {
    fn meta(&self) -> TableInfo {
        TableInfo {
            is_common_handle: false,
            cols: vec![],
        }
    }
    fn physical_id(&self) -> i64 {
        0
    }
}
impl TableInfo {
    fn cols(&self) -> &[ColumnInfo] {
        &self.cols
    }
}
impl Table {
    fn meta(&self) -> &TableInfo {
        &self.info
    }
    fn partitioned(&self) -> Option<PartitionedTable> {
        None
    }
    fn as_physical(&self) -> Result<PhysicalTable, Error> {
        Err(Error::new("physical table dependency"))
    }
}
impl PartitionedTable {
    /// 从分区表解析指定分区；非分区表不得带分区名。
    fn get_partition(&self, _: String) -> Result<PhysicalTable, Error> {
        Err(Error::new("partition dependency"))
    }
}
impl StatementContext {
    /// 构造 Helper 并初始化空 RegionCache。
    fn new() -> StatementContext {
        StatementContext
    }
    fn with_time_zone(_: &str) -> StatementContext {
        StatementContext
    }
    fn set_time_zone(&mut self, _: &str) {}
    fn time_zone(&self) -> &str {
        "UTC"
    }
    fn handle_error<T>(&self, v: Result<T, Error>) -> Result<T, Error> {
        v
    }
}
impl Datum {
    fn from_string(s: &str) -> Datum {
        Datum::String(s.into())
    }
    fn convert_to(self, _: &StatementContext, _: &ColumnInfo) -> Result<Datum, Error> {
        Ok(self)
    }
}
impl InfoSchema {
    fn table_by_name(&self, _: &str, _: &str) -> Result<Table, Error> {
        Err(Error::new("schema dependency"))
    }
}
impl UrlValues {
    fn get(&self, _: &String) -> Option<Vec<String>> {
        None
    }
}
impl Handle {
    fn common(v: Vec<u8>) -> Result<Handle, Error> {
        Ok(Handle::Common(v))
    }
}
impl Error {
    fn new<T: Into<String>>(message: T) -> Error {
        Error {
            message: message.into(),
            bad_request: false,
        }
    }
    fn bad_request<T: Into<String>>(message: T) -> Error {
        Error {
            message: message.into(),
            bad_request: true,
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn is_bad_request(&self) -> bool {
        self.bad_request
    }
}
impl From<std::num::ParseIntError> for Error {
    fn from(error: std::num::ParseIntError) -> Error {
        Error::new(error.to_string())
    }
}
impl Index {
    fn gen_index_key(
        &self,
        _: &StatementContext,
        _: &[Datum],
        _: &Handle,
    ) -> Result<Vec<u8>, Error> {
        Err(Error::new("index key dependency"))
    }
}
/// 查找主键索引定义。
fn find_primary_index(_: &TableInfo) -> Option<Index> {
    None
}
/// 按索引定义截断 Datum（对齐 Go 语义）。
fn truncate_index_values(_: &TableInfo, _: &Index, _: &[Datum]) {}
/// 按时区编码主键/索引 key。
fn encode_key(_: &str, _: &[Datum]) -> Result<Result<Vec<u8>, Error>, Error> {
    Ok(Ok(vec![]))
}
/// 将正式索引键转换为临时索引键。
pub(crate) fn index_key_to_temp_index_key(key: &[u8]) -> Vec<u8> {
    const PREFIX_LEN: usize = 11; // `t` + table ID (8) + `_i` (2)
    const TEMP_INDEX_PREFIX: i64 = 0x7fff_0000_0000_0000;
    let mut result = key.to_vec();
    if result.len() >= PREFIX_LEN + 8 {
        let encoded = u64::from_be_bytes(
            result[PREFIX_LEN..PREFIX_LEN + 8]
                .try_into()
                .expect("checked index ID width"),
        );
        let index_id = (encoded ^ 0x8000_0000_0000_0000) as i64;
        let temporary_id = TEMP_INDEX_PREFIX | index_id;
        let encoded_temporary = (temporary_id as u64 ^ 0x8000_0000_0000_0000).to_be_bytes();
        result[PREFIX_LEN..PREFIX_LEN + 8].copy_from_slice(&encoded_temporary);
    }
    result
}

/// Parse an integer with Go strconv.ParseInt's base-0 prefix rules.
fn parse_go_base_zero_int(raw: &str) -> Result<i64, Error> {
    if raw.is_empty() {
        return Err(Error::new("invalid integer handle"));
    }
    let (negative, unsigned) = match raw.as_bytes()[0] {
        b'-' => (true, &raw[1..]),
        b'+' => (false, &raw[1..]),
        _ => (false, raw),
    };
    if unsigned.is_empty() {
        return Err(Error::new("invalid integer handle"));
    }
    let (base, digits) =
        if unsigned.len() > 2 && (unsigned.starts_with("0x") || unsigned.starts_with("0X")) {
            (16, &unsigned[2..])
        } else if unsigned.len() > 2 && (unsigned.starts_with("0b") || unsigned.starts_with("0B")) {
            (2, &unsigned[2..])
        } else if unsigned.len() > 2 && (unsigned.starts_with("0o") || unsigned.starts_with("0O")) {
            (8, &unsigned[2..])
        } else if unsigned.len() > 1 && unsigned.starts_with('0') {
            (8, &unsigned[1..])
        } else {
            (10, unsigned)
        };
    if digits.is_empty() {
        return Err(Error::new("invalid integer handle"));
    }
    let magnitude =
        u64::from_str_radix(digits, base).map_err(|_| Error::new("invalid integer handle"))?;
    if negative {
        if magnitude == 1u64 << 63 {
            Ok(i64::MIN)
        } else {
            i64::try_from(magnitude)
                .ok()
                .and_then(|value| value.checked_neg())
                .ok_or_else(|| Error::new("invalid integer handle"))
        }
    } else {
        i64::try_from(magnitude).map_err(|_| Error::new("invalid integer handle"))
    }
}
/// 字节序列转为大写十六进制字符串。
fn hex_upper(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02X}")).collect()
}
/// 解析十六进制字符串为字节。
fn hex_decode(value: &str) -> Result<Vec<u8>, Error> {
    if !value.len().is_multiple_of(2) {
        return Err(Error::new("invalid hex key"));
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            std::str::from_utf8(pair)
                .ok()
                .and_then(|text| u8::from_str_radix(text, 16).ok())
                .ok_or_else(|| Error::new("invalid hex key"))
        })
        .collect()
}
/// 解析 `name(partition)` 形式的表名。
fn extract_table_and_partition_name(s: &str) -> (String, String) {
    if let (Some(start), Some(end)) = (s.find('('), s.find(')')) {
        (s[..start].to_owned(), s[start + 1..end].to_owned())
    } else {
        (s.into(), String::new())
    }
}
/// 按名查找分区（元数据依赖占位）。
fn find_partition_by_name(_: &TableInfo, _: &str) -> Result<String, Error> {
    Err(Error::new("partition metadata dependency"))
}
/// 从 Storage 取 Domain（外部依赖占位）。
fn get_domain(_: &Storage) -> Result<Domain, Error> {
    Err(Error::new("domain dependency"))
}
/// 服务域：持有 InfoSchema 等。
pub struct Domain;
impl Domain {
    fn info_schema(&self) -> InfoSchema {
        InfoSchema
    }
}

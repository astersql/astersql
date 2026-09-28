// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// 索引/表数据不一致诊断报告器。
//
// 对应 Go `pkg/util/logutil/consistency`：在 index lookup 或 admin check
// 发现行与索引不一致时，从存储拉取 MVCC（多版本并发控制）信息，
// 按脱敏策略写入结构化错误日志，并返回带错误码的 `ConsistencyError`。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::Arc;

pub use model;
use serde::{Deserialize, Serialize};
use serde_json::Value;
pub use tablecodec::{kv, types};

/// MVCC 解码回调写入的输出映射（键 → JSON 值）。
pub type MvccOutMap = BTreeMap<String, Value>;
/// 按 StartTs 组织的已解码列名 → 字符串值。
type DecodedMvccData = BTreeMap<String, BTreeMap<String, String>>;

fn isZeroI32(value: &i32) -> bool {
    *value == 0
}

fn isZeroU64(value: &u64) -> bool {
    *value == 0
}

fn isFalse(value: &bool) -> bool {
    !*value
}

/// Go `encoding/json` marshals `[]byte` as standard padded Base64 rather than
/// a JSON array. Keep the codec local so this crate does not need a new
/// dependency or a Cargo.lock change.
mod goBytes {
    use serde::{Deserialize, Deserializer, Serializer};

    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    pub fn serialize<S>(value: &Vec<u8>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&encode(value))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let encoded = String::deserialize(deserializer)?;
        decode(&encoded).map_err(serde::de::Error::custom)
    }

    fn encode(input: &[u8]) -> String {
        let mut output = String::with_capacity(input.len().div_ceil(3) * 4);
        for chunk in input.chunks(3) {
            let first = chunk[0];
            let second = chunk.get(1).copied().unwrap_or(0);
            let third = chunk.get(2).copied().unwrap_or(0);
            output.push(ALPHABET[(first >> 2) as usize] as char);
            output.push(ALPHABET[(((first & 0x03) << 4) | (second >> 4)) as usize] as char);
            if chunk.len() > 1 {
                output.push(ALPHABET[(((second & 0x0f) << 2) | (third >> 6)) as usize] as char);
            } else {
                output.push('=');
            }
            if chunk.len() > 2 {
                output.push(ALPHABET[(third & 0x3f) as usize] as char);
            } else {
                output.push('=');
            }
        }
        output
    }

    fn decode(input: &str) -> Result<Vec<u8>, String> {
        // Go's encoding/base64 decoder ignores CR and LF in otherwise valid input.
        let bytes = input
            .bytes()
            .filter(|byte| *byte != b'\r' && *byte != b'\n')
            .collect::<Vec<_>>();
        if !bytes.len().is_multiple_of(4) {
            return Err("invalid base64 length".to_string());
        }
        let mut output = Vec::with_capacity(bytes.len() / 4 * 3);
        let chunks = bytes.chunks_exact(4);
        let chunk_count = chunks.len();
        for (index, chunk) in chunks.enumerate() {
            let is_last = index + 1 == chunk_count;
            let first = sextet(chunk[0]).ok_or_else(|| "invalid base64 character".to_string())?;
            let second = sextet(chunk[1]).ok_or_else(|| "invalid base64 character".to_string())?;
            output.push((first << 2) | (second >> 4));

            if chunk[2] == b'=' {
                if !is_last || chunk[3] != b'=' || second & 0x0f != 0 {
                    return Err("invalid base64 padding".to_string());
                }
                continue;
            }
            let third = sextet(chunk[2]).ok_or_else(|| "invalid base64 character".to_string())?;
            output.push((second << 4) | (third >> 2));

            if chunk[3] == b'=' {
                if !is_last || third & 0x03 != 0 {
                    return Err("invalid base64 padding".to_string());
                }
                continue;
            }
            let fourth = sextet(chunk[3]).ok_or_else(|| "invalid base64 character".to_string())?;
            output.push((third << 6) | fourth);
        }
        Ok(output)
    }

    fn sextet(byte: u8) -> Option<u8> {
        match byte {
            b'A'..=b'Z' => Some(byte - b'A'),
            b'a'..=b'z' => Some(byte - b'a' + 26),
            b'0'..=b'9' => Some(byte - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
}

/// The MVCC write fields used by the reporter. Values are retained for decoded
/// diagnostics and removed from the separate metadata log fields, like Go.
/// 报告器用的 MVCC write 字段；完整 short_value 留给解码诊断，
/// 单独的元数据日志字段会清空 short_value（对齐 Go）。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct MvccWrite {
    #[serde(rename = "type", skip_serializing_if = "isZeroI32")]
    /// 写入类型（Put/Delete 等，数值与 TiKV 对齐）。
    pub Type: i32,
    #[serde(rename = "start_ts", skip_serializing_if = "isZeroU64")]
    /// 事务开始时间戳（start_ts）。
    pub StartTs: u64,
    #[serde(rename = "commit_ts", skip_serializing_if = "isZeroU64")]
    /// 事务提交时间戳（commit_ts）。
    pub CommitTs: u64,
    #[serde(
        rename = "short_value",
        with = "goBytes",
        skip_serializing_if = "Vec::is_empty"
    )]
    /// 写入记录中内联的短值（可能为空，大值在 Values 中）。
    pub ShortValue: Vec<u8>,
    #[serde(rename = "has_overlapped_rollback", skip_serializing_if = "isFalse")]
    /// 当前 write 是否与 rollback 记录重叠。
    pub HasOverlappedRollback: bool,
    #[serde(rename = "has_gc_fence", skip_serializing_if = "isFalse")]
    /// 是否携带 GC fence。
    pub HasGcFence: bool,
    #[serde(rename = "gc_fence", skip_serializing_if = "isZeroU64")]
    /// GC fence 时间戳。
    pub GcFence: u64,
    #[serde(rename = "last_change_ts", skip_serializing_if = "isZeroU64")]
    /// 最近一次值变化的时间戳。
    pub LastChangeTs: u64,
    #[serde(rename = "versions_to_last_change", skip_serializing_if = "isZeroU64")]
    /// 距最近一次值变化的版本数。
    pub VersionsToLastChange: u64,
}

/// MVCC value 版本：某 start_ts 下的完整 value 载荷。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct MvccValue {
    #[serde(rename = "start_ts", skip_serializing_if = "isZeroU64")]
    /// 产生该 value 的事务 start_ts。
    pub StartTs: u64,
    #[serde(
        rename = "value",
        with = "goBytes",
        skip_serializing_if = "Vec::is_empty"
    )]
    /// 原始 value 字节。
    pub Value: Vec<u8>,
}

/// 单键的 MVCC 诊断快照：writes、values 与可选 lock。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct MvccInfo {
    #[serde(rename = "lock", skip_serializing_if = "Option::is_none")]
    /// Lock is deliberately opaque here. The reporter serializes it but never
    /// interprets it; storage adapters can preserve their diagnostic payload.
    /// Lock 在此保持不透明：只序列化不解释，便于存储适配器保留原诊断载荷。
    pub Lock: Option<Value>,
    #[serde(rename = "writes", skip_serializing_if = "Vec::is_empty")]
    /// write CF 上的版本列表。
    pub Writes: Vec<MvccWrite>,
    #[serde(rename = "values", skip_serializing_if = "Vec::is_empty")]
    /// default CF 上的版本列表。
    pub Values: Vec<MvccValue>,
}

/// 按编码键查询 MVCC 的存储响应。
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct MvccGetByKeyResponse {
    #[serde(rename = "region_error", skip_serializing_if = "Option::is_none")]
    /// Region 级错误信息（如 epoch not match）。
    pub RegionError: Option<Value>,
    #[serde(rename = "error", skip_serializing_if = "String::is_empty")]
    /// 其它键级错误描述。
    pub Error: String,
    #[serde(rename = "info", skip_serializing_if = "Option::is_none")]
    /// MVCC 详情；查询失败或空键时可能为 None。
    pub Info: Option<MvccInfo>,
}

/// Minimal storage boundary used by the consistency reporter. The historical
/// helper package does not expose MVCC writes/values, so the adapter keeps the
/// full diagnostic response at this package boundary instead of dropping it.
/// 一致性报告器使用的最小存储边界；在此保留完整诊断响应而不丢弃字段。
pub trait Storage: Send + Sync {
    /// 按编码后的键查询 MVCC。
    fn get_mvcc_by_encoded_key(&self, key: &kv::Key) -> Result<MvccGetByKeyResponse, String>;
    /// 解析键所属 Region ID（失败时上层可回退为 0）。
    fn region_id_by_key(&self, key: &kv::Key) -> Result<u64, String>;
}

/// 将原始 MVCC 响应解码并写入 `MvccOutMap` 的回调类型。
pub type DecodeMvccFn<'a> = dyn Fn(&kv::Key, &MvccGetByKeyResponse, &mut MvccOutMap) + 'a;

// GetMVCCByKeyResp gets the MVCC response.
/// 从存储直接取回指定键的 MVCC 响应。
pub fn GetMVCCByKeyResp(
    tikvStore: &dyn Storage,
    key: kv::Key,
) -> Result<MvccGetByKeyResponse, String> {
    tikvStore.get_mvcc_by_encoded_key(&key)
}

// GetMvccByKey gets the MVCC value by key and returns JSON including decoded data.
/// 按键组装含 key/regionID/mvcc/decoded 的 JSON 字符串；失败或 nil 键返回空串。
pub fn GetMvccByKey(
    tikvStore: &dyn Storage,
    key: Option<kv::Key>,
    decodeMvccFn: Option<&DecodeMvccFn<'_>>,
) -> String {
    let Some(key) = key else {
        return String::new();
    };
    let Ok(mvccResp) = GetMVCCByKeyResp(tikvStore, key.clone()) else {
        return String::new();
    };

    let mut resp = MvccOutMap::new();
    resp.insert(
        "key".to_string(),
        Value::String(hex::encode(key.as_ref()).to_uppercase()),
    );
    resp.insert(
        "regionID".to_string(),
        Value::from(getRegionIDByKey(tikvStore, &key)),
    );
    let Ok(mvcc) = serde_json::to_value(&mvccResp) else {
        return String::new();
    };
    resp.insert("mvcc".to_string(), mvcc);

    if let Some(decode) = decodeMvccFn {
        decode(&key, &mvccResp, &mut resp);
    }
    let Ok(mut output) = toGoJSON(&resp) else {
        return String::new();
    };

    // 限制诊断 JSON 长度，避免撑爆日志；在字符边界截断后追加标记
    const maxMvccInfoLen: usize = 5000;
    if output.len() > maxMvccInfoLen {
        let mut boundary = maxMvccInfoLen;
        while !output.is_char_boundary(boundary) {
            boundary -= 1;
        }
        output.truncate(boundary);
        output.push_str("[truncated]...");
    }
    output
}

/// 查询键所在 Region ID；失败时返回 0（对齐 Go 非致命处理）。
fn getRegionIDByKey(tikvStore: &dyn Storage, encodedKey: &kv::Key) -> u64 {
    tikvStore.region_id_by_key(encodedKey).unwrap_or(0)
}

// DecodeRowMvccData captures table metadata for use by GetMvccByKey.
/// 捕获表元数据，返回用于 `GetMvccByKey` 的行 MVCC 解码闭包。
pub fn DecodeRowMvccData(
    tableInfo: &model::TableInfo,
) -> impl Fn(&kv::Key, &MvccGetByKeyResponse, &mut MvccOutMap) + '_ {
    move |_key, respValue, outMap| {
        let colMap: HashMap<i64, Box<types::FieldType>> = tableInfo
            .Columns
            .iter()
            .map(|column| (column.ID, Box::new(column.FieldType.clone())))
            .collect();
        let Some(info) = &respValue.Info else {
            return;
        };

        let mut err = None;
        let mut datas = DecodedMvccData::new();
        // 优先从 write 的 short_value 解码行记录
        for write in &info.Writes {
            if !write.ShortValue.is_empty() {
                let (record, next_err) =
                    decodeMvccRecordValue(&write.ShortValue, &colMap, tableInfo);
                datas.insert(write.StartTs.to_string(), record);
                err = next_err;
            }
        }
        // 再处理 default CF 中的完整 value
        for value in &info.Values {
            if !value.Value.is_empty() {
                let (record, next_err) = decodeMvccRecordValue(&value.Value, &colMap, tableInfo);
                datas.insert(value.StartTs.to_string(), record);
                err = next_err;
            }
        }
        insertDecodedData(outMap, datas, err);
    }
}

// DecodeIndexMvccData captures index metadata for use by GetMvccByKey.
/// 捕获索引元数据，返回用于 `GetMvccByKey` 的索引 handle 解码闭包。
pub fn DecodeIndexMvccData(
    indexInfo: &model::IndexInfo,
) -> impl Fn(&kv::Key, &MvccGetByKeyResponse, &mut MvccOutMap) + '_ {
    move |key, respValue, outMap| {
        let Some(info) = &respValue.Info else {
            return;
        };
        let mut err = None;
        let mut datas = DecodedMvccData::new();
        for write in &info.Writes {
            if !write.ShortValue.is_empty() {
                match decodeIndexHandle(key, &write.ShortValue, indexInfo.Columns.len()) {
                    Ok(handle) => {
                        datas.insert(
                            write.StartTs.to_string(),
                            BTreeMap::from([("handle".to_string(), handle.String())]),
                        );
                        err = None;
                    }
                    Err(next_err) => err = Some(next_err.to_string()),
                }
            }
        }
        for value in &info.Values {
            if !value.Value.is_empty() {
                match decodeIndexHandle(key, &value.Value, indexInfo.Columns.len()) {
                    Ok(handle) => {
                        datas.insert(
                            value.StartTs.to_string(),
                            BTreeMap::from([("handle".to_string(), handle.String())]),
                        );
                        err = None;
                    }
                    Err(next_err) => err = Some(next_err.to_string()),
                }
            }
        }
        insertDecodedData(outMap, datas, err);
    }
}

/// 从索引键/值解码出行 handle（主键或行标识）。
fn decodeIndexHandle(
    key: &kv::Key,
    value: &[u8],
    column_count: usize,
) -> Result<Box<dyn kv::Handle>, String> {
    // A TiDB index key starts with `t` + table ID + `_i` + index ID. The
    // migrated tablecodec implementation slices this prefix before checking
    // length, while the Go codec reports malformed input as an error.
    // TiDB 索引键前缀：`t` + table ID + `_i` + index ID（共 19 字节）。
    const indexPrefixLen: usize = 19;
    if key.0.len() < indexPrefixLen {
        return Err("invalid index key: shorter than table/index prefix".to_string());
    }
    tablecodec::DecodeIndexHandle(key.0.clone(), value.to_vec(), column_count)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "index value does not contain a handle".to_string())
}

/// 将解码结果写入 outMap 的 `decoded`，可选附带 `decode_error`。
fn insertDecodedData(outMap: &mut MvccOutMap, datas: DecodedMvccData, err: Option<String>) {
    if datas.is_empty() {
        return;
    }
    outMap.insert(
        "decoded".to_string(),
        serde_json::to_value(datas).expect("string maps are serializable"),
    );
    if let Some(err) = err {
        outMap.insert("decode_error".to_string(), Value::String(err));
    }
}

/// 将行编码字节解码为列名 → 显示字符串，并汇总解码错误。
fn decodeMvccRecordValue(
    bs: &[u8],
    colMap: &HashMap<i64, Box<types::FieldType>>,
    table: &model::TableInfo,
) -> (BTreeMap<String, String>, Option<String>) {
    let (row, mut err) = match tablecodec::DecodeRowToDatumMap(
        Some(bs.to_vec()),
        colMap.clone(),
        Some(tablecodec::time::UTC),
    ) {
        Ok(row) => (row, None),
        Err(error) => (HashMap::new(), Some(error.to_string())),
    };
    let mut record = BTreeMap::new();
    for column in &table.Columns {
        let Some(datum) = row.get(&column.ID) else {
            continue;
        };
        let mut data = "nil".to_string();
        if !datum.IsNull() {
            match datum.ToString() {
                Ok(value) => {
                    data = value;
                    err = None;
                }
                Err(error) => err = Some(error.to_string()),
            }
        }
        record.insert(column.Name.O.clone(), data);
    }
    (record, err)
}

/// 结构化日志的单个键值字段。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LogField {
    /// 字段名。
    pub Key: String,
    /// 字段值（已按脱敏策略处理）。
    pub Value: String,
}

impl LogField {
    /// 构造日志字段。
    fn new(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            Key: key.into(),
            Value: value.into(),
        }
    }
}

/// 一条错误级诊断日志：消息 + 字段列表。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LogEntry {
    /// 人类可读消息。
    pub Message: String,
    /// 附加结构化字段。
    pub Fields: Vec<LogField>,
}

/// 日志下沉接口，便于测试注入观测实现。
pub trait LogSink: Send + Sync {
    /// 写入一条错误日志。
    fn error(&self, entry: LogEntry);
}

/// 默认实现：转发到 `log::error!`。
#[derive(Default)]
pub struct StandardLogSink;

impl LogSink for StandardLogSink {
    fn error(&self, entry: LogEntry) {
        log::error!("{} {:?}", entry.Message, entry.Fields);
    }
}

/// 一致性错误分类，对应不同 TiDB 错误码。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConsistencyErrorKind {
    /// Admin check 发现索引行与表行不一致。
    AdminCheck,
    /// Index lookup 统计的索引行数与表行数不匹配。
    LookupMismatchCount,
    /// Admin check 附带具体列值差异信息。
    AdminCheckWithColumnInfo,
}

/// 对外返回的一致性错误：种类、错误码与格式化参数。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConsistencyError {
    /// 错误种类。
    pub Kind: ConsistencyErrorKind,
    /// TiDB 风格数字错误码。
    pub Code: u16,
    /// 用于消息模板的参数列表。
    pub Args: Vec<String>,
    /// 生成错误时的脱敏模式。
    pub RedactMode: String,
}

impl fmt::Display for ConsistencyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let arg = |index: usize| self.Args.get(index).map(String::as_str).unwrap_or("");
        match self.Kind {
            ConsistencyErrorKind::AdminCheck => write!(
                formatter,
                "[admin:{}]data inconsistency in table: {}, index: {}, handle: {}, index-values:{} != record-values:{}",
                self.Code,
                arg(0),
                arg(1),
                redactErrorArg(&self.RedactMode, arg(2), false),
                redactErrorArg(&self.RedactMode, arg(3), true),
                redactErrorArg(&self.RedactMode, arg(4), true),
            ),
            ConsistencyErrorKind::LookupMismatchCount => write!(
                formatter,
                "[executor:{}]data inconsistency in table: {}, index: {}, index-count:{} != record-count:{}",
                self.Code,
                arg(0),
                arg(1),
                arg(2),
                arg(3),
            ),
            ConsistencyErrorKind::AdminCheckWithColumnInfo => write!(
                formatter,
                "[executor:{}]data inconsistency in table: {}, index: {}, col: {}, handle: {}, index-values:{} != record-values:{}, compare err:{}",
                self.Code,
                arg(0),
                arg(1),
                arg(2),
                redactErrorArg(&self.RedactMode, arg(3), true),
                redactErrorArg(&self.RedactMode, arg(4), true),
                redactErrorArg(&self.RedactMode, arg(5), true),
                redactErrorArg(&self.RedactMode, arg(6), true),
            ),
        }
    }
}

impl std::error::Error for ConsistencyError {}

/// Go 别名：AdminCheck 不一致。
pub const ErrAdminCheckInconsistent: ConsistencyErrorKind = ConsistencyErrorKind::AdminCheck;
/// Go 别名：Lookup 计数不一致。
pub const ErrLookupInconsistent: ConsistencyErrorKind = ConsistencyErrorKind::LookupMismatchCount;
/// Go 别名：带列信息的 AdminCheck 不一致。
pub const ErrAdminCheckInconsistentWithColInfo: ConsistencyErrorKind =
    ConsistencyErrorKind::AdminCheckWithColumnInfo;

// RecordData is the record data composed of a handle and values.
/// 由 handle 与列值组成的一行记录快照，用于诊断日志。
pub struct RecordData {
    /// 行标识（整数或公共 handle）。
    pub Handle: Box<dyn kv::Handle>,
    /// 列 Datum 列表。
    pub Values: Vec<types::Datum>,
}

impl RecordData {
    /// 构造记录数据。
    pub fn new(Handle: Box<dyn kv::Handle>, Values: Vec<types::Datum>) -> Self {
        Self { Handle, Values }
    }

    /// 格式化为 `handle: ..., values: [...]` 文本。
    pub fn String(&self) -> String {
        let values = self
            .Values
            .iter()
            .map(types::Datum::String)
            .collect::<Vec<_>>()
            .join(" ");
        format!("handle: {}, values: [{}]", self.Handle.String(), values)
    }
}

impl Clone for RecordData {
    fn clone(&self) -> Self {
        Self {
            Handle: self.Handle.Copy(),
            Values: self.Values.clone(),
        }
    }
}

impl fmt::Display for RecordData {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.String())
    }
}

/// 将行 handle 编码为表记录键。
type HandleEncoder = dyn Fn(&dyn kv::Handle) -> kv::Key + Send + Sync;
/// 将索引行编码为索引查找键。
type IndexEncoder = dyn Fn(&RecordData) -> kv::Key + Send + Sync;

// Reporter is a helper to generate a report.
/// 一致性诊断报告器：汇总表/索引元数据、存储与日志下沉。
pub struct Reporter {
    /// 行 handle → 表记录键编码器。
    pub HandleEncode: Arc<HandleEncoder>,
    /// 索引行 → 索引键编码器。
    pub IndexEncode: Arc<IndexEncoder>,
    /// 表元信息。
    pub Tbl: model::TableInfo,
    /// 索引元信息。
    pub Idx: model::IndexInfo,
    /// 日志脱敏模式：`ON` / `OFF` / `MARKER` 等。
    pub EnableRedactLog: String,
    /// 可选存储适配器；缺失时跳过 MVCC 查询。
    pub Storage: Option<Arc<dyn Storage>>,
    /// 日志下沉。
    pub Logger: Arc<dyn LogSink>,
}

impl Reporter {
    #[allow(clippy::too_many_arguments)]
    /// 组装报告器实例。
    pub fn new(
        HandleEncode: Arc<HandleEncoder>,
        IndexEncode: Arc<IndexEncoder>,
        Tbl: model::TableInfo,
        Idx: model::IndexInfo,
        EnableRedactLog: String,
        Storage: Option<Arc<dyn Storage>>,
        Logger: Arc<dyn LogSink>,
    ) -> Self {
        Self {
            HandleEncode,
            IndexEncode,
            Tbl,
            Idx,
            EnableRedactLog,
            Storage,
            Logger,
        }
    }

    // ReportLookupInconsistent reports when index rows outnumber record rows.
    /// 索引回查发现索引行数与表行数不一致时记录日志并返回错误码 8133。
    pub fn ReportLookupInconsistent(
        &self,
        idxCnt: i32,
        tblCnt: i32,
        missHd: &[Box<dyn kv::Handle>],
        fullHd: &[Box<dyn kv::Handle>],
        missRowIdx: &[RecordData],
    ) -> ConsistencyError {
        // 全量 handle 列表过长时仅展示前 50 个，避免日志膨胀
        const maxFullHandleCnt: usize = 50;
        let displayFullHdCnt = fullHd.len().min(maxFullHandleCnt);
        let mut fields = vec![
            LogField::new("table_name", self.Tbl.Name.O.clone()),
            LogField::new("index_name", self.Idx.Name.O.clone()),
            LogField::new("index_cnt", idxCnt.to_string()),
            LogField::new("table_cnt", tblCnt.to_string()),
            LogField::new(
                "missing_handles",
                redactString(&self.EnableRedactLog, &formatHandles(missHd)),
            ),
            LogField::new(
                "total_handles",
                redactString(
                    &self.EnableRedactLog,
                    &formatHandles(&fullHd[..displayFullHdCnt]),
                ),
            ),
        ];

        // 未开启 ON 脱敏时，补充缺失行/索引的 MVCC JSON
        if self.EnableRedactLog != "ON"
            && let Some(store) = &self.Storage
        {
            for (index, handle) in missHd.iter().enumerate() {
                fields.push(LogField::new(
                    format!("row_mvcc_{index}"),
                    redactString(
                        &self.EnableRedactLog,
                        &GetMvccByKey(
                            store.as_ref(),
                            Some((self.HandleEncode)(handle.as_ref())),
                            Some(&DecodeRowMvccData(&self.Tbl)),
                        ),
                    ),
                ));
            }
            for (index, row) in missRowIdx.iter().enumerate() {
                fields.push(LogField::new(
                    format!("index_mvcc_{index}"),
                    redactString(
                        &self.EnableRedactLog,
                        &GetMvccByKey(
                            store.as_ref(),
                            Some((self.IndexEncode)(row)),
                            Some(&DecodeIndexMvccData(&self.Idx)),
                        ),
                    ),
                ));
            }
        }
        addStack(&mut fields);
        self.log("indexLookup found data inconsistency", fields);
        ConsistencyError {
            Kind: ConsistencyErrorKind::LookupMismatchCount,
            Code: 8133,
            Args: vec![
                self.Tbl.Name.O.clone(),
                self.Idx.Name.O.clone(),
                idxCnt.to_string(),
                tblCnt.to_string(),
            ],
            RedactMode: self.EnableRedactLog.clone(),
        }
    }

    // ReportAdminCheckInconsistentWithColInfo reports a mismatched column value.
    /// Admin check 发现列值不一致时记录日志并返回错误码 8134。
    pub fn ReportAdminCheckInconsistentWithColInfo<I, T, E>(
        &self,
        handle: &dyn kv::Handle,
        colName: &str,
        idxDat: &I,
        tblDat: &T,
        err: &E,
        idxRow: &RecordData,
    ) -> ConsistencyError
    where
        I: fmt::Display + ?Sized,
        T: fmt::Display + ?Sized,
        E: fmt::Display + ?Sized,
    {
        let idx_value = idxDat.to_string();
        let table_value = tblDat.to_string();
        let error = err.to_string();
        let mut fields = vec![
            LogField::new("table_name", self.Tbl.Name.O.clone()),
            LogField::new("index_name", self.Idx.Name.O.clone()),
            LogField::new("col", colName),
            LogField::new(
                "row_id",
                redactString(&self.EnableRedactLog, &handle.String()),
            ),
            LogField::new("idxDatum", redactString(&self.EnableRedactLog, &idx_value)),
            LogField::new(
                "rowDatum",
                redactString(&self.EnableRedactLog, &table_value),
            ),
        ];
        if self.EnableRedactLog != "ON"
            && let Some(store) = &self.Storage
        {
            fields.push(LogField::new(
                "row_mvcc",
                redactString(
                    &self.EnableRedactLog,
                    &GetMvccByKey(
                        store.as_ref(),
                        Some((self.HandleEncode)(handle)),
                        Some(&DecodeRowMvccData(&self.Tbl)),
                    ),
                ),
            ));
            fields.push(LogField::new(
                "index_mvcc",
                redactString(
                    &self.EnableRedactLog,
                    &GetMvccByKey(
                        store.as_ref(),
                        Some((self.IndexEncode)(idxRow)),
                        Some(&DecodeIndexMvccData(&self.Idx)),
                    ),
                ),
            ));
        }
        fields.push(LogField::new("error", error.clone()));
        addStack(&mut fields);
        self.log("admin check found data inconsistency", fields);
        ConsistencyError {
            Kind: ConsistencyErrorKind::AdminCheckWithColumnInfo,
            Code: 8134,
            Args: vec![
                self.Tbl.Name.O.clone(),
                self.Idx.Name.O.clone(),
                colName.to_string(),
                handle.String(),
                idx_value,
                table_value,
                error,
            ],
            RedactMode: self.EnableRedactLog.clone(),
        }
    }

    // ReportAdminCheckInconsistent reports an index row missing from record rows.
    /// Admin check 发现索引行与表行不一致时记录日志并返回错误码 8223。
    pub fn ReportAdminCheckInconsistent(
        &self,
        handle: &dyn kv::Handle,
        idxRow: Option<&RecordData>,
        tblRow: Option<&RecordData>,
    ) -> ConsistencyError {
        let mut fields = vec![
            LogField::new("table_name", self.Tbl.Name.O.clone()),
            LogField::new("index_name", self.Idx.Name.O.clone()),
            LogField::new(
                "row_id",
                redactString(&self.EnableRedactLog, &handle.String()),
            ),
            LogField::new(
                "index",
                redactString(
                    &self.EnableRedactLog,
                    &idxRow.map(RecordData::String).unwrap_or_default(),
                ),
            ),
            LogField::new(
                "row",
                redactString(
                    &self.EnableRedactLog,
                    &tblRow.map(RecordData::String).unwrap_or_default(),
                ),
            ),
        ];
        if handle.IsInt() {
            fields.push(LogField::new("int_handle", handle.IntValue().to_string()));
        }

        // 始终尝试附加清空 value 后的 write/value 元数据；完整 MVCC JSON 受脱敏控制
        if let Some(store) = &self.Storage {
            if let Ok(response) = GetMVCCByKeyResp(store.as_ref(), (self.HandleEncode)(handle))
                && response.Info.is_some()
            {
                addMVCCFields("row", &response, &mut fields);
            }
            if let Some(index_row) = idxRow
                && let Ok(response) =
                    GetMVCCByKeyResp(store.as_ref(), (self.IndexEncode)(index_row))
                && response.Info.is_some()
            {
                addMVCCFields("index", &response, &mut fields);
            }
            if self.EnableRedactLog != "ON" {
                fields.push(LogField::new(
                    "row_mvcc",
                    redactString(
                        &self.EnableRedactLog,
                        &GetMvccByKey(
                            store.as_ref(),
                            Some((self.HandleEncode)(handle)),
                            Some(&DecodeRowMvccData(&self.Tbl)),
                        ),
                    ),
                ));
                if let Some(index_row) = idxRow {
                    fields.push(LogField::new(
                        "index_mvcc",
                        redactString(
                            &self.EnableRedactLog,
                            &GetMvccByKey(
                                store.as_ref(),
                                Some((self.IndexEncode)(index_row)),
                                Some(&DecodeIndexMvccData(&self.Idx)),
                            ),
                        ),
                    ));
                }
            }
        }
        addStack(&mut fields);
        self.log("admin check found data inconsistency", fields);
        ConsistencyError {
            Kind: ConsistencyErrorKind::AdminCheck,
            Code: 8223,
            Args: vec![
                self.Tbl.Name.O.clone(),
                self.Idx.Name.O.clone(),
                handle.String(),
                idxRow.map(RecordData::String).unwrap_or_default(),
                tblRow.map(RecordData::String).unwrap_or_default(),
            ],
            RedactMode: self.EnableRedactLog.clone(),
        }
    }

    /// 通过 `LogSink` 写出一条错误级诊断日志。
    fn log(&self, message: &str, fields: Vec<LogField>) {
        self.Logger.error(LogEntry {
            Message: message.to_string(),
            Fields: fields,
        });
    }
}

/// 附加清空载荷后的 write/value 元数据 JSON，避免在元数据字段中泄漏原文。
fn addMVCCFields(title: &str, response: &MvccGetByKeyResponse, fields: &mut Vec<LogField>) {
    let Some(info) = &response.Info else {
        return;
    };
    for (index, write) in info.Writes.iter().enumerate() {
        let mut metadata = write.clone();
        metadata.ShortValue.clear();
        if let Ok(json) = toGoJSON(&metadata) {
            fields.push(LogField::new(format!("{title}_mvcc_write_{index}"), json));
        }
    }
    for (index, value) in info.Values.iter().enumerate() {
        let mut metadata = value.clone();
        metadata.Value.clear();
        if let Ok(json) = toGoJSON(&metadata) {
            fields.push(LogField::new(format!("{title}_mvcc_value_{index}"), json));
        }
    }
}

/// Match Go `encoding/json`'s default HTML-safe escaping. `serde_json` already
/// performs all other JSON escaping, so only the five runes treated specially
/// by Go need a second pass.
fn toGoJSON<T: Serialize>(value: &T) -> Result<String, serde_json::Error> {
    let json = serde_json::to_string(value)?;
    let mut output = String::with_capacity(json.len());
    for character in json.chars() {
        match character {
            '<' => output.push_str(r"\u003c"),
            '>' => output.push_str(r"\u003e"),
            '&' => output.push_str(r"\u0026"),
            '\u{2028}' => output.push_str(r"\u2028"),
            '\u{2029}' => output.push_str(r"\u2029"),
            _ => output.push(character),
        }
    }
    Ok(output)
}

/// Apply the redaction indices and formatting verbs from the three Go errno
/// templates. `%#v` string arguments are quoted after ON replacement, while
/// MARKER wraps the already formatted argument.
fn redactErrorArg(mode: &str, value: &str, quoted: bool) -> String {
    let format_value = |input: &str| {
        if quoted {
            format!("{input:?}")
        } else {
            input.to_string()
        }
    };
    match mode {
        "OFF" => format_value(value),
        "ON" => format_value("?"),
        "MARKER" => redactString("MARKER", &format_value(value)),
        _ => String::new(),
    }
}

/// 将 handle 列表格式化为 `[h1 h2 ...]`。
fn formatHandles(handles: &[Box<dyn kv::Handle>]) -> String {
    format!(
        "[{}]",
        handles
            .iter()
            .map(|handle| handle.String())
            .collect::<Vec<_>>()
            .join(" ")
    )
}

/// 按 `EnableRedactLog` 模式脱敏：ON 清空、MARKER 包络、OFF 原样。
fn redactString(mode: &str, input: &str) -> String {
    match mode {
        "MARKER" => {
            // 用 ‹› 包络，并对已有标记字符做转义（双写）
            let mut output = String::with_capacity(input.len() + "‹›".len());
            output.push('‹');
            for character in input.chars() {
                output.push(character);
                if character == '‹' || character == '›' {
                    output.push(character);
                }
            }
            output.push('›');
            output
        }
        "OFF" => input.to_string(),
        "ON" => String::new(),
        _ => {
            debug_assert!(false, "invalid redact mode");
            String::new()
        }
    }
}

/// 追加当前调用栈，便于定位报告触发点。
fn addStack(fields: &mut Vec<LogField>) {
    fields.push(LogField::new(
        "stack",
        std::backtrace::Backtrace::capture().to_string(),
    ));
}

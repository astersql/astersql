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

// 事务驱动错误类型、重复键提取与写冲突美化打印。
//
// 写冲突（write conflict）发生在乐观事务提交时发现键已被其他事务写入；
// TxnLockNotFound 表示两阶段提交（2PC）第二阶段找不到锁。本模块将原始键
// 解码为 table / index / meta 可读形式，并生成与 Go 侧一致的错误文案。

use std::collections::HashMap;
use std::error::Error;
use std::fmt::{self, Display, Formatter};

use astersql_kv as kv;
use astersql_tablecodec as tablecodec;
use errors::ErrorArg;

use crate::Key;

pub use tablecodec::model::{ColumnInfo, IndexColumn, IndexInfo, TableInfo};

/// 事务驱动层统一错误枚举。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DriverError {
    /// 键未找到（通用 not found）。
    NotFound,
    /// client-go 风格的键不存在哨兵（ErrNotExist）。
    ClientNotExist,
    /// SQL 层重复键：带可读名称与取值。
    KeyExists { value: String, name: String },
    /// 后端报告键已存在（含原始键与值）。
    BackendKeyExists { key: Key, value: Vec<u8> },
    /// 写冲突详情。
    WriteConflict(WriteConflict),
    /// 可重试事务错误（如锁相关）。
    Retryable(String),
    /// 非法事务选项。
    InvalidOption(String),
    /// 尚未实现的操作。
    NotImplemented,
    /// 提交器（committer）正在工作中。
    CommitterWorking,
    /// 后端透传错误消息。
    Backend(String),
}

impl DriverError {
    /// 是否为“键不存在”类错误（含 ClientNotExist）。
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::NotFound | Self::ClientNotExist)
    }
}

impl Display for DriverError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => formatter.write_str("key not found"),
            Self::ClientNotExist => formatter.write_str("tikv key does not exist"),
            Self::KeyExists { value, name } => {
                write!(formatter, "Duplicate entry '{value}' for key '{name}'")
            }
            Self::BackendKeyExists { key, .. } => {
                write!(formatter, "backend key already exists: {}", hex(key))
            }
            Self::WriteConflict(conflict) => write!(
                formatter,
                "Write conflict, txnStartTS={}, conflictStartTS={}, conflictCommitTS={}, key={}{} primary={}{} reason={}",
                conflict.start_ts,
                conflict.conflict_ts,
                conflict.conflict_commit_ts,
                conflict.key_table_id,
                conflict.key_rest,
                conflict.primary_table_id,
                conflict.primary_rest,
                conflict.reason
            ),
            Self::Retryable(message) => write!(formatter, "retryable transaction error: {message}"),
            Self::InvalidOption(message) => {
                write!(formatter, "invalid transaction option: {message}")
            }
            Self::NotImplemented => formatter.write_str("operation is not implemented"),
            Self::CommitterWorking => formatter.write_str("committer is working"),
            Self::Backend(message) => formatter.write_str(message),
        }
    }
}

impl Error for DriverError {}

/// 写冲突的结构化字段：时间戳、键、主键及美化后的片段。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WriteConflict {
    /// 本事务开始时间戳（start_ts）。
    pub start_ts: u64,
    /// 冲突事务的开始时间戳。
    pub conflict_ts: u64,
    /// 冲突事务的提交时间戳。
    pub conflict_commit_ts: u64,
    /// 冲突键原始字节。
    pub key: Key,
    /// 冲突事务的主键（primary key）字节。
    pub primary: Key,
    /// 冲突原因标签（如 Optimistic / Unknown）。
    pub reason: String,
    /// 美化后的键 tableID 前缀片段。
    pub key_table_id: String,
    /// 美化后的键剩余片段。
    pub key_rest: String,
    /// 美化后的主键 tableID 前缀片段。
    pub primary_table_id: String,
    /// 美化后的主键剩余片段。
    pub primary_rest: String,
}

/// 构造 SQL 风格的 Duplicate entry 错误。
fn gen_key_exists_error(name: impl Into<String>, value: impl Into<String>) -> DriverError {
    DriverError::KeyExists {
        value: value.into(),
        name: name.into(),
    }
}

/// 字节切片转小写十六进制字符串。
fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        result.push(DIGITS[(byte >> 4) as usize] as char);
        result.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    result
}

fn key_string(key: &[u8]) -> String {
    tablecodec::kv::Key(key.to_vec()).String()
}

fn format_binary_string(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for byte in value.bytes() {
        if (32..127).contains(&byte) {
            output.push(byte as char);
        } else {
            output.push_str(&format!("\\x{byte:02X}"));
        }
    }
    output
}

fn is_binary_or_bit(field_type: &tablecodec::types::FieldType) -> bool {
    tablecodec::types::IsBinaryStr(field_type) || field_type.GetType() == tablecodec::mysql::TypeBit
}

fn rowcodec_columns(
    index: &IndexInfo,
    table: &TableInfo,
) -> Option<Vec<tablecodec::rowcodec::ColInfo>> {
    index
        .Columns
        .iter()
        .map(|index_column| {
            let column = table
                .Columns
                .get(usize::try_from(index_column.Offset).ok()?)?;
            Some(tablecodec::rowcodec::ColInfo {
                ID: column.ID,
                IsPKHandle: false,
                VirtualGenCol: false,
                Ft: column.FieldType.clone(),
            })
        })
        .collect()
}

/// Returns a duplicate-primary-key error from a record key and row value.
///
/// 从行记录键与行值提取主键重复错误（含无符号整型与复合主键）。
pub fn ExtractKeyExistsErrFromHandle(key: &[u8], value: &[u8], table: &TableInfo) -> DriverError {
    let name = format!("{}.PRIMARY", table.Name);
    let (_, handle) = match tablecodec::DecodeRecordKey(tablecodec::kv::Key(key.to_vec())) {
        Ok(decoded) => decoded,
        Err(_) => return gen_key_exists_error(name, key_string(key)),
    };

    if handle.IsInt() {
        if table
            .GetPkColInfo()
            .is_some_and(|column| tablecodec::mysql::HasUnsignedFlag(column.GetFlag()))
        {
            return gen_key_exists_error(name, (handle.IntValue() as u64).to_string());
        }
        return gen_key_exists_error(name, handle.String());
    }

    if value.is_empty() {
        return gen_key_exists_error(name, handle.String());
    }
    let Some(primary) = table.Indices.iter().find(|index| index.Primary) else {
        return gen_key_exists_error(name, handle.String());
    };

    let columns = table
        .Columns
        .iter()
        .map(|column| (column.ID, Box::new(column.FieldType.clone())))
        .collect::<HashMap<_, _>>();
    let Some(handle_column_ids) = primary
        .Columns
        .iter()
        .map(|index_column| {
            table
                .Columns
                .get(usize::try_from(index_column.Offset).ok()?)
                .map(|column| column.ID)
        })
        .collect::<Option<Vec<_>>>()
    else {
        return gen_key_exists_error(name, key_string(key));
    };
    let row = match tablecodec::DecodeRowToDatumMap(
        Some(value.to_vec()),
        columns.clone(),
        Some(tablecodec::time::UTC),
    ) {
        Ok(row) => row,
        Err(_) => return gen_key_exists_error(name, handle.String()),
    };
    let data = match tablecodec::DecodeHandleToDatumMap(
        Some(handle),
        handle_column_ids,
        columns,
        Some(tablecodec::time::UTC),
        Some(row),
    ) {
        Ok(data) => data,
        Err(_) => return gen_key_exists_error(name, key_string(key)),
    };

    let mut values = Vec::with_capacity(primary.Columns.len());
    for index_column in &primary.Columns {
        let Some(column) = usize::try_from(index_column.Offset)
            .ok()
            .and_then(|offset| table.Columns.get(offset))
        else {
            return gen_key_exists_error(name, key_string(key));
        };
        let Some(datum) = data.get(&column.ID) else {
            return gen_key_exists_error(name, key_string(key));
        };
        let mut rendered = match datum.ToString() {
            Ok(rendered) => rendered,
            Err(_) => return gen_key_exists_error(name, key_string(key)),
        };
        if index_column.Length > 0 && rendered.len() > index_column.Length as usize {
            let mut boundary = index_column.Length as usize;
            while !rendered.is_char_boundary(boundary) {
                boundary -= 1;
            }
            rendered.truncate(boundary);
        }
        if is_binary_or_bit(&column.FieldType) {
            rendered = format_binary_string(&rendered);
        }
        values.push(rendered);
    }
    gen_key_exists_error(name, values.join("-"))
}

/// Returns a duplicate-index-key error, including binary and prefix columns.
///
/// 从索引键提取唯一索引重复错误（支持二进制列与前缀索引）。
pub fn ExtractKeyExistsErrFromIndex(
    key: &[u8],
    value: &[u8],
    table: &TableInfo,
    index_id: i64,
) -> DriverError {
    let Some(index) = table.Indices.iter().find(|index| index.ID == index_id) else {
        return gen_key_exists_error("UNKNOWN", key_string(key));
    };
    let name = format!("{}.{}", table.Name, index.Name);
    if value.is_empty() {
        return gen_key_exists_error(name, key_string(key));
    }

    let Some(column_info) = rowcodec_columns(index, table) else {
        return gen_key_exists_error(name, key_string(key));
    };
    let encoded_values = match tablecodec::DecodeIndexKV(
        key.to_vec(),
        value.to_vec(),
        index.Columns.len(),
        tablecodec::HandleNotNeeded,
        column_info.clone(),
    ) {
        Ok(values) => values,
        Err(_) => return gen_key_exists_error(name, key_string(key)),
    };
    let mut values = Vec::with_capacity(encoded_values.len());
    for (position, encoded) in encoded_values.into_iter().enumerate() {
        let Some(info) = column_info.get(position) else {
            return gen_key_exists_error(name, key_string(key));
        };
        let datum = match tablecodec::DecodeColumnValue(
            encoded,
            Box::new(info.Ft.clone()),
            Some(tablecodec::time::UTC),
        ) {
            Ok(datum) => datum,
            Err(_) => return gen_key_exists_error(name, key_string(key)),
        };
        let mut rendered = match datum.ToString() {
            Ok(rendered) => rendered,
            Err(_) => return gen_key_exists_error(name, key_string(key)),
        };
        if is_binary_or_bit(&info.Ft) {
            rendered = format_binary_string(&rendered);
        }
        values.push(rendered);
    }
    gen_key_exists_error(name, values.join("-"))
}

/// 将可选错误规范化：写冲突转为 Backend 字符串，可重试错误附加美化键。
pub fn extractKeyErr(error: Option<DriverError>) -> Result<(), DriverError> {
    match error {
        None => Ok(()),
        Some(DriverError::WriteConflict(conflict)) => Err(DriverError::Backend(
            newWriteConflictError(Some(conflict)).to_string(),
        )),
        Some(DriverError::Retryable(retry)) => {
            let detail = prettyLockNotFoundKey(&retry);
            Err(DriverError::Retryable(format!("{retry} {detail}")))
        }
        Some(error) => Err(error),
    }
}

/// Formats a write-conflict using the same kv.ErrWriteConflict template as Go.
///
/// 使用与 Go 相同的 `kv.ErrWriteConflict` 模板格式化写冲突。
pub fn newWriteConflictError(conflict: Option<WriteConflict>) -> errors::SharedError {
    let Some(conflict) = conflict else {
        return errors::SharedError::new((**kv::ErrWriteConflict).clone());
    };
    let (key_table_id, mut key_rest) = prettyWriteKey(&conflict.key);
    key_rest.push_str(&format!(", originalKey={}", hex(&conflict.key)));
    key_rest.push_str(", primary=");
    let (primary_table_id, mut primary_rest) = prettyWriteKey(&conflict.primary);
    primary_rest.push_str(&format!(", originalPrimaryKey={}", hex(&conflict.primary)));
    kv::ErrWriteConflict.FastGenByArgs(&[
        ErrorArg::from(conflict.start_ts),
        ErrorArg::from(conflict.conflict_ts),
        ErrorArg::from(conflict.conflict_commit_ts),
        ErrorArg::from(key_table_id),
        ErrorArg::from(key_rest),
        ErrorArg::from(primary_table_id),
        ErrorArg::from(primary_rest),
        ErrorArg::from(conflict.reason),
    ])
}

/// Splits a TiDB key into the table-id prefix and human-readable remainder.
///
/// 将 TiDB 键拆为 tableID 前缀与可读剩余部分（索引 / 行 / meta）。
pub fn prettyWriteKey(key: &[u8]) -> (String, String) {
    let owned = tablecodec::kv::Key(key.to_vec());
    if let Ok((table_id, index_id, index_values)) = tablecodec::DecodeIndexKey(owned.clone()) {
        let mut rest = format!(", indexID={index_id}, indexValues={{");
        for value in index_values {
            rest.push_str(&value);
            rest.push_str(", ");
        }
        rest.push_str("}}");
        return (format!("{{tableID={table_id}"), rest);
    }
    if let Ok((table_id, handle)) = tablecodec::DecodeRecordKey(owned.clone()) {
        return (
            format!("{{tableID={table_id}"),
            format!(", handle={}}}", handle.String()),
        );
    }
    if let Ok((meta_key, meta_field)) = tablecodec::DecodeMetaKey(owned) {
        return (
            String::new(),
            format!(
                "{{metaKey=true, key={}, field={}}}",
                String::from_utf8_lossy(&meta_key),
                String::from_utf8_lossy(&meta_field)
            ),
        );
    }
    (String::new(), format!("{key:?}"))
}

/// Extracts and pretty-prints the JSON byte array embedded in TxnLockNotFound.
///
/// 从 TxnLockNotFound 原文中提取嵌入的字节数组并美化打印。
pub fn prettyLockNotFoundKey(raw_retry: &str) -> String {
    if !raw_retry.contains("TxnLockNotFound") {
        return String::new();
    }
    let Some(start) = raw_retry.find('[') else {
        return String::new();
    };
    let Some(relative_end) = raw_retry[start..].find(']') else {
        return String::new();
    };
    // 解析 `[u8, u8, ...]` 形式的键字节。
    let raw = &raw_retry[start + 1..start + relative_end];
    let mut key = Vec::new();
    for number in raw
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let Ok(byte) = number.parse::<u8>() else {
            return String::new();
        };
        key.push(byte);
    }
    let (table_id, rest) = prettyWriteKey(&key);
    format!("{table_id}{rest}")
}

/// 解码表键头部：返回 (table_id, index_id, is_record)。
pub(crate) fn decode_table_key_head(key: &[u8]) -> Option<(i64, i64, bool)> {
    tablecodec::DecodeKeyHead(tablecodec::kv::Key(key.to_vec())).ok()
}

// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// TiDB 键编码/解码辅助（codec）：表记录键、索引键与 Handle。
//
// 提供规划器侧用于构造与解析 TiKV 键前缀的工具：根据表/索引元信息与行数据
// 编码 Handle（整数主键或 common handle）、记录键（`t{table}_r{handle}`）
// 与索引键（`t{table}_i{index}_{values}`），并支持分区表名解析与 Datum 转 JSON。

use std::collections::BTreeMap;

/// SQL 值在编解码路径中的统一表示。
#[derive(Clone, Debug, PartialEq)]
pub enum Datum {
    Null,
    Int(i64),
    UInt(u64),
    Float(f64),
    Bytes(Vec<u8>),
    String(String),
    Bool(bool),
    Json(String),
}

impl Datum {
    /// 将 Datum 转为可用于键拼接的字符串形式。
    pub fn key_string(&self) -> String {
        match self {
            Self::Null => "NULL".into(),
            Self::Int(v) => v.to_string(),
            Self::UInt(v) => v.to_string(),
            Self::Float(v) => v.to_string(),
            Self::Bytes(v) => v.iter().map(|b| format!("{b:02x}")).collect(),
            Self::String(v) | Self::Json(v) => v.clone(),
            Self::Bool(v) => v.to_string(),
        }
    }
}

fn handle_key_string(handle: &Handle) -> String {
    match handle {
        Handle::Int(value) => value.to_string(),
        Handle::Common(values) => values
            .iter()
            .map(Datum::key_string)
            .collect::<Vec<_>>()
            .join("|"),
    }
}

fn json_string(value: &str) -> String {
    let mut output = String::with_capacity(value.len() + 2);
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\u{08}' => output.push_str("\\b"),
            '\u{0c}' => output.push_str("\\f"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            value if value <= '\u{1f}' => {
                output.push_str(&format!("\\u{:04x}", value as u32));
            }
            value => output.push(value),
        }
    }
    output.push('"');
    output
}

fn decode_hex(value: &str) -> Result<Vec<u8>, String> {
    if !value.len().is_multiple_of(2) {
        return Err("invalid hex key".into());
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = (pair[0] as char)
                .to_digit(16)
                .ok_or_else(|| "invalid hex key".to_owned())?;
            let low = (pair[1] as char)
                .to_digit(16)
                .ok_or_else(|| "invalid hex key".to_owned())?;
            Ok(((high << 4) | low) as u8)
        })
        .collect()
}

/// 编解码用的列元信息。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CodecColumn {
    pub id: i64,
    pub name: String,
    /// 是否为整数主键列（integer handle）。
    pub primary_key: bool,
    pub unsigned: bool,
}
/// 编解码用的索引元信息。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CodecIndex {
    pub id: i64,
    pub name: String,
    pub column_ids: Vec<i64>,
    pub unique: bool,
}
/// 编解码用的表元信息，含列、索引与分区映射。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CodecTable {
    pub id: i64,
    pub name: String,
    pub columns: Vec<CodecColumn>,
    pub indices: Vec<CodecIndex>,
    /// 是否使用 clustered 索引式的 common handle（非整数主键）。
    pub common_handle: bool,
    /// 分区 ID → 分区名。
    pub partitions: BTreeMap<i64, String>,
}

/// 行定位 Handle：整数主键或 common handle 多列值。
#[derive(Clone, Debug, PartialEq)]
pub enum Handle {
    Int(i64),
    Common(Vec<Datum>),
}

/// 一行数据：列 ID → Datum。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CodecRow {
    pub values: BTreeMap<i64, Datum>,
}

/// TiDB 编解码函数帮助器（与 Go `TiDBCodecFuncHelper` 对应）。
#[derive(Clone, Debug, Default)]
pub struct TiDBCodecFuncHelper;

impl TiDBCodecFuncHelper {
    /// 从行数据编码记录键：`t{tableID}_r{handle}`。
    pub fn encodeHandleFromRow(
        &self,
        table: &CodecTable,
        row: &CodecRow,
    ) -> Result<Vec<u8>, String> {
        let handle = self.buildHandle(table, row)?;
        let body = handle_key_string(&handle);
        Ok(format!("t{}_r{}", table.id, body).into_bytes())
    }

    /// 按表名（可带 `table(partition)`）查找表，并解析分区 ID。
    pub fn findCommonOrPartitionedTable<'a>(
        &self,
        tables: &'a [CodecTable],
        name: &str,
    ) -> Result<(&'a CodecTable, Option<i64>), String> {
        let (table, partition) = self.extractTablePartition(name);
        let found = tables
            .iter()
            .find(|candidate| candidate.name.eq_ignore_ascii_case(&table))
            .ok_or_else(|| format!("table {table} does not exist"))?;
        let partition_id = partition
            .filter(|partition| !partition.is_empty())
            .map(|partition| {
                if found.partitions.is_empty() {
                    return Err("not a partitioned table".to_owned());
                }
                found
                    .partitions
                    .iter()
                    .find_map(|(id, value)| value.eq_ignore_ascii_case(&partition).then_some(*id))
                    .ok_or_else(|| format!("partition {partition} does not exist"))
            })
            .transpose()?;
        Ok((found, partition_id))
    }

    /// 解析 `table(partition)` 形式；无括号则整串为表名。
    pub fn extractTablePartition(&self, value: &str) -> (String, Option<String>) {
        let Some(start) = value.find('(') else {
            return (value.to_owned(), None);
        };
        let Some(end) = value.find(')') else {
            return (value.to_owned(), None);
        };
        if end < start {
            return (value.to_owned(), None);
        }
        (
            value[..start].to_owned(),
            Some(value[start + '('.len_utf8()..end].to_owned()),
        )
    }

    /// 根据表是 common handle 还是整数主键，从行中构造 Handle。
    pub fn buildHandle(&self, table: &CodecTable, row: &CodecRow) -> Result<Handle, String> {
        if table.common_handle {
            // common handle：主键索引各列值拼接为 Handle::Common。
            let primary = table
                .indices
                .iter()
                .find(|index| index.name.eq_ignore_ascii_case("PRIMARY"))
                .ok_or_else(|| "common handle table has no primary index".to_owned())?;
            let values = primary
                .column_ids
                .iter()
                .map(|id| {
                    row.values
                        .get(id)
                        .cloned()
                        .ok_or_else(|| format!("column {id} is missing"))
                })
                .collect::<Result<Vec<_>, _>>()?;
            return Ok(Handle::Common(values));
        }
        // 整数 handle：取标记为 primary_key 的整型列。
        let primary = table
            .columns
            .iter()
            .find(|column| column.primary_key)
            .ok_or_else(|| "integer handle column is missing".to_owned())?;
        match row.values.get(&primary.id) {
            Some(Datum::Int(value)) => Ok(Handle::Int(*value)),
            Some(Datum::UInt(value)) if *value <= i64::MAX as u64 => Ok(Handle::Int(*value as i64)),
            Some(Datum::UInt(value)) => Ok(Handle::Int(i64::from_ne_bytes(value.to_ne_bytes()))),
            _ => Err(format!("column {} is not an integer handle", primary.name)),
        }
    }

    /// 从行数据编码索引键：`t{tableID}_i{indexID}_{colValues}`。
    pub fn encodeIndexKeyFromRow(
        &self,
        table: &CodecTable,
        index: &CodecIndex,
        row: &CodecRow,
    ) -> Result<Vec<u8>, String> {
        let values = index
            .column_ids
            .iter()
            .map(|id| {
                row.values
                    .get(id)
                    .map(Datum::key_string)
                    .ok_or_else(|| format!("index column {id} is missing"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        // Go always builds the handle, even when a unique index does not need
        // to append it to the key, so missing/invalid PK input remains an error.
        let handle = self.buildHandle(table, row)?;
        let mut body = values.join("|");
        if !index.unique {
            body.push('|');
            body.push_str(&handle_key_string(&handle));
        }
        Ok(format!("t{}_i{}_{}", table.id, index.id, body).into_bytes())
    }

    /// 将键字符串解码为可读描述（表/记录/索引）。
    pub fn decodeKeyFromString(&self, key: &str, tables: &[CodecTable]) -> Result<String, String> {
        let decoded = decode_hex(key)?;
        let key = std::str::from_utf8(&decoded).map_err(|_| "invalid key encoding")?;
        if !key.starts_with('t') {
            return Err("invalid table key prefix".into());
        }
        if let Some((head, handle)) = key.split_once("_r") {
            return self.decodeRecordKey(head, handle, tables);
        }
        if let Some((head, body)) = key.split_once("_i") {
            return self.decodeIndexKey(head, body, tables);
        }
        let table_id = key[1..].parse::<i64>().map_err(|_| "invalid table id")?;
        Ok(self.decodeTableKey(
            table_id,
            tables
                .iter()
                .find(|table| table.id == table_id || table.partitions.contains_key(&table_id)),
        ))
    }

    /// 解码记录键中的表 ID 与 handle 部分。
    fn decodeRecordKey(
        &self,
        head: &str,
        handle: &str,
        tables: &[CodecTable],
    ) -> Result<String, String> {
        let table_id = head
            .trim_start_matches('t')
            .parse::<i64>()
            .map_err(|_| "invalid table id")?;
        let table = tables
            .iter()
            .find(|table| table.id == table_id || table.partitions.contains_key(&table_id));
        let (base, partition) = table.map_or((table_id, None), |table| {
            (
                table.id,
                table.partitions.get(&table_id).map(String::as_str),
            )
        });
        let mut fields = Vec::new();
        if let Some(table) = table
            && table.common_handle
        {
            let primary = table
                .indices
                .iter()
                .find(|index| index.name.eq_ignore_ascii_case("PRIMARY"))
                .ok_or_else(|| "primary key not found when decoding record key".to_owned())?;
            let values = handle.split('|').collect::<Vec<_>>();
            if values.len() != primary.column_ids.len() {
                return Err("primary key length not match handle columns number in key".into());
            }
            let entries = primary
                .column_ids
                .iter()
                .zip(values)
                .map(|(id, value)| {
                    let name = table
                        .columns
                        .iter()
                        .find(|column| column.id == *id)
                        .ok_or_else(|| "column not found when decoding record key".to_owned())?;
                    Ok(format!(
                        "{}:{}",
                        json_string(&name.name),
                        json_string(value)
                    ))
                })
                .collect::<Result<Vec<_>, String>>()?;
            fields.push(format!("\"handle\":{{{}}}", entries.join(",")));
        } else if let Some(table) = table {
            let name = table
                .columns
                .iter()
                .find(|column| column.primary_key)
                .map_or("_tidb_rowid", |column| column.name.as_str());
            let value = handle
                .parse::<i64>()
                .map_err(|_| "invalid integer handle")?;
            fields.push(format!("{}:{value}", json_string(name)));
        } else {
            fields.push(format!("\"handle\":{}", json_string(handle)));
        }
        if partition.is_some() {
            fields.push(format!("\"partition_id\":{table_id}"));
        }
        // Go's integer-record path serializes table_id as a string, while its
        // common/unknown-handle paths serialize the ID as a number.
        if table.is_some_and(|table| !table.common_handle) {
            fields.push(format!("\"table_id\":{}", json_string(&base.to_string())));
        } else {
            fields.push(format!("\"table_id\":{base}"));
        }
        fields.sort();
        Ok(format!("{{{}}}", fields.join(",")))
    }

    /// 解码索引键中的表 ID、索引 ID 与列值。
    fn decodeIndexKey(
        &self,
        head: &str,
        body: &str,
        tables: &[CodecTable],
    ) -> Result<String, String> {
        let table_id = head
            .trim_start_matches('t')
            .parse::<i64>()
            .map_err(|_| "invalid table id")?;
        let (index_id, values) = body
            .split_once('_')
            .ok_or_else(|| "invalid index key".to_owned())?;
        let index_id = index_id.parse::<i64>().map_err(|_| "invalid index id")?;
        let Some(table) = tables
            .iter()
            .find(|table| table.id == table_id || table.partitions.contains_key(&table_id))
        else {
            let values = values.split('|').collect::<Vec<_>>();
            return Ok(format!(
                "{{\"index_id\":{index_id},\"index_vals\":{},\"table_id\":{table_id}}}",
                json_string(&values.join(", "))
            ));
        };
        let index = table
            .indices
            .iter()
            .find(|index| index.id == index_id)
            .ok_or_else(|| format!("index id {index_id} does not exist"))?;
        let value_parts = values.split('|').collect::<Vec<_>>();
        if value_parts.len() < index.column_ids.len() {
            return Err("invalid index key".into());
        }
        let mut index_values = index
            .column_ids
            .iter()
            .zip(value_parts)
            .map(|(id, value)| {
                let column = table
                    .columns
                    .iter()
                    .find(|column| column.id == *id)
                    .ok_or_else(|| "index column not found".to_owned())?;
                Ok(format!(
                    "{}:{}",
                    json_string(&column.name),
                    json_string(value)
                ))
            })
            .collect::<Result<Vec<_>, String>>()?;
        index_values.sort();
        let mut fields = vec![
            format!("\"index_id\":{index_id}"),
            format!("\"index_vals\":{{{}}}", index_values.join(",")),
            format!("\"table_id\":{}", table.id),
        ];
        if table.partitions.contains_key(&table_id) {
            fields.push(format!("\"partition_id\":{table_id}"));
        }
        fields.sort();
        Ok(format!("{{{}}}", fields.join(",")))
    }

    /// 将 table_id 格式化为 `TABLE name` 或带 PARTITION 的形式。
    fn decodeTableKey(&self, table_id: i64, table: Option<&CodecTable>) -> String {
        match table {
            Some(table) if table.partitions.contains_key(&table_id) => {
                format!("{{\"partition_id\":{table_id},\"table_id\":{}}}", table.id)
            }
            _ => format!("{{\"table_id\":{table_id}}}"),
        }
    }

    /// 将 Datum 转为 JSON 字面量片段（用于展示或调试）。
    pub fn datumToJSONObject(&self, datum: &Datum) -> String {
        match datum {
            Datum::Null => "null".into(),
            Datum::String(value) | Datum::Json(value) => json_string(value),
            Datum::Bytes(value) => json_string(&format!(
                "{}",
                value
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>()
            )),
            other => other.key_string(),
        }
    }
}

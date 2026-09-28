// Copyright 2019 PingCAP, Inc.
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
// Copyright 2026 AsterSQL.

// 将表记录 KV（键值）解码回 SQL 行视图，并枚举对应索引键。
//
// Handle（行句柄）是定位一行的主键/隐式行号；本模块从记录键或索引键解析 Handle，
// 解码原始行数据，必要时重算生成列，并按表索引定义重建索引键以供校验或清理。

use std::collections::{BTreeMap, HashMap};

use encode::{ColumnType, Datum, SessionOptions};

use crate::{
    CollectGeneratedColumnsFromTable, GeneratedCol, NewSession, Session, TableDefinition,
    canonicalIndexInfo, canonicalTableInfo, decodeCanonicalRow, evalGeneratedColumns, fieldType,
    fromCanonicalDatum, toCanonicalDatum,
};

/// 行句柄：整型 handle，或联合主键的 Common Handle（多列 DatumKey）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Handle {
    Int(i64),
    Common(Vec<DatumKey>),
}

/// Common Handle 中单列键值的简化表示。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DatumKey {
    Int(i64),
    UInt(u64),
    String(String),
    Bytes(Vec<u8>),
}

impl Handle {
    fn canonical(&self) -> Result<Box<dyn tablecodec::kv::Handle>, String> {
        match self {
            Handle::Int(value) => Ok(Box::new(tablecodec::kv::IntHandle(*value))),
            Handle::Common(values) => {
                let values = values
                    .iter()
                    .map(|value| match value {
                        DatumKey::Int(value) => Datum::Int(*value),
                        DatumKey::UInt(value) => Datum::UInt(*value),
                        DatumKey::String(value) => Datum::String(value.clone()),
                        DatumKey::Bytes(value) => Datum::Bytes(value.clone()),
                    })
                    .map(|value| toCanonicalDatum(&value))
                    .collect::<Result<Vec<_>, _>>()?;
                let encoded = tablecodec::codec::NewEncoder(false)
                    .EncodeKey(tablecodec::time::UTC, Vec::new(), values)
                    .map_err(|error| error.to_string())?;
                tablecodec::kv::NewCommonHandle(encoded)
                    .map(|handle| Box::new(handle) as Box<dyn tablecodec::kv::Handle>)
                    .map_err(|error| error.to_string())
            }
        }
    }
}

fn fromCanonicalHandle(handle: &dyn tablecodec::kv::Handle) -> Result<Handle, String> {
    if handle.IsInt() {
        return Ok(Handle::Int(handle.IntValue()));
    }
    let values = handle.Data().map_err(|error| error.to_string())?;
    let values = values
        .iter()
        .map(|value| {
            let value = fromCanonicalDatum(value, None)?;
            match value {
                Datum::Int(value) => Ok(DatumKey::Int(value)),
                Datum::UInt(value) => Ok(DatumKey::UInt(value)),
                Datum::String(value) => Ok(DatumKey::String(value)),
                Datum::Bytes(value) => Ok(DatumKey::Bytes(value)),
                other => Err(format!("unsupported common-handle datum {other:?}")),
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Handle::Common(values))
}

/// 表级 KV→SQL 解码器：持有表定义、会话与生成列列表。
pub struct TableKVDecoder {
    tbl: TableDefinition,
    se: Session,
    tableName: String,
    genCols: Vec<GeneratedCol>,
}

impl TableKVDecoder {
    pub fn Name(&self) -> &str {
        &self.tableName
    }

    pub fn Session(&self) -> &Session {
        &self.se
    }

    /// 使用 canonical tablecodec 从记录键解析整数或 Common Handle。
    pub fn DecodeHandleFromRowKey(&self, key: &[u8]) -> Result<Handle, String> {
        let handle = tablecodec::DecodeRowKey(tablecodec::kv::Key(key.to_vec()))
            .map_err(|error| error.to_string())?;
        fromCanonicalHandle(handle.as_ref())
    }

    /// 从索引键中 `_h` 后缀解析 Handle。
    pub fn DecodeHandleFromIndex(
        &self,
        indexID: i64,
        key: &[u8],
        value: &[u8],
    ) -> Result<Handle, String> {
        let column_count = self
            .tbl
            .indices
            .iter()
            .find(|index| index.id == indexID)
            .map(|index| index.columns.len())
            .ok_or_else(|| format!("index {indexID} does not exist"))?;
        let handle = tablecodec::DecodeIndexHandle(key.to_vec(), value.to_vec(), column_count)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "index value does not contain a handle".to_owned())?;
        fromCanonicalHandle(handle.as_ref())
    }

    /// 解码原始行值，并返回 Go 一致的原始列 ID→值映射。
    pub fn DecodeRawRowData(
        &self,
        handle: &Handle,
        value: &[u8],
    ) -> Result<(Vec<Datum>, BTreeMap<i64, Datum>), String> {
        let mut row = decodeCanonicalRow(value, &self.tbl.columns)?;
        let field_types = self
            .tbl
            .columns
            .iter()
            .enumerate()
            .map(|(index, column)| (index as i64 + 1, Box::new(fieldType(column))))
            .collect::<HashMap<_, _>>();
        let decoded = tablecodec::DecodeRowToDatumMap(
            Some(value.to_vec()),
            field_types,
            Some(tablecodec::time::UTC),
        )
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|(column_id, value)| {
            let column = usize::try_from(column_id - 1)
                .ok()
                .and_then(|index| self.tbl.columns.get(index));
            fromCanonicalDatum(&value, column).map(|value| (column_id, value))
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;

        for (index, default) in &self.tbl.defaults {
            let column_id = *index as i64 + 1;
            if !decoded.contains_key(&column_id) {
                let target = row
                    .get_mut(*index)
                    .ok_or_else(|| format!("default column index {index} out of range for row"))?;
                *target = default.clone();
            }
        }

        if self.tbl.pk_is_handle {
            let value = match handle {
                Handle::Int(value) => *value,
                Handle::Common(_) => {
                    return Err("integer primary-key table requires an integer handle".into());
                }
            };
            let (index, column) = self
                .tbl
                .columns
                .iter()
                .enumerate()
                .find(|(_, column)| column.primary_key)
                .ok_or_else(|| "integer primary-key table has no primary-key column".to_owned())?;
            row[index] = if column.column_type == ColumnType::UInt {
                Datum::UInt(value as u64)
            } else {
                Datum::Int(value)
            };
        } else if self.tbl.common_handle {
            let values = match handle {
                Handle::Common(values) => values,
                Handle::Int(_) => {
                    return Err("common-handle table requires a common handle".into());
                }
            };
            let primary_columns = self
                .tbl
                .columns
                .iter()
                .enumerate()
                .filter(|(_, column)| column.primary_key)
                .collect::<Vec<_>>();
            if values.len() != primary_columns.len() {
                return Err(format!(
                    "common handle has {} columns, expected {}",
                    values.len(),
                    primary_columns.len()
                ));
            }
            for ((index, _), value) in primary_columns.into_iter().zip(values) {
                row[index] = match value {
                    DatumKey::Int(value) => Datum::Int(*value),
                    DatumKey::UInt(value) => Datum::UInt(*value),
                    DatumKey::String(value) => Datum::String(value.clone()),
                    DatumKey::Bytes(value) => Datum::Bytes(value.clone()),
                };
            }
        }
        Ok((row, decoded))
    }

    /// 将原始行解码为调试字符串；失败时返回错误描述。
    pub fn DecodeRawRowDataAsStr(&self, handle: &Handle, value: &[u8]) -> String {
        self.DecodeRawRowData(handle, value)
            .map(|(row, _)| datumsToString(&row))
            .unwrap_or_else(|error| format!("/* ERROR: {error} */"))
    }

    /// 根据行数据重算生成列后，枚举该行应有的所有索引键并回调。
    pub fn IterRawIndexKeys(
        &self,
        handle: &Handle,
        rawRow: &[u8],
        mut callback: impl FnMut(&[u8]) -> Result<(), String>,
    ) -> Result<(), String> {
        let (mut row, _) = self.DecodeRawRowData(handle, rawRow)?;
        if !self.genCols.is_empty() {
            evalGeneratedColumns(&mut row, &self.genCols).map_err(|(_, error)| error)?;
        }
        for index in &self.tbl.indices {
            // Common Handle 下主键索引键已由记录键覆盖，跳过。
            if index.primary && self.tbl.common_handle {
                continue;
            }
            let values = index
                .columns
                .iter()
                .map(|column| {
                    row.get(*column).cloned().ok_or_else(|| {
                        format!(
                            "column index {column} out of range for row with {} columns",
                            row.len()
                        )
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            let canonical_values = values
                .iter()
                .map(toCanonicalDatum)
                .collect::<Result<Vec<_>, _>>()?;
            let table_info = canonicalTableInfo(
                &self.tbl.columns,
                &self.tbl.indices,
                self.tbl.pk_is_handle,
                self.tbl.common_handle,
            );
            let index_info = canonicalIndexInfo(index);
            let (key, _) = tablecodec::GenIndexKey(
                tablecodec::codec::NewEncoder(false),
                Some(tablecodec::time::UTC),
                Box::new(table_info),
                Box::new(index_info),
                self.tbl.id,
                canonical_values,
                Some(handle.canonical()?),
                None,
            )
            .map_err(|error| error.to_string())?;
            callback(&key)?;
        }
        Ok(())
    }
}

fn datumsToString(row: &[Datum]) -> String {
    let mut result = String::new();
    if row.len() > 1 {
        result.push('(');
    }
    for (index, datum) in row.iter().enumerate() {
        if index > 0 {
            result.push_str(", ");
        }
        match datum {
            Datum::Null => result.push_str("NULL"),
            Datum::MinNotNull => result.push_str("-inf"),
            Datum::MaxValue => result.push_str("+inf"),
            Datum::Int(value) => appendDatumText(&mut result, &value.to_string(), false),
            Datum::UInt(value) => appendDatumText(&mut result, &value.to_string(), false),
            Datum::Float(value) => appendDatumText(&mut result, &value.to_string(), false),
            Datum::String(value) => appendDatumText(&mut result, value, true),
            Datum::Bytes(value) | Datum::BinaryLiteral(value) | Datum::Bit(value) => {
                appendDatumText(&mut result, &String::from_utf8_lossy(value), false)
            }
            Datum::Json(value)
            | Datum::Decimal(value)
            | Datum::Timestamp(value)
            | Datum::Duration(value) => appendDatumText(&mut result, value, false),
            Datum::Enum { name, .. } | Datum::Set { name, .. } => {
                appendDatumText(&mut result, name, false)
            }
        }
    }
    if row.len() > 1 {
        result.push(')');
    }
    result
}

fn appendDatumText(result: &mut String, value: &str, quoted: bool) {
    const LOG_DATUM_LEN: usize = 2048;
    let original_len = value.len();
    let mut end = original_len.min(LOG_DATUM_LEN);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    if quoted {
        result.push('"');
    }
    result.push_str(&value[..end]);
    if quoted {
        result.push('"');
    }
    if original_len > LOG_DATUM_LEN {
        result.push_str(" len(");
        result.push_str(&original_len.to_string());
        result.push(')');
    }
}

/// 按表定义与会话选项构造 `TableKVDecoder`。
pub fn NewTableKVDecoder(
    tbl: TableDefinition,
    tableName: &str,
    options: &SessionOptions,
) -> Result<TableKVDecoder, String> {
    let se = NewSession(options)?;
    let genCols = CollectGeneratedColumnsFromTable(&tbl);
    Ok(TableKVDecoder {
        tbl,
        se,
        tableName: tableName.into(),
        genCols,
    })
}

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

// 行解码器（RowDecoder）：将编码行字节解码为 Datum，填充默认值并评估生成列。
//
// 对应 Go `pkg/util/rowDecoder`。解码流程大致为：解析行 payload → 把 handle
// （行主键，可能是整数或 common handle）写入列映射 → 按列补默认值/列变更来源 →
// 按 offset 顺序求值生成列（generated column）。同时提供简化的 EncodeRow
// 便于单测构造编码行。

use std::collections::HashMap;
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::sync::Arc;

/// 额外 handle 列 ID，表示整型 handle 未映射到真实列时的占位列。
pub const ExtraHandleID: i64 = -1;
/// 新行格式首字节标记（0x80），与 rowcodec CodecVer 一致。
const NEW_ROW_FORMAT_MARKER: u8 = 0x80;

/// 行解码过程中可能出现的错误类别。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DecodeError {
    /// 编码行字节损坏或格式不符。
    InvalidRow(String),
    /// handle 与目标 PK 列不匹配。
    InvalidHandle(String),
    /// Datum 到目标 FieldType 的强制转换失败。
    Cast(String),
    /// 生成列表达式求值失败。
    Eval(String),
}

impl Display for DecodeError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRow(message) => write!(formatter, "invalid encoded row: {message}"),
            Self::InvalidHandle(message) => write!(formatter, "invalid row handle: {message}"),
            Self::Cast(message) => write!(formatter, "column cast failed: {message}"),
            Self::Eval(message) => write!(formatter, "generated expression failed: {message}"),
        }
    }
}

impl Error for DecodeError {}

/// 简化 Datum 模型，覆盖本解码器单测所需类型。
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Datum {
    #[default]
    Null,
    Int(i64),
    UInt(u64),
    Float(f64),
    Bytes(Vec<u8>),
    String(String),
    Bool(bool),
}

/// 列值期望的 Field 类型种类，用于 cast。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum FieldKind {
    #[default]
    Any,
    Int,
    UInt,
    Float,
    Bytes,
    String,
    Bool,
}

/// 列类型描述（简化版 FieldType）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FieldType {
    pub kind: FieldKind,
}

/// 列元信息：id、在行内的 offset、以及类型。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ColumnInfo {
    pub id: i64,
    pub offset: usize,
    pub field_type: FieldType,
}

/// DDL 列变更时的来源列映射信息。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ChangeStateInfo {
    pub source_column_id: i64,
}

/// 表列定义：元信息、默认值与可选的变更来源。
#[derive(Clone, Debug)]
pub struct TableColumn {
    pub column_info: ColumnInfo,
    pub default_value: Datum,
    pub change_state_info: Option<ChangeStateInfo>,
}

/// 生成列表达式：根据 BuildContext 与当前行求值。
pub trait Expression: Send + Sync {
    fn eval(&self, context: &BuildContext, row: &[Datum]) -> Result<Datum, DecodeError>;
}

impl<F> Expression for F
where
    F: Fn(&BuildContext, &[Datum]) -> Result<Datum, DecodeError> + Send + Sync,
{
    fn eval(&self, context: &BuildContext, row: &[Datum]) -> Result<Datum, DecodeError> {
        self(context, row)
    }
}

/// 待解码列：表列定义加上可选的生成表达式。
#[derive(Clone)]
pub struct Column {
    pub col: Arc<TableColumn>,
    pub gen_expr: Option<Arc<dyn Expression>>,
}

/// 表元数据中与 handle 相关的字段。
#[derive(Clone, Debug, Default)]
pub struct TableMeta {
    /// 是否使用聚集索引（common handle）。
    pub is_common_handle: bool,
    /// 是否以整数列作为 PK handle。
    pub pk_is_handle: bool,
    pub pk_column_id: Option<i64>,
    pub common_pk_column_ids: Vec<i64>,
}

/// 表对象，当前仅携带 meta。
#[derive(Clone, Debug, Default)]
pub struct Table {
    pub meta: TableMeta,
}

/// Schema 中单列，可挂虚拟生成表达式。
#[derive(Clone, Default)]
pub struct SchemaColumn {
    pub virtual_expr: Option<Arc<dyn Expression>>,
}

/// 表 Schema：按 offset 对齐的列列表。
#[derive(Clone, Default)]
pub struct Schema {
    pub columns: Vec<SchemaColumn>,
}

/// 解码/求值上下文（如时区）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BuildContext {
    pub time_zone: String,
}

/// 行 handle：整数 handle 或 common handle 的多列 Datum。
#[derive(Clone, Debug, PartialEq)]
pub enum Handle {
    Int(i64),
    Common(Vec<Datum>),
}

/// Decodes row bytes, fills defaults and evaluates generated columns in offset order.
/// 解码行字节，按 offset 填充默认值并评估生成列。
pub struct RowDecoder {
    table: Table,
    mutable_row: Vec<Datum>,
    column_map: HashMap<i64, Column>,
    column_types: HashMap<i64, FieldType>,
    default_values: Vec<Datum>,
    columns: Vec<Arc<TableColumn>>,
    pk_columns: Vec<i64>,
}

/// 构造 RowDecoder：推导列类型、行宽与 PK 列集合。
pub fn NewRowDecoder(
    table: Table,
    columns: Vec<Arc<TableColumn>>,
    decode_column_map: HashMap<i64, Column>,
) -> RowDecoder {
    let column_types = decode_column_map
        .iter()
        .map(|(id, column)| (*id, column.col.column_info.field_type.clone()))
        .collect();
    // 行宽取最大 offset+1，保证 mutable_row 能覆盖所有列槽位。
    let row_len = columns
        .iter()
        .map(|column| column.column_info.offset)
        .max()
        .map_or(0, |offset| offset + 1);
    // common handle / 整数 PK / 隐式 ExtraHandle 三选一。
    let pk_columns = if table.meta.is_common_handle {
        table.meta.common_pk_column_ids.clone()
    } else if table.meta.pk_is_handle {
        table.meta.pk_column_id.into_iter().collect()
    } else {
        vec![ExtraHandleID]
    };
    RowDecoder {
        table,
        mutable_row: vec![Datum::Null; row_len],
        column_map: decode_column_map,
        column_types,
        default_values: vec![Datum::Null; row_len],
        columns,
        pk_columns,
    }
}

impl RowDecoder {
    /// 解码行并评估剩余生成列，返回完整列 ID → Datum 映射。
    pub fn DecodeAndEvalRowWithMap(
        &mut self,
        context: &BuildContext,
        handle: &Handle,
        encoded_row: &[u8],
        mut row: HashMap<i64, Datum>,
    ) -> Result<HashMap<i64, Datum>, DecodeError> {
        decode_row_with_map(encoded_row, &self.column_types, &mut row)?;
        decode_handle_to_datum_map(handle, &self.pk_columns, &self.column_types, &mut row)?;

        let decoded_columns = self.column_map.values().cloned().collect::<Vec<_>>();
        for decoded_column in &decoded_columns {
            let info = &decoded_column.col.column_info;
            if let Some(value) = row.get(&info.id).cloned() {
                self.set_row_value(info.offset, value);
                continue;
            }
            // 生成列稍后求值，先占位 Null。
            if decoded_column.gen_expr.is_some() {
                self.set_row_value(info.offset, Datum::Null);
                continue;
            }
            // 列变更：从源列取值并 cast；否则用默认值。
            let value = if let Some(change) = &decoded_column.col.change_state_info {
                let source = row
                    .get(&change.source_column_id)
                    .cloned()
                    .unwrap_or_else(|| decoded_column.col.default_value.clone());
                cast_column_value(source, &info.field_type)?
            } else {
                self.column_default(decoded_column.col.as_ref())
            };
            self.set_row_value(info.offset, value);
        }
        self.EvalRemainedExprColumnMap(context, row)
    }

    /// 返回当前可变行快照（含默认值填充结果）。
    pub fn CurrentRowWithDefaultVal(&self) -> Vec<Datum> {
        self.mutable_row.clone()
    }

    /// 只解码已存在列并填默认值，不评估生成列/列变更表达式。
    pub fn DecodeTheExistedColumnMap(
        &mut self,
        _context: &BuildContext,
        handle: &Handle,
        encoded_row: &[u8],
        mut row: HashMap<i64, Datum>,
    ) -> Result<HashMap<i64, Datum>, DecodeError> {
        decode_row_with_map(encoded_row, &self.column_types, &mut row)?;
        decode_handle_to_datum_map(handle, &self.pk_columns, &self.column_types, &mut row)?;

        let decoded_columns = self.column_map.values().cloned().collect::<Vec<_>>();
        for decoded_column in &decoded_columns {
            let info = &decoded_column.col.column_info;
            if let Some(value) = row.get(&info.id).cloned() {
                self.set_row_value(info.offset, value);
                continue;
            }
            if decoded_column.gen_expr.is_some() || decoded_column.col.change_state_info.is_some() {
                self.set_row_value(info.offset, Datum::Null);
                continue;
            }
            let value = self.column_default(decoded_column.col.as_ref());
            row.insert(info.id, value.clone());
            self.set_row_value(info.offset, value);
        }
        Ok(row)
    }

    /// 按 offset 升序评估所有带 gen_expr 的列，并写回 row map。
    pub fn EvalRemainedExprColumnMap(
        &mut self,
        context: &BuildContext,
        mut row: HashMap<i64, Datum>,
    ) -> Result<HashMap<i64, Datum>, DecodeError> {
        let mut ordered = self
            .column_map
            .iter()
            .map(|(id, column)| (column.col.column_info.offset, *id, column.clone()))
            .collect::<Vec<_>>();
        ordered.sort_by_key(|entry| entry.0);
        for (offset, column_id, column) in ordered {
            let Some(expression) = &column.gen_expr else {
                continue;
            };
            let value = expression.eval(context, &self.mutable_row)?;
            let value = cast_column_value(value, &column.col.column_info.field_type)?;
            self.set_row_value(offset, value.clone());
            row.insert(column_id, value);
        }
        Ok(row)
    }

    /// 懒加载并返回列默认值（缓存在 default_values）。
    fn column_default(&mut self, column: &TableColumn) -> Datum {
        let offset = column.column_info.offset;
        if self.default_values.get(offset) == Some(&Datum::Null) {
            self.default_values[offset] = column.default_value.clone();
        }
        self.default_values[offset].clone()
    }

    /// 写入 mutable_row 指定 offset，必要时扩容。
    fn set_row_value(&mut self, offset: usize, value: Datum) {
        if self.mutable_row.len() <= offset {
            self.mutable_row.resize(offset + 1, Datum::Null);
            self.default_values.resize(offset + 1, Datum::Null);
        }
        self.mutable_row[offset] = value;
    }

    /// 返回关联表引用。
    pub fn table(&self) -> &Table {
        &self.table
    }

    /// 返回解码列定义切片。
    pub fn columns(&self) -> &[Arc<TableColumn>] {
        &self.columns
    }
}

/// 根据 Schema 中的虚拟表达式构建完整解码列映射。
pub fn BuildFullDecodeColMap(
    columns: &[Arc<TableColumn>],
    schema: &Schema,
) -> HashMap<i64, Column> {
    columns
        .iter()
        .map(|column| {
            let expression = schema.columns[column.column_info.offset]
                .virtual_expr
                .clone();
            (
                column.column_info.id,
                Column {
                    col: Arc::clone(column),
                    gen_expr: expression,
                },
            )
        })
        .collect()
}

/// 解析编码行，仅将 column_types 中存在的列写入 row map。
fn decode_row_with_map(
    encoded: &[u8],
    column_types: &HashMap<i64, FieldType>,
    row: &mut HashMap<i64, Datum>,
) -> Result<(), DecodeError> {
    // 新格式：0x80 + 2 字节大端列数 + 列序列。
    let (mut input, expected_columns) = if encoded.first() == Some(&NEW_ROW_FORMAT_MARKER) {
        if encoded.len() < 3 {
            return Err(DecodeError::InvalidRow(
                "new row header is truncated".to_owned(),
            ));
        }
        (
            &encoded[3..],
            Some(u16::from_be_bytes([encoded[1], encoded[2]]) as usize),
        )
    } else {
        (encoded, None)
    };
    let mut decoded_columns = 0;
    while !input.is_empty() {
        let id_bytes: [u8; 8] = input
            .get(..8)
            .ok_or_else(|| DecodeError::InvalidRow("column id is truncated".to_owned()))?
            .try_into()
            .expect("length checked");
        let column_id = i64::from_be_bytes(id_bytes);
        input = &input[8..];
        let (value, consumed) = decode_datum(input)?;
        input = &input[consumed..];
        if let Some(field_type) = column_types.get(&column_id) {
            row.insert(column_id, cast_column_value(value, field_type)?);
        }
        decoded_columns += 1;
    }
    if expected_columns.is_some_and(|expected| expected != decoded_columns) {
        return Err(DecodeError::InvalidRow(format!(
            "expected {} columns, decoded {decoded_columns}",
            expected_columns.expect("checked")
        )));
    }
    Ok(())
}

/// 将 handle 写入 row map：整数 handle 按列类型选 Int/UInt，common handle 按列逐个 cast。
fn decode_handle_to_datum_map(
    handle: &Handle,
    pk_columns: &[i64],
    column_types: &HashMap<i64, FieldType>,
    row: &mut HashMap<i64, Datum>,
) -> Result<(), DecodeError> {
    match handle {
        Handle::Int(value) => {
            let Some(column_id) = pk_columns.first() else {
                return Err(DecodeError::InvalidHandle(
                    "integer handle has no target column".to_owned(),
                ));
            };
            let datum = if column_types
                .get(column_id)
                .is_some_and(|field_type| field_type.kind == FieldKind::UInt)
            {
                Datum::UInt(*value as u64)
            } else {
                Datum::Int(*value)
            };
            row.insert(*column_id, datum);
        }
        Handle::Common(values) => {
            if values.len() != pk_columns.len() {
                return Err(DecodeError::InvalidHandle(format!(
                    "common handle has {} values for {} columns",
                    values.len(),
                    pk_columns.len()
                )));
            }
            for (column_id, value) in pk_columns.iter().zip(values) {
                let value = match column_types.get(column_id) {
                    Some(field_type) => cast_column_value(value.clone(), field_type)?,
                    None => value.clone(),
                };
                row.insert(*column_id, value);
            }
        }
    }
    Ok(())
}

/// 按目标 FieldKind 强制转换 Datum。
fn cast_column_value(value: Datum, field_type: &FieldType) -> Result<Datum, DecodeError> {
    match field_type.kind {
        FieldKind::Any => Ok(value),
        FieldKind::Int => match value {
            Datum::Int(value) => Ok(Datum::Int(value)),
            Datum::UInt(value) => Ok(Datum::Int(value as i64)),
            Datum::Float(value) => Ok(Datum::Int(value as i64)),
            Datum::String(value) => value
                .parse::<i64>()
                .map(Datum::Int)
                .map_err(|_| DecodeError::Cast(value)),
            Datum::Bool(value) => Ok(Datum::Int(i64::from(value))),
            Datum::Null => Ok(Datum::Null),
            Datum::Bytes(value) => String::from_utf8(value)
                .map_err(|error| DecodeError::Cast(error.to_string()))
                .and_then(|value| {
                    value
                        .parse::<i64>()
                        .map(Datum::Int)
                        .map_err(|_| DecodeError::Cast(value))
                }),
        },
        FieldKind::UInt => match cast_column_value(
            value,
            &FieldType {
                kind: FieldKind::Int,
            },
        )? {
            Datum::Int(value) => Ok(Datum::UInt(value as u64)),
            Datum::Null => Ok(Datum::Null),
            _ => unreachable!(),
        },
        FieldKind::Float => match value {
            Datum::Float(value) => Ok(Datum::Float(value)),
            Datum::Int(value) => Ok(Datum::Float(value as f64)),
            Datum::UInt(value) => Ok(Datum::Float(value as f64)),
            Datum::String(value) => value
                .parse::<f64>()
                .map(Datum::Float)
                .map_err(|_| DecodeError::Cast(value)),
            Datum::Null => Ok(Datum::Null),
            value => Err(DecodeError::Cast(format!("{value:?}"))),
        },
        FieldKind::Bytes => match value {
            Datum::Bytes(value) => Ok(Datum::Bytes(value)),
            Datum::String(value) => Ok(Datum::Bytes(value.into_bytes())),
            Datum::Null => Ok(Datum::Null),
            value => Ok(Datum::Bytes(format!("{value:?}").into_bytes())),
        },
        FieldKind::String => match value {
            Datum::String(value) => Ok(Datum::String(value)),
            Datum::Bytes(value) => String::from_utf8(value)
                .map(Datum::String)
                .map_err(|error| DecodeError::Cast(error.to_string())),
            Datum::Null => Ok(Datum::Null),
            value => Ok(Datum::String(format!("{value:?}"))),
        },
        FieldKind::Bool => match value {
            Datum::Bool(value) => Ok(Datum::Bool(value)),
            Datum::Int(value) => Ok(Datum::Bool(value != 0)),
            Datum::UInt(value) => Ok(Datum::Bool(value != 0)),
            Datum::Null => Ok(Datum::Null),
            value => Err(DecodeError::Cast(format!("{value:?}"))),
        },
    }
}

/// 按 tag 解析单个 Datum，返回 (值, 消耗字节数)。
fn decode_datum(input: &[u8]) -> Result<(Datum, usize), DecodeError> {
    let Some(tag) = input.first().copied() else {
        return Err(DecodeError::InvalidRow("datum tag is missing".to_owned()));
    };
    match tag {
        0 => Ok((Datum::Null, 1)),
        1 | 2 | 3 => {
            let bytes: [u8; 8] = input
                .get(1..9)
                .ok_or_else(|| DecodeError::InvalidRow("numeric datum is truncated".to_owned()))?
                .try_into()
                .expect("length checked");
            let datum = match tag {
                1 => Datum::Int(i64::from_be_bytes(bytes)),
                2 => Datum::UInt(u64::from_be_bytes(bytes)),
                _ => Datum::Float(f64::from_bits(u64::from_be_bytes(bytes))),
            };
            Ok((datum, 9))
        }
        4 | 5 => {
            let length_bytes: [u8; 4] = input
                .get(1..5)
                .ok_or_else(|| DecodeError::InvalidRow("datum length is truncated".to_owned()))?
                .try_into()
                .expect("length checked");
            let length = u32::from_be_bytes(length_bytes) as usize;
            let bytes = input
                .get(5..5 + length)
                .ok_or_else(|| DecodeError::InvalidRow("datum bytes are truncated".to_owned()))?;
            let datum = if tag == 4 {
                Datum::Bytes(bytes.to_vec())
            } else {
                Datum::String(
                    String::from_utf8(bytes.to_vec())
                        .map_err(|error| DecodeError::InvalidRow(error.to_string()))?,
                )
            };
            Ok((datum, 5 + length))
        }
        6 => match input.get(1) {
            Some(0) => Ok((Datum::Bool(false), 2)),
            Some(1) => Ok((Datum::Bool(true), 2)),
            _ => Err(DecodeError::InvalidRow("invalid boolean datum".to_owned())),
        },
        _ => Err(DecodeError::InvalidRow(format!("unknown datum tag {tag}"))),
    }
}

/// 将列 ID 与 Datum 列表编码为行字节；`new_format` 时写入 0x80 头与列数。
pub fn EncodeRow(values: &[(i64, Datum)], new_format: bool) -> Result<Vec<u8>, DecodeError> {
    let mut output = Vec::new();
    if new_format {
        output.push(NEW_ROW_FORMAT_MARKER);
        let count = u16::try_from(values.len())
            .map_err(|_| DecodeError::InvalidRow("too many columns".to_owned()))?;
        output.extend_from_slice(&count.to_be_bytes());
    }
    for (column_id, value) in values {
        output.extend_from_slice(&column_id.to_be_bytes());
        encode_datum(value, &mut output)?;
    }
    Ok(output)
}

/// 将单个 Datum 追加到输出缓冲。
fn encode_datum(value: &Datum, output: &mut Vec<u8>) -> Result<(), DecodeError> {
    match value {
        Datum::Null => output.push(0),
        Datum::Int(value) => {
            output.push(1);
            output.extend_from_slice(&value.to_be_bytes());
        }
        Datum::UInt(value) => {
            output.push(2);
            output.extend_from_slice(&value.to_be_bytes());
        }
        Datum::Float(value) => {
            output.push(3);
            output.extend_from_slice(&value.to_bits().to_be_bytes());
        }
        Datum::Bytes(value) => encode_bytes_datum(4, value, output)?,
        Datum::String(value) => encode_bytes_datum(5, value.as_bytes(), output)?,
        Datum::Bool(value) => output.extend_from_slice(&[6, u8::from(*value)]),
    }
    Ok(())
}

/// 编码变长字节/字符串：tag + 4 字节大端长度 + 内容。
fn encode_bytes_datum(tag: u8, value: &[u8], output: &mut Vec<u8>) -> Result<(), DecodeError> {
    let length = u32::try_from(value.len())
        .map_err(|_| DecodeError::InvalidRow("datum is too large".to_owned()))?;
    output.push(tag);
    output.extend_from_slice(&length.to_be_bytes());
    output.extend_from_slice(value);
    Ok(())
}

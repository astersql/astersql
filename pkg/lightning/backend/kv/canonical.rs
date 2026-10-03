// Copyright 2026 AsterSQL.

//! Lightning 导入侧数据模型与 TiDB canonical 编解码模型之间的适配层。
//!
//! 负责 Datum 双向转换、列类型与表/索引元数据映射、行编解码，以及根据表定义构造
//! 整数或 Common Handle，确保 Lightning 写入的 KV 能被 TiDB tablecodec 正确识别。

use std::collections::HashMap;

use encode::{Column, ColumnType, Datum};
use tablecodec::{codec, kv, model, mysql, rowcodec, types};

fn shared_error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

/// 将 Lightning Datum 转为 tablecodec 使用的 canonical Datum。
///
/// JSON、十进制、时间和时长在此完成语法解析；解析失败会阻止生成不合法的 KV。
pub fn toCanonicalDatum(value: &Datum) -> Result<types::Datum, String> {
    let mut output = types::Datum::default();
    match value {
        Datum::Null => {}
        Datum::MinNotNull => output.SetMinNotNull(),
        Datum::MaxValue => return Ok(types::MaxValueDatum()),
        Datum::Int(value) => output.SetInt64(*value),
        Datum::UInt(value) => output.SetUint64(*value),
        Datum::Float(value) => output.SetFloat64(*value),
        Datum::Bytes(value) => output.SetBytes(value.clone()),
        Datum::String(value) => output.SetString(
            value.clone(),
            types::mysql::DefaultCollationName.to_string(),
        ),
        Datum::Json(value) => {
            output.SetMysqlJSON(types::ParseBinaryJSONFromString(value).map_err(shared_error)?)
        }
        Datum::BinaryLiteral(value) => output.SetBinaryLiteral(types::BinaryLiteral(value.clone())),
        Datum::Bit(value) => output.SetMysqlBit(types::BinaryLiteral(value.clone())),
        Datum::Enum { name, value } => output.SetMysqlEnum(
            types::Enum {
                Name: name.clone(),
                Value: *value,
            },
            types::mysql::DefaultCollationName.to_string(),
        ),
        Datum::Set { name, value } => output.SetMysqlSet(
            types::Set {
                Name: name.clone(),
                Value: *value,
            },
            types::mysql::DefaultCollationName.to_string(),
        ),
        Datum::Decimal(value) => {
            let mut decimal = types::MyDecimal::default();
            decimal.FromString(value.as_bytes()).map_err(shared_error)?;
            output.SetMysqlDecimal(decimal);
        }
        Datum::Timestamp(value) => {
            let context = &*types::DefaultStmtNoWarningContext;
            let parsed = types::ParseTime(context, value, mysql::TypeTimestamp, types::MaxFsp)
                .map_err(shared_error)?;
            output.SetMysqlTime(parsed);
        }
        Datum::Duration(value) => {
            let context = &*types::DefaultStmtNoWarningContext;
            let (parsed, is_null) =
                types::ParseDuration(context, value, types::MaxFsp).map_err(shared_error)?;
            if !is_null {
                output.SetMysqlDuration(parsed);
            }
        }
    }
    Ok(output)
}

fn enumName(elements: &[String], value: u64) -> String {
    // ENUM 的数值从 1 开始；0 或越界值没有可用名称。
    value
        .checked_sub(1)
        .and_then(|index| elements.get(index as usize))
        .cloned()
        .unwrap_or_default()
}

fn setName(elements: &[String], value: u64) -> String {
    // SET 用位图保存成员，输出名称顺序必须与列定义顺序一致。
    elements
        .iter()
        .enumerate()
        .filter(|(index, _)| value & (1_u64 << index) != 0)
        .map(|(_, name)| name.as_str())
        .collect::<Vec<_>>()
        .join(",")
}

/// 将 canonical Datum 还原为 Lightning Datum。
///
/// canonical Kind 不能完全区分字符串、字节串、BIT、ENUM 等类型，因此优先使用列元数据
/// 消除歧义；没有列元数据时保持 canonical 值表达的默认类型。
pub fn fromCanonicalDatum(value: &types::Datum, column: Option<&Column>) -> Result<Datum, String> {
    let column_type = column.map_or(ColumnType::Auto, |column| column.column_type);
    Ok(match value.Kind() {
        types::KindNull => Datum::Null,
        types::KindMinNotNull => Datum::MinNotNull,
        types::KindMaxValue => Datum::MaxValue,
        types::KindInt64 => match column_type {
            ColumnType::Duration => Datum::Duration(
                types::Duration {
                    Duration: value.GetInt64(),
                    Fsp: types::MaxFsp,
                }
                .String(),
            ),
            _ => Datum::Int(value.GetInt64()),
        },
        types::KindUint64 => match column_type {
            ColumnType::Enum => {
                let number = value.GetUint64();
                Datum::Enum {
                    name: enumName(column.map_or(&[], |column| &column.elements), number),
                    value: number,
                }
            }
            ColumnType::Set => {
                let number = value.GetUint64();
                Datum::Set {
                    name: setName(column.map_or(&[], |column| &column.elements), number),
                    value: number,
                }
            }
            ColumnType::Bit | ColumnType::BinaryLiteral => {
                let number = value.GetUint64();
                let bytes = number.to_be_bytes();
                // 去掉整数编码产生的前导零，但为数值 0 保留一个字节。
                let first = bytes.iter().position(|byte| *byte != 0).unwrap_or(7);
                if column_type == ColumnType::Bit {
                    Datum::Bit(bytes[first..].to_vec())
                } else {
                    Datum::BinaryLiteral(bytes[first..].to_vec())
                }
            }
            _ => Datum::UInt(value.GetUint64()),
        },
        types::KindFloat32 | types::KindFloat64 => Datum::Float(value.GetFloat64()),
        types::KindString => match column_type {
            ColumnType::Bytes => Datum::Bytes(value.GetBytes()),
            ColumnType::BinaryLiteral => Datum::BinaryLiteral(value.GetBytes()),
            ColumnType::Bit => Datum::Bit(value.GetBytes()),
            _ => Datum::String(value.GetString()),
        },
        types::KindBytes => {
            let bytes = value.GetBytes();
            if matches!(column_type, ColumnType::String | ColumnType::Timestamp)
                || (column_type == ColumnType::Auto
                    && column.is_some_and(|column| column.charset != "binary"))
            {
                let text = String::from_utf8(bytes).map_err(shared_error)?;
                if column_type == ColumnType::Timestamp {
                    Datum::Timestamp(text)
                } else {
                    Datum::String(text)
                }
            } else {
                Datum::Bytes(bytes)
            }
        }
        types::KindBinaryLiteral => Datum::BinaryLiteral(value.GetBinaryLiteral().0),
        types::KindMysqlBit => {
            if column_type == ColumnType::BinaryLiteral {
                Datum::BinaryLiteral(value.GetMysqlBit().0)
            } else {
                Datum::Bit(value.GetMysqlBit().0)
            }
        }
        types::KindMysqlDecimal => Datum::Decimal(
            String::from_utf8_lossy(&value.GetMysqlDecimal().ToString()).into_owned(),
        ),
        types::KindMysqlDuration => Datum::Duration(value.GetMysqlDuration().String()),
        types::KindMysqlEnum => {
            let value = value.GetMysqlEnum();
            Datum::Enum {
                name: value.Name,
                value: value.Value,
            }
        }
        types::KindMysqlSet => {
            let value = value.GetMysqlSet();
            Datum::Set {
                name: value.Name,
                value: value.Value,
            }
        }
        types::KindMysqlTime => Datum::Timestamp(value.GetMysqlTime().String()),
        types::KindMysqlJSON => Datum::Json(value.GetMysqlJSON().String()),
        kind => return Err(format!("unsupported canonical datum kind {kind}")),
    })
}

/// 将 Lightning 列定义映射为 tablecodec 解码所需的字段类型。
pub(crate) fn fieldType(column: &Column) -> types::FieldType {
    let mysql_type = match column.column_type {
        ColumnType::Auto => mysql::TypeUnspecified,
        ColumnType::Int | ColumnType::UInt => mysql::TypeLonglong,
        ColumnType::Float => mysql::TypeDouble,
        ColumnType::Bytes => mysql::TypeBlob,
        ColumnType::String => mysql::TypeVarString,
        ColumnType::Json => mysql::TypeJSON,
        ColumnType::BinaryLiteral | ColumnType::Bit => mysql::TypeBit,
        ColumnType::Enum => mysql::TypeEnum,
        ColumnType::Set => mysql::TypeSet,
        ColumnType::Decimal => mysql::TypeNewDecimal,
        ColumnType::Timestamp => mysql::TypeTimestamp,
        ColumnType::Duration => mysql::TypeDuration,
    };
    let mut field_type = types::NewFieldType(mysql_type);
    if column.column_type == ColumnType::UInt {
        field_type.AddFlag(mysql::UnsignedFlag);
    }
    if matches!(
        column.column_type,
        ColumnType::Bit | ColumnType::BinaryLiteral
    ) {
        // 轻量 Column 模型尚无显示宽度字段，默认一字节可避免生成无效的零位 BIT。
        field_type.SetFlen(8);
    }
    if !column.charset.is_empty() {
        field_type.SetCharset(column.charset.clone());
    }
    if !column.elements.is_empty() {
        field_type.SetElems(column.elements.clone());
    }
    *field_type
}

/// Encode the stored columns using their persistent IDs. Integer primary keys
/// live in the row handle, as in table.AddRecord, rather than in the row value.
pub(crate) fn encodeCanonicalRowWithMeta(
    datums: &[Datum],
    meta: &model::TableInfo,
    new_format: bool,
) -> Result<Vec<u8>, String> {
    let mut values = Vec::new();
    let mut column_ids = Vec::new();
    for column in &meta.Columns {
        if meta.PKIsHandle && mysql::HasPriKeyFlag(column.FieldType.GetFlag()) {
            continue;
        }
        if !column.GeneratedExprString.is_empty() && !column.GeneratedStored {
            continue;
        }
        values.push(toCanonicalDatum(&datums[column.Offset as usize])?);
        column_ids.push(column.ID);
    }
    tablecodec::EncodeRow(
        Some(tablecodec::time::UTC),
        values,
        column_ids,
        Vec::new(),
        None,
        None,
        rowcodec::Encoder::new(new_format),
    )
    .map_err(shared_error)
}

/// 使用连续的 1-based 列 ID 和 UTC 时区编码一行，并按 `new_format` 选择行格式。
pub(crate) fn encodeCanonicalRow(
    datums: &[Datum],
    columns: &[Column],
    new_format: bool,
) -> Result<Vec<u8>, String> {
    let values = datums
        .iter()
        .map(toCanonicalDatum)
        .collect::<Result<Vec<_>, _>>()?;
    let column_ids = (1..=datums.len()).map(|id| id as i64).collect();
    tablecodec::EncodeRow(
        Some(tablecodec::time::UTC),
        values,
        column_ids,
        Vec::new(),
        None,
        None,
        rowcodec::Encoder::new(new_format),
    )
    .map_err(shared_error)
}

/// 按列定义解码 canonical 行，并将编码中缺失的列补为 NULL。
pub(crate) fn decodeCanonicalRow(input: &[u8], columns: &[Column]) -> Result<Vec<Datum>, String> {
    let field_types = columns
        .iter()
        .enumerate()
        .map(|(index, column)| (index as i64 + 1, Box::new(fieldType(column))))
        .collect::<HashMap<_, _>>();
    let decoded = tablecodec::DecodeRowToDatumMap(
        Some(input.to_vec()),
        field_types,
        Some(tablecodec::time::UTC),
    )
    .map_err(shared_error)?;
    columns
        .iter()
        .enumerate()
        .map(|(index, column)| {
            decoded
                .get(&(index as i64 + 1))
                .map_or(Ok(Datum::Null), |value| {
                    fromCanonicalDatum(value, Some(column))
                })
        })
        .collect()
}

/// 构造 tablecodec 所需的最小表元数据，列 ID 按输入顺序从 1 开始。
pub(crate) fn canonicalTableInfo(
    columns: &[Column],
    indices: &[crate::IndexDefinition],
    pk_is_handle: bool,
    common_handle: bool,
) -> model::TableInfo {
    let model_columns = columns
        .iter()
        .enumerate()
        .map(|(index, column)| {
            let mut info = model::ColumnInfo::default();
            info.ID = index as i64 + 1;
            info.Offset = index as isize;
            info.FieldType = fieldType(column);
            info
        })
        .collect();
    let model_indices = indices
        .iter()
        .map(|index| canonicalIndexInfo(index))
        .collect();
    model::TableInfo {
        Columns: model_columns,
        Indices: model_indices,
        PKIsHandle: pk_is_handle,
        IsCommonHandle: common_handle,
        CommonHandleVersion: 0,
        ..Default::default()
    }
}

/// 将 Lightning 索引定义映射为 canonical 索引元数据。
pub(crate) fn canonicalIndexInfo(index: &crate::IndexDefinition) -> model::IndexInfo {
    model::IndexInfo {
        ID: index.id,
        Columns: index
            .columns
            .iter()
            .map(|offset| model::IndexColumn {
                Offset: *offset as isize,
                Length: types::UnspecifiedLength as isize,
                ..Default::default()
            })
            .collect(),
        Unique: index.unique,
        Primary: index.primary,
        ..Default::default()
    }
}

/// 根据表的主键模式构造记录 Handle。
///
/// Common Handle 编码所有主键列；整型主键取首列；其余情况使用分配得到的 `row_id`。
pub(crate) fn canonicalHandle(
    table: &crate::TableDefinition,
    record: &[Datum],
    row_id: i64,
) -> Result<Box<dyn kv::Handle>, String> {
    if table.common_handle {
        let mut values = Vec::new();
        for (index, column) in table
            .columns
            .iter()
            .enumerate()
            .filter(|(_, column)| column.primary_key)
        {
            let value = record
                .get(index)
                .ok_or_else(|| format!("missing primary-key column {}", column.name))?;
            values.push(toCanonicalDatum(value)?);
        }
        let encoded = codec::NewEncoder(false)
            .EncodeKey(tablecodec::time::UTC, Vec::new(), values)
            .map_err(shared_error)?;
        return kv::NewCommonHandle(encoded)
            .map(|handle| Box::new(handle) as Box<dyn kv::Handle>)
            .map_err(shared_error);
    }
    let handle = if table.pk_is_handle {
        let primary_key_index = table
            .columns
            .iter()
            .position(|column| column.primary_key)
            .ok_or_else(|| "pk_is_handle table has no primary-key column".to_string())?;
        match record.get(primary_key_index) {
            Some(Datum::Int(value)) => *value,
            Some(Datum::UInt(value)) => *value as i64,
            Some(value) => {
                return Err(format!(
                    "integer primary-key column requires an integer datum, got {value:?}"
                ));
            }
            None => {
                return Err(format!(
                    "missing primary-key column {}",
                    table.columns[primary_key_index].name
                ));
            }
        }
    } else {
        row_id
    };
    Ok(Box::new(kv::IntHandle(handle)))
}

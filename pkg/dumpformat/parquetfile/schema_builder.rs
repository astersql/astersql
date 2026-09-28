// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

use crate::column_type::{Column, ColumnInfo, LogicalType, PhysicalType, TimeUnit, to_column_type};
use crate::{Error, Result};
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 字段重复度：Required 或 Optional（可空）。
pub enum Repetition {
    /// 不允许空值。
    Required,
    /// 允许 definition level 0（NULL）。
    Optional,
}
#[derive(Clone, Debug, Eq, PartialEq)]
/// Parquet 叶子字段节点（简化版 schema.Node）。
pub struct PrimitiveNode {
    pub name: String,
    pub repetition: Repetition,
    pub physical: PhysicalType,
    pub logical: LogicalType,
    pub type_length: i32,
}
#[derive(Clone, Debug, Eq, PartialEq)]
/// 根名为 "schema" 的字段列表。
pub struct Schema {
    pub name: String,
    pub fields: Vec<PrimitiveNode>,
}
/// 预检列名非空；DECIMAL 的 scale 必须在 [0, precision]。
pub fn validate_column_info(info: &ColumnInfo) -> Result<()> {
    if info.name.is_empty() {
        return Err(Error("parquet column name is empty".into()));
    }
    if info
        .database_type_name
        .trim()
        .eq_ignore_ascii_case("DECIMAL")
        && info.precision > 0
        && info.precision <= 38
        && (info.scale < 0 || info.scale > info.precision)
    {
        return Err(Error(format!(
            "parquet decimal column {} has invalid scale {} for precision {}",
            info.name, info.scale, info.precision
        )));
    }
    Ok(())
}
/// 校验并映射每列，组装 Schema 与解析后的 Column 列表。
pub fn build_parquet_schema_from_columns(infos: &[ColumnInfo]) -> Result<(Schema, Vec<Column>)> {
    let mut fields = Vec::with_capacity(infos.len());
    let mut columns = Vec::with_capacity(infos.len());
    for info in infos {
        validate_column_info(info)?;
        let kind = to_column_type(info);
        // TIMESTAMP 可能有非法 MySQL 值，写入路径会编码为 NULL，故强制允许空值。
        let allows = info.nullable || matches!(kind.logical, LogicalType::Timestamp { .. });
        let column = Column {
            info: info.clone(),
            timestamp_unit: match kind.logical {
                LogicalType::Timestamp { unit, .. } | LogicalType::Time { unit, .. } => unit,
                _ => TimeUnit::Micros,
            },
            column_type: kind.clone(),
            allows_null_encoding: allows,
        };
        fields.push(new_primitive_node(&column)?);
        columns.push(column);
    }
    Ok((
        Schema {
            name: "schema".into(),
            fields,
        },
        columns,
    ))
}
/// 由 Column 构造叶子节点；定长类型要求 type_length > 0。
pub fn new_primitive_node(column: &Column) -> Result<PrimitiveNode> {
    if column.column_type.physical == PhysicalType::FixedLenByteArray
        && column.column_type.type_length <= 0
    {
        return Err(Error(format!(
            "invalid fixed-size byte width {}",
            column.column_type.type_length
        )));
    }
    Ok(PrimitiveNode {
        name: column.info.name.clone(),
        repetition: if column.allows_null_encoding {
            Repetition::Optional
        } else {
            Repetition::Required
        },
        physical: column.column_type.physical,
        logical: column.column_type.logical.clone(),
        type_length: column.column_type.type_length,
    })
}
/// Go 风格别名。
pub fn buildParquetSchemaFromColumns(i: &[ColumnInfo]) -> Result<(Schema, Vec<Column>)> {
    build_parquet_schema_from_columns(i)
}
/// Go 风格别名。
pub fn validateColumnInfo(i: &ColumnInfo) -> Result<()> {
    validate_column_info(i)
}
/// Go 风格别名。
pub fn newPrimitiveNode(c: &Column) -> Result<PrimitiveNode> {
    new_primitive_node(c)
}

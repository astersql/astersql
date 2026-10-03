// Copyright 2026 AsterSQL.
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

// 表级 KV 编码器：将解析后的行数据转为 TiKV 记录/索引键值对。
//
// 在 Lightning `BaseKVEncoder` 之上处理字段映射、SET 列赋值、类型转换，
// 以及非聚簇表的隐式 `_tidb_rowid`（自动行号）。MVCC 版本由后续写入路径附加，此处只产出逻辑 KV。

use std::collections::HashMap;
use std::sync::Arc;

use astersql_lightning_backend_encode::{Column, ColumnType, Datum, EncodingConfig};
use astersql_lightning_backend_kv::{
    AllocatorType, BaseKVEncoder, NewBaseKVEncoder, Pairs, TableDefinition,
};
use astersql_meta_model::{StatePublic, TableInfo};
use astersql_parser_mysql as mysql;
use astersql_parser_mysql::r#type as mysql_type;
use astersql_table as table;
use astersql_types as types;

use crate::import::{
    ColAssignExpression, FieldMapping, ImportDatumConverter, LoadDataController,
    tableVisCols2FieldMappings,
};

/// 面向单表导入的 KV 编码器。
///
/// 持有字段映射、插入列、SET 表达式与行缓冲，将 parser 输出编码为 data/index KV 对。
pub struct TableKVEncoder {
    /// 底层通用 KV 编码器（会话、列元信息、自增分配等）。
    pub BaseKVEncoder: BaseKVEncoder,
    /// SET 子句对应的列赋值表达式。
    column_assignments: Vec<Arc<dyn ColAssignExpression>>,
    /// 输入字段到列或用户变量的映射。
    field_mappings: Vec<FieldMapping>,
    /// 实际写入的目标列（含 SET 赋值列）。
    insert_columns: Vec<Arc<table::Column>>,
    /// 将原始 Datum 转为列类型的转换器。
    datum_converter: Arc<dyn ImportDatumConverter>,
    /// 表是否需要隐式自动行号列（非聚簇主键）。
    table_has_auto_row_id: bool,
    /// 按 insert_columns 顺序缓存输入值。
    insert_column_row_cache: Vec<Datum>,
    /// 按表列偏移缓存最终行值。
    row_cache: Vec<Datum>,
    /// 标记对应列是否已有显式输入值。
    has_value_cache: Vec<bool>,
}

/// 使用控制器上的字段映射与插入列构造编码器（常规 LOAD DATA / IMPORT INTO 路径）。
pub fn NewTableKVEncoder(
    config: &EncodingConfig,
    controller: &LoadDataController,
) -> Result<TableKVEncoder, String> {
    newTableKVEncoderInner(
        config,
        controller,
        controller.FieldMappings.clone(),
        controller.InsertColumns.clone(),
    )
}

/// Construct the standard visible-column encoder from persistent metadata.
/// Canonical runtimes use this when their SQL tables do not implement the
/// optional table mutation interface. The same fillRow/Record2KV pipeline is
/// used, including hidden handles, generated columns and index encoding.
pub fn NewTableKVEncoderFromMeta(
    config: &EncodingConfig,
    meta: &TableInfo,
    datum_converter: Arc<dyn ImportDatumConverter>,
) -> Result<TableKVEncoder, String> {
    let base = NewBaseKVEncoder(config)?;
    if base.Columns.len() != meta.Columns.len() {
        return Err("encoding metadata column count mismatch".into());
    }
    let columns: Vec<_> = meta
        .Columns
        .iter()
        .filter(|column| !column.Hidden)
        .map(|column| {
            Arc::new(table::Column {
                ColumnInfo: Box::new(column.clone()),
                GeneratedExpr: None,
                DefaultExpr: None,
            })
        })
        .collect();
    Ok(TableKVEncoder {
        BaseKVEncoder: base,
        field_mappings: columns
            .iter()
            .map(|column| FieldMapping {
                Column: Some(column.clone()),
                UserVar: None,
            })
            .collect(),
        insert_columns: columns,
        column_assignments: Vec::new(),
        datum_converter,
        table_has_auto_row_id: table_has_auto_row_id(meta),
        insert_column_row_cache: Vec::new(),
        row_cache: Vec::new(),
        has_value_cache: Vec::new(),
    })
}

/// Type conversion shared by metadata-backed import runtimes. Conversion uses
/// canonical SQL types and preserves binary values without UTF-8 replacement.
pub struct CanonicalImportDatumConverter(pub types::Flags);
impl ImportDatumConverter for CanonicalImportDatumConverter {
    fn CastColumnValue(&self, value: Datum, column: &table::Column) -> Result<Datum, String> {
        let value = astersql_lightning_backend_kv::toCanonicalDatum(&value)?
            .ConvertTo(
                types::DefaultStmtNoWarningContext.WithFlags(self.0),
                &column.ColumnInfo.FieldType,
            )
            .map_err(|error| error.to_string())?;
        astersql_lightning_backend_kv::fromCanonicalDatum(&value, None)
    }
    fn CurrentTime(&self, column: &table::Column) -> Result<Datum, String> {
        let time = chrono::Utc::now()
            .format("%Y-%m-%d %H:%M:%S%.6f")
            .to_string();
        self.CastColumnValue(Datum::String(time), column)
    }
}

/// 为重复键解析路径构造编码器：按可见列全量映射，忽略原始列顺序限制。
pub fn NewTableKVEncoderForDupResolve(
    config: &EncodingConfig,
    controller: &LoadDataController,
) -> Result<TableKVEncoder, String> {
    let (mappings, _) = tableVisCols2FieldMappings(controller.Table.as_ref());
    newTableKVEncoderInner(config, controller, mappings, controller.Table.VisibleCols())
}

/// 将持久化表元数据转换为 Lightning KV 编码器使用的表定义。
///
/// IMPORT INTO 的大小采样不需要依赖完整的 SQL 执行表工厂，但必须使用与导入
/// 相同的记录/索引 KV 编码。这里复用 `TableDefinition`，保留列类型、handle
/// 形态和 Public 索引元数据，避免在 SDK 中实现另一套 KV 估算逻辑。
pub fn NewTableDefinitionFromMeta(meta: &TableInfo) -> Result<TableDefinition, String> {
    let columns = meta
        .Columns
        .iter()
        .map(|column| Column {
            name: column.Name.O.clone(),
            charset: column.FieldType.GetCharset().to_owned(),
            column_type: encodingColumnType(column.FieldType.GetType(), column.FieldType.GetFlag()),
            elements: column.GetElems().to_vec(),
            generated: !column.GeneratedExprString.is_empty(),
            // Integer clustered primary keys have no separate IndexInfo in Go.
            primary_key: mysql_type::HasPriKeyFlag(column.FieldType.GetFlag())
                || meta.Indices.iter().any(|index| {
                    index.Primary
                        && index
                            .Columns
                            .iter()
                            .any(|indexed| indexed.Offset == column.Offset)
                }),
            ..Column::default()
        })
        .collect::<Vec<_>>();

    let indices = meta
        .Indices
        .iter()
        .filter(|index| index.State == StatePublic)
        .map(|index| {
            let mut column_offsets = Vec::with_capacity(index.Columns.len());
            for indexed in &index.Columns {
                let offset = if indexed.Offset >= 0 {
                    indexed.Offset as usize
                } else {
                    meta.Columns
                        .iter()
                        .position(|column| column.Name.L == indexed.Name.L)
                        .ok_or_else(|| {
                            format!("index {} references unknown column", index.Name.O)
                        })?
                };
                if offset >= columns.len() {
                    return Err(format!(
                        "index {} references column offset {}",
                        index.Name.O, offset
                    ));
                }
                column_offsets.push(offset);
            }
            Ok(astersql_lightning_backend_kv::IndexDefinition {
                id: index.ID,
                columns: column_offsets,
                primary: index.Primary,
                unique: index.Unique,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    Ok(TableDefinition {
        source_meta: Some(Arc::new(meta.clone())),
        name: meta.Name.O.clone(),
        id: meta.ID,
        columns,
        indices,
        pk_is_handle: meta.PKIsHandle,
        common_handle: meta.IsCommonHandle,
        shard_row_id_bits: meta.ShardRowIDBits.min(u8::MAX as u64) as u8,
        auto_random_bits: meta.AutoRandomBits.min(u8::MAX as u64) as u8,
        allocators: astersql_lightning_backend_kv::NewPanickingAllocators(false),
        ..TableDefinition::default()
    })
}

/// 将 MySQL 字段类型映射为编码器的 canonical 列类型。
fn encodingColumnType(field_type: u8, flags: usize) -> ColumnType {
    match field_type {
        mysql_type::TypeTiny
        | mysql_type::TypeShort
        | mysql_type::TypeInt24
        | mysql_type::TypeLong
        | mysql_type::TypeLonglong
        | mysql_type::TypeBit
        | mysql_type::TypeYear => {
            if mysql_type::HasUnsignedFlag(flags) {
                ColumnType::UInt
            } else {
                ColumnType::Int
            }
        }
        mysql_type::TypeFloat | mysql_type::TypeDouble => ColumnType::Float,
        mysql_type::TypeDate | mysql_type::TypeDatetime | mysql_type::TypeTimestamp => {
            ColumnType::Timestamp
        }
        mysql_type::TypeDuration => ColumnType::Duration,
        mysql_type::TypeJSON => ColumnType::Json,
        mysql_type::TypeEnum => ColumnType::Enum,
        mysql_type::TypeSet => ColumnType::Set,
        mysql_type::TypeNewDecimal => ColumnType::Decimal,
        mysql_type::TypeString
        | mysql_type::TypeVarString
        | mysql_type::TypeVarchar
        | mysql_type::TypeBlob
        | mysql_type::TypeTinyBlob
        | mysql_type::TypeMediumBlob
        | mysql_type::TypeLongBlob
        | mysql_type::TypeGeometry => ColumnType::Bytes,
        _ => ColumnType::String,
    }
}

/// 内部构造：校验编码列数与表列数一致，并初始化行缓冲。
fn newTableKVEncoderInner(
    config: &EncodingConfig,
    controller: &LoadDataController,
    field_mappings: Vec<FieldMapping>,
    insert_columns: Vec<Arc<table::Column>>,
) -> Result<TableKVEncoder, String> {
    let base_encoder = NewBaseKVEncoder(config)?;
    let (column_assignments, _) =
        controller.CreateColAssignSimpleExprs(base_encoder.SessionCtx.GetExprCtx())?;
    let table_columns = controller.Table.Cols();
    if base_encoder.Columns.len() != table_columns.len() {
        return Err(format!(
            "encoding table column count {} does not match target table column count {}",
            base_encoder.Columns.len(),
            table_columns.len()
        ));
    }
    Ok(TableKVEncoder {
        BaseKVEncoder: base_encoder,
        column_assignments,
        field_mappings,
        insert_columns,
        datum_converter: Arc::clone(&controller.DatumConverter),
        table_has_auto_row_id: table_has_auto_row_id(controller.Table.Meta()),
        insert_column_row_cache: Vec::new(),
        row_cache: Vec::new(),
        has_value_cache: Vec::new(),
    })
}

impl TableKVEncoder {
    /// 将一行 parser 数据编码为 KV 对，并截断会话警告。
    pub fn Encode(&mut self, parser_data: &[Datum], row_id: i64) -> Result<Pairs, String> {
        let result = self.encodeRow(parser_data, row_id);
        self.BaseKVEncoder.TruncateWarns();
        result
    }

    /// 解析→填行→Record2KV 的完整编码流水线。
    fn encodeRow(&mut self, parser_data: &[Datum], row_id: i64) -> Result<Pairs, String> {
        let record = self.parserData2TableData(parser_data, row_id)?;
        let encoded_row_id = if self.table_has_auto_row_id {
            match record.last() {
                Some(Datum::Int(value)) => *value,
                Some(Datum::UInt(value)) => *value as i64,
                _ => row_id,
            }
        } else {
            row_id
        };
        self.BaseKVEncoder
            .Record2KV(record, parser_data, encoded_row_id)
    }

    /// 将 parser 字段按 FieldMapping 填入表行，并求值 SET 赋值表达式。
    fn parserData2TableData(
        &mut self,
        parser_data: &[Datum],
        row_id: i64,
    ) -> Result<Vec<Datum>, String> {
        self.insert_column_row_cache.clear();
        self.insert_column_row_cache
            .reserve(self.insert_columns.len());
        self.row_cache.clear();
        self.row_cache
            .resize(self.BaseKVEncoder.Columns.len(), Datum::Null);
        self.has_value_cache.clear();
        self.has_value_cache
            .resize(self.BaseKVEncoder.Columns.len(), false);
        // 先假定所有 insert 列都有值，缺输入时再清标记。
        for column in &self.insert_columns {
            self.has_value_cache[column.ColumnInfo.Offset as usize] = true;
        }

        for mapping_index in 0..self.field_mappings.len() {
            let mapping = self.field_mappings[mapping_index].clone();
            let input = parser_data.get(mapping_index);
            match (mapping.Column, input) {
                // 映射到用户变量：写入或清除会话变量。
                (None, value) => {
                    let variable = mapping.UserVar.as_ref().ok_or_else(|| {
                        "field mapping has neither column nor user variable".to_owned()
                    })?;
                    let name = variable.Name.to_ascii_lowercase();
                    match value.filter(|value| !matches!(value, Datum::Null)) {
                        Some(value) => self
                            .BaseKVEncoder
                            .SessionCtx
                            .SetUserVarVal(&name, value.clone()),
                        None => self.BaseKVEncoder.SessionCtx.UnsetUserVar(&name),
                    }
                }
                // 列存在但输入缺失：时间 NOT NULL 用当前时间，否则记为无值。
                (Some(column), None) => {
                    if types::field::IsTypeTime(column.ColumnInfo.GetType())
                        && mysql::r#type::HasNotNullFlag(column.GetFlag())
                    {
                        self.insert_column_row_cache
                            .push(self.datum_converter.CurrentTime(&column)?);
                    } else {
                        self.insert_column_row_cache.push(Datum::Null);
                        self.has_value_cache[column.ColumnInfo.Offset as usize] = false;
                    }
                }
                (Some(_), Some(value)) => self.insert_column_row_cache.push(value.clone()),
            }
        }
        // 追加 SET 子句表达式求值结果。
        for assignment in &self.column_assignments {
            self.insert_column_row_cache
                .push(assignment.Eval(&self.BaseKVEncoder.SessionCtx)?);
        }
        self.getRow(row_id)
    }

    /// 对 insert 列做类型转换，写入 row_cache，再补全缺省/生成列。
    fn getRow(&mut self, row_id: i64) -> Result<Vec<Datum>, String> {
        for (index, column) in self.insert_columns.iter().enumerate() {
            let value = self
                .insert_column_row_cache
                .get(index)
                .cloned()
                .ok_or_else(|| {
                    format!(
                        "missing input value for column {}",
                        column.ColumnInfo.Name.O
                    )
                })?;
            let casted = self
                .datum_converter
                .CastColumnValue(value, column)
                .map_err(|error| {
                    let logged = self.BaseKVEncoder.LogKVConvertFailed(
                        &self.insert_column_row_cache,
                        index as i32,
                        &column.ColumnInfo.Name.O,
                        &error,
                    );
                    format!(
                        "[Import:ErrCastValue]Value conversion failed for column '{}'. Expected type: {}, received value: {}. Reason: {}; {logged}",
                        column.ColumnInfo.Name.O,
                        column.ColumnInfo.FieldType,
                        datum_for_cast_error(&self.insert_column_row_cache[index]),
                        error,
                    )
                })?;
            self.row_cache[column.ColumnInfo.Offset as usize] = casted;
        }
        self.fillRow(row_id)
    }

    /// 按表列顺序 ProcessColDatum，必要时追加自动行号并求值生成列。
    fn fillRow(&mut self, row_id: i64) -> Result<Vec<Datum>, String> {
        let mut record = self.BaseKVEncoder.GetOrCreateRecord();
        record.clear();
        for column_index in 0..self.BaseKVEncoder.Columns.len() {
            let input = self.has_value_cache[column_index].then(|| &self.row_cache[column_index]);
            let value = self
                .BaseKVEncoder
                .ProcessColDatum(
                    column_index,
                    row_id,
                    input,
                    !self.has_value_cache[column_index],
                )
                .map_err(|error| {
                    self.BaseKVEncoder.LogKVConvertFailed(
                        &self.row_cache,
                        column_index as i32,
                        &self.BaseKVEncoder.Columns[column_index].name,
                        &error,
                    )
                })?;
            record.push(value);
        }

        // 非聚簇表追加隐式 row id，并 rebase 分配器水位。
        if self.table_has_auto_row_id {
            record.push(Datum::Int((self.BaseKVEncoder.AutoIDFn)(row_id)));
            self.BaseKVEncoder
                .TableAllocators()
                .Get(AllocatorType::RowIDAllocType)
                .Rebase(row_id, false);
        }
        if !self.BaseKVEncoder.GenCols.is_empty() {
            self.BaseKVEncoder
                .EvalGeneratedColumns(&mut record)
                .map_err(|(column_index, error)| {
                    self.BaseKVEncoder.LogEvalGenExprFailed(
                        &self.row_cache,
                        &self.BaseKVEncoder.Columns[column_index].name,
                        &error,
                    )
                })?;
        }
        Ok(record)
    }

    /// 关闭底层会话上下文，释放编码器资源。
    pub fn Close(&mut self) -> Result<(), String> {
        self.BaseKVEncoder.SessionCtx.Close();
        Ok(())
    }
}

fn datum_for_cast_error(value: &Datum) -> String {
    match value {
        Datum::String(value) => format!("{value:?}"),
        Datum::Bytes(value) => format!("{:?}", String::from_utf8_lossy(value)),
        Datum::Int(value) => value.to_string(),
        Datum::UInt(value) => value.to_string(),
        Datum::Float(value) => value.to_string(),
        Datum::Null => "NULL".to_owned(),
        value => format!("{value:?}"),
    }
}

/// 返回需要单独生成索引 KV 的索引数量。
pub fn GetNumOfIndexGenKV(table_info: &TableInfo) -> usize {
    GetIndicesGenKV(table_info).len()
}

/// 非聚簇主键表需要额外的自动行号列。
fn table_has_auto_row_id(table_info: &TableInfo) -> bool {
    !table_info.PKIsHandle && !table_info.IsCommonHandle
}

/// 描述会生成独立索引 KV 的索引元信息。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GenKVIndex {
    /// 索引名（小写）。
    pub name: String,
    /// 是否唯一索引。
    pub Unique: bool,
}

/// 收集状态为 Public、且非聚簇主键索引的索引 ID → 元信息映射。
///
/// 聚簇主键的记录本身已是主键索引，无需再生成额外索引 KV。
pub fn GetIndicesGenKV(table_info: &TableInfo) -> HashMap<i64, GenKVIndex> {
    let mut result = HashMap::with_capacity(table_info.Indices.len());
    for index_info in &table_info.Indices {
        if index_info.State != StatePublic {
            continue;
        }
        if index_info.Primary && table_info.HasClusteredIndex() {
            continue;
        }
        result.insert(
            index_info.ID,
            GenKVIndex {
                name: index_info.Name.L.clone(),
                Unique: index_info.Unique,
            },
        );
    }
    result
}

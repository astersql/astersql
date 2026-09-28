// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// IMPORT INTO 的 KV 体量采样器。
//
// 从导入文件中抽取少量样本行并编码为 TiKV 键值对，估算源文件字节数以及
// Data/Index KV 大小，供调度拆分引擎与评估索引占比。

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use astersql_lightning_backend_encode::{ColumnType, Datum};
use astersql_lightning_backend_encode::{EncodingConfig, SessionOptions};
use astersql_lightning_backend_kv::NewBaseKVEncoder;
use astersql_lightning_mydump::{Datum as ParserDatum, MydumpError, Parser, SourceFileMeta};
use astersql_meta_model::TableInfo;
use astersql_parser_ast as ast;
use astersql_parser_mysql::r#const::SQLMode;
use astersql_table::{self as table, Table};

use crate::{
    DataFormatCSV, DataFormatParquet, DataFormatSQL, EncodedKVGroupBatch, FieldMapping,
    LineFieldsInfo, LoadDataController, NewEncodedKVGroupBatch, NewTableDefinitionFromMeta,
    TableKVEncoder, buildFieldMappings, buildInsertColumns,
};

/// 最多参与采样的文件个数。
pub const maxSampleFileCount: usize = 3;
/// 全体采样文件合计读取的最大行数（均分到各选中文件）。
pub const totalSampleRowCount: usize = maxSampleFileCount * 10;
/// 单个非 Parquet 文件采样时的最大字节偏移上限。
pub static maxSampleFileSize: i64 = 10 * 1024 * 1024;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 一次采样得到的源大小与 Data/Index KV 字节统计。
pub struct SampledKVSizeResult {
    /// 采样消费的源文件字节数（或 Parquet 行长度合计）。
    pub SourceSize: i64,
    /// 表数据（行）编码后的 KV 字节数。
    pub DataKVSize: u64,
    /// 二级索引编码后的 KV 字节数。
    pub IndexKVSize: u64,
}

impl SampledKVSizeResult {
    /// Data 与 Index KV 字节数之和。
    pub fn TotalKVSize(&self) -> i64 {
        self.DataKVSize.wrapping_add(self.IndexKVSize) as i64
    }
}

#[derive(Clone, Debug, Default)]
/// 驱动解析器与编码器的采样配置（格式、字符集、字段分隔等）。
pub struct KVSizeSampleConfig {
    pub Format: String,
    pub SQLMode: SQLMode,
    pub Charset: Option<String>,
    pub ImportantSysVars: std::collections::HashMap<String, String>,
    pub FieldNullDef: Vec<String>,
    pub LineFieldsInfo: LineFieldsInfo,
    pub IgnoreLines: u64,
    pub ColumnsAndUserVars: Vec<ast::ColumnNameOrUserVar>,
    pub ColumnAssignments: Vec<ast::Assignment>,
}

/// 采样所需的外部工厂：按文件构建 Parser 与 TableKVEncoder。
pub trait KVSizeSamplerService {
    /// 按源文件与采样配置创建行解析器。
    fn NewParser(
        &self,
        file: &SourceFileMeta,
        config: &KVSizeSampleConfig,
    ) -> Result<Box<dyn Parser + Send>, String>;
    /// 按字段映射与插入列创建表级 KV 编码器。
    fn NewEncoder(
        &self,
        file: &SourceFileMeta,
        config: &KVSizeSampleConfig,
        table: Arc<dyn Table>,
        field_mappings: &[FieldMapping],
        insert_columns: &[Arc<table::Column>],
    ) -> Result<TableKVEncoder, String>;
}

/// 仅提供行解析器的采样服务。
///
/// SDK 的大小估算不具备完整 IMPORT INTO 控制器，因此直接复用 Lightning 的
/// `BaseKVEncoder`；解析器仍通过这个小接口注入，保证对象存储和压缩读取路径与
/// 正式导入一致。
pub trait KVSizeParserService {
    fn NewParser(
        &self,
        file: &SourceFileMeta,
        config: &KVSizeSampleConfig,
    ) -> Result<Box<dyn Parser>, String>;
}

/// 使用持久化表元数据和真实 BaseKVEncoder 执行文件 KV 采样。
pub fn SampleFileImportKVSizeWithTableInfo(
    config: KVSizeSampleConfig,
    table_info: &TableInfo,
    data_files: &[SourceFileMeta],
    keyspace_codec: &[u8],
    service: &dyn KVSizeParserService,
) -> Result<SampledKVSizeResult, String> {
    if !config.ColumnsAndUserVars.is_empty() || !config.ColumnAssignments.is_empty() {
        return Err("KV size sampling does not support column assignments".into());
    }
    validateKVSizeSampleConfig(&config)?;
    let table = NewTableDefinitionFromMeta(table_info)?;
    let visible_offsets = table_info
        .Columns
        .iter()
        .enumerate()
        .filter(|(_, column)| {
            column.State == astersql_meta_model::StatePublic
                && !column.Hidden
                && column.GeneratedExprString.is_empty()
        })
        .map(|(offset, _)| offset)
        .collect::<Vec<_>>();
    let selected = sample_file_indices(data_files.len());
    if selected.is_empty() {
        return Ok(SampledKVSizeResult::default());
    }
    let rows_per_file = totalSampleRowCount / selected.len();
    let mut result = SampledKVSizeResult::default();
    let mut first_error = None;
    for index in selected {
        match sampleTableInfoFile(
            &config,
            &table,
            &visible_offsets,
            &data_files[index],
            keyspace_codec,
            rows_per_file,
            service,
        ) {
            Ok(sampled) => {
                result.SourceSize = result.SourceSize.saturating_add(sampled.SourceSize);
                result.DataKVSize = result.DataKVSize.saturating_add(sampled.DataKVSize);
                result.IndexKVSize = result.IndexKVSize.saturating_add(sampled.IndexKVSize);
            }
            Err(error) if first_error.is_none() => first_error = Some(error),
            Err(_) => {}
        }
    }
    first_error.map_or(Ok(result), Err)
}

fn sampleTableInfoFile(
    config: &KVSizeSampleConfig,
    table: &astersql_lightning_backend_kv::TableDefinition,
    visible_offsets: &[usize],
    file: &SourceFileMeta,
    keyspace_codec: &[u8],
    maximum_row_count: usize,
    service: &dyn KVSizeParserService,
) -> Result<SampledKVSizeResult, String> {
    let mut parser = service.NewParser(file, config)?;
    let sampled = (|| {
        for _ in 0..config.IgnoreLines {
            match parser.ReadRow() {
                Ok(()) => {}
                Err(MydumpError::Eof) => return Ok(SampledKVSizeResult::default()),
                Err(error) => return Err(error.to_string()),
            }
        }
        parser.SetRowID(0);
        let encoding_config = EncodingConfig {
            SessionOptions: SessionOptions {
                SQLMode: config.SQLMode.0 as u64,
                SysVars: config.ImportantSysVars.clone(),
                Timestamp: 0,
                ..Default::default()
            },
            Path: file.path.clone(),
            Table: Some(Arc::new(table.clone())),
            ..Default::default()
        };
        let mut encoder = NewBaseKVEncoder(&encoding_config)?;
        let mut batch = NewEncodedKVGroupBatch(keyspace_codec, maximum_row_count);
        let mut source_size = 0_i64;
        let mut count = 0_usize;
        while count < maximum_row_count {
            let (start, _) = parser.Pos();
            if config.Format != DataFormatParquet && start >= maxSampleFileSize {
                break;
            }
            match parser.ReadRow() {
                Ok(()) => {}
                Err(MydumpError::Eof) => break,
                Err(error) => return Err(error.to_string()),
            }
            let parsed = parser.LastRow();
            let end = parser.Pos().0;
            source_size = source_size.saturating_add(if config.Format == DataFormatParquet {
                parsed.length as i64
            } else if end > start {
                end - start
            } else {
                parsed.length as i64
            });
            if parsed.row.len() > visible_offsets.len() {
                return Err(format!(
                    "source row has {} fields but table accepts {}",
                    parsed.row.len(),
                    visible_offsets.len()
                ));
            }
            let mut row = vec![Datum::Null; table.columns.len()];
            for (value, offset) in parsed.row.iter().zip(visible_offsets) {
                row[*offset] = parser_datum_to_encoder_datum_for_column(
                    value,
                    table.columns[*offset].column_type,
                )?;
            }
            let pairs = encoder.Record2KV(row, &[], parsed.row_id)?;
            parser.RecycleRow(parsed);
            batch.Add(&pairs).map_err(|(_, error)| error)?;
            count += 1;
        }
        let (data_size, index_size) = batch.group_checksum.DataAndIndexSumSize();
        Ok(SampledKVSizeResult {
            SourceSize: source_size,
            DataKVSize: data_size,
            IndexKVSize: index_size,
        })
    })();
    // Go uses deferred cleanup and only logs close failures, so closing must
    // never replace the sampling result.
    let _ = parser.Close();
    sampled
}

/// 对给定数据文件列表执行 KV 体量采样并返回汇总结果。
pub fn SampleFileImportKVSize(
    config: KVSizeSampleConfig,
    table: Arc<dyn Table>,
    data_files: &[SourceFileMeta],
    keyspace_codec: &[u8],
    service: &dyn KVSizeSamplerService,
) -> Result<SampledKVSizeResult, String> {
    let mut sampler = newKVSizeSampler(config, table, data_files.to_vec())?;
    sampler.sample(keyspace_codec, service)
}

/// 持有配置、目标表与选中文件列表的采样器状态。
pub struct KVSizeSampler {
    config: KVSizeSampleConfig,
    table: Arc<dyn Table>,
    data_files: Vec<SourceFileMeta>,
    field_mappings: Vec<FieldMapping>,
    insert_columns: Vec<Arc<table::Column>>,
}

/// 校验配置并构建字段映射后创建采样器。
pub fn newKVSizeSampler(
    config: KVSizeSampleConfig,
    table: Arc<dyn Table>,
    data_files: Vec<SourceFileMeta>,
) -> Result<KVSizeSampler, String> {
    validateKVSizeSampleConfig(&config)?;
    let (field_mappings, column_names) =
        buildFieldMappings(table.as_ref(), &config.ColumnsAndUserVars)?;
    let insert_columns =
        buildInsertColumns(table.as_ref(), &column_names, &config.ColumnAssignments)?;
    Ok(KVSizeSampler {
        config,
        table,
        data_files,
        field_mappings,
        insert_columns,
    })
}

/// 校验导入格式，并确保 ENCLOSED BY 与 TERMINATED BY 不会互相成为前缀。
pub fn validateKVSizeSampleConfig(config: &KVSizeSampleConfig) -> Result<(), String> {
    if !matches!(
        config.Format.as_str(),
        DataFormatCSV | DataFormatSQL | DataFormatParquet
    ) {
        return Err(format!("unsupported import format {}", config.Format));
    }
    let enclosed = &config.LineFieldsInfo.FieldsEnclosedBy;
    let terminated = &config.LineFieldsInfo.FieldsTerminatedBy;
    if !enclosed.is_empty()
        && (enclosed.starts_with(terminated) || terminated.starts_with(enclosed))
    {
        return Err("FIELDS ENCLOSED BY and TERMINATED BY must not prefix each other".into());
    }
    Ok(())
}

impl KVSizeSampler {
    /// 选取文件子集，逐文件采样并累加结果；保留首个错误供调用方处理。
    pub fn sample(
        &mut self,
        keyspace_codec: &[u8],
        service: &dyn KVSizeSamplerService,
    ) -> Result<SampledKVSizeResult, String> {
        if self.data_files.is_empty() {
            return Ok(SampledKVSizeResult::default());
        }
        // 伪随机挑选最多 maxSampleFileCount 个文件，行数在文件间均分。
        let selected = sample_file_indices(self.data_files.len());
        let rows_per_file = totalSampleRowCount / selected.len();
        let mut result = SampledKVSizeResult::default();
        let mut first_error = None;
        for index in selected {
            match self.sampleOneFile(
                &self.data_files[index],
                keyspace_codec,
                rows_per_file,
                service,
            ) {
                Ok(sampled) => {
                    result.SourceSize = result.SourceSize.saturating_add(sampled.SourceSize);
                    result.DataKVSize = result.DataKVSize.saturating_add(sampled.DataKVSize);
                    result.IndexKVSize = result.IndexKVSize.saturating_add(sampled.IndexKVSize);
                }
                Err(error) if first_error.is_none() => first_error = Some(error),
                Err(_) => {}
            }
        }
        first_error.map_or(Ok(result), Err)
    }

    /// 解析并编码单个文件的前若干行；出错时仍尝试关闭 parser/encoder。
    fn sampleOneFile(
        &self,
        file: &SourceFileMeta,
        keyspace_codec: &[u8],
        maximum_row_count: usize,
        service: &dyn KVSizeSamplerService,
    ) -> Result<SampledKVSizeResult, String> {
        let mut parser = service.NewParser(file, &self.config)?;
        let sampled = (|| {
            // 跳过文件头 IgnoreLines 行（CSV 等场景的表头）。
            if self.config.IgnoreLines > 0 {
                for _ in 0..self.config.IgnoreLines {
                    match parser.ReadRow() {
                        Ok(()) => {}
                        Err(MydumpError::Eof) => return Ok(SampledKVSizeResult::default()),
                        Err(error) => return Err(error.to_string()),
                    }
                }
            }
            parser.SetRowID(0);
            let mut encoder = service.NewEncoder(
                file,
                &self.config,
                Arc::clone(&self.table),
                &self.field_mappings,
                &self.insert_columns,
            )?;
            let result = self.sampleRows(
                parser.as_mut(),
                &mut encoder,
                keyspace_codec,
                maximum_row_count,
            );
            // Match Go's deferred cleanup: encoder close errors are warnings.
            let _ = encoder.Close();
            result
        })();
        // The parser is closed on every path after successful construction,
        // including skip errors and encoder construction failures.
        let _ = parser.Close();
        sampled
    }

    /// 循环读行、累计源字节并编码进 checksum 批次，直到行数或文件大小上限。
    fn sampleRows(
        &self,
        parser: &mut dyn Parser,
        encoder: &mut TableKVEncoder,
        keyspace_codec: &[u8],
        maximum_row_count: usize,
    ) -> Result<SampledKVSizeResult, String> {
        let mut source_size = 0_i64;
        let mut count = 0_usize;
        let mut batch: EncodedKVGroupBatch =
            NewEncodedKVGroupBatch(keyspace_codec, maximum_row_count);
        while count < maximum_row_count {
            let (start_position, _) = parser.Pos();
            // 非 Parquet：按文件偏移截断，避免超长文件拖慢采样。
            if self.config.Format != DataFormatParquet && start_position >= maxSampleFileSize {
                break;
            }
            match parser.ReadRow() {
                Ok(()) => {}
                Err(MydumpError::Eof) => break,
                Err(error) => return Err(error.to_string()),
            }
            let parsed = parser.LastRow();
            source_size = source_size.saturating_add(self.sampledRowSourceSize(
                parser,
                start_position,
                parsed.length,
            ));
            let row = parsed
                .row
                .iter()
                .map(parser_datum_to_encoder_datum)
                .collect::<Vec<_>>();
            let encoded = encoder.Encode(&row, parsed.row_id);
            parser.RecycleRow(parsed);
            batch.Add(&encoded?).map_err(|(_, error)| error)?;
            count += 1;
        }
        let (data_size, index_size) = batch.group_checksum.DataAndIndexSumSize();
        Ok(SampledKVSizeResult {
            SourceSize: source_size,
            DataKVSize: data_size,
            IndexKVSize: index_size,
        })
    }

    /// 估算一行对应的源字节：Parquet 用行长度，其余用解析器位置差。
    fn sampledRowSourceSize(&self, parser: &dyn Parser, start: i64, row_length: u64) -> i64 {
        if self.config.Format == DataFormatParquet {
            return row_length as i64;
        }
        let (end, _) = parser.Pos();
        let delta = end.saturating_sub(start);
        if delta > 0 { delta } else { row_length as i64 }
    }
}

impl LoadDataController {
    /// 从导入计划与 AST 参数构造采样配置。
    pub fn buildKVSizeSampleConfig(&self) -> KVSizeSampleConfig {
        KVSizeSampleConfig {
            Format: self.Plan.Format.clone(),
            SQLMode: self.Plan.SQLMode,
            Charset: self.Plan.Charset.clone(),
            ImportantSysVars: self.Plan.ImportantSysVars.clone(),
            FieldNullDef: self.Plan.FieldNullDef.clone(),
            LineFieldsInfo: self.Plan.LineFieldsInfo.clone(),
            IgnoreLines: self.Plan.IgnoreLines,
            ColumnsAndUserVars: self.ASTArgs.ColumnsAndUserVars.clone(),
            ColumnAssignments: self.ASTArgs.ColumnAssignments.clone(),
        }
    }

    /// 使用控制器内数据文件执行一次 KV 体量采样。
    pub fn sampleKVSize(
        &self,
        keyspace_codec: &[u8],
        service: &dyn KVSizeSamplerService,
    ) -> Result<SampledKVSizeResult, String> {
        SampleFileImportKVSize(
            self.buildKVSizeSampleConfig(),
            Arc::clone(&self.Table),
            self.DataFiles(),
            keyspace_codec,
            service,
        )
    }

    /// 采样后计算 IndexKVSize / DataKVSize；数据 KV 为 0 时返回 0。
    pub fn sampleIndexSizeRatio(
        &self,
        keyspace_codec: &[u8],
        service: &dyn KVSizeSamplerService,
    ) -> Result<f64, String> {
        let result = self.sampleKVSize(keyspace_codec, service)?;
        if result.DataKVSize == 0 {
            Ok(0.0)
        } else {
            Ok(result.IndexKVSize as f64 / result.DataKVSize as f64)
        }
    }
}

/// 以时间戳为种子做 Fisher-Yates 洗牌，等价于 Go `rand.Perm` 的唯一子集契约。
pub(crate) fn sample_file_indices(file_count: usize) -> Vec<usize> {
    let sample_count = file_count.min(maxSampleFileCount);
    let mut seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0x9e37_79b9_7f4a_7c15, |duration| duration.as_nanos() as u64);
    let mut indices = (0..file_count).collect::<Vec<_>>();
    for upper in (1..file_count).rev() {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let selected = (seed as usize) % (upper + 1);
        indices.swap(upper, selected);
    }
    indices.truncate(sample_count);
    indices
}

/// 将 mydump Parser 的 Datum 转为编码器使用的 Datum。
fn parser_datum_to_encoder_datum(value: &ParserDatum) -> Datum {
    match value {
        ParserDatum::Null => Datum::Null,
        ParserDatum::I64(value) => Datum::Int(*value),
        ParserDatum::Bytes(value) | ParserDatum::Binary(value) => Datum::Bytes(value.clone()),
    }
}

/// CSV parser 将未加引号的字段也作为字节串返回；按目标列类型恢复数值，
/// 对齐正式导入中的 `CastColumnValue`，尤其保证整数主键可作为 handle 编码。
fn parser_datum_to_encoder_datum_for_column(
    value: &ParserDatum,
    column_type: ColumnType,
) -> Result<Datum, String> {
    let datum = parser_datum_to_encoder_datum(value);
    match (column_type, datum) {
        (ColumnType::Int, Datum::Bytes(bytes)) => String::from_utf8(bytes)
            .map_err(|error| error.to_string())?
            .trim()
            .parse::<i64>()
            .map(Datum::Int)
            .map_err(|error| error.to_string()),
        (ColumnType::UInt, Datum::Bytes(bytes)) => String::from_utf8(bytes)
            .map_err(|error| error.to_string())?
            .trim()
            .parse::<u64>()
            .map(Datum::UInt)
            .map_err(|error| error.to_string()),
        (ColumnType::Float, Datum::Bytes(bytes)) => String::from_utf8(bytes)
            .map_err(|error| error.to_string())?
            .trim()
            .parse::<f64>()
            .map(Datum::Float)
            .map_err(|error| error.to_string()),
        (_, datum) => Ok(datum),
    }
}

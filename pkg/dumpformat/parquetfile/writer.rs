// Parquet 文件写入器：把 SQL 行（`Option<Vec<u8>>` / RawBytes）按列缓冲，
// 在内存阈值或 `Close` 时通过 parquet-rs 刷成标准 row group 与页脚。
//
// 前半为可运行的 Rust 实现；文件后半 `/* ... */` 块保留贴近 Go 原版的机械翻译草稿，
// 便于对照 row group 内存估算、definition level 与压缩选项等行为。
//
// Row group：Parquet 中一批行的列式存储单元，写满或达内存上限后 flush。
// Definition level：可空列用 0/1 标记该行该列是否为 NULL。

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

// ParquetWriter 如何把 SQL RawBytes 行缓冲成 Parquet row group，并在阈值或 Close 时 flush。
//
// DefaultCompressionType 对应 Go 的默认 parquet 压缩类型。
// pub const DefaultCompressionType: compressedio::CompressType = compressedio::Snappy;
// DefaultRowGroupMemoryLimitBytes 对应 Go 的 row-group 内存阈值，按估算内存字节触发 flush。
// pub const DefaultRowGroupMemoryLimitBytes: i64 = 120 * units::MiB;
// const definitionLevelMemoryBytes: i64 = 2;
//
// ColumnInfo describes a SQL result column to be written into Parquet.
// ColumnInfo 对应 Go 的导出结构体，描述 database/sql 列元数据。
// pub struct ColumnInfo {
//     pub Name: String,
// DatabaseTypeName must be the canonical database/sql ColumnType.DatabaseTypeName() value.
//     pub DatabaseTypeName: String,
//     pub Nullable: bool,
//     pub Precision: i64,
//     pub Scale: i64,
// }
//
// columnType describes the physical and logical Parquet type for a SQL column.
// columnType 对应 Go 的内部结构，缓存物理类型、逻辑类型和 DECIMAL 元数据。
// pub struct columnType {
//     pub Physical: parquet::Type,
//     pub Logical: schema::LogicalType,
//     pub TypeLength: i32,
//     pub Precision: i32,
//     pub Scale: i32,
// }
//
// column 对应 Go 的组合结构，嵌入 ColumnInfo 与 columnType，并保存写入时需要的额外状态。
// pub struct column {
//     pub ColumnInfo: ColumnInfo,
//     pub columnType: columnType,
//     pub Repetition: parquet::Repetition,
//     pub allowsNullEncoding: bool,
//     pub timestampUnit: schema::TimeUnitType,
// }
//
// timestampUnitFromLogicalType 对应 Go 的类型断言辅助函数。
// 非 TIMESTAMP logical type 返回 TimeUnitUnknown，避免每行重复解析。
// pub fn timestampUnitFromLogicalType(logicalType: schema::LogicalType) -> schema::TimeUnitType {
//     if let schema::LogicalType::TimestampLogicalType(timestampLogicalType) = logicalType {
//         return timestampLogicalType.TimeUnit();
//     }
//     schema::TimeUnitUnknown
// }
//
// parsedColumnValue 对应 Go 的中间值，isNull 表示该列最终应写 definition level 0。
// pub struct parsedColumnValue {
//     pub value: any,
//     pub isNull: bool,
// }
//
// countingWriter 对应 Go 的 io.Writer 包装器，用来统计已经写入底层 sink 的字节数。
// pub struct countingWriter<W: io::Writer> {
//     pub writer: W,
//     pub writtenBytes: i64,
// }
//
// impl<W: io::Writer> countingWriter<W> {
// Write 对应 Go 方法：先转发写入，再累加实际写入字节。
//     pub fn Write(&mut self, p: &[u8]) -> Result<usize, Error> {
//         let n = self.writer.Write(p)?;
//         self.writtenBytes += n as i64;
//         Ok(n)
//     }
//
// Close 对应 Go 方法：底层实现 io.Closer 时才真正关闭，否则返回 nil。
//     pub fn Close(&mut self) -> Result<(), Error> {
//         if let Some(closer) = self.writer.as_closer() {
//             return closer.Close();
//         }
//         Ok(())
//     }
// }
// */
use crate::column_buffer::{ColumnBuffer, new_column_buffers};
use crate::column_type::{Column, ColumnInfo, PhysicalType};
use crate::column_value::{
    ColumnValue, account_column_value_memory_bytes, append_column_value, parse_raw_column_value,
};
use crate::schema_builder::build_parquet_schema_from_columns;
use crate::{Error, Result};
use parquet::basic::{LogicalType as ParquetLogical, Repetition, TimeUnit as ParquetTimeUnit};
use parquet::column::writer::ColumnWriter;
use parquet::data_type::{ByteArray, FixedLenByteArray};
use parquet::file::properties::WriterProperties;
use parquet::file::writer::SerializedFileWriter;
use parquet::schema::types::Type;
use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
/// 默认 row group 内存阈值（约 120MiB）：缓冲估算字节达到该值时触发 flush。
pub const DEFAULT_ROW_GROUP_MEMORY_LIMIT_BYTES: i64 = 120 * 1024 * 1024;
/// 每个 definition level（i16）计入缓冲内存估算的字节数。
const DEFINITION_LEVEL_MEMORY_BYTES: i64 = 2;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Dumping/dumpling 侧压缩类型枚举，经 `compression_codec` 映射到 Parquet 压缩。
pub enum CompressionType {
    /// 不压缩。
    None,
    /// Gzip 压缩。
    Gzip,
    /// Snappy 压缩（默认回退目标）。
    Snappy,
    /// Zstd 压缩。
    Zstd,
    /// 未知类型：映射时退回 Snappy。
    Unknown,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Parquet 写入使用的压缩算法（对齐 Arrow/parquet 库的 codec 语义）。
pub enum Compression {
    /// 无压缩。
    Uncompressed,
    /// Gzip。
    Gzip,
    /// Snappy。
    Snappy,
    /// Zstd。
    Zstd,
}
/// 将 `CompressionType` 映射为 `Compression`；`Unknown` 回退为 Snappy。
pub fn compression_codec(kind: CompressionType) -> Compression {
    match kind {
        CompressionType::None => Compression::Uncompressed,
        CompressionType::Gzip => Compression::Gzip,
        CompressionType::Snappy => Compression::Snappy,
        CompressionType::Zstd => Compression::Zstd,
        CompressionType::Unknown => Compression::Snappy,
    }
}
#[derive(Clone, Debug)]
/// 写入器运行时选项：压缩、data page 大小、row group 内存上限。
pub struct WriterOptions {
    /// 压缩算法。
    pub compression: Compression,
    /// 可选 data page 大小提示（当前简化实现可能未完全消费）。
    pub data_page_size: Option<i64>,
    /// 触发 flush 的缓冲内存估算上限（字节）；<=0 表示不按内存自动 flush。
    pub row_group_memory_limit_bytes: i64,
}
/// 默认：Snappy + 120MiB row group 内存上限。
impl Default for WriterOptions {
    fn default() -> Self {
        Self {
            compression: Compression::Snappy,
            data_page_size: None,
            row_group_memory_limit_bytes: DEFAULT_ROW_GROUP_MEMORY_LIMIT_BYTES,
        }
    }
}
#[derive(Clone, Debug)]
/// 构造 `ParquetWriter` 时的单项配置（函数式选项的枚举版）。
pub enum WriterOption {
    /// 设置压缩。
    Compression(Compression),
    /// 设置 data page 大小。
    DataPageSize(i64),
    /// 设置 row group 内存上限（仅 >0 生效）。
    RowGroupMemoryLimit(i64),
}
/// 将函数式选项折叠为 WriterOptions；非正内存限额保留默认。
fn writer_options(options: &[WriterOption]) -> WriterOptions {
    let mut result = WriterOptions::default();
    for option in options {
        match option {
            WriterOption::Compression(v) => result.compression = *v,
            WriterOption::DataPageSize(v) => result.data_page_size = Some(*v),
            WriterOption::RowGroupMemoryLimit(v) if *v > 0 => {
                result.row_group_memory_limit_bytes = *v
            }
            _ => {}
        }
    }
    result
}

fn parquet_schema(columns: &[Column]) -> Result<Arc<Type>> {
    use crate::column_type::{LogicalType, TimeUnit};
    let mut fields = Vec::with_capacity(columns.len());
    for column in columns {
        let physical = match column.column_type.physical {
            PhysicalType::Boolean => parquet::basic::Type::BOOLEAN,
            PhysicalType::Int32 => parquet::basic::Type::INT32,
            PhysicalType::Int64 => parquet::basic::Type::INT64,
            PhysicalType::Float => parquet::basic::Type::FLOAT,
            PhysicalType::Double => parquet::basic::Type::DOUBLE,
            PhysicalType::Int96 => parquet::basic::Type::INT96,
            PhysicalType::ByteArray => parquet::basic::Type::BYTE_ARRAY,
            PhysicalType::FixedLenByteArray => parquet::basic::Type::FIXED_LEN_BYTE_ARRAY,
        };
        let unit = |unit| match unit {
            TimeUnit::Millis => ParquetTimeUnit::MILLIS,
            TimeUnit::Micros => ParquetTimeUnit::MICROS,
            TimeUnit::Nanos => ParquetTimeUnit::NANOS,
        };
        let logical = match column.column_type.logical {
            LogicalType::None => None,
            LogicalType::String => Some(ParquetLogical::String),
            LogicalType::Decimal { precision, scale } => {
                Some(ParquetLogical::decimal(scale, precision))
            }
            LogicalType::Timestamp {
                adjusted_to_utc,
                unit: value,
            } => Some(ParquetLogical::timestamp(adjusted_to_utc, unit(value))),
            LogicalType::Date => Some(ParquetLogical::Date),
            LogicalType::Time {
                adjusted_to_utc,
                unit: value,
            } => Some(ParquetLogical::time(adjusted_to_utc, unit(value))),
        };
        let field = Type::primitive_type_builder(&column.info.name, physical)
            .with_repetition(if column.allows_null_encoding {
                Repetition::OPTIONAL
            } else {
                Repetition::REQUIRED
            })
            .with_length(column.column_type.type_length)
            .with_precision(column.column_type.precision)
            .with_scale(column.column_type.scale)
            .with_logical_type(logical)
            .build()
            .map_err(|e| Error(e.to_string()))?;
        fields.push(Arc::new(field));
    }
    Type::group_type_builder("schema")
        .with_repetition(Repetition::REQUIRED)
        .with_fields(fields)
        .build()
        .map(Arc::new)
        .map_err(|e| Error(e.to_string()))
}
/// Go 风格命名：构造压缩选项。
pub fn WithCompression(v: Compression) -> WriterOption {
    WriterOption::Compression(v)
}
/// Go 风格命名：构造 data page 大小选项。
pub fn WithDataPageSize(v: i64) -> WriterOption {
    WriterOption::DataPageSize(v)
}
/// Go 风格命名：构造 row group 内存上限选项。
pub fn WithRowGroupMemoryLimit(v: i64) -> WriterOption {
    WriterOption::RowGroupMemoryLimit(v)
}
/// Go 风格命名：压缩类型转换入口。
pub fn CompressionCodec(v: CompressionType) -> Compression {
    compression_codec(v)
}
/// 包装底层 `Write`，统计已写出字节数，供文件大小估算与页脚统计。
pub struct CountingWriter<W: Write> {
    /// 实际输出目标。
    inner: W,
    /// 累计已写入字节。
    written_bytes: Arc<AtomicI64>,
}
/// `CountingWriter` 的写入与取出实现。
impl<W: Write> CountingWriter<W> {
    /// 写入全部字节并累加计数。
    /// 返回已写入底层的字节数。
    pub fn written_bytes(&self) -> i64 {
        self.written_bytes.load(Ordering::Relaxed)
    }
    /// 消费包装器，取回底层 writer。
    pub fn into_inner(self) -> W {
        self.inner
    }
}
impl<W: Write> Write for CountingWriter<W> {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        let written = self.inner.write(data)?;
        self.written_bytes
            .fetch_add(written as i64, Ordering::Relaxed);
        Ok(written)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}
#[derive(Clone, Debug)]
/// 单列解析结果：具体值与是否应按 NULL（definition level 0）写出。
struct ParsedColumnValue {
    /// 非空时的列值；NULL 行为由 `is_null` 控制。
    value: Option<ColumnValue>,
    /// true 表示写 definition level 0，不追加物理值。
    is_null: bool,
}
/// 将 SQL 行写入标准 Parquet 文件的写入器。
pub struct ParquetWriter<W: Write + Send> {
    writer: Option<SerializedFileWriter<CountingWriter<W>>>,
    written_bytes: Arc<AtomicI64>,
    /// 已解析的列描述（物理/逻辑类型与可空编码标志）。
    columns: Vec<Column>,
    /// 当前 row group 的按列缓冲。
    buffers: Vec<ColumnBuffer>,
    /// 运行时选项。
    options: WriterOptions,
    /// 当前缓冲行数。
    buffered_rows: usize,
    /// 当前缓冲估算内存（含 definition level 与列值）。
    buffered_memory_bytes: i64,
    /// 是否已 Close；关闭后禁止再 Write。
    closed: bool,
}
/// `ParquetWriter` 的构造、写入、flush 与关闭逻辑。
impl<W: Write + Send> ParquetWriter<W> {
    /// 按列元数据建 schema/缓冲与 parquet-rs writer，返回就绪写入器。
    pub fn new(output: W, column_infos: &[ColumnInfo], options: &[WriterOption]) -> Result<Self> {
        let (_schema, columns) = build_parquet_schema_from_columns(column_infos)?;
        let buffers = new_column_buffers(&columns, 0)?;
        let options = writer_options(options);
        let schema = parquet_schema(&columns)?;
        let compression = match options.compression {
            Compression::Uncompressed => parquet::basic::Compression::UNCOMPRESSED,
            Compression::Gzip => parquet::basic::Compression::GZIP(Default::default()),
            Compression::Snappy => parquet::basic::Compression::SNAPPY,
            Compression::Zstd => parquet::basic::Compression::ZSTD(Default::default()),
        };
        let mut properties = WriterProperties::builder().set_compression(compression);
        if let Some(size) = options.data_page_size {
            properties = properties.set_data_page_size_limit(size.max(1) as usize);
        }
        let written_bytes = Arc::new(AtomicI64::new(0));
        let counted = CountingWriter {
            inner: output,
            written_bytes: written_bytes.clone(),
        };
        let writer = SerializedFileWriter::new(counted, schema, Arc::new(properties.build()))
            .map_err(|e| Error(e.to_string()))?;
        Ok(Self {
            writer: Some(writer),
            written_bytes,
            columns,
            buffers,
            options,
            buffered_rows: 0,
            buffered_memory_bytes: 0,
            closed: false,
        })
    }
    /// 解析并追加一行；若缓冲内存达上限则 flush 当前 row group。
    pub fn write(&mut self, row: &[Option<Vec<u8>>]) -> Result<()> {
        if self.closed {
            return Err(Error("parquet writer is closed".into()));
        }
        self.parse_and_append_row(row)?;
        if self.options.row_group_memory_limit_bytes > 0
            && self.buffered_memory_bytes >= self.options.row_group_memory_limit_bytes
        {
            self.flush_rows()?;
        }
        Ok(())
    }
    /// 解析单列 RawBytes；NULL 时校验是否允许 optional 编码。
    fn parse_column_value(&self, index: usize, raw: &Option<Vec<u8>>) -> Result<ParsedColumnValue> {
        let column = &self.columns[index];
        let Some(raw) = raw else {
            if !column.allows_null_encoding {
                return Err(Error("required column receives NULL".into()));
            }
            return Ok(ParsedColumnValue {
                value: None,
                is_null: true,
            });
        };
        let (value, is_null) = parse_raw_column_value(raw, column)?;
        Ok(ParsedColumnValue { value, is_null })
    }
    /// 将解析结果写入列缓冲，并累加 definition level / 值内存估算。
    fn append_parsed(&mut self, index: usize, parsed: ParsedColumnValue) -> Result<()> {
        let column = &self.columns[index];
        let buffer = &mut self.buffers[index];
        if column.allows_null_encoding {
            self.buffered_memory_bytes += DEFINITION_LEVEL_MEMORY_BYTES;
            if parsed.is_null {
                buffer.definition_levels.push(0);
                return Ok(());
            }
            buffer.definition_levels.push(1);
        }
        if let Some(value) = parsed.value {
            self.buffered_memory_bytes += account_column_value_memory_bytes(&value);
            append_column_value(buffer, column, value)?;
        }
        Ok(())
    }
    /// 校验列数后逐列解析并追加，成功则增加 buffered_rows。
    fn parse_and_append_row(&mut self, row: &[Option<Vec<u8>>]) -> Result<()> {
        if row.len() != self.columns.len() {
            return Err(Error(format!(
                "parquet row has {} values, expected {}",
                row.len(),
                self.columns.len()
            )));
        }
        for (index, raw) in row.iter().enumerate() {
            let value = self.parse_column_value(index, raw).map_err(|e| {
                Error(format!(
                    "convert parquet column {}: {e}",
                    self.columns[index].info.name
                ))
            })?;
            self.append_parsed(index, value).map_err(|e| {
                Error(format!(
                    "convert parquet column {}: {e}",
                    self.columns[index].info.name
                ))
            })?;
        }
        self.buffered_rows += 1;
        Ok(())
    }
    /// 将列缓冲编码为简化二进制块（levels + 按物理类型的值序列）。
    fn encode_buffer(
        column: &Column,
        buffer: &ColumnBuffer,
        buffered_rows: usize,
        out: &mut Vec<u8>,
    ) -> Result<()> {
        if column.allows_null_encoding && buffer.definition_levels.len() != buffered_rows {
            return Err(Error(format!(
                "definition level count {} does not match buffered row count {buffered_rows}",
                buffer.definition_levels.len()
            )));
        }
        let expected_values = if column.allows_null_encoding {
            buffer
                .definition_levels
                .iter()
                .filter(|level| **level > 0)
                .count()
        } else {
            buffered_rows
        };
        let actual_values = match column.column_type.physical {
            PhysicalType::Boolean => buffer.bool_values.len(),
            PhysicalType::Int32 => buffer.int32_values.len(),
            PhysicalType::Int64 => buffer.int64_values.len(),
            PhysicalType::Float => buffer.float32_values.len(),
            PhysicalType::Double => buffer.float64_values.len(),
            PhysicalType::ByteArray => buffer.byte_array_values.len(),
            PhysicalType::FixedLenByteArray => buffer.fixed_len_byte_array_values.len(),
            PhysicalType::Int96 => {
                return Err(Error("unsupported parquet physical type Int96".into()));
            }
        };
        if actual_values != expected_values {
            return Err(Error(format!(
                "column value count {actual_values} does not match expected {expected_values} for {buffered_rows} rows"
            )));
        }
        out.extend_from_slice(&(buffer.definition_levels.len() as u32).to_le_bytes());
        for level in &buffer.definition_levels {
            out.extend_from_slice(&level.to_le_bytes());
        }
        macro_rules! nums {
            ($values:expr) => {
                out.extend_from_slice(&($values.len() as u32).to_le_bytes());
                for value in $values {
                    out.extend_from_slice(&value.to_le_bytes());
                }
            };
        }
        match column.column_type.physical {
            PhysicalType::Boolean => {
                out.extend_from_slice(&(buffer.bool_values.len() as u32).to_le_bytes());
                out.extend(buffer.bool_values.iter().map(|v| u8::from(*v)));
            }
            PhysicalType::Int32 => {
                nums!(&buffer.int32_values);
            }
            PhysicalType::Int64 => {
                nums!(&buffer.int64_values);
            }
            PhysicalType::Float => {
                nums!(&buffer.float32_values);
            }
            PhysicalType::Double => {
                nums!(&buffer.float64_values);
            }
            PhysicalType::ByteArray => {
                out.extend_from_slice(&(buffer.byte_array_values.len() as u32).to_le_bytes());
                for value in &buffer.byte_array_values {
                    out.extend_from_slice(&(value.len() as u32).to_le_bytes());
                    out.extend_from_slice(value);
                }
            }
            PhysicalType::FixedLenByteArray => {
                out.extend_from_slice(
                    &(buffer.fixed_len_byte_array_values.len() as u32).to_le_bytes(),
                );
                for value in &buffer.fixed_len_byte_array_values {
                    if value.len() != column.column_type.type_length as usize {
                        return Err(Error("fixed byte width changed while buffering".into()));
                    }
                    out.extend_from_slice(value);
                }
            }
            PhysicalType::Int96 => {
                return Err(Error("unsupported parquet physical type Int96".into()));
            }
        }
        Ok(())
    }
    /// 若有缓冲行则写出一个 row group（RG 头 + 各列编码），并 reset 缓冲。
    pub fn flush_rows(&mut self) -> Result<()> {
        if self.buffered_rows == 0 {
            return Ok(());
        }
        let writer = self
            .writer
            .as_mut()
            .ok_or_else(|| Error("parquet writer is closed".into()))?;
        let mut group = writer.next_row_group().map_err(|e| Error(e.to_string()))?;
        for (index, column) in self.columns.iter().enumerate() {
            let mut column_writer = group
                .next_column()
                .map_err(|e| Error(e.to_string()))?
                .ok_or_else(|| Error("parquet schema has fewer columns than metadata".into()))?;
            let buffer = &self.buffers[index];
            let defs = column
                .allows_null_encoding
                .then_some(buffer.definition_levels.as_slice());
            let result = match column_writer.untyped() {
                ColumnWriter::BoolColumnWriter(w) => w.write_batch(&buffer.bool_values, defs, None),
                ColumnWriter::Int32ColumnWriter(w) => {
                    w.write_batch(&buffer.int32_values, defs, None)
                }
                ColumnWriter::Int64ColumnWriter(w) => {
                    w.write_batch(&buffer.int64_values, defs, None)
                }
                ColumnWriter::FloatColumnWriter(w) => {
                    w.write_batch(&buffer.float32_values, defs, None)
                }
                ColumnWriter::DoubleColumnWriter(w) => {
                    w.write_batch(&buffer.float64_values, defs, None)
                }
                ColumnWriter::ByteArrayColumnWriter(w) => {
                    let values: Vec<_> = buffer
                        .byte_array_values
                        .iter()
                        .cloned()
                        .map(ByteArray::from)
                        .collect();
                    w.write_batch(&values, defs, None)
                }
                ColumnWriter::FixedLenByteArrayColumnWriter(w) => {
                    let values: Vec<_> = buffer
                        .fixed_len_byte_array_values
                        .iter()
                        .cloned()
                        .map(FixedLenByteArray::from)
                        .collect();
                    w.write_batch(&values, defs, None)
                }
                _ => return Err(Error("unsupported parquet physical writer type".into())),
            };
            result.map_err(|e| Error(format!("write parquet column {}: {e}", column.info.name)))?;
            column_writer
                .close()
                .map_err(|e| Error(format!("close parquet column {}: {e}", column.info.name)))?;
        }
        group.close().map_err(|e| Error(e.to_string()))?;
        self.buffered_rows = 0;
        self.buffered_memory_bytes = 0;
        for buffer in &mut self.buffers {
            buffer.reset();
        }
        Ok(())
    }
    /// 标记关闭、flush 剩余行，再写 row_groups 计数与尾部 PAR1；合并 flush/页脚错误。
    pub fn close(&mut self) -> Result<()> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        let flush = self.flush_rows();
        let footer = self.writer.as_mut().map_or(Ok(()), |writer| {
            writer
                .finish()
                .map(|_| ())
                .map_err(|e| Error(e.to_string()))
        });
        match (flush, footer) {
            (Err(a), Err(b)) => Err(Error(format!("{a}; {b}"))),
            (Err(e), _) | (_, Err(e)) => Err(e),
            _ => Ok(()),
        }
    }
    /// 估算文件大小：已写出字节 + 尚未 flush 的缓冲内存。
    pub fn estimate_file_size(&self) -> u64 {
        (self.total_written_bytes() + self.buffered_memory_bytes).max(0) as u64
    }
    /// 已实际写入底层 sink 的字节数。
    pub fn total_written_bytes(&self) -> i64 {
        self.written_bytes.load(Ordering::Relaxed)
    }
    /// Go 风格别名：转发到 `write`。
    pub fn Write(&mut self, r: &[Option<Vec<u8>>]) -> Result<()> {
        self.write(r)
    }
    /// Go 风格别名：转发到 `close`。
    pub fn Close(&mut self) -> Result<()> {
        self.close()
    }
    /// Go 风格别名：转发到 `estimate_file_size`。
    pub fn EstimateFileSize(&self) -> u64 {
        self.estimate_file_size()
    }
}
/// Go 风格工厂：等价于 `ParquetWriter::new`。
pub fn NewWriter<W: Write + Send>(
    output: W,
    columns: &[ColumnInfo],
    options: &[WriterOption],
) -> Result<ParquetWriter<W>> {
    ParquetWriter::new(output, columns, options)
}
/*

ParquetWriter writes SQL rows into a Parquet file using parquet/file.Writer.
ParquetWriter 对应 Go 的主写入器，持有 parquet writer、列缓冲和 row group 统计信息。
pub struct ParquetWriter<W: io::Writer> {
    pub writer: *mut file::Writer,
    pub output: countingWriter<W>,
    pub columns: Vec<column>,
    pub buffers: Vec<columnBuffer>,
    pub rowGroupMemoryLimitBytes: i64,
    pub bufferedRows: i32,
    pub bufferedMemoryBytes: i64,
    pub closed: bool,
}

writerOptions 对应 Go 的内部配置结构。
pub struct writerOptions {
    pub writerProperties: Vec<parquet::WriterProperty>,
    pub rowGroupMemoryLimitBytes: i64,
}

WriterOption 对应 Go 的函数式配置项。
pub type WriterOption = fn(writerOptions) -> writerOptions;

defaultWriterOptions 对应 Go 的默认配置：Snappy 压缩和 120MiB row group 内存阈值。
pub fn defaultWriterOptions() -> writerOptions {
    writerOptions {
        writerProperties: vec![parquet::WithCompression(CompressionCodec(DefaultCompressionType))],
        rowGroupMemoryLimitBytes: DefaultRowGroupMemoryLimitBytes,
    }
}

CompressionCodec converts dumpling compression type to parquet codec.
CompressionCodec 对应 Go 的压缩类型转换，未知值递归退回默认压缩。
pub fn CompressionCodec(compressType: compressedio::CompressType) -> compress::Compression {
    match compressType {
        compressedio::NoCompression => compress::Codecs::Uncompressed,
        compressedio::Gzip => compress::Codecs::Gzip,
        compressedio::Snappy => compress::Codecs::Snappy,
        compressedio::Zstd => compress::Codecs::Zstd,
        _ => CompressionCodec(DefaultCompressionType),
    }
}

WithCompression sets parquet writer compression codec.
WithCompression 对应 Go 的函数式 option：追加 parquet.WithCompression。
pub fn WithCompression(codec: compress::Compression) -> WriterOption {
    move |mut options: writerOptions| {
        options.writerProperties.push(parquet::WithCompression(codec));
        options
    }
}

WithDataPageSize sets parquet writer data page size in bytes.
pub fn WithDataPageSize(pageSize: i64) -> WriterOption {
    move |mut options: writerOptions| {
        options.writerProperties.push(parquet::WithDataPageSize(pageSize));
        options
    }
}

WithRowGroupMemoryLimit 对应 Go 的阈值配置；非正数保持默认值不变。
pub fn WithRowGroupMemoryLimit(limitBytes: i64) -> WriterOption {
    move |mut options: writerOptions| {
        if limitBytes > 0 {
            options.rowGroupMemoryLimitBytes = limitBytes;
        }
        options
    }
}

NewWriter creates a Parquet writer for SQL result rows.
NewWriter 对应 Go 构造函数：校验输出、构造 schema、初始化 column buffers，再创建 parquet/file.Writer。
pub fn NewWriter<W: io::Writer>(
    w: W,
    columns: Vec<&ColumnInfo>,
    options: Vec<WriterOption>,
) -> Result<ParquetWriter<W>, Error> {
    let mut output = countingWriter { writer: w, writtenBytes: 0 };

    let (parquetSchema, parsedColumns) = buildParquetSchemaFromColumns(columns)?;
    let localOptions = newWriterOptions(options);
    let props = parquet::NewWriterProperties(localOptions.writerProperties);

    let buffers = newColumnBuffers(&parsedColumns, 0)?;
    Ok(ParquetWriter {
        writer: file::NewParquetWriter(&mut output, parquetSchema, file::WithWriterProps(props)),
        output,
        columns: parsedColumns,
        buffers,
        rowGroupMemoryLimitBytes: localOptions.rowGroupMemoryLimitBytes,
        bufferedRows: 0,
        bufferedMemoryBytes: 0,
        closed: false,
    })
}

newWriterOptions 对应 Go 的 option 折叠逻辑；nil option 在 实现中用 Option 语义注释保留。
pub fn newWriterOptions(options: Vec<WriterOption>) -> writerOptions {
    let mut localOptions = defaultWriterOptions();
    for option in options {
        localOptions = option(localOptions);
    }
    localOptions
}

impl<W: io::Writer> ParquetWriter<W> {
    // Write appends one row from SQL raw column bytes.
    // 任意写入失败后 Go 注释要求调用方停止继续写；这里保留同样错误传播方式。
    pub fn Write(&mut self, src: Vec<sql::RawBytes>) -> Result<(), Error> {
        if self.closed {
            return Err(Error::new("parquet writer is closed"));
        }
        self.parseAndAppendRow(src)?;
        if self.rowGroupMemoryLimitBytes > 0 && self.bufferedMemoryBytes >= self.rowGroupMemoryLimitBytes {
            return self.flushRows();
        }
        Ok(())
    }

    // Close flushes buffered rows and closes the Parquet writer.
    // Go 使用 errors.Join 合并 flush 和 close 错误；保留合并错误语义。
    pub fn Close(&mut self) -> Result<(), Error> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        let flushErr = self.flushRows();
        let closeErr = self.writer.Close();
        errors::Join(flushErr, closeErr)
    }

    // EstimateFileSize 对应 Go 的估算函数：已写入底层 sink 的字节 + 当前内存缓冲估算。
    pub fn EstimateFileSize(&self) -> u64 {
        let estimatedBytes = self.totalWrittenBytes() + self.bufferedMemoryBytes;
        if estimatedBytes <= 0 {
            return 0;
        }
        estimatedBytes as u64
    }

    // totalWrittenBytes 对应 Go 的 nil-safe 统计读取。
    pub fn totalWrittenBytes(&self) -> i64 {
        self.output.writtenBytes
    }

    // parseAndAppendRow 对应 Go 的单行解析和列缓冲追加。
    // 长度不匹配立即报错；列级错误补上列名，方便定位坏数据。
    pub fn parseAndAppendRow(&mut self, rawRow: Vec<sql::RawBytes>) -> Result<(), Error> {
        if rawRow.len() != self.columns.len() {
            return Err(Error::new(format!(
                "parquet row has {} values, expected {}",
                rawRow.len(),
                self.columns.len()
            )));
        }

        for (i, rawValue) in rawRow.into_iter().enumerate() {
            let parsedValue = self
                .parseColumnValue(i, rawValue)
                .map_err(|err| Error::wrap(format!("convert parquet column {}", self.columns[i].Name), err))?;
            self.appendParsedColumnValue(i, parsedValue)
                .map_err(|err| Error::wrap(format!("convert parquet column {}", self.columns[i].Name), err))?;
        }

        self.bufferedRows += 1;
        Ok(())
    }

    // flushRows 对应 Go 的 row group flush。
    // 写列失败时 Go 会尝试关闭当前 column writer；这里保留资源收尾分支。
    pub fn flushRows(&mut self) -> Result<(), Error> {
        if self.bufferedRows == 0 {
            return Ok(());
        }

        let mut rowGroupWriter = self.writer.AppendRowGroup();
        for i in 0..self.columns.len() {
            let mut columnWriter = rowGroupWriter.NextColumn()?;
            if let Err(err) = writeColumnBatch(&mut columnWriter, &self.columns[i], &self.buffers[i]) {
                let _ = columnWriter.Close();
                return Err(Error::wrap(format!("write parquet column {}", self.columns[i].Name), err));
            }
            columnWriter
                .Close()
                .map_err(|err| Error::wrap(format!("close parquet column {}", self.columns[i].Name), err))?;
        }
        rowGroupWriter.Close()?;

        self.bufferedRows = 0;
        self.bufferedMemoryBytes = 0;
        for buffer in &mut self.buffers {
            buffer.reset();
        }
        Ok(())
    }

    // parseColumnValue 对应 Go 的列值解析。
    // SQL NULL 只有在 allowsNullEncoding 为 true 时可写入 definition level 0。
    pub fn parseColumnValue(
        &self,
        colIdx: usize,
        rawValue: sql::RawBytes,
    ) -> Result<parsedColumnValue, Error> {
        let column = &self.columns[colIdx];
        if rawValue.is_nil() {
            if !column.allowsNullEncoding {
                return Err(Error::new("required column receives NULL"));
            }
            return Ok(parsedColumnValue { value: any::nil(), isNull: true });
        }

        let (parsedValue, isNull) = parseRawColumnValue(rawValue, column)?;
        Ok(parsedColumnValue { value: parsedValue, isNull })
    }

    // appendParsedColumnValue 对应 Go 的缓冲追加逻辑。
    // optional 列先写 definition level；真实 NULL 不再写 physical value。
    pub fn appendParsedColumnValue(
        &mut self,
        colIdx: usize,
        parsedValue: parsedColumnValue,
    ) -> Result<(), Error> {
        let column = &self.columns[colIdx];
        let buffer = &mut self.buffers[colIdx];

        if column.allowsNullEncoding {
            self.bufferedMemoryBytes += definitionLevelMemoryBytes;
            if parsedValue.isNull {
                buffer.defLevels.push(0);
                return Ok(());
            }
            buffer.defLevels.push(1);
        }

        if parsedValue.isNull {
            return Ok(());
        }
        appendColumnValue(buffer, column, parsedValue.value.clone())?;
        self.bufferedMemoryBytes += accountColumnValueMemoryBytes(column, parsedValue.value);
        Ok(())
    }
}
*/

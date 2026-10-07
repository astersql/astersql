// 测试用 Parquet 写入辅助：经 TiDB objstore 抽象写出真实 parquet-rs 文件，
// 支持按列生成器、definition level、row group 切片与 WriterProperties 选项。
//
// 对齐 Arrow Go 测试助手的语义（含 no-op Seek/Read）；生产导出请用 dumpformat/parquetfile。

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

use std::io::{self, Write};
use std::sync::Arc;

use anyhow::{Context as _, Result, anyhow, bail};
use objstore::parse::ParseBackend;
use objstore::storage::{Context, NewWithDefaultOpt, ObjectWriter, StorageRef};
use parquet::basic::{Compression, ConvertedType, LogicalType, Repetition, Type as PhysicalType};
use parquet::column::writer::ColumnWriter;
use parquet::data_type::{ByteArray, FixedLenByteArray, Int96};
use parquet::file::metadata::KeyValue;
use parquet::file::properties::{WriterProperties, WriterPropertiesBuilder};
use parquet::file::writer::SerializedFileWriter;
use parquet::schema::types::{ColumnPath, Type, TypePtr};

/// The concrete value slice variants accepted by Arrow Go's column writer type switch.
#[derive(Clone, Debug, PartialEq)]
/// 列写入 type switch 接受的具体值缓冲变体（INT96/INT64/…/BOOLEAN）。
pub enum ParquetValueBuffer {
    Int96(Vec<Int96>),
    Int64(Vec<i64>),
    Float32(Vec<f32>),
    Float64(Vec<f64>),
    ByteArray(Vec<ByteArray>),
    FixedLenByteArray(Vec<FixedLenByteArray>),
    Int32(Vec<i32>),
    Boolean(Vec<bool>),
}

/// 值缓冲的长度、类型名与子切片操作。
impl ParquetValueBuffer {
    /// 当前缓冲中的值个数。
    pub fn len(&self) -> usize {
        match self {
            Self::Int96(values) => values.len(),
            Self::Int64(values) => values.len(),
            Self::Float32(values) => values.len(),
            Self::Float64(values) => values.len(),
            Self::ByteArray(values) => values.len(),
            Self::FixedLenByteArray(values) => values.len(),
            Self::Int32(values) => values.len(),
            Self::Boolean(values) => values.len(),
        }
    }

    /// 是否为空缓冲。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 返回物理类型名字符串，用于校验与错误信息。
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Int96(_) => "INT96",
            Self::Int64(_) => "INT64",
            Self::Float32(_) => "FLOAT",
            Self::Float64(_) => "DOUBLE",
            Self::ByteArray(_) => "BYTE_ARRAY",
            Self::FixedLenByteArray(_) => "FIXED_LEN_BYTE_ARRAY",
            Self::Int32(_) => "INT32",
            Self::Boolean(_) => "BOOLEAN",
        }
    }

    /// 按值下标切片，保持缓冲变体类型不变。
    fn slice(&self, start: usize, end: usize) -> Result<Self> {
        if start > end || end > self.len() {
            bail!(
                "value range [{start}, {end}) is outside {} buffer of length {}",
                self.kind(),
                self.len()
            );
        }
        Ok(match self {
            Self::Int96(values) => Self::Int96(values[start..end].to_vec()),
            Self::Int64(values) => Self::Int64(values[start..end].to_vec()),
            Self::Float32(values) => Self::Float32(values[start..end].to_vec()),
            Self::Float64(values) => Self::Float64(values[start..end].to_vec()),
            Self::ByteArray(values) => Self::ByteArray(values[start..end].to_vec()),
            Self::FixedLenByteArray(values) => Self::FixedLenByteArray(values[start..end].to_vec()),
            Self::Int32(values) => Self::Int32(values[start..end].to_vec()),
            Self::Boolean(values) => Self::Boolean(values[start..end].to_vec()),
        })
    }
}

/// Generated values and optional row-level definition levels for one column.
#[derive(Clone, Debug, PartialEq)]
/// 单列生成结果：值缓冲 + 可选按行 definition levels。
pub struct ParquetColumnData {
    /// 物理值序列（可空列时长度由 def levels 中非零个数决定）。
    pub vals: ParquetValueBuffer,
    /// 可选 definition levels；`None` 表示每行都有值。
    pub def_levels: Option<Vec<i16>>,
}

/// 构造辅助。
impl ParquetColumnData {
    /// 组装一列的值与 levels。
    pub fn new(vals: ParquetValueBuffer, def_levels: Option<Vec<i16>>) -> Self {
        Self { vals, def_levels }
    }
}

/// Converts a row range into the matching compacted value range.
/// 将行区间映射为值缓冲中的压缩区间（跳过 def level==0 的空值行）。
pub fn calc_value_range(
    def_levels: Option<&[i16]>,
    row_start: usize,
    row_end: usize,
) -> Result<(usize, usize)> {
    if row_start > row_end {
        bail!("invalid row range [{row_start}, {row_end})");
    }
    let Some(def_levels) = def_levels else {
        return Ok((row_start, row_end));
    };
    if row_end > def_levels.len() {
        bail!(
            "row range [{row_start}, {row_end}) exceeds definition levels length {}",
            def_levels.len()
        );
    }

    let value_start = def_levels[..row_start]
        .iter()
        .filter(|level| **level > 0)
        .count();
    let value_end = value_start
        + def_levels[row_start..row_end]
            .iter()
            .filter(|level| **level > 0)
            .count();
    Ok((value_start, value_end))
}

/// Slices generated column data to a row group while retaining Go's nil-level semantics.
/// 按行区间切片列数据；无 levels 时行列一一对应（对齐 Go 的 nil-level 语义）。
pub fn slice_column_data(
    col: &ParquetColumnData,
    row_start: usize,
    row_end: usize,
) -> Result<(ParquetValueBuffer, Option<Vec<i16>>)> {
    let (value_start, value_end) = calc_value_range(col.def_levels.as_deref(), row_start, row_end)?;
    let row_def_levels = col
        .def_levels
        .as_ref()
        .map(|levels| levels[row_start..row_end].to_vec());
    Ok((col.vals.slice(value_start, value_end)?, row_def_levels))
}

/// Properties corresponding to the Arrow Go writer options used by TiDB tests.
#[derive(Clone, Debug, PartialEq)]
/// 对应 Arrow Go writer properties 的测试选项（row group 长度、压缩、字典等）。
pub enum WriterProperty {
    /// 单个 row group 最大行数。
    MaxRowGroupLength(usize),
    /// data page 大小上限。
    DataPageSize(usize),
    /// 写入 batch 大小。
    BatchSize(usize),
    /// 指定列的压缩算法。
    CompressionFor(String, Compression),
    /// 指定列是否启用字典编码。
    DictionaryFor(String, bool),
    /// 文件 created_by 元数据。
    CreatedBy(String),
}

/// File-level metadata corresponding to Arrow Go's `file.WriteOption` category.
#[derive(Clone, Debug, Default, PartialEq)]
/// 文件级元数据（created_by / key-value），对应 Go `file.WriteOption`。
pub struct WriteMetadata {
    /// 可选 created_by 字符串。
    pub created_by: Option<String>,
    /// 可选键值元数据列表。
    pub key_value_metadata: Option<Vec<KeyValue>>,
}

#[derive(Clone, Debug, PartialEq)]
/// 写入选项包装：目前仅承载文件元数据。
pub enum WriteOption {
    /// 设置文件级元数据。
    Metadata(WriteMetadata),
}

/// Rust's closed equivalent of Go's `...any` option list and its type switch.
#[derive(Clone, Debug, PartialEq)]
/// Go `...any` 选项列表的封闭枚举等价物（含 Unsupported 分支）。
pub enum ParquetWriterOption {
    /// WriterProperties 类选项。
    WriterProperty(WriterProperty),
    /// 文件 WriteOption 类选项。
    WriteOption(WriteOption),
    /// 不支持的选项类型名（测试期望报错）。
    Unsupported(String),
}

/// Properties of a test-only Parquet column.
/// 测试列值生成器：输入行数，输出值缓冲与可选 def levels。
pub type ParquetGenerator = dyn Fn(i32) -> (ParquetValueBuffer, Option<Vec<i16>>);

#[allow(non_snake_case)]
/// 测试专用列描述：物理/转换/逻辑类型与生成器（字段名保持 Go 风格）。
pub struct ParquetColumn {
    /// 列名。
    pub Name: String,
    /// Parquet 物理类型。
    pub Type: PhysicalType,
    /// 旧版 ConvertedType（无 Logical 时使用）。
    pub Converted: ConvertedType,
    /// 可选 LogicalType；若有则优先于 Converted。
    pub Logical: Option<LogicalType>,
    /// FIXED_LEN_BYTE_ARRAY 等类型的字节长度；<=0 表示由库默认。
    pub TypeLen: i32,
    /// DECIMAL 精度。
    pub Precision: i32,
    /// DECIMAL 小数位。
    pub Scale: i32,
    /// 按行数生成列数据的闭包。
    pub Gen: Box<ParquetGenerator>,
}

/// 列构造。
impl ParquetColumn {
    #[allow(clippy::too_many_arguments)]
    /// 组装测试列；`generator` 会被 box 成 `ParquetGenerator`。
    pub fn new<F>(
        name: impl Into<String>,
        physical_type: PhysicalType,
        converted_type: ConvertedType,
        logical_type: Option<LogicalType>,
        type_len: i32,
        precision: i32,
        scale: i32,
        generator: F,
    ) -> Self
    where
        F: Fn(i32) -> (ParquetValueBuffer, Option<Vec<i16>>) + 'static,
    {
        Self {
            Name: name.into(),
            Type: physical_type,
            Converted: converted_type,
            Logical: logical_type,
            TypeLen: type_len,
            Precision: precision,
            Scale: scale,
            Gen: Box::new(generator),
        }
    }
}

/// Adapts TiDB's object writer to parquet-rs' `std::io::Write` contract.
/// 将 TiDB ObjectWriter 适配为 `std::io::Write`，供 parquet-rs 使用。
pub struct WriteWrapper {
    /// 底层对象存储写入器。
    writer: Box<dyn ObjectWriter>,
    /// 调用 ObjectWriter 时使用的上下文。
    context: Context,
    /// 是否已关闭，避免重复 Close。
    closed: bool,
}

/// Seek/Read 空操作与 Close 实现（对齐 Arrow Go 测试助手）。
impl WriteWrapper {
    /// 包装 ObjectWriter，使用 background Context。
    pub fn new(writer: Box<dyn ObjectWriter>) -> Self {
        Self {
            writer,
            context: Context::background(),
            closed: false,
        }
    }

    /// Arrow Go requires `io.WriteSeeker`; its test helper seek is deliberately a no-op.
    /// Arrow Go 要求 WriteSeeker；测试助手的 Seek 故意为空操作。
    pub fn seek(&mut self, _offset: i64, _whence: i32) -> Result<i64> {
        Ok(0)
    }

    /// Arrow Go's writer adapter also provides a no-op read method.
    /// Arrow Go 适配器同样提供空操作 Read。
    pub fn read(&mut self, _buf: &mut [u8]) -> Result<usize> {
        Ok(0)
    }

    /// 关闭底层 ObjectWriter（幂等）。
    pub fn close(&mut self) -> Result<()> {
        if !self.closed {
            self.writer.close(&self.context)?;
            self.closed = true;
        }
        Ok(())
    }
}

/// 将 write/flush 转发到 ObjectWriter。
impl Write for WriteWrapper {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.writer
            .write(&self.context, buf)
            .map_err(|error| io::Error::other(error.to_string()))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// 解析对象存储路径并创建默认配置的 Storage。
pub fn get_store(path: &str) -> Result<StorageRef> {
    let backend = ParseBackend(path, None).context("parse object storage backend")?;
    NewWithDefaultOpt(&Context::background(), &backend).context("create object storage")
}

/// 由测试列描述构建 optional 叶子与 required 根 schema。
fn make_schema(columns: &[ParquetColumn]) -> Result<TypePtr> {
    let mut fields = Vec::with_capacity(columns.len());
    for column in columns {
        let type_len = if column.TypeLen > 0 {
            column.TypeLen
        } else {
            -1
        };
        let mut builder = Type::primitive_type_builder(&column.Name, column.Type)
            .with_repetition(Repetition::OPTIONAL)
            .with_length(type_len)
            .with_precision(column.Precision)
            .with_scale(column.Scale);
        builder = if column.Logical.is_some() {
            // Go gives logical type precedence and does not also apply Converted.
            builder.with_logical_type(column.Logical.clone())
        } else {
            builder.with_converted_type(column.Converted)
        };
        fields.push(Arc::new(builder.build().with_context(|| {
            format!("build parquet schema field {}", column.Name)
        })?));
    }

    Ok(Arc::new(
        Type::group_type_builder("schema")
            .with_repetition(Repetition::REQUIRED)
            .with_fields(fields)
            .build()
            .context("build parquet root schema")?,
    ))
}

/// 将单项 WriterProperty 应用到 WriterPropertiesBuilder。
fn apply_writer_property(
    builder: WriterPropertiesBuilder,
    property: WriterProperty,
) -> Result<WriterPropertiesBuilder> {
    Ok(match property {
        WriterProperty::MaxRowGroupLength(0) => bail!("max row group length must be positive"),
        WriterProperty::MaxRowGroupLength(value) => {
            builder.set_max_row_group_row_count(Some(value))
        }
        WriterProperty::DataPageSize(0) => bail!("data page size must be positive"),
        WriterProperty::DataPageSize(value) => builder.set_data_page_size_limit(value),
        WriterProperty::BatchSize(0) => bail!("batch size must be positive"),
        WriterProperty::BatchSize(value) => builder.set_write_batch_size(value),
        WriterProperty::CompressionFor(column, compression) => {
            builder.set_column_compression(ColumnPath::from(column), compression)
        }
        WriterProperty::DictionaryFor(column, enabled) => {
            builder.set_column_dictionary_enabled(ColumnPath::from(column), enabled)
        }
        WriterProperty::CreatedBy(created_by) => builder.set_created_by(created_by),
    })
}

/// 默认 Snappy+字典，再叠加调用方选项。
fn build_writer_properties(
    columns: &[ParquetColumn],
    options: Vec<ParquetWriterOption>,
) -> Result<WriterProperties> {
    let mut builder = WriterProperties::builder();
    for column in columns {
        let path = ColumnPath::from(column.Name.clone());
        builder = builder
            .set_column_dictionary_enabled(path.clone(), true)
            .set_column_compression(path, Compression::SNAPPY);
    }

    for option in options {
        builder = match option {
            ParquetWriterOption::WriterProperty(property) => {
                apply_writer_property(builder, property)?
            }
            ParquetWriterOption::WriteOption(WriteOption::Metadata(metadata)) => {
                let mut next = builder;
                if let Some(created_by) = metadata.created_by {
                    next = next.set_created_by(created_by);
                }
                next.set_key_value_metadata(metadata.key_value_metadata)
            }
            ParquetWriterOption::Unsupported(type_name) => {
                bail!("unsupported parquet writer option type {type_name}")
            }
        };
    }
    Ok(builder.build())
}

/// 校验生成值类型/数量与 definition level 行数一致。
fn validate_generated_column(
    column: &ParquetColumn,
    data: &ParquetColumnData,
    rows: usize,
) -> Result<()> {
    let expected_kind = match column.Type {
        PhysicalType::INT96 => "INT96",
        PhysicalType::INT64 => "INT64",
        PhysicalType::FLOAT => "FLOAT",
        PhysicalType::DOUBLE => "DOUBLE",
        PhysicalType::BYTE_ARRAY => "BYTE_ARRAY",
        PhysicalType::FIXED_LEN_BYTE_ARRAY => "FIXED_LEN_BYTE_ARRAY",
        PhysicalType::INT32 => "INT32",
        PhysicalType::BOOLEAN => "BOOLEAN",
    };
    if data.vals.kind() != expected_kind {
        bail!(
            "parquet column {} expects {expected_kind} values, got {}",
            column.Name,
            data.vals.kind()
        );
    }

    let expected_values = match &data.def_levels {
        Some(levels) => {
            if levels.len() != rows {
                bail!(
                    "column {} definition levels length {} does not match row count {rows}",
                    column.Name,
                    levels.len()
                );
            }
            levels.iter().filter(|level| **level > 0).count()
        }
        None => rows,
    };
    if data.vals.len() != expected_values {
        bail!(
            "column {} generated {} values, expected {expected_values} from definition levels",
            column.Name,
            data.vals.len()
        );
    }
    Ok(())
}

/// 按物理类型把值批写入对应 ColumnWriter。
fn write_parquet_column_batch(
    writer: &mut ColumnWriter<'_>,
    values: &ParquetValueBuffer,
    def_levels: Option<&[i16]>,
) -> Result<()> {
    match (writer, values) {
        (ColumnWriter::Int96ColumnWriter(writer), ParquetValueBuffer::Int96(values)) => {
            writer.write_batch(values, def_levels, None)?;
        }
        (ColumnWriter::Int64ColumnWriter(writer), ParquetValueBuffer::Int64(values)) => {
            writer.write_batch(values, def_levels, None)?;
        }
        (ColumnWriter::DoubleColumnWriter(writer), ParquetValueBuffer::Float64(values)) => {
            writer.write_batch(values, def_levels, None)?;
        }
        (ColumnWriter::ByteArrayColumnWriter(writer), ParquetValueBuffer::ByteArray(values)) => {
            writer.write_batch(values, def_levels, None)?;
        }
        (
            ColumnWriter::FixedLenByteArrayColumnWriter(writer),
            ParquetValueBuffer::FixedLenByteArray(values),
        ) => {
            writer.write_batch(values, def_levels, None)?;
        }
        (ColumnWriter::Int32ColumnWriter(writer), ParquetValueBuffer::Int32(values)) => {
            writer.write_batch(values, def_levels, None)?;
        }
        (ColumnWriter::BoolColumnWriter(writer), ParquetValueBuffer::Boolean(values)) => {
            writer.write_batch(values, def_levels, None)?;
        }
        (ColumnWriter::FloatColumnWriter(writer), ParquetValueBuffer::Float32(values)) => {
            writer.write_batch(values, def_levels, None)?;
        }
        (_, values) => {
            bail!(
                "parquet column writer and {} value buffer type mismatch",
                values.kind()
            )
        }
    }
    Ok(())
}

/// Writes a real Parquet file through TiDB's object storage abstraction.
/// 经对象存储写出真实 Parquet：生成列数据、按 row group 切片写入并始终 Close wrapper。
pub fn write_parquet_file(
    path: &str,
    file_name: &str,
    columns: &[ParquetColumn],
    rows: i32,
    options: Vec<ParquetWriterOption>,
) -> Result<()> {
    // Go accepts a negative `int` here: generators still receive it, while
    // `rowStart < rows` makes the row-group loop empty.
    let row_count = rows.max(0) as usize;
    let schema = make_schema(columns)?;
    let properties = Arc::new(build_writer_properties(columns, options)?);

    let store = get_store(path)?;
    let object_writer = store
        .Create(&Context::background(), file_name, None)
        .with_context(|| format!("create parquet object {file_name}"))?;
    let mut wrapper = WriteWrapper::new(object_writer);

    let write_result = (|| -> Result<()> {
        let mut file_writer = SerializedFileWriter::new(&mut wrapper, schema, properties.clone())?;

        let mut column_data = Vec::with_capacity(columns.len());
        for column in columns {
            let (vals, def_levels) = (column.Gen)(rows);
            let data = ParquetColumnData::new(vals, def_levels);
            validate_generated_column(column, &data, row_count)?;
            column_data.push(data);
        }

        let row_group_len = properties
            .max_row_group_row_count()
            .unwrap_or_else(|| row_count.max(1));
        let mut row_start = 0usize;
        while row_start < row_count {
            let row_end = row_count.min(row_start.saturating_add(row_group_len));
            let mut row_group_writer = file_writer.next_row_group()?;
            for (column_index, data) in column_data.iter().enumerate() {
                let mut column_writer = row_group_writer.next_column()?.ok_or_else(|| {
                    anyhow!("missing parquet column writer at index {column_index}")
                })?;
                let (row_values, row_def_levels) = slice_column_data(data, row_start, row_end)?;
                write_parquet_column_batch(
                    column_writer.untyped(),
                    &row_values,
                    row_def_levels.as_deref(),
                )?;
                column_writer.close()?;
            }
            if row_group_writer.next_column()?.is_some() {
                bail!("parquet schema contains more columns than generated data");
            }
            row_group_writer.close()?;
            row_start = row_end;
        }

        file_writer.close()?;
        Ok(())
    })();

    // Go defers ParquetWriter.Close and therefore always closes the object writer.
    let _ = wrapper.close();
    write_result
}

// Go-shaped compatibility names for legacy callers.
#[allow(non_snake_case)]
/// Go 风格别名：`calc_value_range`。
pub fn calcValueRange(
    def_levels: Option<&[i16]>,
    row_start: usize,
    row_end: usize,
) -> Result<(usize, usize)> {
    calc_value_range(def_levels, row_start, row_end)
}

#[allow(non_snake_case)]
/// Go 风格别名：`slice_column_data`。
pub fn sliceColumnData(
    col: &ParquetColumnData,
    row_start: usize,
    row_end: usize,
) -> Result<(ParquetValueBuffer, Option<Vec<i16>>)> {
    slice_column_data(col, row_start, row_end)
}

#[allow(non_snake_case)]
/// Go 风格别名：`get_store`。
pub fn getStore(path: &str) -> Result<StorageRef> {
    get_store(path)
}

#[allow(non_snake_case)]
/// Go 风格别名：`write_parquet_file`。
pub fn WriteParquetFile(
    path: &str,
    file_name: &str,
    columns: &[ParquetColumn],
    rows: i32,
    options: Vec<ParquetWriterOption>,
) -> Result<()> {
    write_parquet_file(path, file_name, columns, rows, options)
}

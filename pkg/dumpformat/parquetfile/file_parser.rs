// Copyright 2026 AsterSQL.

//! Streaming row decoder backed by the same preload buffers used for metadata.
use crate::parser::ConvertedType;
use crate::source_reader::SourceReader;
use crate::spark_rebase::{
    AppVersion, SparkFileMeta, SparkRebaseMicrosLookup, spark_rebase_time_zone_id,
};
use crate::type_converter::{self as convert, Datum, ParquetColumnType};
use crate::{Error, Result};
use chrono::{Offset, TimeZone};
use parquet::basic::{ConvertedType as CT, LogicalType, TimeUnit};
use parquet::column::reader::ColumnReader;
use parquet::file::reader::FileReader;
use parquet::file::serialized_reader::SerializedFileReader;
use std::collections::VecDeque;

/// Logical annotations take precedence; legacy annotations are normalized once.
/// parquet-rs calls the Parquet NULL annotation `Unknown`.
fn normalized_logical_type(
    descriptor: &parquet::schema::types::ColumnDescriptor,
) -> Result<Option<LogicalType>> {
    let logical = match descriptor.logical_type_ref() {
        Some(LogicalType::_Unknown { .. }) | None => match descriptor.converted_type() {
            CT::NONE => None,
            CT::UTF8 => Some(LogicalType::String),
            CT::ENUM => Some(LogicalType::Enum),
            CT::JSON => Some(LogicalType::Json),
            CT::BSON => Some(LogicalType::Bson),
            CT::DECIMAL => Some(LogicalType::decimal(
                descriptor.type_scale(),
                descriptor.type_precision(),
            )),
            CT::DATE => Some(LogicalType::Date),
            CT::TIME_MILLIS => Some(LogicalType::time(true, TimeUnit::MILLIS)),
            CT::TIME_MICROS => Some(LogicalType::time(true, TimeUnit::MICROS)),
            CT::TIMESTAMP_MILLIS => Some(LogicalType::timestamp(true, TimeUnit::MILLIS)),
            CT::TIMESTAMP_MICROS => Some(LogicalType::timestamp(true, TimeUnit::MICROS)),
            CT::UINT_8 => Some(LogicalType::integer(8, false)),
            CT::UINT_16 => Some(LogicalType::integer(16, false)),
            CT::UINT_32 => Some(LogicalType::integer(32, false)),
            CT::UINT_64 => Some(LogicalType::integer(64, false)),
            CT::INT_8 => Some(LogicalType::integer(8, true)),
            CT::INT_16 => Some(LogicalType::integer(16, true)),
            CT::INT_32 => Some(LogicalType::integer(32, true)),
            CT::INT_64 => Some(LogicalType::integer(64, true)),
            other => {
                return Err(Error(format!(
                    "column {:?}: unsupported parquet logical type {other:?}",
                    descriptor.name()
                )));
            }
        },
        logical => logical.cloned(),
    };
    validate_parquet_logical_type(
        &logical,
        descriptor.physical_type(),
        descriptor.type_length(),
        descriptor.name(),
    )?;
    Ok(logical)
}

pub(crate) fn validate_parquet_logical_type(
    logical: &Option<LogicalType>,
    physical: parquet::basic::Type,
    length: i32,
    name: &str,
) -> Result<()> {
    use parquet::basic::{Repetition, Type as P};
    if matches!(
        logical,
        Some(LogicalType::List | LogicalType::Map | LogicalType::Float16 | LogicalType::Variant(_))
    ) {
        return Err(Error(format!(
            "column {name:?}: {}",
            convert::unsupported_logical(logical)
        )));
    }
    if matches!(logical, Some(LogicalType::Integer(i)) if !matches!(i.bit_width, 8 | 16 | 32 | 64))
    {
        return Err(Error(format!(
            "column {name:?}: logical type {logical:?} is not applicable to physical type {physical:?}"
        )));
    }
    // Reuse the upstream schema validator for physical width/precision constraints.
    let (scale, precision) = match logical {
        Some(LogicalType::Decimal(d)) => (d.scale, d.precision),
        _ => (-1, -1),
    };
    parquet::schema::types::Type::primitive_type_builder(name, physical)
        .with_repetition(Repetition::OPTIONAL)
        .with_length(if physical == P::FIXED_LEN_BYTE_ARRAY { length } else { -1 })
        .with_logical_type(logical.clone()).with_scale(scale).with_precision(precision)
        .build().map_err(|e| Error(format!("column {name:?}: logical type {logical:?} is not applicable to physical type {physical:?}: {e}")))?;
    Ok(())
}

struct Column {
    reader: ColumnReader,
    batch_size: usize,
    info: ParquetColumnType,
    nullable: bool,
    location: chrono_tz::Tz,
    rows: VecDeque<Result<Datum>>,
}
fn error(e: impl std::fmt::Display) -> Error {
    Error(e.to_string())
}
impl Column {
    fn next(&mut self) -> Result<Datum> {
        if self.rows.is_empty() {
            macro_rules! read {
                ($reader:expr, $convert:expr) => {{
                    let mut values = Vec::new();
                    let mut levels = Vec::new();
                    let (records, _, _) = $reader
                        .read_records(
                            self.batch_size,
                            self.nullable.then_some(&mut levels),
                            None,
                            &mut values,
                        )
                        .map_err(error)?;
                    let mut values = values.into_iter();
                    for row in 0..records {
                        let value = if self.nullable && levels[row] == 0 {
                            Ok(Datum::Null)
                        } else {
                            let value = values
                                .next()
                                .ok_or_else(|| Error("missing parquet column value".into()))?;
                            if matches!(self.info.logical, Some(LogicalType::Unknown)) {
                                Err(convert::unsupported_logical(&self.info.logical))
                            } else {
                                ($convert)(value)
                            }
                        };
                        self.rows.push_back(value);
                    }
                }};
            }
            let info = &self.info;
            let location = self.location;
            match &mut self.reader {
                ColumnReader::BoolColumnReader(r) => {
                    read!(r, |v: bool| Ok::<_, Error>(Datum::Int(i64::from(v))))
                }
                ColumnReader::Int32ColumnReader(r) => {
                    read!(r, |v| convert::convert_logical_int32(v, info, location))
                }
                ColumnReader::Int64ColumnReader(r) => {
                    read!(r, |v| convert::convert_logical_int64(v, info, location))
                }
                ColumnReader::Int96ColumnReader(r) => read!(r, |v: parquet::data_type::Int96| {
                    let mut bytes = [0; 12];
                    for (index, word) in v.data().iter().enumerate() {
                        bytes[index * 4..index * 4 + 4].copy_from_slice(&word.to_le_bytes());
                    }
                    convert::int96_micros_with_rebase(bytes, info.spark_rebase.as_ref())
                        .map(Datum::TimeMicros)
                }),
                ColumnReader::FloatColumnReader(r) => {
                    read!(r, |v| Ok::<_, Error>(Datum::Float32(v)))
                }
                ColumnReader::DoubleColumnReader(r) => {
                    read!(r, |v| Ok::<_, Error>(Datum::Float64(v)))
                }
                ColumnReader::ByteArrayColumnReader(r) => {
                    read!(r, |v: parquet::data_type::ByteArray| {
                        convert::convert_logical_bytes(v.data(), info, false)
                    })
                }
                ColumnReader::FixedLenByteArrayColumnReader(r) => {
                    read!(r, |v: parquet::data_type::FixedLenByteArray| {
                        convert::convert_logical_bytes(v.data(), info, true)
                    })
                }
            }
        }
        let mut value = self.rows.pop_front().ok_or_else(|| Error("EOF".into()))??;
        // INT96 keeps its historical UTC semantics even without an annotation.
        if matches!(self.reader, ColumnReader::Int96ColumnReader(_)) {
            if let Datum::TimeMicros(micros) = &mut value {
                let offset = self
                    .location
                    .timestamp_opt(micros.div_euclid(1_000_000), 0)
                    .single()
                    .ok_or_else(|| Error("timestamp out of range".into()))?
                    .offset()
                    .fix()
                    .local_minus_utc();
                *micros += i64::from(offset) * 1_000_000;
            }
        }
        Ok(value)
    }
}
/// A real Parquet parser: at most one row group and one batch per column are
/// retained. `SourceReader` owns the shared compressed preload allocation.
pub struct FileParser {
    reader: SerializedFileReader<SourceReader>,
    source: SourceReader,
    columns: Vec<Column>,
    names: Vec<String>,
    group: usize,
    group_rows: i64,
    group_read: i64,
    pub read_rows: i64,
    first_group_preload_bytes: i64,
    batch_size: usize,
    pub row_id: i64,
    closed: bool,
    location: chrono_tz::Tz,
    infos: Vec<ParquetColumnType>,
}
impl FileParser {
    pub fn new(source: SourceReader) -> Result<Self> {
        Self::new_with_location(source, "UTC")
    }
    pub fn new_with_location(source: SourceReader, location: &str) -> Result<Self> {
        let location = if location.is_empty() { "UTC" } else { location }
            .parse::<chrono_tz::Tz>()
            .map_err(|_| Error(format!("invalid location {location}")))?;
        let reader = crate::parser::open_file_reader(source.clone())?;
        let metadata = reader.metadata();
        let schema = metadata.file_metadata().schema_descr();
        if schema
            .columns()
            .iter()
            .any(|c| c.max_rep_level() > 0 || c.path().parts().len() != 1)
        {
            return Err(Error(
                "nested or repeated Parquet fields are unsupported".into(),
            ));
        }
        for descriptor in schema.columns() {
            normalized_logical_type(descriptor)?;
        }
        let names = schema
            .columns()
            .iter()
            .map(|c| c.name().to_lowercase())
            .collect();
        let mut ranges = vec![];
        let mut column_ranges = vec![];
        use parquet::file::reader::Length;
        let old_parquet_mr = metadata
            .file_metadata()
            .created_by()
            .and_then(|created| created.strip_prefix("parquet-mr version "))
            .and_then(|version| {
                let mut parts = version.split(|c: char| !c.is_ascii_digit());
                Some((
                    parts.next()?.parse::<u32>().ok()?,
                    parts.next()?.parse::<u32>().ok()?,
                    parts.next()?.parse::<u32>().ok()?,
                ))
            })
            .is_some_and(|version| version < (1, 2, 9));
        let file_meta = crate::reader_wrapper::FileMeta {
            source_size: source.len() as i64,
            old_parquet_mr,
            row_groups: metadata
                .row_groups()
                .iter()
                .map(|group| {
                    group
                        .columns()
                        .iter()
                        .map(|column| crate::reader_wrapper::ColumnChunkMeta {
                            data_page_offset: column.data_page_offset(),
                            dictionary_page_offset: column.dictionary_page_offset(),
                            total_compressed_size: column.compressed_size(),
                        })
                        .collect()
                })
                .collect(),
        };
        for index in 0..file_meta.row_groups.len() {
            let range = crate::reader_wrapper::row_group_range_from_meta(&file_meta, index)?;
            if range.column_starts.is_empty() {
                continue;
            }
            if range.start < 0 || range.end < range.start {
                return Err(Error("invalid column chunk range".into()));
            }
            ranges.push((range.start as u64, range.end as u64));
            column_ranges.extend(
                range
                    .column_starts
                    .into_iter()
                    .zip(range.column_ends)
                    .map(|(start, end)| (start as u64, end as u64)),
            );
        }
        let first_group_preload_bytes = ranges.first().map_or(0, |&(start, end)| {
            if end - start <= crate::source_reader::ROW_GROUP_THRESHOLD {
                (end - start) as i64
            } else {
                0
            }
        });
        source.set_ranges(ranges, column_ranges).map_err(error)?;
        let mut parser = Self {
            reader,
            source,
            columns: vec![],
            names,
            group: 0,
            group_rows: 0,
            group_read: 0,
            read_rows: 0,
            first_group_preload_bytes,
            batch_size: crate::parser::READ_BATCH_SIZE,
            row_id: 0,
            closed: false,
            location,
            infos: vec![],
        };
        if parser.reader.num_row_groups() > 0 {
            parser.build_group()?;
        }
        Ok(parser)
    }
    fn build_group(&mut self) -> Result<()> {
        self.columns.clear();
        self.infos.clear();
        let group = self.reader.get_row_group(self.group).map_err(error)?;
        self.group_rows = self.reader.metadata().row_group(self.group).num_rows();
        self.group_read = 0;
        for index in 0..group.num_columns() {
            let descriptor = self
                .reader
                .metadata()
                .file_metadata()
                .schema_descr()
                .column(index);
            let logical = normalized_logical_type(&descriptor)?;
            let metadata = self.reader.metadata().file_metadata();
            let spark = SparkFileMeta {
                created_by: metadata.created_by().unwrap_or_default().into(),
                key_values: metadata
                    .key_value_metadata()
                    .into_iter()
                    .flatten()
                    .filter_map(|kv| kv.value.clone().map(|value| (kv.key.clone(), value)))
                    .collect(),
            };
            let int96 = descriptor.physical_type() == parquet::basic::Type::INT96;
            let temporal = int96
                || matches!(logical, Some(LogicalType::Date))
                || matches!(&logical, Some(LogicalType::Timestamp(t)) if t.unit != TimeUnit::NANOS);
            let rebase = if temporal {
                let cutoff =
                    AppVersion::parse_spark(if int96 { "3.1.0" } else { "3.0.0" }).unwrap();
                let zone = spark_rebase_time_zone_id(
                    &spark,
                    &cutoff,
                    if int96 {
                        "org.apache.spark.legacyINT96"
                    } else {
                        "org.apache.spark.legacyDateTime"
                    },
                    self.location.name(),
                );
                if zone.is_empty() {
                    None
                } else {
                    Some(SparkRebaseMicrosLookup::new(&zone)?)
                }
            } else {
                None
            };
            let info = ParquetColumnType {
                logical,
                spark_rebase: rebase,
            };
            self.infos.push(info.clone());
            self.columns.push(Column {
                reader: group.get_column_reader(index).map_err(error)?,
                batch_size: self.batch_size,
                nullable: descriptor.max_def_level() > 0,
                location: self.location,
                info,
                rows: VecDeque::new(),
            });
        }
        Ok(())
    }
    /// Mirrors the Go test's readBatchSize override without changing global state.
    pub fn set_batch_size(&mut self, size: usize) -> Result<()> {
        if size == 0 {
            return Err(Error("batch size must be positive".into()));
        }
        self.batch_size = size;
        for column in &mut self.columns {
            column.batch_size = size;
        }
        Ok(())
    }
    pub fn read_row(&mut self) -> Result<Vec<Datum>> {
        if self.closed {
            return Err(Error("parser is closed".into()));
        }
        while self.group_read >= self.group_rows {
            self.columns.clear();
            self.group += 1;
            if self.group >= self.reader.num_row_groups() {
                return Err(Error("EOF".into()));
            }
            self.build_group()?;
        }
        let row = self
            .columns
            .iter_mut()
            .map(Column::next)
            .collect::<Result<Vec<_>>>()?;
        self.group_read += 1;
        self.read_rows += 1;
        self.row_id += 1;
        Ok(row)
    }
    pub fn columns(&self) -> &[String] {
        &self.names
    }
    pub fn column_is_utf8(&self, index: usize) -> bool {
        let descriptor = self
            .reader
            .metadata()
            .file_metadata()
            .schema_descr()
            .column(index);
        matches!(descriptor.logical_type_ref(), Some(LogicalType::String))
            || descriptor.converted_type() == CT::UTF8
    }
    pub fn total_rows(&self) -> i64 {
        self.reader.metadata().file_metadata().num_rows()
    }
    /// Replace the previous first-row-group preload charge with the shared
    /// whole-file allocation; leave the runtime's decoder peak estimate intact.
    pub fn adjust_memory_estimate(&self, previous_peak: i64) -> Result<i64> {
        if !self.source.whole_file_preloaded() || self.reader.num_row_groups() == 0 {
            return Ok(previous_peak);
        }
        let old_preload = self.first_group_preload_bytes;
        previous_peak
            .checked_sub(old_preload)
            .and_then(|peak| peak.checked_add(self.source.buffer_bytes() as i64))
            .filter(|&peak| peak >= 0)
            .ok_or_else(|| Error("invalid parquet memory estimate".into()))
    }
    pub fn source(&self) -> &SourceReader {
        &self.source
    }
    pub fn close(&mut self) {
        self.columns.clear();
        self.source.close();
        self.closed = true;
    }
}

/// Adapter retains the mydump parser/checkpoint interface for import consumers.
pub struct ImportParser {
    pub inner: FileParser,
    last: astersql_lightning_mydump::Row,
    names: Vec<String>,
    row_pool: Vec<Vec<astersql_lightning_mydump::Datum>>,
}
impl ImportParser {
    pub fn new(inner: FileParser) -> Self {
        Self {
            names: inner.columns().to_vec(),
            inner,
            last: Default::default(),
            row_pool: vec![],
        }
    }
}
fn mydump_error(error: Error) -> astersql_lightning_mydump::MydumpError {
    if error.0 == "EOF" {
        astersql_lightning_mydump::MydumpError::Eof
    } else {
        astersql_lightning_mydump::MydumpError::Io(error.0)
    }
}
impl astersql_lightning_mydump::Parser for ImportParser {
    fn Pos(&self) -> (i64, i64) {
        (self.inner.read_rows, self.last.row_id)
    }
    fn SetPos(
        &mut self,
        pos: i64,
        row: i64,
    ) -> std::result::Result<(), astersql_lightning_mydump::MydumpError> {
        for _ in 0..pos - self.last.row_id {
            self.inner.read_row().map_err(mydump_error)?;
        }
        self.last.row_id = row;
        Ok(())
    }
    fn ScannedPos(&mut self) -> std::result::Result<i64, astersql_lightning_mydump::MydumpError> {
        use parquet::file::reader::Length;
        let file_size = self.inner.source.len() as i64;
        let total_rows = self.inner.total_rows();
        if total_rows <= 0 || self.inner.read_rows == total_rows {
            return Ok(file_size);
        }
        let progress = self.inner.read_rows as f64 / total_rows as f64;
        Ok((progress * file_size as f64) as i64)
    }
    fn Close(&mut self) -> std::result::Result<(), astersql_lightning_mydump::MydumpError> {
        self.inner.close();
        Ok(())
    }
    fn ReadRow(&mut self) -> std::result::Result<(), astersql_lightning_mydump::MydumpError> {
        self.last.row_id += 1;
        self.last.length = 0;
        let row = self.inner.read_row().map_err(mydump_error)?;
        self.last.length = datum_row_size(&row);
        let mut buffer = self.row_pool.pop().unwrap_or_default();
        buffer.clear();
        for (index, value) in row.into_iter().enumerate() {
            use astersql_lightning_mydump::Datum as D;
            let value: std::result::Result<D, astersql_lightning_mydump::MydumpError> = match value
            {
                Datum::Null => Ok(D::Null),
                Datum::Int(v) => Ok(D::I64(v)),
                Datum::UInt(v) => Ok(D::Bytes(v.to_string().into_bytes())),
                Datum::Bytes(v) => Ok(D::Bytes(v)),
                Datum::Decimal(v) => Ok(D::Bytes(v.into_bytes())),
                Datum::Float32(v) => Ok(D::Bytes(v.to_string().into_bytes())),
                Datum::Float64(v) => Ok(D::Bytes(v.to_string().into_bytes())),
                Datum::DurationMicros(v) => {
                    let text = format!(
                        "{:02}:{:02}:{:02}.{:06}",
                        v / 3_600_000_000,
                        v / 60_000_000 % 60,
                        v / 1_000_000 % 60,
                        v % 1_000_000
                    );
                    Ok(D::Bytes(text.into_bytes()))
                }
                Datum::TimeMicros(v) => {
                    let time = chrono::DateTime::from_timestamp_micros(v).ok_or_else(|| {
                        astersql_lightning_mydump::MydumpError::Io("timestamp out of range".into())
                    })?;
                    let text = if matches!(self.inner.infos[index].logical, Some(LogicalType::Date))
                    {
                        time.format("%Y-%m-%d").to_string()
                    } else {
                        time.format("%Y-%m-%d %H:%M:%S%.6f").to_string()
                    };
                    Ok(D::Bytes(text.into_bytes()))
                }
            };
            buffer.push(value?);
        }
        self.last.row = buffer;
        Ok(())
    }
    fn LastRow(&self) -> astersql_lightning_mydump::Row {
        self.last.clone()
    }
    fn RecycleRow(&mut self, row: astersql_lightning_mydump::Row) {
        self.row_pool.push(row.row);
    }
    fn Columns(&self) -> &[String] {
        &self.names
    }
    fn SetColumns(&mut self, c: Vec<String>) {
        self.names = c;
    }
    fn SetRowID(&mut self, id: i64) {
        self.last.row_id = id;
    }
}

fn datum_row_size(row: &[Datum]) -> u64 {
    row.iter()
        .map(|value| match value {
            Datum::Null => 0,
            Datum::Bytes(bytes) => bytes.len() as u64,
            _ => 8,
        })
        .sum()
}

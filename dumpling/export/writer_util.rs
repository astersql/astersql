// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

//
// 写入格式与缓冲工具，对应 Go `export/writer_util.go`。
// 负责 SQL/CSV/Parquet INSERT 生成、LazyStringWriter、writerPipe 切分逻辑及 WriteMeta。
// Writer 与 FileFormat 均依赖本模块完成“行 IR → 字节流”的转换。
// MakeRowReceiver/RowReceiver 在 ir 包定义，本模块只负责缓冲与 flush。

// 单条 INSERT/CSV 缓冲上限 1MiB，超过则 flush 到 ObjectWriter，与 Go lengthLimit 一致。
pub const lengthLimit: usize = 1_048_576;
// conf.FileType 字符串常量，与 Go 包级变量同名。
pub const FileFormatSQLTextString: &str = "sql";
pub const FileFormatCSVString: &str = "csv";
pub const FileFormatParquetString: &str = "parquet";

// 输出文件格式枚举，String/Extension 供日志与文件名后缀使用。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(i32)]
pub enum FileFormat {
    #[default]
    // 未识别或配置缺失时的占位值。
    FileFormatUnknown = 0,
    // 多行 INSERT SQL 文本。
    // SQL 文本 INSERT。
    FileFormatSQLText = 1,
    // RFC4180 风格 CSV（可配 dialect）。
    FileFormatCSV = 2,
    // 标准 Parquet 二进制。
    FileFormatParquet = 3,
}

impl FileFormat {
    // 人类可读格式名，用于日志字段。
    pub fn String(self) -> &'static str {
        match self {
            Self::FileFormatSQLText => "SQL",
            Self::FileFormatCSV => "CSV",
            Self::FileFormatParquet => "PARQUET",
            Self::FileFormatUnknown => "unknown",
        }
    }
    // 输出文件扩展名，与 conf.FileType 小写串一致。
    pub fn Extension(self) -> &'static str {
        match self {
            Self::FileFormatSQLText => FileFormatSQLTextString,
            Self::FileFormatCSV => FileFormatCSVString,
            Self::FileFormatParquet => FileFormatParquetString,
            Self::FileFormatUnknown => "unknown_format",
        }
    }
    // 按格式分派到具体 WriteInsert* 实现。
    pub fn WriteInsert(
        self,
        tctx: &tcontext::Context,
        conf: &Config,
        meta: &dyn TableMeta,
        ir: &mut dyn TableDataIR,
        writer: &mut dyn ObjectWriter,
        metrics: Option<&metrics>,
    ) -> Result<()> {
        match self {
            Self::FileFormatSQLText => WriteInsertSQL(tctx, conf, meta, ir, writer, metrics),
            Self::FileFormatCSV => WriteInsertInCsv(tctx, conf, meta, ir, writer, metrics),
            Self::FileFormatParquet => WriteInsertInParquet(tctx, conf, meta, ir, writer, metrics),
            // 未知格式显式报错，避免静默写错扩展名。
            Self::FileFormatUnknown => Err(errors_new("unknown file format")),
        }
    }
}

// CSV 转义与分隔选项，binaryFormat 由 dialect 映射决定二进制列编码。
#[derive(Clone, Debug)]
pub struct csvOption {
    // 列分隔符字节（默认逗号）。
    pub separator: Vec<u8>,
    // 字段引号字节（默认双引号）。
    pub delimiter: Vec<u8>,
    // NULL 字面量，MySQL 传统为 \N。
    pub nullValue: String,
    // 行结束符，默认 CRLF。
    pub lineTerminator: Vec<u8>,
    // 二进制列编码方式，随 CsvOutputDialect 变化。
    pub binaryFormat: BinaryFormat,
}

impl Default for csvOption {
    fn default() -> Self {
        // 默认 MySQL 兼容 CSV：逗号分隔、双引号、\\N NULL、CRLF。
        Self {
            separator: b",".to_vec(),
            delimiter: b"\"".to_vec(),
            // MySQL LOAD DATA 传统 NULL 表示。
            nullValue: "\\N".into(),
            lineTerminator: b"\r\n".to_vec(),
            binaryFormat: BinaryFormat::BinaryFormatUTF8,
        }
    }
}

// MySQL 反引号标识符转义：内部反引号加倍。
pub fn wrapBackTicks(identifier: &str) -> String {
    format!("`{}`", identifier.replace('`', "``"))
}

// 标识符转义：仅将内部反引号加倍，与 Go escapeString 一致。
pub fn escapeString(s: &str) -> String {
    s.replace('`', "``")
}

// 生成多行 VALUES 的 INSERT 语句；CompleteInsert 时在表名后附带列名列表。
pub fn WriteInsertSQL(
    _tctx: &tcontext::Context,
    conf: &Config,
    meta: &dyn TableMeta,
    ir: &mut dyn TableDataIR,
    writer: &mut dyn ObjectWriter,
    metrics: Option<&metrics>,
) -> Result<()> {
    let mut iter = ir.Rows();
    // Go 在空结果上返回迭代器错误，不写 INSERT 头。
    if !iter.HasNext() {
        if let Some(err) = iter.Error() {
            let _ = iter.Close();
            return Err(err);
        }
        return iter.Close();
    }
    let selected_field = meta.SelectedField();
    let mut bf = Vec::with_capacity(lengthLimit);
    let mut comments = meta.SpecialComments();
    while comments.HasNext() {
        bf.extend_from_slice(comments.Next().as_bytes());
        bf.push(b'\n');
    }
    let mut row_receiver = MakeRowReceiver(&meta.ColumnTypes());
    let mut rows_written = 0.0_f64;
    while iter.HasNext() {
        if bf.is_empty() || rows_written == 0.0 {
            // 新语句块：写 INSERT INTO ... VALUES 头。
            if !selected_field.is_empty() && selected_field != "*" {
                bf.extend_from_slice(
                    format!(
                        "INSERT INTO {} ({}) VALUES\n",
                        wrapBackTicks(&escapeString(meta.TableName())),
                        selected_field
                    )
                    .as_bytes(),
                );
            } else {
                bf.extend_from_slice(
                    format!(
                        "INSERT INTO {} VALUES\n",
                        wrapBackTicks(&escapeString(meta.TableName()))
                    )
                    .as_bytes(),
                );
            }
        } else {
            // 同语句内下一行 VALUES 前加逗号换行。
            bf.push(b',');
            bf.push(b'\n');
        }
        if !selected_field.is_empty() {
            iter.Decode(&mut row_receiver)?;
            // EscapeBackslash 控制 \0 \n 等 C 转义。
            row_receiver.WriteToBuffer(&mut bf, conf.EscapeBackslash);
        } else {
            // 全部列均为生成列时 Go 仍按 SELECT '' 推进行数，但输出空 tuple。
            bf.extend_from_slice(b"()");
        }
        rows_written += 1.0;
        // 缓冲达上限则 flush 并累计 metrics。
        if bf.len() >= lengthLimit {
            writer.Write(&bf)?;
            if let Some(m) = metrics {
                AddGauge(Some(&m.finishedSizeGauge), bf.len() as f64);
                AddGauge(Some(&m.finishedRowsGauge), rows_written);
            }
            rows_written = 0.0;
            bf.clear();
        }
        iter.Next();
        if let Some(err) = iter.Error() {
            // Go writerPipe preserves the successfully decoded prefix as a valid
            // INSERT statement, but does not count it as finished metrics.
            if !bf.is_empty() {
                bf.extend_from_slice(b";\n");
                writer.Write(&bf)?;
            }
            let _ = iter.Close();
            return Err(err);
        }
    }
    if !bf.is_empty() {
        // 语句收尾分号，与 MySQL 客户端习惯一致。
        bf.push(b';');
        bf.push(b'\n');
        writer.Write(&bf)?;
        if let Some(m) = metrics {
            AddGauge(Some(&m.finishedSizeGauge), bf.len() as f64);
            AddGauge(Some(&m.finishedRowsGauge), rows_written);
        }
    }
    iter.Close()
}

// CSV 行输出：可选 header、按 dialect 写 NULL/二进制，同样按 lengthLimit 分块 flush。
pub fn WriteInsertInCsv(
    _tctx: &tcontext::Context,
    conf: &Config,
    meta: &dyn TableMeta,
    ir: &mut dyn TableDataIR,
    writer: &mut dyn ObjectWriter,
    metrics: Option<&metrics>,
) -> Result<()> {
    let mut iter = ir.Rows();
    // Go 在空结果上返回迭代器错误，并且不会单独写 header。
    if !iter.HasNext() {
        if let Some(err) = iter.Error() {
            let _ = iter.Close();
            return Err(err);
        }
        return iter.Close();
    }
    let opt = csvOption {
        separator: conf.CsvSeparator.as_bytes().to_vec(),
        delimiter: conf.CsvDelimiter.as_bytes().to_vec(),
        nullValue: conf.CsvNullValue.clone(),
        lineTerminator: conf.CsvLineTerminator.as_bytes().to_vec(),
        binaryFormat: DialectBinaryFormatMap(conf.CsvOutputDialect),
    };
    let mut bf = Vec::with_capacity(lengthLimit);
    let selected_field = meta.SelectedField();
    if !conf.NoHeader && !meta.ColumnNames().is_empty() && !selected_field.is_empty() {
        // 首行写列名，字段用 delimiter 包裹。
        let names = meta.ColumnNames();
        for (i, n) in names.iter().enumerate() {
            if i > 0 {
                bf.extend_from_slice(&opt.separator);
            }
            bf.extend_from_slice(&opt.delimiter);
            escapeCSV(n.as_bytes(), &mut bf, conf.EscapeBackslash, &opt);
            bf.extend_from_slice(&opt.delimiter);
        }
        bf.extend_from_slice(&opt.lineTerminator);
    }
    let mut row_receiver = MakeRowReceiver(&meta.ColumnTypes());
    let mut rows_written = 0.0_f64;
    while iter.HasNext() {
        if !selected_field.is_empty() {
            iter.Decode(&mut row_receiver)?;
            // CSV 转义规则与 SQL 不同，由 WriteToBufferInCsv 处理。
            row_receiver.WriteToBufferInCsv(&mut bf, conf.EscapeBackslash, &opt);
        }
        bf.extend_from_slice(&opt.lineTerminator);
        rows_written += 1.0;
        if bf.len() >= lengthLimit {
            writer.Write(&bf)?;
            if let Some(m) = metrics {
                AddGauge(Some(&m.finishedSizeGauge), bf.len() as f64);
                AddGauge(Some(&m.finishedRowsGauge), rows_written);
            }
            rows_written = 0.0;
            bf.clear();
        }
        iter.Next();
        // 与 SQL writer 及 Go writerPipe 对齐：行迭代错误必须在已成功
        // 解码的行之后立刻传播，不能被最终 Close 吞掉。
        if let Some(err) = iter.Error() {
            // Preserve the successfully decoded CSV prefix. Metrics are only
            // committed on successful completion, matching Go's rollback.
            if !bf.is_empty() {
                writer.Write(&bf)?;
            }
            let _ = iter.Close();
            return Err(err);
        }
    }
    if !bf.is_empty() {
        writer.Write(&bf)?;
        if let Some(m) = metrics {
            AddGauge(Some(&m.finishedSizeGauge), bf.len() as f64);
            AddGauge(Some(&m.finishedRowsGauge), rows_written);
        }
    }
    iter.Close()
}

struct ParquetObjectWriter<'a> {
    inner: &'a mut dyn ObjectWriter,
    written: Arc<AtomicU64>,
}

impl std::io::Write for ParquetObjectWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let size = self
            .inner
            .Write(buf)
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        self.written.fetch_add(size as u64, Ordering::Relaxed);
        Ok(size)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn parquet_columns(meta: &dyn TableMeta) -> Result<Vec<astersql_dumpformat_parquetfile::Column>> {
    let infos = meta
        .ColumnInfos()
        .into_iter()
        .map(|info| -> Result<_> {
            Ok(astersql_dumpformat_parquetfile::ColumnInfo {
                name: info.Name,
                database_type_name: info.DatabaseTypeName,
                nullable: info.Nullable,
                precision: i32::try_from(info.Precision)
                    .map_err(|_| errors_new("parquet column precision is out of range"))?,
                scale: i32::try_from(info.Scale)
                    .map_err(|_| errors_new("parquet column scale is out of range"))?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    astersql_dumpformat_parquetfile::schema_builder::build_parquet_schema_from_columns(&infos)
        .map(|(_, columns)| columns)
        .map_err(|error| errors_new(error.to_string()))
}

fn parquet_schema(
    columns: &[astersql_dumpformat_parquetfile::Column],
) -> Result<parquet::schema::types::TypePtr> {
    use astersql_dumpformat_parquetfile::{LogicalType, PhysicalType, TimeUnit};
    use parquet::basic::{LogicalType as ParquetLogical, Repetition, TimeUnit as ParquetTimeUnit};
    use parquet::schema::types::Type;

    let mut fields = Vec::with_capacity(columns.len());
    for column in columns {
        let physical = match column.column_type.physical {
            PhysicalType::Boolean => parquet::basic::Type::BOOLEAN,
            PhysicalType::Int32 => parquet::basic::Type::INT32,
            PhysicalType::Int64 => parquet::basic::Type::INT64,
            PhysicalType::Float => parquet::basic::Type::FLOAT,
            PhysicalType::Double => parquet::basic::Type::DOUBLE,
            PhysicalType::ByteArray => parquet::basic::Type::BYTE_ARRAY,
            PhysicalType::FixedLenByteArray => parquet::basic::Type::FIXED_LEN_BYTE_ARRAY,
            PhysicalType::Int96 => parquet::basic::Type::INT96,
        };
        let logical = match column.column_type.logical {
            LogicalType::None => None,
            LogicalType::String => Some(ParquetLogical::String),
            LogicalType::Decimal { precision, scale } => {
                Some(ParquetLogical::decimal(scale, precision))
            }
            LogicalType::Timestamp {
                adjusted_to_utc,
                unit,
            } => Some(ParquetLogical::timestamp(
                adjusted_to_utc,
                match unit {
                    TimeUnit::Millis => ParquetTimeUnit::MILLIS,
                    TimeUnit::Micros => ParquetTimeUnit::MICROS,
                    TimeUnit::Nanos => ParquetTimeUnit::NANOS,
                },
            )),
            LogicalType::Date => Some(ParquetLogical::Date),
            LogicalType::Time {
                adjusted_to_utc,
                unit,
            } => Some(ParquetLogical::time(
                adjusted_to_utc,
                match unit {
                    TimeUnit::Millis => ParquetTimeUnit::MILLIS,
                    TimeUnit::Micros => ParquetTimeUnit::MICROS,
                    TimeUnit::Nanos => ParquetTimeUnit::NANOS,
                },
            )),
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
            .map_err(|error| errors_new(error.to_string()))?;
        fields.push(Arc::new(field));
    }
    Type::group_type_builder("schema")
        .with_repetition(Repetition::REQUIRED)
        .with_fields(fields)
        .build()
        .map(Arc::new)
        .map_err(|error| errors_new(error.to_string()))
}

fn write_parquet_group<W: std::io::Write + Send>(
    parquet_writer: &mut parquet::file::writer::SerializedFileWriter<W>,
    columns: &[astersql_dumpformat_parquetfile::Column],
    rows: &[Vec<Option<Vec<u8>>>],
) -> Result<()> {
    use astersql_dumpformat_parquetfile::{ColumnValue, PhysicalType};
    use parquet::column::writer::ColumnWriter;
    use parquet::data_type::{ByteArray, FixedLenByteArray};

    let mut group = parquet_writer
        .next_row_group()
        .map_err(|error| errors_new(error.to_string()))?;
    for (column_index, column) in columns.iter().enumerate() {
        let mut values = Vec::with_capacity(rows.len());
        let mut levels = column
            .allows_null_encoding
            .then(|| Vec::with_capacity(rows.len()));
        for row in rows {
            let raw = row
                .get(column_index)
                .ok_or_else(|| errors_new("parquet row has fewer values than columns"))?;
            let parsed = match raw {
                Some(raw) => astersql_dumpformat_parquetfile::column_value::parse_raw_column_value(
                    raw, column,
                )
                .map_err(|error| errors_new(error.to_string()))?,
                None if column.allows_null_encoding => (None, true),
                None => return Err(errors_new("required parquet column receives NULL")),
            };
            if let Some(levels) = &mut levels {
                levels.push(if parsed.1 { 0 } else { 1 });
            }
            if let Some(value) = parsed.0 {
                values.push(value);
            }
        }
        let mut writer = group
            .next_column()
            .map_err(|error| errors_new(error.to_string()))?
            .ok_or_else(|| errors_new("parquet schema has fewer columns than metadata"))?;
        let defs = levels.as_deref();
        match (column.column_type.physical, writer.untyped()) {
            (PhysicalType::Boolean, ColumnWriter::BoolColumnWriter(w)) => {
                let data = values
                    .into_iter()
                    .map(|v| match v {
                        ColumnValue::Bool(v) => Ok(v),
                        _ => Err(errors_new("parquet boolean type mismatch")),
                    })
                    .collect::<Result<Vec<_>>>()?;
                w.write_batch(&data, defs, None)
                    .map_err(|e| errors_new(e.to_string()))?;
            }
            (PhysicalType::Int32, ColumnWriter::Int32ColumnWriter(w)) => {
                let data = values
                    .into_iter()
                    .map(|v| match v {
                        ColumnValue::Int32(v) => Ok(v),
                        _ => Err(errors_new("parquet int32 type mismatch")),
                    })
                    .collect::<Result<Vec<_>>>()?;
                w.write_batch(&data, defs, None)
                    .map_err(|e| errors_new(e.to_string()))?;
            }
            (PhysicalType::Int64, ColumnWriter::Int64ColumnWriter(w)) => {
                let data = values
                    .into_iter()
                    .map(|v| match v {
                        ColumnValue::Int64(v) => Ok(v),
                        _ => Err(errors_new("parquet int64 type mismatch")),
                    })
                    .collect::<Result<Vec<_>>>()?;
                w.write_batch(&data, defs, None)
                    .map_err(|e| errors_new(e.to_string()))?;
            }
            (PhysicalType::Float, ColumnWriter::FloatColumnWriter(w)) => {
                let data = values
                    .into_iter()
                    .map(|v| match v {
                        ColumnValue::Float32(v) => Ok(v),
                        _ => Err(errors_new("parquet float type mismatch")),
                    })
                    .collect::<Result<Vec<_>>>()?;
                w.write_batch(&data, defs, None)
                    .map_err(|e| errors_new(e.to_string()))?;
            }
            (PhysicalType::Double, ColumnWriter::DoubleColumnWriter(w)) => {
                let data = values
                    .into_iter()
                    .map(|v| match v {
                        ColumnValue::Float64(v) => Ok(v),
                        _ => Err(errors_new("parquet double type mismatch")),
                    })
                    .collect::<Result<Vec<_>>>()?;
                w.write_batch(&data, defs, None)
                    .map_err(|e| errors_new(e.to_string()))?;
            }
            (PhysicalType::ByteArray, ColumnWriter::ByteArrayColumnWriter(w)) => {
                let data = values
                    .into_iter()
                    .map(|v| match v {
                        ColumnValue::Bytes(v) => Ok(ByteArray::from(v)),
                        _ => Err(errors_new("parquet byte array type mismatch")),
                    })
                    .collect::<Result<Vec<_>>>()?;
                w.write_batch(&data, defs, None)
                    .map_err(|e| errors_new(e.to_string()))?;
            }
            (PhysicalType::FixedLenByteArray, ColumnWriter::FixedLenByteArrayColumnWriter(w)) => {
                let data = values
                    .into_iter()
                    .map(|v| match v {
                        ColumnValue::FixedBytes(v) => Ok(FixedLenByteArray::from(v)),
                        _ => Err(errors_new("parquet fixed byte array type mismatch")),
                    })
                    .collect::<Result<Vec<_>>>()?;
                w.write_batch(&data, defs, None)
                    .map_err(|e| errors_new(e.to_string()))?;
            }
            _ => return Err(errors_new("parquet physical writer type mismatch")),
        }
        writer
            .close()
            .map_err(|error| errors_new(error.to_string()))?;
    }
    if group
        .next_column()
        .map_err(|error| errors_new(error.to_string()))?
        .is_some()
    {
        return Err(errors_new("parquet schema has more columns than metadata"));
    }
    group
        .close()
        .map_err(|error| errors_new(error.to_string()))?;
    Ok(())
}

// Parquet 导出：使用 parquet-rs 生成标准文件，并复用 dumpformat 的 Go 等价类型转换。
pub fn WriteInsertInParquet(
    tctx: &tcontext::Context,
    conf: &Config,
    meta: &dyn TableMeta,
    ir: &mut dyn TableDataIR,
    writer: &mut dyn ObjectWriter,
    metrics: Option<&metrics>,
) -> Result<()> {
    let mut iter = ir.Rows();
    if !iter.HasNext() {
        if let Some(err) = iter.Error() {
            let _ = iter.Close();
            return Err(err);
        }
        return iter.Close();
    }
    let columns = parquet_columns(meta)?;
    let schema = parquet_schema(&columns)?;
    let compression = match conf.ParquetCompressType {
        CompressType::NoCompression => parquet::basic::Compression::UNCOMPRESSED,
        CompressType::Gzip => parquet::basic::Compression::GZIP(Default::default()),
        CompressType::Snappy => parquet::basic::Compression::SNAPPY,
        CompressType::Zstd => parquet::basic::Compression::ZSTD(Default::default()),
        CompressType::Lzo => parquet::basic::Compression::LZO,
    };
    let properties = parquet::file::properties::WriterProperties::builder()
        .set_compression(compression)
        .set_data_page_size_limit(conf.ParquetPageSize.max(1) as usize)
        .build();
    let written = Arc::new(AtomicU64::new(0));
    let output = ParquetObjectWriter {
        inner: writer,
        written: written.clone(),
    };
    let mut parquet_writer =
        parquet::file::writer::SerializedFileWriter::new(output, schema, Arc::new(properties))
            .map_err(|error| errors_new(error.to_string()))?;
    let mut count = 0u64;
    let mut row_receiver = MakeRowReceiver(&meta.ColumnTypes());
    let selected_fields = meta.SelectedField();
    let group_limit = conf.ParquetRowGroupSize.max(1) as usize;
    let mut buffered_bytes = 0usize;
    let mut rows = Vec::new();
    while iter.HasNext() {
        if !selected_fields.is_empty() {
            iter.Decode(&mut row_receiver)?;
            let row = row_receiver
                .GetRawBytes()
                .into_iter()
                .map(|raw| raw.0)
                .collect::<Vec<_>>();
            buffered_bytes += row
                .iter()
                .map(|value| value.as_ref().map_or(0, Vec::len))
                .sum::<usize>();
            rows.push(row);
        }
        count += 1;
        iter.Next();
        if let Some(err) = iter.Error() {
            let _ = iter.Close();
            return Err(err);
        }
        let reached_group = buffered_bytes >= group_limit;
        let reached_file = conf.FileSize != UnspecifiedSize
            && written.load(Ordering::Relaxed) + buffered_bytes as u64 >= conf.FileSize;
        if !rows.is_empty() && (reached_group || reached_file || !iter.HasNext()) {
            write_parquet_group(&mut parquet_writer, &columns, &rows)?;
            rows.clear();
            buffered_bytes = 0;
        }
        if reached_file {
            break;
        }
    }
    parquet_writer
        .close()
        .map_err(|error| errors_new(error.to_string()))?;
    let finished_size = written.load(Ordering::Relaxed);
    if let Some(m) = metrics {
        AddGauge(Some(&m.finishedSizeGauge), finished_size as f64);
        AddGauge(Some(&m.finishedRowsGauge), count as f64);
        ObserveHistogram(Some(&m.writeTimeHistogram), 0.0);
    }
    tctx.L().Debug(
        "finish dumping parquet data",
        [Field::string("rows", count.to_string())],
    );
    iter.Close()
}

// 延迟打开 Storage 文件的 ObjectWriter：首次 Write 时才 Create 路径。
pub struct LazyStringWriter {
    pub storage: Arc<dyn Storage>,
    // path 为 storage 内相对路径，由 OutputTemplate 生成。
    pub path: String,
    // w 在 ensure 前为 None，避免空文件被创建。
    pub w: Option<Box<dyn ObjectWriter>>,
}

impl LazyStringWriter {
    pub fn new(storage: Arc<dyn Storage>, path: impl Into<String>) -> Self {
        Self {
            storage,
            path: path.into(),
            w: None,
        }
    }
    // 懒创建底层 ObjectWriter，失败则向上传播。
    fn ensure(&mut self) -> Result<()> {
        if self.w.is_none() {
            self.w = Some(self.storage.Create(&self.path)?);
        }
        Ok(())
    }
}

impl ObjectWriter for LazyStringWriter {
    fn Write(&mut self, data: &[u8]) -> Result<usize> {
        self.ensure()?;
        // ensure 成功后委托底层 storage writer。
        self.w.as_mut().unwrap().Write(data)
    }
    fn Close(&mut self) -> Result<()> {
        // 从未 Write 时 w 仍为 None，Close 直接成功。
        if let Some(mut w) = self.w.take() {
            w.Close()?;
        }
        Ok(())
    }
}

// 跟踪当前文件/语句累计字节，供 FileSize/StatementSize/Rows 切分决策。
pub struct writerPipe {
    // 当前输出文件已写字节数。
    pub currentFileSize: u64,
    // 当前 INSERT 语句已写字节数（可与文件计数不同步切分）。
    pub currentStatementSize: u64,
    // fileSizeLimit 为 UnspecifiedSize 时不切文件。
    pub fileSizeLimit: u64,
    // statementSizeLimit 为 UnspecifiedSize 时不切语句。
    pub statementSizeLimit: u64,
    // metrics 原始指针占位，与 Go writerPipe 字段对齐。
    pub metrics: Option<*const metrics>,
}

pub fn newWriterPipe(
    _w: Option<&mut dyn ObjectWriter>,
    file_size_limit: u64,
    statement_size_limit: u64,
    _metrics: Option<&metrics>,
    _labels: Option<&Labels>,
) -> writerPipe {
    // 计数器从 0 开始，limit 来自 conf.FileSize/StatementSize。
    writerPipe {
        currentFileSize: 0,
        currentStatementSize: 0,
        fileSizeLimit: file_size_limit,
        statementSizeLimit: statement_size_limit,
        metrics: None,
    }
}

impl writerPipe {
    // 写入 nbytes 后同时累加文件级与语句级计数。
    pub fn AddFileSize(&mut self, file_size: u64) {
        self.currentFileSize += file_size;
        self.currentStatementSize += file_size;
    }
    // 仅文件字节达限时切新文件。
    pub fn ShouldSwitchFile(&self) -> bool {
        // UnspecifiedSize 表示该维度不切分。
        self.fileSizeLimit != UnspecifiedSize && self.currentFileSize >= self.fileSizeLimit
    }
    // 语句切分条件宽于文件：file limit 也会触发 statement switch。
    pub fn ShouldSwitchStatement(&self) -> bool {
        // Go: file-size limit also forces a statement switch.
        (self.fileSizeLimit != UnspecifiedSize && self.currentFileSize >= self.fileSizeLimit)
            || (self.statementSizeLimit != UnspecifiedSize
                && self.currentStatementSize >= self.statementSizeLimit)
    }
}

// 元数据写出：special comments + MetaSQL，对应 Go WriteMeta。
pub fn WriteMeta(
    tctx: &tcontext::Context,
    meta: &mut dyn MetaIR,
    w: &mut dyn ObjectWriter,
) -> Result<()> {
    tctx.L().Debug(
        "start dumping meta data",
        [Field::string("target", meta.TargetName().to_string())],
    );
    // 先写 conditional comments（如 SET NAMES），再写 DDL。
    let mut spec = meta.SpecialComments();
    while spec.HasNext() {
        let line = format!("{}\n", spec.Next());
        w.Write(line.as_bytes())?;
    }
    let sql = meta.MetaSQL();
    // MetaSQL 通常已含末尾换行，不再额外追加。
    w.Write(sql.as_bytes())?;
    tctx.L().Debug(
        "finish dumping meta data",
        [Field::string("target", meta.TargetName().to_string())],
    );
    Ok(())
}

// 薄封装：把字符串写入 ObjectWriter，错误由 writer 返回。
pub fn write(tctx: &tcontext::Context, w: &mut dyn ObjectWriter, s: &str) -> Result<()> {
    // tctx 预留日志扩展，当前未使用。
    let _ = tctx;
    w.Write(s.as_bytes())?;
    Ok(())
}

// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// Parquet 导入解析器：按 row group 读行、拒绝不支持类型、估算内存。
//
// 前半为贴近 Go 的机械翻译草稿；后半为可编译的简化 Parser 实现。
// limitations under the License.

// Parquet 导入解析器如何按 row group/column reader 读取数据并估算内存。
//
// defaultBufSize specifies the default size of skip buffer.
// Skip buffer is used when reading data from the cloud. If there is a gap
// between the current read position and the last read position, these
// data is stored in this buffer to avoid potentially reopening the
// underlying file when the gap size is less than the buffer size.
// pub const defaultBufSize: usize = 64 * 1024;
//
// unsupportedParquetTypes 对应 Go 的包级 map，用于拒绝当前导入路径暂不支持的 logical/converted type。
// pub fn unsupportedParquetTypes() -> HashMap<schema::ConvertedType, struct{}> {
//     let mut types = HashMap::new();
// TODO(joechenrh): support read list type as vector
//     types.insert(schema::ConvertedTypes::List, struct{} {});
// 以下类型不是 aurora/snowflake 导出的常见数据形态，Go 实现暂不支持。
//     types.insert(schema::ConvertedTypes::Map, struct{} {});
//     types.insert(schema::ConvertedTypes::MapKeyValue, struct{} {});
//     types.insert(schema::ConvertedTypes::Interval, struct{} {});
//     types.insert(schema::ConvertedTypes::NA, struct{} {});
//     types
// }
//
// readBatchSize is the number of rows to read in a single batch
// from parquet column reader. Modified in test.
// pub static mut readBatchSize: i32 = 128;
//
// FileMeta contains some analyzed metadata for a parquet file.
// FileMeta 对应 Go 的导出结构体，允许调用方传入 allocator 与时区。
// pub struct FileMeta {
//     pub allocator: memory::Allocator,
//     pub Loc: *mut time::Location,
// }
//
// estimateRowSize 对应 Go 的行大小估算：字符串按实际字节数，其他非 NULL Datum 粗略按 8 字节。
// pub fn estimateRowSize(row: &[types::Datum]) -> i32 {
//     let mut length = 0;
//     for v in row {
//         if v.IsNull() {
//             continue;
//         }
//         if v.Kind() == types::KindString {
//             length += v.GetBytes().len() as i32;
//         } else {
//             length += 8;
//         }
//     }
//     length
// }
//
// innerReader 对应 Go 泛型接口：从 Parquet column reader 当前 page 中读取一批 typed values。
// pub trait innerReader<T: parquet::ColumnTypes> {
//     fn ReadBatchInPage(
//         &mut self,
//         batchSize: i64,
//         values: &mut [T],
//         defLvls: &mut [i16],
//         repLvls: &mut [i16],
//     ) -> Result<(i64, i32), Error>;
// }
//
// iterator 对应 Go 的内部 trait：rowGroupParser 只依赖设置 reader、读取下一个 Datum 和关闭资源。
// pub trait iterator {
//     fn SetReader(&mut self, colReader: file::ColumnChunkReader);
//     fn Next(&mut self, datum: &mut types::Datum) -> Result<(), Error>;
//     fn Close(&mut self) -> Result<(), Error>;
// }
//
// columnIterator 对应 Go 的泛型迭代器，缓存 definition/repetition level 与 values 批次。
// 该迭代器不支持并发使用，字段偏移值需要按 Next 调用顺序推进。
// pub struct columnIterator<T: parquet::ColumnTypes, R: innerReader<T>> {
//     pub baseReader: file::ColumnChunkReader,
//     pub reader: R,
//     pub batchSize: i64,
//     pub valueOffset: i32,
//     pub valuesBuffered: i32,
//     pub levelOffset: i64,
//     pub levelsBuffered: i64,
//     pub defLevels: Vec<i16>,
//     pub repLevels: Vec<i16>,
//     pub values: Vec<T>,
//     pub setter: setter<T>,
// }
//
// newColumnIterator creates a new generic column iterator
// The iterator should not be used in parallel.
// newColumnIterator 对应 Go 构造函数，按 batchSize 预分配 level 和 value 缓冲。
// pub fn newColumnIterator<T: parquet::ColumnTypes, R: innerReader<T>>(
//     batchSize: i32,
//     getter: setter<T>,
// ) -> columnIterator<T, R> {
//     columnIterator {
//         baseReader: file::ColumnChunkReader::nil(),
//         reader: R::default(),
//         batchSize: batchSize as i64,
//         valueOffset: 0,
//         valuesBuffered: 0,
//         levelOffset: 0,
//         levelsBuffered: 0,
//         defLevels: vec![0; batchSize as usize],
//         repLevels: vec![0; batchSize as usize],
//         values: vec![T::default(); batchSize as usize],
//         setter: getter,
//     }
// }
//
// impl<T: parquet::ColumnTypes, R: innerReader<T>> iterator for columnIterator<T, R> {
// SetReader sets the column reader for the iterator.
// Remember to call Close() before setting a new reader.
//     fn SetReader(&mut self, colReader: file::ColumnChunkReader) {
//         self.baseReader = colReader;
// Go 使用类型断言 colReader.(R)；保留该动态 reader 绑定语义。
//         self.reader = self.baseReader.as_inner_reader::<R>();
//     }
//
//     fn Close(&mut self) -> Result<(), Error> {
//         if self.baseReader.is_nil() {
//             return Ok(());
//         }
//         let err = self.baseReader.Close();
//         self.baseReader = file::ColumnChunkReader::nil();
//         err
//     }
//
//     fn Next(&mut self, d: &mut types::Datum) -> Result<(), Error> {
//         if self.levelOffset == self.levelsBuffered {
//             self.readNextBatch().map_err(errors::Trace)?;
//             if self.levelsBuffered == 0 {
//                 return Err(io::EOF);
//             }
//         }
//
// definition level 小于列最大 definition level 时代表 NULL，不消费 valueOffset。
//         let defLevel = self.defLevels[self.levelOffset as usize];
//         self.levelOffset += 1;
//         if defLevel < self.baseReader.Descriptor().MaxDefinitionLevel() {
//             d.SetNull();
//             return Ok(());
//         }
//
//         let value = self.values[self.valueOffset as usize].clone();
//         self.valueOffset += 1;
//         (self.setter)(value, d)
//     }
// }
//
// impl<T: parquet::ColumnTypes, R: innerReader<T>> columnIterator<T, R> {
// readNextBatch 对应 Go 的批量读取：values 可能浅拷贝自 page 内部 buffer，调用者不能长期持有。
//     pub fn readNextBatch(&mut self) -> Result<(), Error> {
//         let (levelsBuffered, valuesBuffered) = self.reader.ReadBatchInPage(
//             self.batchSize,
//             &mut self.values,
//             &mut self.defLevels,
//             &mut self.repLevels,
//         )?;
//         self.levelsBuffered = levelsBuffered;
//         self.valuesBuffered = valuesBuffered;
//         self.valueOffset = 0;
//         self.levelOffset = 0;
//         Ok(())
//     }
// }
//
// createColumnIterator 对应 Go 的 physical type switch，绑定对应 column reader 与 Datum setter。
// pub fn createColumnIterator(
//     tp: parquet::Type,
//     converted: &convertedType,
//     loc: *mut time::Location,
//     batchSize: i32,
// ) -> Option<Box<dyn iterator>> {
//     match tp {
//         parquet::Types::Boolean => Some(Box::new(newColumnIterator::<bool, file::BooleanColumnChunkReader>(
//             batchSize,
//             getBoolDataSetter,
//         ))),
//         parquet::Types::Int32 => Some(Box::new(newColumnIterator::<i32, file::Int32ColumnChunkReader>(
//             batchSize,
//             getInt32Setter(converted, loc),
//         ))),
//         parquet::Types::Int64 => Some(Box::new(newColumnIterator::<i64, file::Int64ColumnChunkReader>(
//             batchSize,
//             getInt64Setter(converted, loc),
//         ))),
//         parquet::Types::Float => Some(Box::new(newColumnIterator::<f32, file::Float32ColumnChunkReader>(
//             batchSize,
//             setFloat32Data,
//         ))),
//         parquet::Types::Double => Some(Box::new(newColumnIterator::<f64, file::Float64ColumnChunkReader>(
//             batchSize,
//             setFloat64Data,
//         ))),
//         parquet::Types::Int96 => Some(Box::new(newColumnIterator::<parquet::Int96, file::Int96ColumnChunkReader>(
//             batchSize,
//             getInt96Setter(converted, loc),
//         ))),
//         parquet::Types::ByteArray => Some(Box::new(newColumnIterator::<parquet::ByteArray, file::ByteArrayColumnChunkReader>(
//             batchSize,
//             getByteArraySetter(converted),
//         ))),
//         parquet::Types::FixedLenByteArray => Some(Box::new(newColumnIterator::<parquet::FixedLenByteArray, file::FixedLenByteArrayColumnChunkReader>(
//             batchSize,
//             getFixedLenByteArraySetter(converted),
//         ))),
//         _ => None,
//     }
// }
//
// convertedType is older representation of the logical type in parquet
// ref: https://github.com/apache/parquet-format/blob/master/LogicalTypes.md
// convertedType 对应 Go 的内部结构，缓存 logical/converted type、decimal metadata 与 Spark rebase 信息。
// pub struct convertedType {
//     pub converted: schema::ConvertedType,
//     pub decimalMeta: schema::DecimalMetadata,
// true 表示 epoch 数值具有 instant 语义，需要转到 parser location；false 保留“as-if UTC”墙上时间。
//     pub IsAdjustedToUTC: bool,
// sparkRebaseMicros 非空时说明 footer 标记了 Spark legacy 日历，需要古早日期/时间戳 rebase。
//     pub sparkRebaseMicros: sparkRebaseMicrosLookup,
// }
//
// rowGroupParser parses rows from one parquet row group.
// rowGroupParser 对应 Go 的单 row group 解析器，持有每列 reader 和 iterator。
// pub struct rowGroupParser {
//     pub rowGroup: i32,
//     pub readRows: i64,
//     pub totalRows: i64,
//     pub readers: Vec<file::Reader>,
//     pub iterators: Vec<Box<dyn iterator>>,
// }
//
// impl rowGroupParser {
// init creates column iterators for each column.
// 如果中途失败，Go defer 会关闭已经创建的 iterator；这里用显式错误收尾保留资源释放语义。
//     pub fn init(&mut self, colTypes: &[convertedType], loc: *mut time::Location) -> Result<(), Error> {
//         let meta = self.readers[0].MetaData();
//         let numCols = meta.Schema.NumColumns();
//         self.iterators = Vec::with_capacity(numCols as usize);
//
//         for idx in 0..numCols {
//             let tp = meta.Schema.Column(idx).PhysicalType();
//             let mut iter = match createColumnIterator(tp, &colTypes[idx as usize], loc, unsafe { readBatchSize }) {
//                 Some(iter) => iter,
//                 None => {
//                     self.closeIteratorsQuietly();
//                     return Err(Error::new(format!("unsupported parquet type {}", tp.String())));
//                 }
//             };
//
//             let rowGroup = self.readers[idx as usize].RowGroup(self.rowGroup);
//             let colReader = match rowGroup.Column(idx) {
//                 Ok(reader) => reader,
//                 Err(err) => {
//                     self.closeIteratorsQuietly();
//                     return Err(errors::Trace(err));
//                 }
//             };
//             iter.SetReader(colReader);
//             self.iterators.push(iter);
//         }
//         self.totalRows = meta.RowGroups[self.rowGroup as usize].NumRows;
//         Ok(())
//     }
//
//     pub fn isDone(&self) -> bool {
//         self.readRows == self.totalRows
//     }
//
//     pub fn readRow(&mut self, row: &mut [types::Datum]) -> Result<(), Error> {
//         if self.isDone() {
//             return Err(io::EOF);
//         }
//         for (col, iter) in self.iterators.iter_mut().enumerate() {
//             iter.Next(&mut row[col])
//                 .map_err(|err| errors::Annotate(err, "parquet read column failed"))?;
//         }
//         self.readRows += 1;
//         Ok(())
//     }
//
//     pub fn Close(&mut self) -> Result<(), Error> {
//         let mut onceErr = common::OnceError::default();
//         for iter in &mut self.iterators {
//             onceErr.Set(iter.Close());
//         }
//         for r in &mut self.readers {
//             onceErr.Set(r.Close());
//         }
//         onceErr.Get()
//     }
//
//     fn closeIteratorsQuietly(&mut self) {
//         for iter in &mut self.iterators {
//             let _ = iter.Close();
//         }
//     }
// }
//
// Parser parses a parquet file for import
// Parser 对应 Go 的主解析器，维护文件元数据、列类型、对象存储读取状态、row pool 和进度计数。
// pub struct Parser {
//     pub fileMeta: metadata::FileMetaData,
//     pub colTypes: Vec<convertedType>,
//     pub colNames: Vec<String>,
//     pub ctx: context::Context,
//     pub store: storeapi::Storage,
//     pub path: String,
//     pub prop: parquet::ReaderProperties,
//     pub loc: *mut time::Location,
//     pub alloc: memory::Allocator,
//     pub rowGroup: Option<rowGroupParser>,
//     pub rowPool: zeropool::Pool<Vec<types::Datum>>,
//     pub curRowGroup: i32,
//     pub totalRowGroup: i32,
//     pub totalRows: i64,
//     pub totalReadRows: i64,
//     pub totalReadBytes: i32,
//     pub lastRow: parsedef::Row,
//     pub logger: log::Logger,
// }
//
// impl Parser {
// Init initializes the Parquet parser and allocate necessary buffers
//     pub fn Init(&mut self, loc: *mut time::Location) -> Result<(), Error> {
//         self.totalRowGroup = self.fileMeta.NumRowGroups();
//         if self.totalRowGroup == 0 {
//             return Ok(());
//         }
//         self.totalRows = self.fileMeta.NumRows;
//         self.loc = if loc.is_null() { timeutil::SystemLocation() } else { loc };
//         self.buildRowGroupParser()
//     }
//
//     pub fn buildRowGroupParser(&mut self) -> Result<(), Error> {
//         let builder = self.getBuilder()?;
//         let (mut eg, egCtx) = util::NewErrorGroupWithRecoverWithCtx(self.ctx.clone());
//         eg.SetLimit(8);
//
//         let mut readers: Vec<Option<file::Reader>> = vec![None; self.fileMeta.NumColumns() as usize];
//         for i in 0..self.fileMeta.NumColumns() {
// Go 在每列上并发打开 reader；失败时 defer 关闭已打开 reader。
//             eg.Go(|| {
//                 if egCtx.Done().is_closed() {
//                     return Err(egCtx.Err());
//                 }
//                 let wrapper = builder(i).map_err(errors::Trace)?;
//                 let reader = match file::NewParquetReader(
//                     wrapper,
//                     file::WithReadProps(self.prop.clone()),
//                     file::WithMetadata(self.fileMeta.clone()),
//                 ) {
//                     Ok(reader) => reader,
//                     Err(err) => {
//                         let _ = wrapper.Close();
//                         return Err(errors::Trace(err));
//                     }
//                 };
//                 readers[i as usize] = Some(reader);
//                 Ok(())
//             });
//         }
//
//         if let Err(err) = eg.Wait() {
//             for reader in readers.iter_mut().flatten() {
//                 let _ = reader.Close();
//             }
//             return Err(errors::Trace(err));
//         }
//
//         let mut rgp = rowGroupParser {
//             rowGroup: self.curRowGroup,
//             readRows: 0,
//             totalRows: 0,
//             readers: readers.into_iter().map(|r| r.unwrap()).collect(),
//             iterators: Vec::new(),
//         };
//         rgp.init(&self.colTypes, self.loc).map_err(errors::Trace)?;
//         self.rowGroup = Some(rgp);
//         Ok(())
//     }
//
//     pub fn getBuilder(&mut self) -> Result<Box<dyn Fn(i32) -> Result<Box<dyn readerAtSeekerCloser>, Error>>, Error> {
//         let ranges = rowGroupRangeFromMeta(&self.fileMeta, self.curRowGroup)?;
//         if ranges.end - ranges.start <= rowGroupInMemoryThreshold as i64 {
//             let base = newInMemoryReaderBase(self.ctx.clone(), self.store.clone(), self.path.clone(), ranges.clone())
//                 .map_err(errors::Trace)?;
//             self.logger.Debug("use in memory reader for parquet file", zap::String("path", &self.path));
//             return Ok(Box::new(move |c| {
//                 Ok(Box::new(inMemoryReaderWrapper {
//                     base: base.clone(),
//                     pos: ranges.columnStarts[c as usize],
//                     fileSize: self.fileMeta.GetSourceFileSize(),
//                 }))
//             }));
//         }
//
//         Ok(Box::new(move |c| {
//             newReaderWrapper(
//                 self.ctx.clone(),
//                 self.store.clone(),
//                 self.path.clone(),
//                 Some(storeapi::ReaderOption {
//                     StartOffset: Some(ranges.columnStarts[c as usize]),
//                     EndOffset: Some(ranges.columnEnds[c as usize]),
//                 }),
//             )
//         }))
//     }
//
//     pub fn moveToNextRowGroup(&mut self) -> Result<(), Error> {
//         if let Some(rowGroup) = &mut self.rowGroup {
//             rowGroup.Close()?;
//             self.rowGroup = None;
//         }
//         self.curRowGroup += 1;
//         if self.curRowGroup >= self.totalRowGroup {
//             return Err(io::EOF);
//         }
//         self.buildRowGroupParser()
//     }
//
// readSingleRow read one row internally and store them in the row buffer.
// The data read is shallow copied from the internal buffer of parquet reader,
// so copy it if you need to keep the data before the next read.
//     pub fn readSingleRow(&mut self, row: &mut [types::Datum]) -> Result<(), Error> {
//         if self.rowGroup.as_ref().map_or(true, |rgp| rgp.isDone()) {
//             self.moveToNextRowGroup()?;
//         }
//         self.rowGroup.as_mut().unwrap().readRow(row)?;
//         self.totalReadBytes += estimateRowSize(row);
//         self.totalReadRows += 1;
//         Ok(())
//     }
//
// Pos returns the currently row number of the parquet file
//     pub fn Pos(&self) -> (i64, i64) {
//         (self.totalReadRows, self.lastRow.RowID)
//     }
//
// SetPos implements the Parser interface.
// For parquet file, this interface will read and discard the first `pos` rows,
// and set the current row ID to `rowID`
//     pub fn SetPos(&mut self, pos: i64, rowID: i64) -> Result<(), Error> {
//         let mut row = self.rowPool.Get();
// Go 用 defer 归还 row pool；显式在函数尾和错误路径归还。
//         let toRead = pos - self.lastRow.RowID;
//         for _ in 0..toRead {
//             if let Err(err) = self.readSingleRow(&mut row) {
//                 self.rowPool.Put(row);
//                 return Err(err);
//             }
//         }
//         self.rowPool.Put(row);
//         self.lastRow.RowID = rowID;
//         Ok(())
//     }
//
// ScannedPos implements the Parser interface.
// For parquet we use the size of all read datum to estimate the scanned position.
//     pub fn ScannedPos(&self) -> Result<i64, Error> {
//         Ok(self.totalReadBytes as i64)
//     }
//
// Close closes the parquet file of the parser.
// It implements the Parser interface.
//     pub fn Close(&mut self) -> Result<(), Error> {
// Go defer 中如果 allocator 实现 Close 会关闭 allocator；这里保留资源收尾顺序。
//         let mut onceErr = common::OnceError::default();
//         if let Some(rowGroup) = &mut self.rowGroup {
//             if let Err(err) = rowGroup.Close() {
//                 onceErr.Set(Err(err.clone()));
//                 self.logger.Warn("Close parquet parser get error", zap::Error(err));
//             }
//             self.rowGroup = None;
//         }
//         if let Some(closer) = self.alloc.as_closer() {
//             closer.Close();
//         }
//         onceErr.Get()
//     }
//
// ReadRow reads a row in the parquet file by the parser.
// The read data is shallow copied from the internal buffer of parquet reader,
// so it's only valid before the next ReadRow call.
//     pub fn ReadRow(&mut self) -> Result<(), Error> {
//         self.lastRow.RowID += 1;
//         self.lastRow.Length = 0;
//         let mut row = self.rowPool.Get();
//         if let Err(err) = self.readSingleRow(&mut row) {
//             self.rowPool.Put(row);
//             return Err(err);
//         }
//         self.lastRow.Row = row;
//         self.lastRow.Length = estimateRowSize(&self.lastRow.Row);
//         Ok(())
//     }
//
// LastRow gets the last row parsed by the parser.
// It implements the Parser interface.
//     pub fn LastRow(&self) -> parsedef::Row {
//         self.lastRow.clone()
//     }
//
// RecycleRow implements the Parser interface.
//     pub fn RecycleRow(&mut self, row: parsedef::Row) {
//         self.rowPool.Put(row.Row);
//     }
//
// Columns returns the _lower-case_ column names corresponding to values in
// the LastRow.
//     pub fn Columns(&self) -> Vec<String> {
//         self.colNames.clone()
//     }
//
// SetColumns set restored column names to parser；Go 实现为空操作。
//     pub fn SetColumns(&mut self, _columns: Vec<String>) {}
//
// SetLogger sets the logger used in the parser.
// It implements the Parser interface.
//     pub fn SetLogger(&mut self, l: log::Logger) {
//         self.logger = l;
//     }
//
// SetRowID sets the rowID in a parquet file when we start a compressed file.
// It implements the Parser interface.
//     pub fn SetRowID(&mut self, rowID: i64) {
//         self.lastRow.RowID = rowID;
//     }
// }
//
// ReadRowCount reads the parquet file row count.
// pub fn ReadRowCount(ctx: context::Context, store: storeapi::Storage, path: String) -> Result<i64, Error> {
//     let r = store.Open(ctx, path, None).map_err(errors::Trace)?;
// Go defer 关闭 reader；在读取 metadata 后显式关闭。
//     let reader = file::NewParquetReader(readerWrapper { ReadSeekCloser: r.clone() })?;
//     let rows = reader.MetaData().NumRows;
//     let _ = r.Close();
//     Ok(rows)
// }
//
// NewParser generates a parquet parser.
// NewParser 对应 Go 构造函数：建立 Parquet reader、分析 schema、初始化 row pool 并构建首个 row group parser。
// pub fn NewParser(
//     ctx: context::Context,
//     store: storeapi::Storage,
//     r: storeapi::ReadSeekCloser,
//     path: String,
//     meta: FileMeta,
// ) -> Result<Parser, Error> {
//     let logger = log::Wrap(logutil::Logger(ctx.clone()));
//     let wrapper = readerWrapper { ReadSeekCloser: r.clone() };
//     let allocator = if meta.allocator.is_nil() {
//         memory::NewGoAllocator()
//     } else {
//         meta.allocator
//     };
//     let mut prop = parquet::NewReaderProperties(allocator.clone());
//     prop.BufferedStreamEnabled = true;
//     prop.BufferSize = 1024;
//
//     let reader = file::NewParquetReader(wrapper, file::WithReadProps(prop.clone())).map_err(errors::Trace)?;
//     let fileMeta = reader.MetaData();
//     let fileSchema = fileMeta.Schema;
//     let mut colTypes = vec![convertedType::default(); fileSchema.NumColumns() as usize];
//     let mut colNames = Vec::with_capacity(fileSchema.NumColumns() as usize);
//     let effectiveLoc = if meta.Loc.is_null() { timeutil::SystemLocation() } else { meta.Loc };
//
//     for i in 0..colTypes.len() {
//         let desc = fileSchema.Column(i as i32);
//         colNames.push(strings::ToLower(desc.Name()));
//         let logicalType = desc.LogicalType();
//         if logicalType.IsValid() {
//             (colTypes[i].converted, colTypes[i].decimalMeta) = logicalType.ToConvertedType();
//             if let schema::LogicalType::TimestampLogicalType(t) = logicalType {
// ToConvertedType 可能在 IsAdjustedToUTC=false 时返回 none，因此 Go 这里再次按 unit 补 converted type。
//                 match t.TimeUnit() {
//                     schema::TimeUnitMillis => colTypes[i].converted = schema::ConvertedTypes::TimestampMillis,
//                     schema::TimeUnitMicros => colTypes[i].converted = schema::ConvertedTypes::TimestampMicros,
//                     _ => return Err(Error::new(format!("unsupported timestamp time unit {}", t.TimeUnit()))),
//                 }
//                 colTypes[i].IsAdjustedToUTC = t.IsAdjustedToUTC();
//             } else if let schema::LogicalType::TimeLogicalType(t) = logicalType {
//                 match t.TimeUnit() {
//                     schema::TimeUnitMillis => colTypes[i].converted = schema::ConvertedTypes::TimeMillis,
//                     schema::TimeUnitMicros => colTypes[i].converted = schema::ConvertedTypes::TimeMicros,
//                     _ => {}
//                 }
//                 colTypes[i].IsAdjustedToUTC = t.IsAdjustedToUTC();
//             } else {
//                 colTypes[i].IsAdjustedToUTC = true;
//             }
//         } else {
//             colTypes[i].converted = desc.ConvertedType();
//             colTypes[i].IsAdjustedToUTC = true;
//             let pnode = desc.SchemaNode().as_primitive_node();
//             colTypes[i].decimalMeta = pnode.DecimalMetadata();
//         }
//
//         if unsupportedParquetTypes().contains_key(&colTypes[i].converted) {
//             return Err(Error::new(format!(
//                 "unsupported parquet logical type {}",
//                 colTypes[i].converted.String()
//             )));
//         }
//
//         match desc.PhysicalType() {
//             parquet::Types::Int32 if colTypes[i].converted == schema::ConvertedTypes::Date => {
//                 colTypes[i].sparkRebaseMicros = sparkRebaseMicrosFromMetadata(
//                     &fileMeta,
//                     sparkDatetimeRebaseCutoff,
//                     sparkLegacyDateTimeMetadataKey,
//                     effectiveLoc,
//                 )?;
//             }
//             parquet::Types::Int64
//                 if colTypes[i].converted == schema::ConvertedTypes::TimestampMillis
//                     || colTypes[i].converted == schema::ConvertedTypes::TimestampMicros =>
//             {
//                 colTypes[i].sparkRebaseMicros = sparkRebaseMicrosFromMetadata(
//                     &fileMeta,
//                     sparkDatetimeRebaseCutoff,
//                     sparkLegacyDateTimeMetadataKey,
//                     effectiveLoc,
//                 )?;
//             }
//             parquet::Types::Int96 => {
//                 colTypes[i].sparkRebaseMicros = sparkRebaseMicrosFromMetadata(
//                     &fileMeta,
//                     sparkINT96RebaseCutoff,
//                     sparkLegacyINT96MetadataKey,
//                     effectiveLoc,
//                 )?;
//             }
//             _ => {}
//         }
//     }
//
//     let numColumns = colTypes.len();
//     let pool = zeropool::New(|| vec![types::Datum::default(); numColumns]);
//     let mut parser = Parser {
//         fileMeta,
//         colTypes,
//         colNames,
//         ctx,
//         store,
//         path,
//         prop,
//         loc: effectiveLoc,
//         alloc: allocator,
//         rowGroup: None,
//         rowPool: pool,
//         curRowGroup: 0,
//         totalRowGroup: 0,
//         totalRows: 0,
//         totalReadRows: 0,
//         totalReadBytes: 0,
//         lastRow: parsedef::Row::default(),
//         logger,
//     };
//     parser.Init(effectiveLoc).map_err(errors::Trace)?;
//     let _ = r.Close();
//     Ok(parser)
// }
//
// SampleStatisticsFromParquet samples row size of the parquet file.
// pub fn SampleStatisticsFromParquet(
//     ctx: context::Context,
//     path: String,
//     store: storeapi::Storage,
// ) -> Result<(i64, f64), Error> {
//     let r = store.Open(ctx.clone(), path.clone(), None)?;
//     let mut parser = NewParser(ctx, store, r, path, FileMeta::default())?;
//     let mut rowSize: i64 = 0;
//     let meta = parser.fileMeta.clone();
//     if meta.NumRowGroups() == 0 || meta.RowGroups[0].NumRows == 0 {
//         let _ = parser.Close();
//         return Ok((0, 0.0));
//     }
//
//     let totalReadRows = meta.NumRows;
//     let readRows = std::cmp::min(totalReadRows, 1024);
//     let mut rowCount = 0;
//     for _ in 0..readRows {
//         match parser.ReadRow() {
//             Ok(()) => {
//                 let lastRow = parser.LastRow();
//                 rowSize += lastRow.Length as i64;
//                 parser.RecycleRow(lastRow);
//                 rowCount += 1;
//             }
//             Err(err) if errors::Cause(&err) == io::EOF => break,
//             Err(err) => return Err(err),
//         }
//     }
//     let _ = parser.Close();
//     Ok((totalReadRows, rowSize as f64 / rowCount as f64))
// }
//
// addressOf returns the address of a buffer, return 0 if the buffer is nil or
// empty. It's used to create unique identifiers for tracking buffer allocations.
// pub fn addressOf(buf: &[u8]) -> usize {
//     if buf.is_empty() {
//         return 0;
//     }
//     buf.as_ptr() as usize
// }
//
// trackingAllocator is a simple memory allocator that tracks current and peak
// memory allocation. It's used to estimate the memory consumption of parquet
// parser.
// pub struct trackingAllocator {
//     pub currentAllocation: atomic::Int64,
//     pub peakAllocation: atomic::Int64,
//     pub allocMap: sync::Map<usize, i32>,
// }
//
// pub const allocatorAlignment: usize = 64;
//
// roundUpToAlignment 对应 Go 的按 64 字节向上对齐位运算。
// pub fn roundUpToAlignment(addr: usize) -> usize {
//     (addr + allocatorAlignment - 1) & !(allocatorAlignment - 1)
// }
//
// impl trackingAllocator {
//     pub fn allocateAligned(&self, size: i32) -> Vec<u8> {
//         if size <= 0 {
//             return Vec::new();
//         }
// Go 额外分配 allocatorAlignment 字节，以便返回 64 字节对齐切片。
//         let mut buf = vec![0u8; size as usize + allocatorAlignment];
//         let addr = addressOf(&buf);
//         let next = roundUpToAlignment(addr);
//         let shift = next - addr;
//         let allocBytes = size as usize + allocatorAlignment;
//         self.updateAllocation(allocBytes as i64);
//         self.allocMap.Store(next, allocBytes as i32);
//         buf[shift..shift + size as usize].to_vec()
//     }
//
//     pub fn updateAllocation(&self, delta: i64) {
//         let current = self.currentAllocation.Add(delta);
//         if delta <= 0 {
//             return;
//         }
//         loop {
//             let oldPeak = self.peakAllocation.Load();
//             if current <= oldPeak {
//                 return;
//             }
//             if self.peakAllocation.CompareAndSwap(oldPeak, current) {
//                 return;
//             }
//         }
//     }
//
//     pub fn Allocate(&self, n: i32) -> Vec<u8> {
//         self.allocateAligned(n)
//     }
//
//     pub fn Free(&self, b: Vec<u8>) {
//         let addr = addressOf(&b);
//         if let Some(bytes) = self.allocMap.LoadAndDelete(addr) {
//             self.currentAllocation.Add(-(bytes as i64));
//         }
//     }
//
//     pub fn Reallocate(&self, size: i32, b: Vec<u8>) -> Vec<u8> {
//         if b.capacity() >= size as usize {
//             return b[..size as usize].to_vec();
//         }
//         let mut nb = self.allocateAligned(size);
//         nb[..b.len()].copy_from_slice(&b);
//         self.Free(b);
//         nb
//     }
// }
//
// pub fn estimateInMemoryRowGroupBufferBytes(fileMeta: &metadata::FileMetaData) -> Result<i64, Error> {
//     if fileMeta.is_nil() || fileMeta.NumRowGroups() == 0 {
//         return Ok(0);
//     }
//     let rgRange = rowGroupRangeFromMeta(fileMeta, 0)?;
//     let preloadBytes = rgRange.end - rgRange.start;
//     if preloadBytes <= 0 || preloadBytes > rowGroupInMemoryThreshold as i64 {
//         return Ok(0);
//     }
//     Ok(preloadBytes)
// }
//
// EstimateParquetReaderMemory estimates the peak memory usage for parsing a
// single parquet file by reading through the first row group with a tracking
// allocator. Returns the peak memory in bytes.
// pub fn EstimateParquetReaderMemory(
//     ctx: context::Context,
//     store: storeapi::Storage,
//     path: String,
// ) -> Result<i64, Error> {
//     let r = store.Open(ctx.clone(), path.clone(), None)?;
//     let allocator = trackingAllocator::default();
//     let mut parser = NewParser(
//         ctx.clone(),
//         store,
//         r.clone(),
//         path.clone(),
//         FileMeta { allocator: allocator.clone(), Loc: std::ptr::null_mut() },
//     )
//     .map_err(|err| {
//         let _ = r.Close();
//         err
//     })?;
//
//     let meta = parser.fileMeta.clone();
//     if meta.NumRowGroups() == 0 {
//         let _ = parser.Close();
//         return Ok(0);
//     }
//     let preloadBufferBytes = estimateInMemoryRowGroupBufferBytes(&meta)?;
//
//     for _ in 0..meta.RowGroups[0].NumRows {
//         if let Err(err) = ctx.Err() {
//             return Err(err);
//         }
//         match parser.ReadRow() {
//             Ok(()) => parser.RecycleRow(parser.LastRow()),
//             Err(err) if errors::Cause(&err) == io::EOF => break,
//             Err(err) => return Err(err),
//         }
//     }
//
//     let peak = allocator.peakAllocation.Load() + preloadBufferBytes;
//     logutil::Logger(ctx).Info(
//         "estimated parquet reader memory",
//         zap::String("path", path),
//         zap::Int64("in-memory-preload-bytes", preloadBufferBytes),
//         zap::Int64("peak-memory-bytes", peak),
//     );
//     let _ = parser.Close();
//     Ok(peak)
// }
// */
use crate::column_type::{LogicalType, PhysicalType};
use crate::column_value::{ColumnValue, account_column_value_memory_bytes};
use crate::{Error, Result};
use std::collections::BTreeMap;
/// 默认 skip buffer 大小：云存储上小跨度前跳时可避免重新 Open。
pub const DEFAULT_BUFFER_SIZE: usize = 64 * 1024;
/// 单次从 column reader 读取的行批大小（测试中可改）。
pub const READ_BATCH_SIZE: usize = 128;
#[derive(Clone, Debug, Eq, PartialEq)]
/// Parquet ConvertedType / 逻辑类型摘要，用于拒绝暂不支持的形态。
pub enum ConvertedType {
    None,
    Utf8,
    Decimal,
    Date,
    TimeMillis,
    TimeMicros,
    TimestampMillis,
    TimestampMicros,
    List,
    Map,
    MapKeyValue,
    Interval,
    NA,
}
#[derive(Clone, Debug)]
/// 文件中一列的物理/逻辑类型与时区调整标记。
pub struct ColumnDescriptor {
    pub name: String,
    pub physical: PhysicalType,
    pub logical: LogicalType,
    pub converted: ConvertedType,
    pub adjusted_to_utc: bool,
}
#[derive(Clone, Debug, Default)]
/// 一个 row group 的行数据与压缩字节数估算。
pub struct RowGroup {
    pub rows: Vec<Vec<Option<ColumnValue>>>,
    pub compressed_bytes: i64,
}
#[derive(Clone, Debug, Default)]
/// 简化的内存中 Parquet 文件模型（列描述 + row groups + metadata）。
pub struct ParquetFile {
    pub columns: Vec<ColumnDescriptor>,
    pub row_groups: Vec<RowGroup>,
    pub metadata: BTreeMap<String, String>,
    pub created_by: String,
    pub source_size: i64,
}
impl ParquetFile {
    /// 所有 row group 行数之和。
    pub fn num_rows(&self) -> i64 {
        self.row_groups.iter().map(|g| g.rows.len() as i64).sum()
    }
}
#[derive(Clone, Debug, Default, PartialEq)]
/// 解析器吐出的一行：列值、行号与估算长度。
pub struct ParsedRow {
    pub row: Vec<Option<ColumnValue>>,
    pub row_id: i64,
    pub length: usize,
}
/// 估算行大小：字节列按实际长度，其它非空值粗略按 8 字节。
pub fn estimate_row_size(row: &[Option<ColumnValue>]) -> usize {
    row.iter()
        .flatten()
        .map(|value| match value {
            ColumnValue::Bytes(v) | ColumnValue::FixedBytes(v) => v.len(),
            _ => 8,
        })
        .sum()
}
/// 顺序读取 ParquetFile 行的状态机；不支持并发。
pub struct Parser {
    file: ParquetFile,
    column_names: Vec<String>,
    current_group: usize,
    current_row: usize,
    total_read_rows: i64,
    total_rows: i64,
    last_row: ParsedRow,
    closed: bool,
}
impl Parser {
    /// 构造解析器；拒绝 List/Map/Interval/NA 与 Nanos 时间戳。
    pub fn new(file: ParquetFile) -> Result<Self> {
        for column in &file.columns {
            // 与 Go unsupportedParquetTypes 对齐：导入路径暂不支持这些逻辑类型。
            if matches!(
                column.converted,
                ConvertedType::List
                    | ConvertedType::Map
                    | ConvertedType::MapKeyValue
                    | ConvertedType::Interval
                    | ConvertedType::NA
            ) {
                return Err(Error(format!(
                    "unsupported parquet logical type {:?}",
                    column.converted
                )));
            }
            if matches!(
                column.logical,
                LogicalType::Timestamp {
                    unit: crate::TimeUnit::Nanos,
                    ..
                }
            ) {
                return Err(Error("unsupported timestamp time unit Nanos".into()));
            }
        }
        let names = file
            .columns
            .iter()
            .map(|c| c.name.to_ascii_lowercase())
            .collect();
        let total_rows = file.num_rows();
        Ok(Self {
            file,
            column_names: names,
            current_group: 0,
            current_row: 0,
            total_read_rows: 0,
            total_rows,
            last_row: ParsedRow::default(),
            closed: false,
        })
    }
    /// 从当前 row group 推进一行；耗尽则 EOF。
    fn read_single_row(&mut self) -> Result<ParsedRow> {
        if self.closed {
            return Err(Error("parser is closed".into()));
        }
        if self.current_group >= self.file.row_groups.len() {
            return Err(Error("EOF".into()));
        }
        if self.current_row == self.file.row_groups[self.current_group].rows.len() {
            self.current_group += 1;
            self.current_row = 0;
        }
        let Some(group) = self.file.row_groups.get(self.current_group) else {
            return Err(Error("EOF".into()));
        };
        let Some(row) = group.rows.get(self.current_row).cloned() else {
            return Err(Error("EOF".into()));
        };
        if row.len() != self.file.columns.len() {
            return Err(Error(format!(
                "parquet row has {} columns, schema has {}",
                row.len(),
                self.file.columns.len()
            )));
        }
        self.current_row += 1;
        self.total_read_rows += 1;
        let length = estimate_row_size(&row);
        Ok(ParsedRow {
            row,
            row_id: 0,
            length,
        })
    }
    /// 读取下一行到 `last_row`，并递增 row_id。
    pub fn read_row(&mut self) -> Result<()> {
        self.last_row.row_id += 1;
        self.last_row.length = 0;
        let mut row = self.read_single_row()?;
        row.row_id = self.last_row.row_id;
        self.last_row = row;
        Ok(())
    }
    /// 返回最近一次 `read_row` 的结果副本。
    pub fn last_row(&self) -> ParsedRow {
        self.last_row.clone()
    }
    /// 回收行缓冲（简化实现为空操作）。
    pub fn recycle_row(&mut self, _row: ParsedRow) {}
    /// 小写列名列表。
    pub fn columns(&self) -> &[String] {
        &self.column_names
    }
    /// `(已读行数, 当前 row_id)`。
    pub fn pos(&self) -> (i64, i64) {
        (self.total_read_rows, self.last_row.row_id)
    }
    /// 按消费行数比例估算源字节进度，避免预读影响进度。
    pub fn scanned_pos(&self) -> i64 {
        if self.total_rows <= 0 || self.total_read_rows == self.total_rows {
            return self.file.source_size;
        }
        let progress = self.total_read_rows as f64 / self.total_rows as f64;
        (progress * self.file.source_size as f64) as i64
    }
    /// 按 Go 的整数 range 语义跳过正数行；负数差值不移动物理游标。
    pub fn set_pos(&mut self, pos: i64, row_id: i64) -> Result<()> {
        let count = pos - self.last_row.row_id;
        for _ in 0..count {
            let _ = self.read_single_row()?;
        }
        self.last_row.row_id = row_id;
        Ok(())
    }
    /// 仅设置 last_row.row_id，不移动读取游标。
    pub fn set_row_id(&mut self, id: i64) {
        self.last_row.row_id = id
    }
    /// 关闭解析器，后续读取返回错误。
    pub fn close(&mut self) -> Result<()> {
        self.closed = true;
        Ok(())
    }
    /// Go 风格别名。
    pub fn ReadRow(&mut self) -> Result<()> {
        self.read_row()
    }
    /// Go 风格别名。
    pub fn LastRow(&self) -> ParsedRow {
        self.last_row()
    }
    /// Go 风格别名。
    pub fn RecycleRow(&mut self, r: ParsedRow) {
        self.recycle_row(r)
    }
    /// Go 风格别名。
    pub fn Columns(&self) -> &[String] {
        self.columns()
    }
    /// Go 风格别名。
    pub fn Pos(&self) -> (i64, i64) {
        self.pos()
    }
    /// Go 风格别名。
    pub fn SetPos(&mut self, p: i64, r: i64) -> Result<()> {
        self.set_pos(p, r)
    }
    /// Go 风格别名。
    pub fn ScannedPos(&self) -> i64 {
        self.scanned_pos()
    }
    /// Go 风格别名。
    pub fn SetRowID(&mut self, r: i64) {
        self.set_row_id(r)
    }
    /// Go 风格别名。
    pub fn Close(&mut self) -> Result<()> {
        self.close()
    }
}
/// 返回文件总行数。
pub fn ReadRowCount(file: &ParquetFile) -> i64 {
    file.num_rows()
}
/// Go 风格构造入口。
pub fn NewParser(file: ParquetFile) -> Result<Parser> {
    Parser::new(file)
}
/// 采样最多 1024 行，返回总行数与平均行字节数。
pub fn SampleStatisticsFromParquet(file: ParquetFile) -> Result<(i64, f64)> {
    let total = file.num_rows();
    if file
        .row_groups
        .first()
        .is_none_or(|group| group.rows.is_empty())
    {
        return Ok((0, 0.0));
    }
    let mut parser = Parser::new(file)?;
    let mut count = 0i64;
    let mut bytes = 0i64;
    while count < total.min(1024) {
        parser.read_row()?;
        bytes += parser.last_row.length as i64;
        count += 1;
    }
    Ok((total, bytes as f64 / count as f64))
}
#[derive(Debug, Default)]
/// 跟踪分配峰值的简易 allocator（对齐 Go 测试用统计）。
pub struct TrackingAllocator {
    current: i64,
    peak: i64,
    allocations: BTreeMap<usize, usize>,
    next_id: usize,
}
impl TrackingAllocator {
    /// 分配 size 字节并计入峰值（额外 +64 对齐开销）。
    pub fn allocate(&mut self, size: usize) -> (usize, Vec<u8>) {
        if size == 0 {
            return (0, Vec::new());
        }
        let allocated = size + 64;
        self.current += allocated as i64;
        self.peak = self.peak.max(self.current);
        self.next_id += 1;
        self.allocations.insert(self.next_id, allocated);
        (self.next_id, vec![0; size])
    }
    /// 释放指定分配并扣减 current。
    pub fn free(&mut self, id: usize) {
        if let Some(size) = self.allocations.remove(&id) {
            self.current -= size as i64;
        }
    }
    /// 容量足够时复用原分配；扩容时先分配新块再释放旧块，并保留前缀。
    pub fn reallocate(&mut self, id: usize, size: usize, old: &[u8]) -> (usize, Vec<u8>) {
        let capacity = self
            .allocations
            .get(&id)
            .copied()
            .unwrap_or(64)
            .saturating_sub(64);
        if id != 0 && size <= capacity {
            let mut data = vec![0; size];
            let count = data.len().min(old.len());
            data[..count].copy_from_slice(&old[..count]);
            return (id, data);
        }

        // Go allocator 先分配新块、复制，再释放旧块；峰值会同时包含两块。
        let (new_id, mut data) = self.allocate(size);
        let count = data.len().min(old.len());
        data[..count].copy_from_slice(&old[..count]);
        self.free(id);
        (new_id, data)
    }
    /// 历史峰值分配字节。
    pub fn peak(&self) -> i64 {
        self.peak
    }
    /// 当前仍由 allocator 跟踪的分配字节数（包含对齐开销）。
    pub fn current(&self) -> i64 {
        self.current
    }
}
/// 估算读取首个 row group 的内存：可预读压缩块 + 列值 + level 缓冲。
pub fn EstimateParquetReaderMemory(file: &ParquetFile) -> Result<i64> {
    let Some(group) = file.row_groups.first() else {
        return Ok(0);
    };
    // 小 row group（≤128MiB）整组预读进内存，减少对象存储 GET。
    let preload = if group.compressed_bytes > 0 && group.compressed_bytes <= 128 * 1024 * 1024 {
        group.compressed_bytes
    } else {
        0
    };
    let values = group
        .rows
        .iter()
        .flatten()
        .flatten()
        .map(account_column_value_memory_bytes)
        .sum::<i64>();
    Ok(preload + values + (file.columns.len() * READ_BATCH_SIZE * 4) as i64)
}

/// 打开真实 Parquet 文件；对象存储适配器、File 与 Bytes 共用同一读取配置。
/// 该入口按页解码，不预先把文件的所有行物化为 ParquetFile。
pub fn open_file_reader<R: parquet::file::reader::ChunkReader + 'static>(
    source: R,
) -> Result<parquet::file::serialized_reader::SerializedFileReader<R>> {
    let options = parquet::file::serialized_reader::ReadOptionsBuilder::new()
        .with_reader_properties(
            parquet::file::properties::ReaderProperties::builder()
                .set_page_streaming_enabled(true)
                .build(),
        )
        .build();
    parquet::file::serialized_reader::SerializedFileReader::new_with_options(source, options)
        .map_err(|error| Error(format!("open parquet source: {error}")))
}

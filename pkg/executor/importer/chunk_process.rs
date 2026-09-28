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
// limitations under the License.

// 导入器 chunk 处理流水线：读行 → 编码为 KV → 投递到引擎 writer。
//
// 编码与投递在独立线程中通过有界队列解耦；支持文件解析源与
// SELECT 查询 chunk 源。索引 KV 可按 index_id 路由到不同 writer。
//
// KV：键值对，TiKV 中表数据与二级索引均编码为 KV；
// Region：数据按 key 范围切分的分片。

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use astersql_dxf_framework_taskexecutor_execute::Collector;
use astersql_lightning_backend::{EngineWriter, Logger};
use astersql_lightning_backend_encode::{Context, Datum};
use astersql_lightning_backend_kv::{GroupedPairs, MakeRowsFromKvPairs, Pairs};
use astersql_lightning_mydump::{Datum as ParserDatum, MydumpError, Parser};
use astersql_lightning_verification::{KVGroupChecksum, KvPair, NewKVGroupChecksumWithKeyspace};
use astersql_tablecodec as tablecodec;

use crate::TableKVEncoder;

/// 编码线程向投递线程发送批次的有界队列容量。
pub static maxKVQueueSize: usize = 32;
/// 默认凑够多少字节再投递一批编码结果。
pub static DefaultMinDeliverBytes: u64 = 96 * 1024;
/// 默认凑够多少行再投递一批。
pub static DefaultMinDeliverRowCnt: usize = 4096;

/// 待编码的一行：列值、行号与源文件偏移。
pub struct RowToEncode {
    pub row: Vec<Datum>,
    pub row_id: i64,
    pub end_offset: i64,
    pub start_pos: i64,
}

/// 顺序读取待编码行；`reuse` 复用缓冲减少分配。
pub trait EncodeReader: Send {
    fn ReadRow(&mut self, reuse: Vec<Datum>) -> Result<Option<RowToEncode>, String>;
}

/// 基于 mydump Parser 的文件行读取器，读到 `end_offset` 为止。
pub struct ParserEncodeReader {
    parser: Box<dyn Parser + Send>,
    end_offset: i64,
    filename: String,
}

/// 构造文件解析编码读取器。
pub fn parserEncodeReader(
    parser: Box<dyn Parser + Send>,
    end_offset: i64,
    filename: impl Into<String>,
) -> ParserEncodeReader {
    ParserEncodeReader {
        parser,
        end_offset,
        filename: filename.into(),
    }
}

impl EncodeReader for ParserEncodeReader {
    fn ReadRow(&mut self, mut reuse: Vec<Datum>) -> Result<Option<RowToEncode>, String> {
        let (read_position, _) = self.parser.Pos();
        if read_position >= self.end_offset {
            return Ok(None);
        }
        match self.parser.ReadRow() {
            Ok(()) => {}
            Err(MydumpError::Eof) => return Ok(None),
            Err(error) => {
                return Err(format!(
                    "encode {} at offset {}: {}",
                    self.filename, read_position, error
                ));
            }
        }
        let scanned_position = self
            .parser
            .ScannedPos()
            .map_err(|error| error.to_string())?;
        let parsed = self.parser.LastRow();
        reuse.clear();
        reuse.extend(parsed.row.iter().map(parser_datum_to_encoder_datum));
        let row_id = parsed.row_id;
        self.parser.RecycleRow(parsed);
        Ok(Some(RowToEncode {
            row: reuse,
            row_id,
            end_offset: scanned_position,
            start_pos: read_position,
        }))
    }
}

impl Drop for ParserEncodeReader {
    fn drop(&mut self) {
        let _ = self.parser.Close();
    }
}

/// 将解析器 Datum 转为编码器 Datum。
fn parser_datum_to_encoder_datum(value: &ParserDatum) -> Datum {
    match value {
        ParserDatum::Null => Datum::Null,
        ParserDatum::I64(value) => Datum::Int(*value),
        ParserDatum::Bytes(value) | ParserDatum::Binary(value) => Datum::Bytes(value.clone()),
    }
}

#[derive(Clone, Debug, Default)]
/// SELECT 导入管线中的一批行及其起始 row_id 偏移。
pub struct QueryChunk {
    pub rows: Vec<Vec<Datum>>,
    pub row_id_offset: i64,
}

/// 从通道接收 `QueryChunk` 并逐行吐出的读取器。
pub struct QueryChunkEncodeReader {
    chunks: SharedQueryChunkReceiver,
    current: QueryChunk,
    cursor: usize,
}

/// 跨线程共享的 QueryChunk 接收端。
pub type SharedQueryChunkReceiver = Arc<Mutex<mpsc::Receiver<QueryChunk>>>;

/// 构造查询 chunk 编码读取器。
pub fn queryRowEncodeReader(chunks: SharedQueryChunkReceiver) -> QueryChunkEncodeReader {
    QueryChunkEncodeReader {
        chunks,
        current: QueryChunk::default(),
        cursor: 0,
    }
}

impl EncodeReader for QueryChunkEncodeReader {
    fn ReadRow(&mut self, mut reuse: Vec<Datum>) -> Result<Option<RowToEncode>, String> {
        while self.cursor >= self.current.rows.len() {
            self.current = match self
                .chunks
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .recv()
            {
                Ok(chunk) => chunk,
                Err(_) => return Ok(None),
            };
            self.cursor = 0;
        }
        reuse.clear();
        reuse.extend(self.current.rows[self.cursor].iter().cloned());
        self.cursor += 1;
        Ok(Some(RowToEncode {
            row: reuse,
            row_id: self.current.row_id_offset + self.cursor as i64,
            end_offset: -1,
            start_pos: -1,
        }))
    }
}

/// 一批已编码的数据 KV 与按索引分组的索引 KV，附带校验和。
pub struct EncodedKVGroupBatch {
    pub data_kvs: Vec<KvPair>,
    pub index_kvs: HashMap<i64, Vec<KvPair>>,
    /// 生成本批 KV 的源数据行数；不能由记录 KV 数反推。
    pub row_count: usize,
    pub group_checksum: KVGroupChecksum,
}

/// 按 keyspace 初始化空批次。
pub fn NewEncodedKVGroupBatch(keyspace: &[u8], row_count: usize) -> EncodedKVGroupBatch {
    EncodedKVGroupBatch {
        data_kvs: Vec::with_capacity(row_count),
        index_kvs: HashMap::with_capacity(8),
        row_count,
        group_checksum: NewKVGroupChecksumWithKeyspace(keyspace),
    }
}

impl EncodedKVGroupBatch {
    /// 将一对表记录/索引 KV 归入批次并更新校验和，返回新增字节数。
    pub fn Add(&mut self, pairs: &Pairs) -> Result<i64, (i64, String)> {
        let mut kv_bytes = 0_i64;
        for pair in &pairs.Pairs {
            // 记录键进 data_kvs；否则按 index_id 归入 index_kvs。
            if is_record_key(&pair.key) {
                self.data_kvs.push(pair.clone());
                self.group_checksum.UpdateOneDataKV(pair);
            } else {
                let index_id = decode_index_id(&pair.key).map_err(|error| (kv_bytes, error))?;
                self.index_kvs
                    .entry(index_id)
                    .or_insert_with(|| Vec::with_capacity(self.data_kvs.capacity()))
                    .push(pair.clone());
                self.group_checksum.UpdateOneIndexKV(index_id, pair);
            }
            kv_bytes += (pair.key.len() + pair.val.len()) as i64;
        }
        Ok(kv_bytes)
    }
}

/// 读取行、编码为 KV 批次并按阈值阈值发送。
pub struct ChunkEncoder {
    pub chunk_name: String,
    pub offset: i64,
    pub min_deliver_bytes: u64,
    pub min_deliver_row_count: usize,
    pub read_total_duration: Duration,
    pub encode_total_duration: Duration,
    pub group_checksum: KVGroupChecksum,
    reader: Box<dyn EncodeReader>,
    encoder: TableKVEncoder,
    collector: Option<Arc<dyn Collector + Send + Sync>>,
    keyspace: Vec<u8>,
}

/// 构造 chunk 编码器。
pub fn newChunkEncoder(
    chunk_name: impl Into<String>,
    reader: Box<dyn EncodeReader>,
    offset: i64,
    collector: Option<Arc<dyn Collector + Send + Sync>>,
    encoder: TableKVEncoder,
    keyspace: Vec<u8>,
) -> ChunkEncoder {
    ChunkEncoder {
        chunk_name: chunk_name.into(),
        offset,
        min_deliver_bytes: DefaultMinDeliverBytes,
        min_deliver_row_count: DefaultMinDeliverRowCnt,
        read_total_duration: Duration::ZERO,
        encode_total_duration: Duration::ZERO,
        group_checksum: NewKVGroupChecksumWithKeyspace(&keyspace),
        reader,
        encoder,
        collector,
        keyspace,
    }
}

impl ChunkEncoder {
    /// 循环读行编码，达到字节/行阈值后发送批次，结束时汇报已接受偏移。
    pub fn encodeLoop(
        &mut self,
        context: &Context,
        sender: &mpsc::SyncSender<EncodedKVGroupBatch>,
    ) -> Result<(), String> {
        let mut row_cache = Vec::new();
        let mut batch_rows = Vec::<Pairs>::with_capacity(self.min_deliver_row_count);
        let mut batch_bytes = 0_u64;
        let mut current_offset = self.offset;
        loop {
            if context.is_cancelled() {
                return Err("chunk encoding was cancelled".into());
            }
            let read_started = Instant::now();
            let Some(row) = self.reader.ReadRow(row_cache)? else {
                break;
            };
            self.read_total_duration += read_started.elapsed();
            row_cache = row.row.clone();
            current_offset = row.end_offset;
            let encode_started = Instant::now();
            let pairs = self.encoder.Encode(&row.row, row.row_id).map_err(|error| {
                format!(
                    "{} at source offset {}: {}",
                    self.chunk_name, row.start_pos, error
                )
            })?;
            self.encode_total_duration += encode_started.elapsed();
            batch_bytes = batch_bytes.saturating_add(pairs.Size());
            batch_rows.push(pairs);
            // 达到字节或行数阈值则刷出一批。
            if batch_bytes >= self.min_deliver_bytes
                || batch_rows.len() >= self.min_deliver_row_count
            {
                self.accept_offset(current_offset);
                self.send_batch(sender, &mut batch_rows)?;
                batch_bytes = 0;
            }
        }
        self.accept_offset(current_offset);
        self.send_batch(sender, &mut batch_rows)?;
        Ok(())
    }

    fn accept_offset(&mut self, current_offset: i64) {
        if current_offset >= 0 {
            let accepted = current_offset.saturating_sub(self.offset);
            self.offset = current_offset;
            if let Some(collector) = &self.collector {
                collector.Accepted(accepted);
            }
        }
    }

    /// 将积压的 `Pairs` 合并为 `EncodedKVGroupBatch` 并发送。
    fn send_batch(
        &mut self,
        sender: &mpsc::SyncSender<EncodedKVGroupBatch>,
        rows: &mut Vec<Pairs>,
    ) -> Result<(), String> {
        if rows.is_empty() {
            return Ok(());
        }
        let mut batch = NewEncodedKVGroupBatch(&self.keyspace, rows.len());
        let mut total_bytes = 0_i64;
        for pairs in rows.drain(..) {
            total_bytes += batch.Add(&pairs).map_err(|(_, error)| error)?;
        }
        self.group_checksum.Add(&batch.group_checksum);
        let row_count = batch.row_count as i64;
        sender
            .send(batch)
            .map_err(|_| "encoded KV delivery loop stopped".to_owned())?;
        if let Some(collector) = &self.collector {
            collector.Processed(total_bytes, row_count);
        }
        Ok(())
    }

    /// 关闭底层表 KV 编码器。
    pub fn Close(&mut self) -> Result<(), String> {
        self.encoder.Close()
    }
}

/// 将编码批次写入数据引擎与索引引擎。
pub struct DataDeliver {
    pub data_writer: Box<dyn EngineWriter>,
    pub index_writer: Box<dyn EngineWriter>,
    pub deliver_total_duration: Duration,
}

impl DataDeliver {
    /// 从队列取批次，分别 Append 数据行与分组索引行。
    pub fn deliverLoop(
        &mut self,
        context: &Context,
        receiver: mpsc::Receiver<EncodedKVGroupBatch>,
    ) -> Result<(), String> {
        while let Ok(batch) = receiver.recv() {
            if context.is_cancelled() {
                return Err("KV delivery was cancelled".into());
            }
            let started = Instant::now();
            let data_rows = MakeRowsFromKvPairs(batch.data_kvs);
            self.data_writer
                .AppendRows(context, &[], data_rows.as_ref())
                .map_err(|error| error.to_string())?;
            let grouped = GroupedPairs(
                batch
                    .index_kvs
                    .into_iter()
                    .collect::<BTreeMap<i64, Vec<KvPair>>>(),
            );
            self.index_writer
                .AppendRows(context, &[], &grouped)
                .map_err(|error| error.to_string())?;
            self.deliver_total_duration += started.elapsed();
        }
        Ok(())
    }

    /// 依次关闭数据/索引 writer，返回首个错误。
    pub fn Close(&mut self, context: &Context) -> Result<(), String> {
        let data_error = self.data_writer.Close(context).err();
        let index_error = self.index_writer.Close(context).err();
        data_error
            .or(index_error)
            .map_or(Ok(()), |error| Err(error.to_string()))
    }
}

/// 处理单个导入 chunk 的入口。
pub trait ChunkProcessor {
    fn Process(&mut self, context: &Context) -> Result<(), String>;
}

/// 编码与投递并行的默认 chunk 处理器。
pub struct BaseChunkProcessor {
    pub encoder: ChunkEncoder,
    pub deliver: DataDeliver,
    pub group_checksum: Option<Arc<Mutex<KVGroupChecksum>>>,
}

impl ChunkProcessor for BaseChunkProcessor {
    /// 启动投递线程，主线程编码；结束后合并校验并关闭资源。
    fn Process(&mut self, context: &Context) -> Result<(), String> {
        // 有界队列解耦编码与投递，避免无界堆积。
        let (sender, receiver) = mpsc::sync_channel(maxKVQueueSize);
        let (encode_result, deliver_result) = std::thread::scope(|scope| {
            let deliver = &mut self.deliver;
            let delivery = scope.spawn(move || deliver.deliverLoop(context, receiver));
            let encode_result = self.encoder.encodeLoop(context, &sender);
            drop(sender);
            let deliver_result = delivery
                .join()
                .map_err(|_| "KV delivery thread panicked".to_owned())
                .and_then(|result| result);
            (encode_result, deliver_result)
        });
        let process_result = encode_result.and(deliver_result);
        if process_result.is_ok() {
            if let Some(checksum) = &self.group_checksum {
                checksum
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .Add(&self.encoder.group_checksum);
            }
        }
        // Go closes these resources with defers: cleanup runs on every return
        // path, and cleanup failures are logged rather than replacing the
        // processing result. Keep the same lifecycle and error precedence.
        let _ = self.encoder.Close();
        let _ = self.deliver.Close(context);
        process_result
    }
}

/// 构造 SELECT 导入用的 chunk 处理器。
pub fn NewQueryChunkProcessor(
    chunks: SharedQueryChunkReceiver,
    encoder: TableKVEncoder,
    keyspace: Vec<u8>,
    data_writer: Box<dyn EngineWriter>,
    index_writer: Box<dyn EngineWriter>,
    group_checksum: Option<Arc<Mutex<KVGroupChecksum>>>,
    collector: Option<Arc<dyn Collector + Send + Sync>>,
) -> BaseChunkProcessor {
    BaseChunkProcessor {
        encoder: newChunkEncoder(
            "import-from-select",
            Box::new(queryRowEncodeReader(chunks)),
            -1,
            collector,
            encoder,
            keyspace,
        ),
        deliver: DataDeliver {
            data_writer,
            index_writer,
            deliver_total_duration: Duration::ZERO,
        },
        group_checksum,
    }
}

/// 构造文件导入用的 chunk 处理器。
pub fn NewFileChunkProcessor(
    parser: Box<dyn Parser + Send>,
    encoder: TableKVEncoder,
    keyspace: Vec<u8>,
    chunk_name: impl Into<String>,
    offset: i64,
    end_offset: i64,
    data_writer: Box<dyn EngineWriter>,
    index_writer: Box<dyn EngineWriter>,
    group_checksum: Option<Arc<Mutex<KVGroupChecksum>>>,
    collector: Option<Arc<dyn Collector + Send + Sync>>,
) -> BaseChunkProcessor {
    let name = chunk_name.into();
    BaseChunkProcessor {
        encoder: newChunkEncoder(
            name.clone(),
            Box::new(parserEncodeReader(parser, end_offset, name)),
            offset,
            collector,
            encoder,
            keyspace,
        ),
        deliver: DataDeliver {
            data_writer,
            index_writer,
            deliver_total_duration: Duration::ZERO,
        },
        group_checksum,
    }
}

/// 按 index_id 惰性创建索引 writer 的工厂。
pub type WriterFactory =
    Arc<dyn Fn(i64) -> Result<Box<dyn RoutedIndexWriter>, String> + Send + Sync + 'static>;

/// 可按行写入的索引 writer。
pub trait RoutedIndexWriter: Send {
    fn WriteRow(&mut self, context: &Context, key: &[u8], value: &[u8]) -> Result<(), String>;
    fn Close(&mut self, context: &Context) -> Result<(), String>;
}

/// 按 index_id 路由索引 KV 到不同底层 writer。
pub struct IndexRouteWriter {
    writers: HashMap<i64, Box<dyn RoutedIndexWriter>>,
    logger: Logger,
    writer_factory: WriterFactory,
}

/// 构造索引路由 writer。
pub fn NewIndexRouteWriter(logger: Logger, writer_factory: WriterFactory) -> IndexRouteWriter {
    IndexRouteWriter {
        writers: HashMap::new(),
        logger,
        writer_factory,
    }
}

impl EngineWriter for IndexRouteWriter {
    fn AppendRows(
        &mut self,
        context: &Context,
        _column_names: &[String],
        rows: &dyn astersql_lightning_backend_encode::Rows,
    ) -> Result<(), astersql_lightning_backend::BackendError> {
        let grouped = rows
            .as_any()
            .downcast_ref::<GroupedPairs>()
            .ok_or_else(|| {
                astersql_lightning_backend::BackendError::new("invalid grouped pairs")
            })?;
        for (index_id, pairs) in &grouped.0 {
            if !self.writers.contains_key(index_id) {
                let writer = (self.writer_factory)(*index_id)
                    .map_err(astersql_lightning_backend::BackendError::new)?;
                self.writers.insert(*index_id, writer);
            }
            let writer = self.writers.get_mut(index_id).expect("inserted above");
            for pair in pairs {
                writer
                    .WriteRow(context, &pair.key, &pair.val)
                    .map_err(astersql_lightning_backend::BackendError::new)?;
            }
        }
        Ok(())
    }

    fn IsSynced(&self) -> bool {
        true
    }

    fn Close(
        &mut self,
        context: &Context,
    ) -> Result<
        Option<astersql_lightning_backend::ChunkFlushStatus>,
        astersql_lightning_backend::BackendError,
    > {
        let mut first_error = None;
        for writer in self.writers.values_mut() {
            if let Err(error) = writer.Close(context) {
                first_error.get_or_insert(error);
            }
        }
        if let Some(error) = first_error {
            return Err(astersql_lightning_backend::BackendError::new(error));
        }
        let _ = &self.logger;
        Ok(Some(astersql_lightning_backend::ChunkFlushStatus {
            flushed: true,
        }))
    }
}

/// 判断 key 是否为表记录（非索引）键。
fn is_record_key(key: &[u8]) -> bool {
    tablecodec::IsRecordKey(key) || formal_key_component(key, b"_r").is_some()
}

/// 从索引 key 解码 index_id（兼容 tablecodec 与形式化 `_i` 标记）。
fn decode_index_id(key: &[u8]) -> Result<i64, String> {
    if tablecodec::IsIndexKey(key) {
        return tablecodec::DecodeIndexID(tablecodec::kv::Key(key.to_vec()))
            .map_err(|error| error.to_string());
    }
    let digits = formal_key_component(key, b"_i")
        .ok_or_else(|| "KV key is neither a record key nor an index key".to_owned())?;
    let end = digits
        .iter()
        .position(|byte| *byte == b'_')
        .unwrap_or(digits.len());
    std::str::from_utf8(&digits[..end])
        .map_err(|error| error.to_string())?
        .parse::<i64>()
        .map_err(|error| error.to_string())
}

/// 在形式化 key 中定位标记后的组件切片。
fn formal_key_component<'a>(key: &'a [u8], marker: &[u8]) -> Option<&'a [u8]> {
    key.windows(marker.len())
        .position(|window| window == marker)
        .map(|offset| &key[offset + marker.len()..])
}

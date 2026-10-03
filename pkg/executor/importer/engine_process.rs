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

// 导入引擎 chunk 处理入口。
//
// 按数据源类型（文件 / 查询）创建对应的 `ChunkProcessor`，
// 打开 data/index 本地 writer，将一批 chunk 编码写入 Lightning 引擎。
// Region 是 TiKV 的数据分片；此处 writer 面向本地引擎，后续再导入到 Region。

use std::sync::{Arc, Mutex};

use astersql_dxf_framework_taskexecutor_execute::Collector;
use astersql_lightning_backend::{EngineWriter, LocalWriterConfig, OpenedEngine};
use astersql_lightning_backend_encode::Context;
use astersql_lightning_mydump::{Compression, Parser, SourceType};
use astersql_lightning_verification::KVGroupChecksum;
use astersql_meta_model::TableInfo;

use crate::{
    ChunkProcessor, DataSourceType, DataSourceTypeFile, DataSourceTypeQuery, NewFileChunkProcessor,
    NewQueryChunkProcessor, SharedQueryChunkReceiver, TableKVEncoder,
};

/// 描述一次导入要处理的数据分片（chunk）元信息。
///
/// Offset/EndOffset 界定文件字节范围；PrevRowIDMax/RowIDMax 界定自动分配的行号区间。
pub trait ImportChunk: Send + Sync {
    /// chunk 唯一键，用于日志与进度追踪。
    fn Key(&self) -> String;
    /// 源文件路径。
    fn Path(&self) -> &str;
    /// 文件总大小（字节）。
    fn FileSize(&self) -> i64;
    /// Original source bytes; Parquet offsets describe rows rather than bytes.
    fn GetSize(&self) -> i64 {
        if self.SourceType() == SourceType::Parquet {
            self.FileSize()
        } else {
            self.EndOffset() - self.Offset()
        }
    }
    /// 本 chunk 起始字节偏移。
    fn Offset(&self) -> i64;
    /// 本 chunk 结束字节偏移（不含或含依调用约定）。
    fn EndOffset(&self) -> i64;
    /// 本 chunk 可用行号区间下界（上一 chunk 的最大行号）。
    fn PrevRowIDMax(&self) -> i64;
    /// 本 chunk 可用行号区间上界。
    fn RowIDMax(&self) -> i64;
    /// 源类型（CSV/SQL/Parquet 等）。
    fn SourceType(&self) -> SourceType;
    /// 压缩格式；无压缩时为 None。
    fn Compression(&self) -> Compression;
    /// 时间戳（如 parquet 时区相关）。
    fn Timestamp(&self) -> i64;
    /// Parquet temporal values use the plan location selected for this chunk.
    fn ParquetLocation(&self) -> Option<&str> {
        None
    }
}

/// 表级导入运行时：提供数据源类型、表元信息、编码器与解析器。
pub trait TableImporterRuntime {
    /// 返回数据源类型：文件或查询结果。
    fn DataSourceType(&self) -> DataSourceType;
    /// 目标表元信息（含主键/自增配置）。
    fn TableInfo(&self) -> &TableInfo;
    /// 返回 keyspace 前缀字节；多租户场景下用于区分命名空间。
    fn GetKeySpace(&self) -> Vec<u8>;
    /// 为指定 chunk 构造表级 KV 编码器。
    fn GetKVEncoder(&self, chunk: &dyn ImportChunk) -> Result<TableKVEncoder, String>;
    /// 为指定 chunk 打开行解析器（mydump Parser）。
    fn GetParser(
        &self,
        context: &Context,
        chunk: &dyn ImportChunk,
    ) -> Result<Box<dyn Parser + Send>, String>;
    /// 取走查询路径下的 chunk 接收通道（仅 Query 数据源）。
    fn TakeQueryChunks(&self) -> Result<SharedQueryChunkReceiver, String>;
}

/// 处理单个 chunk：打开 data/index 本地 writer 后委托 `ProcessChunkWithWriter`。
///
/// 当表使用有序自增行号（非聚簇主键、无 AutoRandom/Shard、无分区）时，
/// 将 data writer 标记为 KV 已排序，便于后续 Region 分裂与导入优化。
pub fn ProcessChunk(
    context: &Context,
    chunk: &dyn ImportChunk,
    table_importer: &dyn TableImporterRuntime,
    data_engine: &OpenedEngine,
    index_engine: &OpenedEngine,
    group_checksum: Option<Arc<Mutex<KVGroupChecksum>>>,
    collector: Option<Arc<dyn Collector + Send + Sync>>,
) -> Result<(), String> {
    ProcessChunkAndLogger(
        context,
        chunk,
        table_importer,
        data_engine,
        index_engine,
        group_checksum,
        collector,
        &astersql_lightning_log::L(),
    )
}

/// Process the chunk with the caller's structured log context.
pub fn ProcessChunkAndLogger(
    context: &Context,
    chunk: &dyn ImportChunk,
    table_importer: &dyn TableImporterRuntime,
    data_engine: &OpenedEngine,
    index_engine: &OpenedEngine,
    group_checksum: Option<Arc<Mutex<KVGroupChecksum>>>,
    collector: Option<Arc<dyn Collector + Send + Sync>>,
    logger: &astersql_lightning_log::Logger,
) -> Result<(), String> {
    let table_info = table_importer.TableInfo();
    // 判断行号是否天然有序：有序则可向 writer 声明 IsKVSorted。
    let has_ordered_auto_row_id = !table_info.PKIsHandle
        && !table_info.IsCommonHandle
        && table_info.AutoRandomBits == 0
        && table_info.ShardRowIDBits == 0
        && table_info.Partition.is_none();

    let mut data_writer_config = LocalWriterConfig::default();
    data_writer_config.Local.IsKVSorted = has_ordered_auto_row_id;
    let mut data_writer = data_engine
        .LocalWriter(context, &data_writer_config)
        .map_err(|error| error.to_string())?;
    let index_writer = match index_engine.LocalWriter(context, &LocalWriterConfig::default()) {
        Ok(writer) => writer,
        Err(error) => {
            let _ = data_writer.Close(context);
            return Err(error.to_string());
        }
    };
    ProcessChunkWithWriterAndLogger(
        context,
        chunk,
        table_importer,
        data_writer,
        index_writer,
        group_checksum,
        collector,
        logger,
    )
}

/// 在已有 data/index writer 上处理 chunk。
///
/// 按 `DataSourceType` 分支：文件走 `FileChunkProcessor`，查询走 `QueryChunkProcessor`。
pub fn ProcessChunkWithWriter(
    context: &Context,
    chunk: &dyn ImportChunk,
    table_importer: &dyn TableImporterRuntime,
    data_writer: Box<dyn EngineWriter>,
    index_writer: Box<dyn EngineWriter>,
    group_checksum: Option<Arc<Mutex<KVGroupChecksum>>>,
    collector: Option<Arc<dyn Collector + Send + Sync>>,
) -> Result<(), String> {
    ProcessChunkWithWriterAndLogger(
        context,
        chunk,
        table_importer,
        data_writer,
        index_writer,
        group_checksum,
        collector,
        &astersql_lightning_log::L(),
    )
}

/// Process the chunk with the caller's structured log context.
pub fn ProcessChunkWithWriterAndLogger(
    context: &Context,
    chunk: &dyn ImportChunk,
    table_importer: &dyn TableImporterRuntime,
    data_writer: Box<dyn EngineWriter>,
    index_writer: Box<dyn EngineWriter>,
    group_checksum: Option<Arc<Mutex<KVGroupChecksum>>>,
    collector: Option<Arc<dyn Collector + Send + Sync>>,
    logger: &astersql_lightning_log::Logger,
) -> Result<(), String> {
    let encoder = table_importer.GetKVEncoder(chunk)?;
    let keyspace = table_importer.GetKeySpace();
    match table_importer.DataSourceType() {
        DataSourceTypeFile => {
            let parser = match table_importer.GetParser(context, chunk) {
                Ok(parser) => parser,
                Err(error) => {
                    let mut encoder = encoder;
                    let _ = encoder.Close();
                    return Err(error);
                }
            };
            let mut processor = NewFileChunkProcessor(
                parser,
                encoder,
                keyspace,
                chunk.Key(),
                chunk.Offset(),
                chunk.EndOffset(),
                data_writer,
                index_writer,
                group_checksum,
                collector,
            )
            .WithChunkLogger(chunk, logger);
            processor.Process(context)
        }
        DataSourceTypeQuery => {
            let chunks = match table_importer.TakeQueryChunks() {
                Ok(chunks) => chunks,
                Err(error) => {
                    let mut encoder = encoder;
                    let _ = encoder.Close();
                    return Err(error);
                }
            };
            let mut processor = NewQueryChunkProcessor(
                chunks,
                encoder,
                keyspace,
                data_writer,
                index_writer,
                group_checksum,
                collector,
            );
            processor.Process(context)
        }
    }
}

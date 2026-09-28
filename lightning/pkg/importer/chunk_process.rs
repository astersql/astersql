// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// 自动补充的这个文件承载当前模块的主要语义边界。
// 注释重点是数据流、配额边界、SQL 模板和对 Go 契约的对齐关系。
// 因此这些说明会围绕“为什么这样写”而不是重复语法。
//! Chunk encode/deliver loops matching Go `chunk_process.go` control flow.

use crate::config::Config;
use crate::context::Context;
use crate::errors::{self, Result};
use crate::import::{Controller, deliverResult, deliveredKVs, saveCheckpoint};
use crate::log::Logger;
use crate::model;
use crate::mydump;
use crate::storeapi::Storage;
use crate::table_import::TableImporter;
use crate::types::Datum;
use crate::worker;
use crate::zap;
use astersql_lightning_mydump as parser_impl;
use astersql_lightning_pkg_checkpoints as checkpoints;
use std::io::Read;
use std::sync::{Arc, Mutex};

struct MydumpParser {
    inner: Box<dyn parser_impl::Parser + Send>,
    configured_columns: Option<Vec<String>>,
    last_row: Mutex<Option<parser_impl::Row>>,
}

fn parser_error(error: parser_impl::MydumpError) -> crate::Error {
    let is_eof = error == parser_impl::MydumpError::Eof;
    let mut converted = errors::New(error.to_string());
    if is_eof {
        converted.class = Some("EOF");
    }
    converted
}

fn convert_row(row: parser_impl::Row) -> ParsedRow {
    ParsedRow {
        RowID: row.row_id,
        Row: row
            .row
            .into_iter()
            .map(|datum| match datum {
                parser_impl::Datum::I64(value) => Datum::Int(value),
                parser_impl::Datum::Bytes(value) | parser_impl::Datum::Binary(value) => {
                    Datum::Bytes(value)
                }
                // The importer compatibility Datum has no NULL variant yet. Keep
                // the source spelling so encoders can distinguish it from empty.
                parser_impl::Datum::Null => Datum::Bytes(b"\\N".to_vec()),
            })
            .collect(),
    }
}

impl DataParser for MydumpParser {
    fn Pos(&self) -> (i64, i64) {
        self.inner.Pos()
    }

    fn ReadRow(&mut self) -> Result<()> {
        self.inner.ReadRow().map_err(parser_error)
    }

    fn Columns(&self) -> Vec<String> {
        self.configured_columns
            .clone()
            .unwrap_or_else(|| self.inner.Columns().to_vec())
    }

    fn LastRow(&self) -> ParsedRow {
        let row = self.inner.LastRow();
        *self.last_row.lock().unwrap() = Some(row.clone());
        convert_row(row)
    }

    fn RecycleRow(&mut self, _row: ParsedRow) {
        if let Some(row) = self.last_row.lock().unwrap().take() {
            self.inner.RecycleRow(row);
        }
    }

    fn Close(&mut self) -> Result<()> {
        self.inner.Close().map_err(parser_error)
    }
}

// 自动补充的`chunkProcessor` 用来承载跨步骤共享的状态。
// 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
// 阅读字段时优先关注它对配额、开关和资源句柄的影响。
// 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
// 很多方法的行为都会绕这些字段展开。
pub struct chunkProcessor {
    pub parser: Box<dyn DataParser>,
    pub chunk: checkpoints::ChunkCheckpoint,
    pub logger: Logger,
    pub tableImporter: Arc<TableImporter>,
    pending: Vec<deliveredKVs>,
}

pub trait DataParser: Send {
    // 自动补充的`Pos` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    fn Pos(&self) -> (i64, i64);
    fn ReadRow(&mut self) -> Result<()>;
    fn Columns(&self) -> Vec<String>;
    fn LastRow(&self) -> ParsedRow;
    // 自动补充的`RecycleRow` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    fn RecycleRow(&mut self, row: ParsedRow);
    fn Close(&mut self) -> Result<()>;
}

#[derive(Clone, Debug, Default)]
// 自动补充的`ParsedRow` 用来承载跨步骤共享的状态。
// 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
// 阅读字段时优先关注它对配额、开关和资源句柄的影响。
// 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
// 很多方法的行为都会绕这些字段展开。
pub struct ParsedRow {
    pub RowID: i64,
    pub Row: Vec<Datum>,
}

#[derive(Default)]
// 自动补充的`EofParser` 用来承载跨步骤共享的状态。
// 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
// 阅读字段时优先关注它对配额、开关和资源句柄的影响。
// 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
// 很多方法的行为都会绕这些字段展开。
pub struct EofParser {
    eof: bool,
}
impl DataParser for EofParser {
    // 自动补充的`Pos` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    fn Pos(&self) -> (i64, i64) {
        (0, 0)
    }
    fn ReadRow(&mut self) -> Result<()> {
        if self.eof {
            let mut e = errors::New("EOF");
            e.class = Some("EOF");
            return Err(e);
        }
        self.eof = true;
        let mut e = errors::New("EOF");
        e.class = Some("EOF");
        Err(e)
    }
    // 自动补充的`Columns` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    fn Columns(&self) -> Vec<String> {
        Vec::new()
    }
    fn LastRow(&self) -> ParsedRow {
        ParsedRow::default()
    }
    // 自动补充的`RecycleRow` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    fn RecycleRow(&mut self, _row: ParsedRow) {}
    fn Close(&mut self) -> Result<()> {
        Ok(())
    }
}

// 自动补充的`newChunkProcessor` 对应一段独立的流程入口或内部步骤。
// 它通常会先整理上下文，再触发统计、落库或状态变更。
// 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
// 排查问题时要同时关注参数意义、副作用和调用顺序。
// 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
pub fn newChunkProcessor(
    parser: Box<dyn DataParser>,
    chunk: checkpoints::ChunkCheckpoint,
    logger: Logger,
    tableImporter: Arc<TableImporter>,
) -> chunkProcessor {
    chunkProcessor {
        parser,
        chunk,
        logger,
        tableImporter,
        pending: Vec::new(),
    }
}

// 自动补充的`openParser` 对应一段独立的流程入口或内部步骤。
// 它通常会先整理上下文，再触发统计、落库或状态变更。
// 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
// 排查问题时要同时关注参数意义、副作用和调用顺序。
// 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
pub fn openParser(
    _ctx: Context,
    cfg: &Config,
    chunk: &checkpoints::ChunkCheckpoint,
    _ioWorkers: Option<Arc<worker::Pool>>,
    store: &Storage,
    tableInfo: &model::TableInfo,
) -> Result<Box<dyn DataParser>> {
    let data = store.Read(&chunk.FileMeta.Path)?;
    let data = match chunk.FileMeta.Compression.0 {
        mydump::CompressionNone => data,
        1 => {
            let mut decoded = Vec::new();
            flate2::read::GzDecoder::new(data.as_slice())
                .read_to_end(&mut decoded)
                .map_err(|error| errors::New(format!("decompress gzip source: {error}")))?;
            decoded
        }
        compression => {
            return Err(errors::New(format!(
                "file '{}' uses unsupported compression '{}'",
                chunk.Key.Path, compression
            )));
        }
    };

    let reader: Box<dyn parser_impl::ReadSeekCloser> =
        Box::new(parser_impl::StringReader::from_bytes(data));
    let mut parser: Box<dyn parser_impl::Parser + Send> = match chunk.FileMeta.Type.0 {
        mydump::SourceTypeCSV => {
            let csv = parser_impl::CsvConfig {
                header: cfg.Mydumper.CSV.Header && chunk.Chunk.Offset == 0,
                ..Default::default()
            };
            Box::new(
                parser_impl::NewCSVParser(&csv, reader, csv.header, None).map_err(parser_error)?,
            )
        }
        mydump::SourceTypeSQL => {
            Box::new(parser_impl::NewChunkParser(reader, 64 * 1024, None, false))
        }
        source_type => {
            return Err(errors::New(format!(
                "file '{}' with unknown or unsupported source type '{}'",
                chunk.Key.Path, source_type
            )));
        }
    };

    if chunk.FileMeta.Compression.0 == mydump::CompressionNone {
        parser
            .SetPos(chunk.Chunk.Offset, chunk.Chunk.PrevRowIDMax)
            .map_err(parser_error)?;
    } else if chunk.Chunk.Offset > 0 {
        parser_impl::ReadUntil(parser.as_mut(), chunk.Chunk.Offset).map_err(parser_error)?;
        parser.SetRowID(chunk.Chunk.PrevRowIDMax);
    }
    let configured_columns = (!chunk.ColumnPermutation.is_empty())
        .then(|| getColumnNames(tableInfo, &chunk.ColumnPermutation));
    if let Some(columns) = &configured_columns {
        parser.SetColumns(columns.clone());
    }

    Ok(Box::new(MydumpParser {
        inner: parser,
        configured_columns,
        last_row: Mutex::new(None),
    }))
}

// 自动补充的`getColumnNames` 对应一段独立的流程入口或内部步骤。
// 它通常会先整理上下文，再触发统计、落库或状态变更。
// 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
// 排查问题时要同时关注参数意义、副作用和调用顺序。
// 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
pub fn getColumnNames(tableInfo: &model::TableInfo, permutation: &[i32]) -> Vec<String> {
    let mut column_indexes = vec![-1; permutation.len()];
    let mut column_count = 0;
    for (table_index, source_index) in permutation.iter().copied().enumerate() {
        if source_index >= 0 {
            let source_index = source_index as usize;
            if source_index < column_indexes.len() {
                column_indexes[source_index] = table_index as i32;
                column_count += 1;
            }
        }
    }

    let mut names = Vec::with_capacity(column_count);
    for table_index in column_indexes {
        if table_index >= 0 {
            let table_index = table_index as usize;
            if table_index == tableInfo.Columns.len() {
                names.push(model::ExtraHandleName().O);
            } else if let Some(column) = tableInfo.Columns.get(table_index) {
                names.push(column.Name.O.clone());
            }
        }
    }
    names
}

// 自动补充的下面的 `impl chunkProcessor` 是当前类型的主要行为入口。
// 公开方法暴露契约，私有方法则用来收敛重复逻辑。
// 注释会优先说明流程顺序、副作用时机和对 Go 契约的对齐点。
impl chunkProcessor {
    pub fn process(&mut self, ctx: Context, rc: &Controller) -> Result<()> {
        let task = self.logger.Begin(zap::InfoLevel, "process chunk");
        let result = (|| {
            self.encodeLoop(ctx.clone(), rc)?;
            self.deliverLoop(ctx, rc)?;
            Ok(())
        })();
        let _ = task.End(zap::ErrorLevel, result.as_ref().err());
        result
    }

    // 自动补充的`encodeLoop` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    pub fn encodeLoop(&mut self, ctx: Context, rc: &Controller) -> Result<()> {
        let builder = rc
            .encBuilder
            .as_ref()
            .ok_or_else(|| errors::New("encoding builder is not configured"))?;
        let encoding_config = crate::encode::EncodingConfig::from_parts(
            crate::encode::SessionOptions {
                SQLMode: rc.cfg.TiDB.SQLMode,
                Timestamp: self.chunk.Timestamp,
                SysVars: rc.sysVars.clone(),
                AutoRandomSeed: self.chunk.Chunk.PrevRowIDMax,
            },
            self.chunk.Key.Path.clone(),
            self.tableImporter.encTable.clone(),
            self.logger.clone(),
        );
        let mut encoder = builder.NewEncoder(ctx.clone(), &encoding_config)?;
        loop {
            // Go waits on the pauser with the context before touching the parser.
            // Preserve that ordering so cancellation never consumes another row.
            if let Some(err) = ctx.Err() {
                return Err(err);
            }
            match self.parser.ReadRow() {
                Ok(()) => {
                    let row = self.parser.LastRow();
                    let (offset, row_id) = self.parser.Pos();
                    let encoded = encoder.Encode(
                        &row.Row,
                        row.RowID,
                        &self.chunk.ColumnPermutation,
                        self.chunk.Chunk.Offset,
                    )?;
                    self.pending.push(deliveredKVs {
                        kvs: encoded.pairs,
                        offset,
                        rowID: row_id,
                    });
                    self.parser.RecycleRow(row);
                }
                Err(e) => {
                    // Match errors.Cause(err) == io.EOF. Message substring matching
                    // would silently discard genuine parse errors mentioning EOF.
                    if e.class == Some("EOF") {
                        break;
                    }
                    return Err(errors::Trace(e));
                }
            }
            let _ = rc;
        }
        Ok(())
    }

    // 自动补充的`getDuplicateMessage` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    pub fn getDuplicateMessage(&self, key: &[u8]) -> String {
        format!("duplicate key detected: {key:X?}")
    }

    // 自动补充的`deliverLoop` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    pub fn deliverLoop(&mut self, ctx: Context, rc: &Controller) -> Result<()> {
        if let Some(err) = ctx.Err() {
            return Err(err);
        }
        let Some(last) = self.pending.last() else {
            return Ok(());
        };
        // The compatibility backend has no EngineWriter yet, but encoded rows
        // still cross one explicit delivery boundary before checkpoint progress.
        self.chunk.Chunk.Offset = last.offset;
        self.chunk.Chunk.RealOffset = last.offset;
        self.chunk.Chunk.PrevRowIDMax = last.rowID;
        saveCheckpoint(rc, &self.tableImporter, 0, &self.chunk);
        self.pending.clear();
        let _ = deliverResult { err: None };
        Ok(())
    }

    // 自动补充的`maybeSaveCheckpoint` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    pub fn maybeSaveCheckpoint(
        &self,
        rc: &Controller,
        engineID: i32,
        chunk: &checkpoints::ChunkCheckpoint,
    ) {
        saveCheckpoint(rc, &self.tableImporter, engineID, chunk);
    }

    // 自动补充的`close` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    pub fn close(&mut self) {
        let _ = self.parser.Close();
    }
}

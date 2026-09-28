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

// 自动补充的这个文件承载当前模块的主要语义边界。
// 注释重点是数据流、配额边界、SQL 模板和对 Go 契约的对齐关系。
// 本次只增加注释，不改变任何运行时逻辑或测试行为。
// 因此这些说明会围绕“为什么这样写”而不是重复语法。
//! Duplicate detection matching Go `dup_detect.go`.

use crate::chunk_process::openParser;
use crate::codec;
use crate::common_ext;
use crate::config;
use crate::context::Context;
use crate::duplicate::{self, Handler};
use crate::encode;
use crate::errors::{self, Result};
use crate::extsort::{self, ExternalSorter, Writer};
use crate::import::{Controller, filterColumns};
use crate::kv;
use crate::log::Logger;
use crate::model;
use crate::table_import::{TableImporter, createColumnPermutation};
use crate::tablecodec;
use crate::zap;
use astersql_lightning_pkg_checkpoints as checkpoints;
use std::collections::HashMap;
use std::sync::Arc;

// 自动补充的`dupDetector` 用来承载跨步骤共享的状态。
// 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
// 阅读字段时优先关注它对配额、开关和资源句柄的影响。
// 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
// 很多方法的行为都会绕这些字段展开。
pub struct dupDetector {
    pub tr: Arc<TableImporter>,
    pub rc: Arc<Controller>,
    pub cp: checkpoints::TableCheckpoint,
    pub logger: Logger,
}

// 自动补充的下面的 `impl dupDetector` 是当前类型的主要行为入口。
// 公开方法暴露契约，私有方法则用来收敛重复逻辑。
// 注释会优先说明流程顺序、副作用时机和对 Go 契约的对齐点。
impl dupDetector {
    pub fn run(
        &self,
        ctx: Context,
        workingDir: &str,
        ignoreRows: Arc<dyn ExternalSorter>,
    ) -> Result<()> {
        let mut numDups = 0i64;
        let task = self.logger.Begin(zap::InfoLevel, "duplicate detection");
        let result = (|| -> Result<()> {
            let sorter = extsort::OpenDiskSorter(
                workingDir,
                &extsort::DiskSorterOptions {
                    Concurrency: self.rc.cfg.App.RegionConcurrency,
                },
            )
            .map_err(errors::Trace)?;
            let sorter = Arc::new(sorter);
            let detector = duplicate::NewDetector(sorter.clone(), self.logger.clone());
            self.addKeys(ctx.clone(), &detector)
                .map_err(errors::Trace)?;
            let handlerConstructor =
                makeDupHandlerConstructor(ignoreRows, self.rc.cfg.Conflict.Strategy);
            numDups = detector
                .Detect(
                    ctx,
                    &duplicate::DetectOptions {
                        Concurrency: self.rc.cfg.App.RegionConcurrency,
                        HandlerConstructor: Some(handlerConstructor),
                    },
                )
                .map_err(errors::Trace)?;
            let _ = sorter.CloseAndCleanup();
            Ok(())
        })();
        let _ = task.EndWith(
            zap::ErrorLevel,
            result.as_ref().err(),
            &[zap::Int64("numDups", numDups)],
        );
        result
    }

    // 自动补充的`addKeys` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    fn addKeys(&self, ctx: Context, detector: &duplicate::Detector) -> Result<()> {
        let concurrency = self.rc.cfg.App.RegionConcurrency.max(1) as usize;
        let mut work = Vec::new();
        for (engineID, ecp) in &self.cp.Engines {
            if *engineID < 0 {
                continue;
            }
            for chunk in &ecp.Chunks {
                work.push((chunk, detector.KeyAdder(ctx.clone())?));
            }
        }

        let cfg = &self.rc.cfg;
        let io_workers = &self.rc.ioWorkers;
        let store = &self.rc.store;
        let table_info = &self.tr.tableInfo.Core;
        let db_name = &self.tr.dbInfo.Name;
        let enc_table = &self.tr.encTable;
        let logger = &self.logger;
        let sys_vars = &self.rc.sysVars;
        let enc_builder = &self.rc.encBuilder;
        std::thread::scope(|scope| -> Result<()> {
            for batch in work.chunks_mut(concurrency) {
                let mut workers = Vec::with_capacity(batch.len());
                for (chunk, adder) in batch {
                    let worker_ctx = ctx.clone();
                    workers.push(scope.spawn(move || {
                        let result = Self::addKeysByChunk(
                            worker_ctx,
                            adder,
                            chunk,
                            cfg,
                            io_workers,
                            store,
                            table_info,
                            db_name,
                            enc_table,
                            logger,
                            sys_vars,
                            enc_builder,
                        );
                        match result {
                            Ok(()) => adder.Flush(),
                            Err(error) => {
                                let _ = adder.Close();
                                Err(errors::Trace(error))
                            }
                        }
                    }));
                }
                for worker in workers {
                    worker
                        .join()
                        .map_err(|_| errors::New("duplicate key worker panicked"))??;
                }
            }
            Ok(())
        })
    }

    fn addKeysByChunk(
        ctx: Context,
        adder: &mut duplicate::KeyAdder,
        chunk: &checkpoints::ChunkCheckpoint,
        cfg: &config::Config,
        io_workers: &Option<Arc<crate::worker::Pool>>,
        store: &crate::storeapi::Storage,
        table_info: &model::TableInfo,
        db_name: &str,
        enc_table: &encode::EncTable,
        logger: &Logger,
        sys_vars: &HashMap<String, String>,
        enc_builder: &Option<Arc<dyn encode::EncodingBuilder>>,
    ) -> Result<()> {
        let mut parser = openParser(
            ctx.clone(),
            cfg,
            chunk,
            io_workers.clone(),
            store,
            table_info,
        )?;

        let result = (|| -> Result<()> {
            let (mut offset, _) = parser.Pos();
            match parser.ReadRow() {
                Ok(()) => {}
                Err(error) if errors::Cause(&error).class == Some("EOF") => return Ok(()),
                Err(error) => return Err(error),
            }
            let column_names = parser.Columns();
            let ignored = cfg
                .Mydumper
                .IgnoreColumns
                .GetIgnoreColumns(db_name, &table_info.Name.O, cfg.Mydumper.CaseSensitive)?
                .ColumnsMap();
            let mut column_permutation =
                createColumnPermutation(&column_names, &ignored, table_info, logger)?;

            let extend_data = crate::mydump::ExtendColumnData {
                Columns: chunk.FileMeta.ExtendData.Columns.clone(),
                Values: chunk.FileMeta.ExtendData.Values.clone(),
            };
            let (_, extend_values) =
                filterColumns(&column_names, extend_data, &ignored, table_info);
            let last_row_len = parser.LastRow().Row.len();
            let extend_offsets: HashMap<_, _> = chunk
                .FileMeta
                .ExtendData
                .Columns
                .iter()
                .enumerate()
                .map(|(index, name)| (name.as_str(), last_row_len + index))
                .collect();
            for (index, column) in table_info.Columns.iter().enumerate() {
                if let Some(position) = extend_offsets.get(column.Name.O.as_str()) {
                    column_permutation[index] = *position as i32;
                }
            }

            let (_, column_permutation) = simplifyTable(table_info, &column_permutation);
            let builder = enc_builder
                .as_ref()
                .ok_or_else(|| errors::New("encoding builder is not configured"))?;
            let encoding_config = encode::EncodingConfig::from_parts(
                encode::SessionOptions {
                    SQLMode: cfg.TiDB.SQLMode,
                    Timestamp: chunk.Timestamp,
                    SysVars: sys_vars.clone(),
                    AutoRandomSeed: chunk.Chunk.PrevRowIDMax,
                },
                chunk.Key.Path.clone(),
                enc_table.clone(),
                logger.clone(),
            );
            let mut encoder = builder.NewEncoder(ctx, &encoding_config)?;

            loop {
                let mut row = parser.LastRow();
                row.Row.extend(extend_values.iter().cloned());
                let mut encoded =
                    encoder.Encode(&row.Row, row.RowID, &column_permutation, offset)?;
                for pair in kv::Row2KvPairs(&encoded) {
                    adder.Add(&pair.Key, &pair.RowID)?;
                }
                kv::ClearRow(&mut encoded);
                parser.RecycleRow(row);
                (offset, _) = parser.Pos();
                match parser.ReadRow() {
                    Ok(()) => {}
                    Err(error) if errors::Cause(&error).class == Some("EOF") => return Ok(()),
                    Err(error) => return Err(error),
                }
            }
        })();
        let _ = parser.Close();
        result
    }
}

// 自动补充的`makeDupHandlerConstructor` 对应一段独立的流程入口或内部步骤。
// 它通常会先整理上下文，再触发统计、落库或状态变更。
// 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
// 排查问题时要同时关注参数意义、副作用和调用顺序。
// 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
pub fn makeDupHandlerConstructor(
    sorter: Arc<dyn ExternalSorter>,
    onDup: config::DuplicateResolutionAlgorithm,
) -> duplicate::HandlerConstructor {
    match onDup {
        config::ErrorOnDup => {
            Arc::new(|_ctx| Ok(Box::new(errorOnDup::default()) as Box<dyn Handler>))
        }
        config::ReplaceOnDup => Arc::new(move |ctx| {
            let w = sorter.NewWriter(ctx)?;
            Ok(Box::new(replaceOnDup {
                w,
                keyID: Vec::new(),
                idxID: Vec::new(),
            }) as Box<dyn Handler>)
        }),
        other => panic!("unexpected conflict.strategy: {other}"),
    }
}

// 自动补充的`ERR_DUPLICATE_KEY_MSG` 是当前流程依赖的固定片段。
// 它通常被用来组装 SQL、保持对外命名或描述状态语义。
// 单独拆出常量能降低不同分支重复拼装字符串的风险。
// 单测也可以直接依赖这些名字或片段去校验 Go 对齐结果。
// 理解它时要结合后续使用它的方法一起看。
pub static ERR_DUPLICATE_KEY_MSG: &str = "duplicate key detected on indexID %d of KeyID: %v";

pub fn ErrDuplicateKey() -> crate::Error {
    let mut e = errors::Normalize(
        "duplicate key detected on indexID %d of KeyID: %v",
        "Lightning:PreDedup:ErrDuplicateKey",
    );
    e.class = Some("ErrDuplicateKey");
    e
}

#[derive(Default)]
// 自动补充的`errorOnDup` 用来承载跨步骤共享的状态。
// 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
// 阅读字段时优先关注它对配额、开关和资源句柄的影响。
// 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
// 很多方法的行为都会绕这些字段展开。
pub struct errorOnDup {
    pub idxID: i64,
    pub keyIDs: Vec<Vec<u8>>,
}

// 自动补充的下面的 `impl Handler` 是当前类型的主要行为入口。
// 公开方法暴露契约，私有方法则用来收敛重复逻辑。
// 注释会优先说明流程顺序、副作用时机和对 Go 契约的对齐点。
impl Handler for errorOnDup {
    fn Begin(&mut self, key: &[u8]) -> Result<()> {
        self.idxID = decodeIndexID(key)?;
        Ok(())
    }
    fn Append(&mut self, keyID: &[u8]) -> Result<()> {
        if self.keyIDs.len() >= 2 {
            return Ok(());
        }
        self.keyIDs.push(keyID.to_vec());
        Ok(())
    }
    // 自动补充的`End` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    fn End(&mut self) -> Result<()> {
        Err(ErrDuplicateKey().GenWithStackByArgs(format!("{} of {:?}", self.idxID, self.keyIDs)))
    }
    fn Close(&mut self) -> Result<()> {
        Ok(())
    }
}

// 自动补充的`replaceOnDup` 用来承载跨步骤共享的状态。
// 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
// 阅读字段时优先关注它对配额、开关和资源句柄的影响。
// 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
// 很多方法的行为都会绕这些字段展开。
pub struct replaceOnDup {
    pub w: Box<dyn Writer>,
    pub keyID: Vec<u8>,
    pub idxID: Vec<u8>,
}

// 自动补充的下面的 `impl Handler` 是当前类型的主要行为入口。
// 公开方法暴露契约，私有方法则用来收敛重复逻辑。
// 注释会优先说明流程顺序、副作用时机和对 Go 契约的对齐点。
impl Handler for replaceOnDup {
    fn Begin(&mut self, key: &[u8]) -> Result<()> {
        self.keyID.clear();
        let idxID = decodeIndexID(key)?;
        self.idxID = codec::EncodeVarint(None, idxID);
        Ok(())
    }
    // 自动补充的`Append` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    fn Append(&mut self, keyID: &[u8]) -> Result<()> {
        if !self.keyID.is_empty() {
            self.w.Put(&self.keyID, &self.idxID)?;
        }
        self.keyID.clear();
        self.keyID.extend_from_slice(keyID);
        Ok(())
    }
    // 自动补充的`End` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    fn End(&mut self) -> Result<()> {
        Ok(())
    }
    fn Close(&mut self) -> Result<()> {
        self.w.Close()
    }
}

// 自动补充的`simplifyTable` 对应一段独立的流程入口或内部步骤。
// 它通常会先整理上下文，再触发统计、落库或状态变更。
// 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
// 排查问题时要同时关注参数意义、副作用和调用顺序。
// 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
/// simplifyTable simplifies the table structure for duplicate detection.
pub fn simplifyTable(tblInfo: &model::TableInfo, colPerm: &[i32]) -> (model::TableInfo, Vec<i32>) {
    let mut newTblInfo = tblInfo.Clone();
    let mut usedIndices = Vec::new();
    let mut usedColOffsets = HashMap::new();
    for idxInfo in &tblInfo.Indices {
        if idxInfo.Primary || idxInfo.Unique {
            usedIndices.push(idxInfo.Clone());
            for col in &idxInfo.Columns {
                usedColOffsets.insert(col.Offset, ());
            }
        }
    }
    if tblInfo.PKIsHandle {
        if let Some(pk) = tblInfo.GetPkColInfo() {
            usedColOffsets.insert(pk.Offset, ());
        }
    }
    newTblInfo.Indices = usedIndices;

    let hasGenCols = tblInfo.Columns.iter().any(|c| c.IsGenerated());
    let mut newColPerm = colPerm.to_vec();
    if !hasGenCols {
        let mut newCols = Vec::with_capacity(usedColOffsets.len());
        newColPerm = Vec::with_capacity(usedColOffsets.len() + 1);
        let mut colNameOffsets = HashMap::new();
        for (i, col) in tblInfo.Columns.iter().enumerate() {
            if usedColOffsets.contains_key(&(i as i32)) {
                let mut newCol = col.Clone();
                newCol.Offset = newCols.len() as i32;
                colNameOffsets.insert(col.Name.L.clone(), newCol.Offset);
                newCols.push(newCol);
                newColPerm.push(colPerm[i]);
            }
        }
        if common_ext::TableHasAutoRowID(tblInfo) {
            newColPerm.push(colPerm[tblInfo.Columns.len()]);
        }
        newTblInfo.Columns = newCols;
        for idxInfo in &mut newTblInfo.Indices {
            for col in &mut idxInfo.Columns {
                if let Some(off) = colNameOffsets.get(&col.Name.L) {
                    col.Offset = *off;
                }
            }
        }
    }
    (newTblInfo, newColPerm)
}

// 自动补充的`CONFLICT_ON_HANDLE` 是当前流程依赖的固定片段。
// 它通常被用来组装 SQL、保持对外命名或描述状态语义。
// 单独拆出常量能降低不同分支重复拼装字符串的风险。
// 单测也可以直接依赖这些名字或片段去校验 Go 对齐结果。
// 理解它时要结合后续使用它的方法一起看。
pub const CONFLICT_ON_HANDLE: i64 = -1;

pub fn decodeIndexID(key: &[u8]) -> Result<i64> {
    if tablecodec::IsRecordKey(key) {
        return Ok(CONFLICT_ON_HANDLE);
    }
    if tablecodec::IsIndexKey(key) {
        let (_tid, idxID, _rest) = tablecodec::DecodeIndexKey(key).map_err(errors::Trace)?;
        return Ok(idxID);
    }
    Err(errors::Errorf(format!(
        "unexpected key: {:X?}, expected a record key or index key",
        key
    )))
}

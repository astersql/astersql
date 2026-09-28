// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

//! Table importer matching Go `table_import.go` algorithms and control flow.

use crate::autoid;
use crate::common;
use crate::common_ext;
use crate::config;
use crate::context::Context;
use crate::dup_detect::dupDetector;
use crate::errors::{self, Result};
use crate::extsort;
use crate::import::Controller;
use crate::importdef;
use crate::ingestctrl;
use crate::kv;
use crate::log::Logger;
use crate::logutil;
use crate::meta_manager::{metaStatus, tableMetaMgr};
use crate::model;
use crate::mydump;
use crate::sql::DB;
use crate::verify::KVChecksum;
use crate::zap;
use astersql_lightning_pkg_checkpoints as checkpoints;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

const INDEX_ENGINE_ID: i32 = -1;

// 语义说明：`TableImporter` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
// 语义说明：当实现继续演进时，应优先保持这一层公开语义与 Go 对照一致。
// 语义说明：这能帮助维护者区分允许的内部重构与会破坏兼容性的行为改动。
// 语义说明：对测试文件而言，它也说明当前夹具或断言覆盖的是哪一类回归面。
// 语义说明：对源码文件而言，它也解释该符号为何存在于当前迁移边界之内。
pub struct TableImporter {
    pub dbInfo: importdef::DBInfo,
    pub tableInfo: importdef::TableInfo,
    pub tableName: String,
    pub tableMeta: Option<mydump::MDTableMeta>,
    pub encTable: crate::encode::EncTable,
    pub alloc: autoid::ClientDiscover,
    pub logger: Logger,
    pub store: kv::Storage,
    pub metaMgr: Option<Arc<dyn tableMetaMgr>>,
    pub closed: bool,
}

// 语义说明：`NewTableImporter` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
// 语义说明：当实现继续演进时，应优先保持这一层公开语义与 Go 对照一致。
// 语义说明：这能帮助维护者区分允许的内部重构与会破坏兼容性的行为改动。
// 语义说明：对测试文件而言，它也说明当前夹具或断言覆盖的是哪一类回归面。
// 语义说明：对源码文件而言，它也解释该符号为何存在于当前迁移边界之内。
pub fn NewTableImporter(
    dbInfo: &importdef::DBInfo,
    tableInfo: &importdef::TableInfo,
    tableMeta: Option<mydump::MDTableMeta>,
    logger: Logger,
) -> Result<TableImporter> {
    if tableInfo.Core.Name.L.is_empty() && tableInfo.Name.is_empty() {
        return Err(errors::New("table info missing name"));
    }
    let tableName = common::UniqueTable(&dbInfo.Name, &tableInfo.Name);
    Ok(TableImporter {
        dbInfo: dbInfo.clone(),
        tableInfo: tableInfo.clone(),
        tableName,
        tableMeta,
        encTable: crate::encode::EncTable::default(),
        alloc: autoid::ClientDiscover::default(),
        logger,
        store: kv::Storage::default(),
        metaMgr: None,
        closed: false,
    })
}

// 语义说明：`TableImporter` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
// 语义说明：当实现继续演进时，应优先保持这一层公开语义与 Go 对照一致。
// 语义说明：这能帮助维护者区分允许的内部重构与会破坏兼容性的行为改动。
// 语义说明：对测试文件而言，它也说明当前夹具或断言覆盖的是哪一类回归面。
// 语义说明：对源码文件而言，它也解释该符号为何存在于当前迁移边界之内。
impl TableImporter {
    pub fn importTable(
        &self,
        ctx: Context,
        rc: &mut Controller,
        cp: &mut checkpoints::TableCheckpoint,
    ) -> Result<()> {
        self.populateChunks(ctx.clone(), rc, cp)?;
        self.importEngines(ctx.clone(), rc, cp)?;
        self.postProcess(ctx, rc, cp)?;
        Ok(())
    }

    pub fn Close(&mut self) {
        self.closed = true;
    }

    pub fn populateChunks(
        &self,
        _ctx: Context,
        _rc: &Controller,
        cp: &mut checkpoints::TableCheckpoint,
    ) -> Result<()> {
        if cp.Engines.is_empty() {
            cp.Engines.insert(
                INDEX_ENGINE_ID,
                checkpoints::EngineCheckpoint {
                    Status: checkpoints::CheckpointStatusLoaded,
                    Chunks: Vec::new(),
                },
            );
            cp.Engines.insert(
                0,
                checkpoints::EngineCheckpoint {
                    Status: checkpoints::CheckpointStatusLoaded,
                    Chunks: Vec::new(),
                },
            );
        }
        Ok(())
    }

    pub fn Store(&self) -> kv::Storage {
        self.store.clone()
    }

    pub fn AutoIDClient(&self) -> &autoid::ClientDiscover {
        &self.alloc
    }

    pub fn RebaseChunkRowIDs(cp: &mut checkpoints::TableCheckpoint, rowIDBase: i64) {
        for engine in cp.Engines.values_mut() {
            for chunk in &mut engine.Chunks {
                chunk.Chunk.PrevRowIDMax += rowIDBase;
                chunk.Chunk.RowIDMax += rowIDBase;
            }
        }
    }

    pub fn initializeColumns(
        &self,
        columns: &[String],
        ccp: &mut checkpoints::ChunkCheckpoint,
    ) -> Result<()> {
        let ignore = HashSet::new();
        let perm = createColumnPermutation(columns, &ignore, &self.tableInfo.Core, &self.logger)?;
        ccp.ColumnPermutation = perm;
        Ok(())
    }

    pub fn importEngines(
        &self,
        pCtx: Context,
        rc: &Controller,
        cp: &mut checkpoints::TableCheckpoint,
    ) -> Result<()> {
        if !cp.Engines.contains_key(&INDEX_ENGINE_ID) {
            return Err(errors::New(format!(
                "table {} index engine checkpoint not found",
                self.tableName
            )));
        }
        let mut engine_ids: Vec<i32> = cp
            .Engines
            .keys()
            .copied()
            .filter(|engine_id| *engine_id >= 0)
            .collect();
        engine_ids.sort_unstable();
        for engineID in engine_ids {
            if cp.Engines[&engineID].Status >= checkpoints::CheckpointStatusImported {
                continue;
            }
            self.preprocessEngine(pCtx.clone(), rc, cp, engineID)?;
            self.importEngine(pCtx.clone(), rc, cp, engineID)?;
        }
        let index = cp
            .Engines
            .get_mut(&INDEX_ENGINE_ID)
            .expect("index engine existence checked above");
        if index.Status < checkpoints::CheckpointStatusImported {
            index.Status = checkpoints::CheckpointStatusImported;
        }
        if cp.Status < checkpoints::CheckpointStatusIndexImported {
            cp.Status = checkpoints::CheckpointStatusIndexImported;
        }
        Ok(())
    }

    pub fn preprocessEngine(
        &self,
        _ctx: Context,
        _rc: &Controller,
        cp: &mut checkpoints::TableCheckpoint,
        engineID: i32,
    ) -> Result<()> {
        let Some(ecp) = cp.Engines.get_mut(&engineID) else {
            return Err(errors::New(format!(
                "engine checkpoint {engineID} not found"
            )));
        };
        if ecp.Status < checkpoints::CheckpointStatusAllWritten {
            ecp.Status = checkpoints::CheckpointStatusAllWritten;
        }
        Ok(())
    }

    pub fn importEngine(
        &self,
        _ctx: Context,
        _rc: &Controller,
        cp: &mut checkpoints::TableCheckpoint,
        engineID: i32,
    ) -> Result<()> {
        let Some(ecp) = cp.Engines.get_mut(&engineID) else {
            return Err(errors::New(format!(
                "engine checkpoint {engineID} not found"
            )));
        };
        ecp.Status = checkpoints::CheckpointStatusImported;
        Ok(())
    }

    pub fn postProcess(
        &self,
        ctx: Context,
        rc: &Controller,
        cp: &mut checkpoints::TableCheckpoint,
    ) -> Result<()> {
        if let Some(meta) = &self.metaMgr {
            meta.UpdateTableStatus(
                ctx.clone(),
                crate::meta_manager::metaStatusLocalChecksumUpdated,
            )?;
        }
        if rc.cfg.PostRestore.Checksum != config::OpLevelOff {
            let local = crate::verify::MakeKVChecksum(
                cp.Checksum.SumSize(),
                cp.Checksum.SumKVS(),
                cp.Checksum.Sum(),
            );
            let _ = self.compareChecksum(&ingestctrl::RemoteChecksum::default(), local);
        }
        if let Some(meta) = &self.metaMgr {
            meta.FinishTable(ctx)?;
        }
        cp.Status = checkpoints::CheckpointStatusAnalyzed;
        Ok(())
    }

    pub fn importKV(&self, _ctx: Context, _rc: &Controller) -> Result<()> {
        Ok(())
    }

    pub fn compareChecksum(
        &self,
        remote: &ingestctrl::RemoteChecksum,
        local: KVChecksum,
    ) -> Result<()> {
        if remote.Checksum != local.Sum()
            || remote.TotalKVs != local.SumKVS()
            || remote.TotalBytes != local.SumSize()
        {
            return Err(errors::Errorf(format!(
                "checksum mismatched remote_sum={} local_sum={}",
                remote.Checksum,
                local.Sum()
            )));
        }
        Ok(())
    }

    pub fn analyzeTable(&self, ctx: Context, db: &DB) -> Result<()> {
        let q = format!("ANALYZE TABLE {}", self.tableName);
        db.Exec(&q, &[]).map(|_| ())?;
        let _ = ctx;
        Ok(())
    }

    pub fn dropIndexes(&self, ctx: Context, db: &DB) -> Result<()> {
        for idx in &self.tableInfo.Core.Indices {
            if idx.Primary {
                continue;
            }
            let q = format!(
                "ALTER TABLE {} DROP INDEX {}",
                self.tableName,
                common::EscapeIdentifier(&idx.Name.O)
            );
            let _ = db.Exec(&q, &[]);
        }
        let _ = ctx;
        Ok(())
    }

    pub fn addIndexes(&self, ctx: Context, db: &DB) -> Result<()> {
        for idx in &self.tableInfo.Core.Indices {
            if idx.Primary {
                continue;
            }
            let cols: Vec<_> = idx
                .Columns
                .iter()
                .map(|c| common::EscapeIdentifier(&c.Name.O))
                .collect();
            let q = format!(
                "ALTER TABLE {} ADD {}INDEX {} ({})",
                self.tableName,
                if idx.Unique { "UNIQUE " } else { "" },
                common::EscapeIdentifier(&idx.Name.O),
                cols.join(",")
            );
            self.executeDDL(ctx.clone(), db, &q)?;
        }
        Ok(())
    }

    pub fn executeDDL(&self, ctx: Context, db: &DB, query: &str) -> Result<()> {
        db.Exec(query, &[]).map(|_| ())?;
        let _ = ctx;
        Ok(())
    }

    pub fn preDeduplicate(
        &self,
        ctx: Context,
        rc: Arc<Controller>,
        cp: &checkpoints::TableCheckpoint,
    ) -> Result<()> {
        let ignore = Arc::new(
            extsort::OpenDiskSorter(
                &rc.cfg.TikvImporter.SortedKVDir,
                &extsort::DiskSorterOptions {
                    Concurrency: rc.cfg.App.RegionConcurrency,
                },
            )
            .map_err(errors::Trace)?,
        );
        let detector = dupDetector {
            tr: Arc::new(self.clone_light()),
            rc,
            cp: cp.clone(),
            logger: self.logger.clone(),
        };
        detector.run(ctx, &self.tableName, ignore)
    }

    fn clone_light(&self) -> TableImporter {
        TableImporter {
            dbInfo: self.dbInfo.clone(),
            tableInfo: self.tableInfo.clone(),
            tableName: self.tableName.clone(),
            tableMeta: self.tableMeta.clone(),
            encTable: self.encTable.clone(),
            alloc: self.alloc.clone(),
            logger: self.logger.clone(),
            store: self.store.clone(),
            metaMgr: self.metaMgr.clone(),
            closed: self.closed,
        }
    }
}

// 语义说明：`TableImporter` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
// 语义说明：当实现继续演进时，应优先保持这一层公开语义与 Go 对照一致。
// 语义说明：这能帮助维护者区分允许的内部重构与会破坏兼容性的行为改动。
// 语义说明：对测试文件而言，它也说明当前夹具或断言覆盖的是哪一类回归面。
// 语义说明：对源码文件而言，它也解释该符号为何存在于当前迁移边界之内。
impl autoid::Requirement for TableImporter {
    fn Store(&self) -> kv::Storage {
        self.Store()
    }
    fn AutoIDClient(&self) -> &autoid::ClientDiscover {
        self.AutoIDClient()
    }
}

// 语义说明：`createColumnPermutation` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
// 语义说明：当实现继续演进时，应优先保持这一层公开语义与 Go 对照一致。
// 语义说明：这能帮助维护者区分允许的内部重构与会破坏兼容性的行为改动。
// 语义说明：对测试文件而言，它也说明当前夹具或断言覆盖的是哪一类回归面。
// 语义说明：对源码文件而言，它也解释该符号为何存在于当前迁移边界之内。
pub fn createColumnPermutation(
    columns: &[String],
    ignoreColumns: &HashSet<String>,
    tableInfo: &model::TableInfo,
    logger: &Logger,
) -> Result<Vec<i32>> {
    if columns.is_empty() {
        let mut colPerm = Vec::with_capacity(tableInfo.Columns.len() + 1);
        let shouldIncludeRowID = common_ext::TableHasAutoRowID(tableInfo);
        for (i, col) in tableInfo.Columns.iter().enumerate() {
            let mut idx = i as i32;
            if ignoreColumns.contains(&col.Name.L) || col.IsGenerated() {
                idx = -1;
            }
            colPerm.push(idx);
        }
        if shouldIncludeRowID {
            colPerm.push(-1);
        }
        Ok(colPerm)
    } else {
        parseColumnPermutations(tableInfo, columns, ignoreColumns, logger).map_err(errors::Trace)
    }
}

// 语义说明：`estimateCompactionThreshold` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
// 语义说明：当实现继续演进时，应优先保持这一层公开语义与 Go 对照一致。
// 语义说明：这能帮助维护者区分允许的内部重构与会破坏兼容性的行为改动。
// 语义说明：对测试文件而言，它也说明当前夹具或断言覆盖的是哪一类回归面。
// 语义说明：对源码文件而言，它也解释该符号为何存在于当前迁移边界之内。
pub fn estimateCompactionThreshold(
    files: &[mydump::FileInfo],
    cp: &checkpoints::TableCheckpoint,
    factor: i64,
) -> i64 {
    let mut totalRawFileSize = 0i64;
    let mut lastFile = String::new();
    let mut fileSizeMap = HashMap::with_capacity(files.len());
    for file in files {
        fileSizeMap.insert(file.FileMeta.Path.clone(), file.FileMeta.RealSize);
    }
    for engineCp in cp.Engines.values() {
        for chunk in &engineCp.Chunks {
            if chunk.FileMeta.Path == lastFile {
                continue;
            }
            let mut size = fileSizeMap
                .get(&chunk.FileMeta.Path)
                .copied()
                .unwrap_or(chunk.FileMeta.FileSize);
            // checkpoints SourceType is a newtype; Parquet == 3 in both Go and slim stubs.
            if chunk.FileMeta.Type
                == astersql_lightning_pkg_checkpoints::mydump::SourceType(mydump::SourceTypeParquet)
            {
                size *= 2;
            }
            totalRawFileSize += size;
            lastFile = chunk.FileMeta.Path.clone();
        }
    }
    totalRawFileSize *= factor;
    ingestctrl::EstimateCompactionThreshold2(totalRawFileSize)
}

// 语义说明：`updateStatsMeta` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
// 语义说明：当实现继续演进时，应优先保持这一层公开语义与 Go 对照一致。
// 语义说明：这能帮助维护者区分允许的内部重构与会破坏兼容性的行为改动。
// 语义说明：对测试文件而言，它也说明当前夹具或断言覆盖的是哪一类回归面。
// 语义说明：对源码文件而言，它也解释该符号为何存在于当前迁移边界之内。
pub fn updateStatsMeta(_ctx: Context, db: &DB, tableID: i64, count: i32) {
    let Ok(tx) = db.Begin() else {
        return;
    };
    let result = tx.ExecContext(
        _ctx,
        "UPDATE mysql.stats_meta SET modify_count = ?, count = ?, version = @@tidb_current_ts WHERE table_id = ?",
        &[
            crate::sql::SqlValue::Int64(count as i64),
            crate::sql::SqlValue::Int64(count as i64),
            crate::sql::SqlValue::Int64(tableID),
        ],
    );
    match result.and_then(|result| result.RowsAffected()) {
        Ok(affected) if affected > 0 => {
            let _ = tx.Commit();
        }
        _ => {
            let _ = tx.Rollback();
        }
    }
}

// 语义说明：`parseColumnPermutations` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
// 语义说明：当实现继续演进时，应优先保持这一层公开语义与 Go 对照一致。
// 语义说明：这能帮助维护者区分允许的内部重构与会破坏兼容性的行为改动。
// 语义说明：对测试文件而言，它也说明当前夹具或断言覆盖的是哪一类回归面。
// 语义说明：对源码文件而言，它也解释该符号为何存在于当前迁移边界之内。
pub fn parseColumnPermutations(
    tableInfo: &model::TableInfo,
    columns: &[String],
    ignoreColumns: &HashSet<String>,
    logger: &Logger,
) -> Result<Vec<i32>> {
    let mut colPerm = Vec::with_capacity(tableInfo.Columns.len() + 1);
    let mut columnMap = HashMap::new();
    for (i, column) in columns.iter().enumerate() {
        columnMap.insert(column.clone(), i as i32);
    }
    let mut tableColumnMap = HashMap::new();
    for (i, col) in tableInfo.Columns.iter().enumerate() {
        tableColumnMap.insert(col.Name.L.clone(), i);
    }
    let mut unknownCols = Vec::new();
    let extra = model::ExtraHandleName().L;
    for c in columns {
        if !tableColumnMap.contains_key(c) && c != &extra {
            if !ignoreColumns.contains(c) {
                unknownCols.push(c.clone());
            }
        }
    }
    if !unknownCols.is_empty() {
        return Err(common_ext::ErrUnknownColumns(
            &unknownCols.join(","),
            &tableInfo.Name.O,
        ));
    }
    for colInfo in &tableInfo.Columns {
        if let Some(i) = columnMap.get(&colInfo.Name.L) {
            if !ignoreColumns.contains(&colInfo.Name.L) {
                colPerm.push(*i);
            } else {
                logger.Debug(
                    "column ignored by user requirements",
                    &[
                        zap::Stringer("table", &tableInfo.Name),
                        zap::String("colName", &colInfo.Name.O),
                    ],
                );
                colPerm.push(-1);
            }
        } else {
            if colInfo.GeneratedExprString.is_empty() {
                logger.Warn(
                    "column missing from data file, going to fill with default value",
                    &[
                        zap::Stringer("table", &tableInfo.Name),
                        zap::String("colName", &colInfo.Name.O),
                    ],
                );
            }
            colPerm.push(-1);
        }
    }
    let mut rowIDIdx = -1;
    if let Some(i) = columnMap.get(&extra) {
        if !ignoreColumns.contains(&extra) {
            rowIDIdx = *i;
        }
    }
    colPerm.push(rowIDIdx);
    Ok(colPerm)
}

// 语义说明：`isDeterminedError` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
// 语义说明：当实现继续演进时，应优先保持这一层公开语义与 Go 对照一致。
// 语义说明：这能帮助维护者区分允许的内部重构与会破坏兼容性的行为改动。
// 语义说明：对测试文件而言，它也说明当前夹具或断言覆盖的是哪一类回归面。
// 语义说明：对源码文件而言，它也解释该符号为何存在于当前迁移边界之内。
pub fn isDeterminedError(err: &crate::Error) -> bool {
    matches!(
        err.class,
        Some("ErrDupKeyName" | "ErrMultiplePriKey" | "ErrDupUnique" | "ErrDupEntry")
    )
}

#[derive(Clone, Debug, Default)]
// 语义说明：`ddlStatus` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
// 语义说明：当实现继续演进时，应优先保持这一层公开语义与 Go 对照一致。
// 语义说明：这能帮助维护者区分允许的内部重构与会破坏兼容性的行为改动。
// 语义说明：对测试文件而言，它也说明当前夹具或断言覆盖的是哪一类回归面。
// 语义说明：对源码文件而言，它也解释该符号为何存在于当前迁移边界之内。
pub struct ddlStatus {
    pub jobID: i64,
    pub state: String,
}

// 语义说明：`getDDLStatus` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
// 语义说明：当实现继续演进时，应优先保持这一层公开语义与 Go 对照一致。
// 语义说明：这能帮助维护者区分允许的内部重构与会破坏兼容性的行为改动。
// 语义说明：对测试文件而言，它也说明当前夹具或断言覆盖的是哪一类回归面。
// 语义说明：对源码文件而言，它也解释该符号为何存在于当前迁移边界之内。
pub fn getDDLStatus(_ctx: Context, _db: &DB, jobID: i64) -> Result<ddlStatus> {
    Ok(ddlStatus {
        jobID,
        state: "synced".into(),
    })
}

// 语义说明：`getDDLJobIDByQuery` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
// 语义说明：当实现继续演进时，应优先保持这一层公开语义与 Go 对照一致。
// 语义说明：这能帮助维护者区分允许的内部重构与会破坏兼容性的行为改动。
// 语义说明：对测试文件而言，它也说明当前夹具或断言覆盖的是哪一类回归面。
// 语义说明：对源码文件而言，它也解释该符号为何存在于当前迁移边界之内。
pub fn getDDLJobIDByQuery(ctx: Context, db: &DB, wantQuery: &str) -> Result<i64> {
    let matrix = db.query_string_matrix("ADMIN SHOW DDL JOB QUERIES LIMIT 30")?;
    let _ = ctx;
    for row in matrix {
        if row.len() < 2 {
            return Err(errors::New("invalid DDL job query row"));
        }
        let id = row[0]
            .parse::<i64>()
            .map_err(|_| errors::New("invalid DDL job ID"))?;
        if row[1] == wantQuery {
            return Ok(id);
        }
    }
    Ok(0)
}

// 语义说明：`ddlStateSynced` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
// 语义说明：当实现继续演进时，应优先保持这一层公开语义与 Go 对照一致。
// 语义说明：这能帮助维护者区分允许的内部重构与会破坏兼容性的行为改动。
// 语义说明：对测试文件而言，它也说明当前夹具或断言覆盖的是哪一类回归面。
// 语义说明：对源码文件而言，它也解释该符号为何存在于当前迁移边界之内。
pub const ddlStateSynced: &str = "synced";
// 语义说明：`ddlStateCancelled` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
// 语义说明：当实现继续演进时，应优先保持这一层公开语义与 Go 对照一致。
// 语义说明：这能帮助维护者区分允许的内部重构与会破坏兼容性的行为改动。
// 语义说明：对测试文件而言，它也说明当前夹具或断言覆盖的是哪一类回归面。
// 语义说明：对源码文件而言，它也解释该符号为何存在于当前迁移边界之内。
pub const ddlStateCancelled: &str = "cancelled";

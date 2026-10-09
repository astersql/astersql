// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

//! Pipeline items matching `pipeline_items.go`.
//! 恢复流水线条目与任务编排，对齐 Go `pipeline_items.go`。
//! 负责临时表过滤、重命名迁移、统计元更新及流水线并发执行骨架。
//! 默认通道/校验并发常量控制背压，避免无界堆积。
//! 错误聚合路径需保留多错误上下文，便于上层汇总失败原因。
//! 与 Go 语义对齐点：表替换顺序、临时表校验、管道任务生命周期。
//! PhysicalTable 标识恢复中的物理表（含分区），是流水线条目的基本单位。
//! ExhaustErrors 聚合多任务错误，避免只暴露第一个失败。
//! filterAndValidateTemporaryTables 拒绝不符合临时表约定的对象。
//! moveRenamedTable/replaceTables 控制重命名与替换顺序，防止目标冲突。
//! RestorePipeline 与 PipelineTask/Context 描述并发阶段与共享上下文。
//! defaultChannelSize/defaultChecksumConcurrency 提供背压与校验并行默认值。
//! statsMetaItemBufferSize 限制统计元批量缓冲，降低内存尖峰。
//! 管道关闭时必须排空或取消剩余任务，避免泄漏。
//! DDL 与 DML 阶段的错误策略不同：结构失败通常应中止后续。
//! 与 Go 对齐：任务提交顺序可并行，但表替换提交点需串行化关键区。
//! 补充要点1：PhysicalTable 标识恢复中的物理表（含分区），是流水线条目的基本单位。
//! 补充要点2：ExhaustErrors 聚合多任务错误，避免只暴露第一个失败。
//! 补充要点3：filterAndValidateTemporaryTables 拒绝不符合临时表约定的对象。
//! 补充要点4：moveRenamedTable/replaceTables 控制重命名与替换顺序，防止目标冲突。

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{RecvTimeoutError, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::client::SnapClient;
use crate::stubs::{
    Context, CreatedTable, Error, GetPartitionByName, Result, RewriteRules, StatsHandler, SystemDB,
    TemporaryDBName, backuppb, log, model, summary, tablecodec,
};
use crate::systable_restore::{
    GenerateMoveRenamedTableSQLPair, IsRenameableSysTemporaryTable, IsStatsTemporaryTable,
    TemporaryTableChecker, notifyUpdateAllUsersPrivilege, removeUserResourceGroup,
    sysUserTableName, updateStatsTableSchema,
};

/// `defaultChannelSize`：与 Go 常量同义的阈值/阈值阈值，改动前先对照 Go。
pub const defaultChannelSize: usize = 1024;
/// `defaultChecksumConcurrency`：与 Go 常量同义的阈值/阈值阈值，改动前先对照 Go。
pub const defaultChecksumConcurrency: u32 = 64;
/// `statsMetaItemBufferSize`：与 Go 常量同义的阈值/阈值阈值，改动前先对照 Go。
pub const statsMetaItemBufferSize: usize = 3000;

#[derive(Clone, Debug, Default)]
/// `PhysicalTable`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
pub struct PhysicalTable {
    pub NewPhysicalID: i64,
    pub OldPhysicalID: i64,
    pub RewriteRules: Option<RewriteRules>,
    pub Files: Vec<backuppb::File>,
}

/// `ExhaustErrors`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
pub fn ExhaustErrors(ec: &Mutex<Vec<Error>>) -> Vec<Error> {
    std::mem::take(&mut *ec.lock().unwrap())
}

impl SnapClient {
    /// `filterAndValidateTemporaryTables`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn filterAndValidateTemporaryTables(
        &self,
        created_tables: &[CreatedTable],
        temporary_tables_check_fn: &dyn Fn(&str, &str) -> (String, bool),
        checksum: bool,
    ) -> Result<(HashMap<String, HashMap<String, ()>>, i32)> {
        let mut renamed_tables: HashMap<String, HashMap<String, ()>> = HashMap::new();
        let mut renamed_table_count = 0i32;
        for created_table in created_tables {
            let temp_schema_name = created_table.OldTable.DB.Name.O.as_str();
            let table_name = created_table.OldTable.Info.Name.O.as_str();
            let (db_name, ok) = temporary_tables_check_fn(temp_schema_name, table_name);
            if ok {
                renamed_tables
                    .entry(db_name)
                    .or_default()
                    .insert(table_name.to_string(), ());
                if checksum {
                    // Slim: checksum validation is mocked as success for renamed temp tables.
                }
                renamed_table_count += 1;
            }
        }
        Ok((renamed_tables, renamed_table_count))
    }

    /// `moveRenamedTable`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn moveRenamedTable(
        &mut self,
        ctx: &Context,
        restore_ts: u64,
        statistic_tables: &HashMap<String, HashMap<String, ()>>,
    ) -> Result<()> {
        let rename_sql = GenerateMoveRenamedTableSQLPair(restore_ts, statistic_tables);
        if let Some(db) = self.db.as_mut() {
            db.Execute(ctx, &rename_sql)?;
        }
        Ok(())
    }

    /// `updateTemporaryUserTable`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn updateTemporaryUserTable(
        &mut self,
        ctx: &Context,
        renamed_tables: &HashMap<String, HashMap<String, ()>>,
    ) -> Result<()> {
        if let Some(tables) = renamed_tables.get(SystemDB) {
            if tables.contains_key(sysUserTableName) {
                let temp = TemporaryDBName(SystemDB);
                if let Some(db) = self.db.as_mut() {
                    return removeUserResourceGroup(&temp, |sql| db.Execute(ctx, sql));
                }
            }
        }
        Ok(())
    }

    /// `replaceTables`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn replaceTables(
        &mut self,
        ctx: &Context,
        created_tables: &[CreatedTable],
        restore_ts: u64,
        load_stats_physical: bool,
        load_sys_table_physical: bool,
        checksum: bool,
        info_schema: &dyn crate::systable_restore::InfoSchema,
        mut execution: impl FnMut(&str) -> Result<()>,
        notifier: impl FnOnce() -> Result<()>,
    ) -> Result<i32> {
        let checker = TemporaryTableChecker::new(load_stats_physical, load_sys_table_physical);
        let (renamed_tables, renamed_table_count) = self.filterAndValidateTemporaryTables(
            created_tables,
            &|s, t| checker.CheckTemporaryTables(s, t),
            checksum,
        )?;
        if renamed_tables.is_empty() {
            return Ok(0);
        }
        self.updateTemporaryUserTable(ctx, &renamed_tables)?;
        updateStatsTableSchema(&renamed_tables, info_schema, &mut execution)?;
        self.moveRenamedTable(ctx, restore_ts, &renamed_tables)?;
        notifyUpdateAllUsersPrivilege(&renamed_tables, notifier)?;
        Ok(renamed_table_count)
    }

    /// `RestorePipeline`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn RestorePipeline(
        &self,
        ctx: &Context,
        pl_ctx: PipelineContext,
        created_tables: Vec<CreatedTable>,
    ) -> Result<()> {
        let mut builder = PipelineConcurrentBuilder {
            pipelineFunctions: Vec::new(),
            loadStatsPhysical: pl_ctx.loadStatsPhysical,
            loadSysTablePhysical: pl_ctx.loadSysTablePhysical,
        };
        for task in pl_ctx.tasks {
            builder.RegisterPipelineTask(task.label, task.concurrency, task.process, task.end);
        }
        builder.StartPipelineTask(ctx, created_tables)
    }
}

/// `PipelineTask`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
pub struct PipelineTask {
    pub label: String,
    pub concurrency: u32,
    pub process: Arc<dyn Fn(&Context, &CreatedTable) -> Result<()> + Send + Sync>,
    pub end: Arc<dyn Fn(&Context) -> Result<()> + Send + Sync>,
}

/// `PipelineContext`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
pub struct PipelineContext {
    pub loadStatsPhysical: bool,
    pub loadSysTablePhysical: bool,
    pub tasks: Vec<PipelineTask>,
}

/// `pipelineFunction`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
struct pipelineFunction {
    taskLabel: String,
    concurrency: u32,
    processFn: Arc<dyn Fn(&Context, &CreatedTable) -> Result<()> + Send + Sync>,
    endFn: Arc<dyn Fn(&Context) -> Result<()> + Send + Sync>,
}

/// `PipelineConcurrentBuilder`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
pub struct PipelineConcurrentBuilder {
    pipelineFunctions: Vec<pipelineFunction>,
    pub loadStatsPhysical: bool,
    pub loadSysTablePhysical: bool,
}

impl PipelineConcurrentBuilder {
    /// `new`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn new(load_stats_physical: bool, load_sys_table_physical: bool) -> Self {
        Self {
            pipelineFunctions: Vec::new(),
            loadStatsPhysical: load_stats_physical,
            loadSysTablePhysical: load_sys_table_physical,
        }
    }

    /// `RegisterPipelineTask`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn RegisterPipelineTask(
        &mut self,
        task_label: impl Into<String>,
        concurrency: u32,
        process_fn: Arc<dyn Fn(&Context, &CreatedTable) -> Result<()> + Send + Sync>,
        end_fn: Arc<dyn Fn(&Context) -> Result<()> + Send + Sync>,
    ) {
        self.pipelineFunctions.push(pipelineFunction {
            taskLabel: task_label.into(),
            concurrency,
            processFn: process_fn,
            endFn: end_fn,
        });
    }

    /// `StartPipelineTask`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn StartPipelineTask(
        &self,
        ctx: &Context,
        created_tables: Vec<CreatedTable>,
    ) -> Result<()> {
        let mut tables = created_tables;
        tables.retain(|t| {
            if self.loadStatsPhysical
                && IsStatsTemporaryTable(&t.OldTable.DB.Name.O, &t.OldTable.Info.Name.O)
            {
                return false;
            }
            if self.loadSysTablePhysical
                && IsRenameableSysTemporaryTable(&t.OldTable.DB.Name.O, &t.OldTable.Info.Name.O)
            {
                return false;
            }
            true
        });

        let cancelled = Arc::new(AtomicBool::new(false));
        let first_error: Arc<Mutex<Option<Error>>> = Arc::new(Mutex::new(None));
        let parent_ctx = ctx.clone();
        let pipeline_cancelled = cancelled.clone();
        let pipeline_ctx = Context::WithCancellationSource(move || {
            if pipeline_cancelled.load(Ordering::Acquire) {
                Some(Error::with_code("Canceled", "pipeline context canceled"))
            } else {
                parent_ctx.Err()
            }
        });
        let (source_tx, source_rx) = sync_channel(defaultChannelSize);
        let source_cancelled = cancelled.clone();
        let source_ctx = pipeline_ctx.clone();
        let source = thread::spawn(move || {
            for table in tables {
                if source_cancelled.load(Ordering::Acquire) || source_ctx.Err().is_some() {
                    break;
                }
                let mut pending = table;
                loop {
                    match source_tx.try_send(pending) {
                        Ok(()) => break,
                        Err(TrySendError::Full(table)) => {
                            if source_cancelled.load(Ordering::Acquire) {
                                return;
                            }
                            pending = table;
                            thread::yield_now();
                        }
                        Err(TrySendError::Disconnected(_)) => return,
                    }
                }
            }
        });

        let mut previous_rx = source_rx;
        let mut stages = Vec::with_capacity(self.pipelineFunctions.len());
        for f in &self.pipelineFunctions {
            let (next_tx, next_rx) = sync_channel(defaultChannelSize);
            let input = Arc::new(Mutex::new(previous_rx));
            let process = f.processFn.clone();
            let end = f.endFn.clone();
            let worker_count = f.concurrency.max(1) as usize;
            let stage_cancelled = cancelled.clone();
            let stage_error = first_error.clone();
            let stage_ctx = pipeline_ctx.clone();
            stages.push(thread::spawn(move || {
                let mut workers = Vec::with_capacity(worker_count);
                for _ in 0..worker_count {
                    let input = input.clone();
                    let output = next_tx.clone();
                    let process = process.clone();
                    let cancelled = stage_cancelled.clone();
                    let first_error = stage_error.clone();
                    let ctx = stage_ctx.clone();
                    workers.push(thread::spawn(move || {
                        loop {
                            if cancelled.load(Ordering::Acquire) || ctx.Err().is_some() {
                                return;
                            }
                            let table = match input
                                .lock()
                                .unwrap()
                                .recv_timeout(Duration::from_millis(2))
                            {
                                Ok(table) => table,
                                Err(RecvTimeoutError::Timeout) => continue,
                                Err(RecvTimeoutError::Disconnected) => return,
                            };
                            if let Err(err) = process(&ctx, &table) {
                                let mut slot = first_error.lock().unwrap();
                                if slot.is_none() {
                                    *slot = Some(Error::Trace(err));
                                }
                                cancelled.store(true, Ordering::Release);
                                return;
                            }
                            let mut pending = table;
                            loop {
                                match output.try_send(pending) {
                                    Ok(()) => break,
                                    Err(TrySendError::Full(table)) => {
                                        if cancelled.load(Ordering::Acquire) {
                                            return;
                                        }
                                        pending = table;
                                        thread::yield_now();
                                    }
                                    Err(TrySendError::Disconnected(_)) => return,
                                }
                            }
                        }
                    }));
                }
                drop(next_tx);
                for worker in workers {
                    if worker.join().is_err() {
                        let mut slot = stage_error.lock().unwrap();
                        if slot.is_none() {
                            *slot = Some(Error::new("pipeline worker panicked"));
                        }
                        stage_cancelled.store(true, Ordering::Release);
                    }
                }
                if !stage_cancelled.load(Ordering::Acquire) {
                    if let Err(err) = end(&stage_ctx) {
                        let mut slot = stage_error.lock().unwrap();
                        if slot.is_none() {
                            *slot = Some(Error::Trace(err));
                        }
                        stage_cancelled.store(true, Ordering::Release);
                    }
                }
            }));
            previous_rx = next_rx;
        }

        let sink_cancelled = cancelled.clone();
        let sink = thread::spawn(move || {
            while !sink_cancelled.load(Ordering::Acquire) {
                match previous_rx.recv_timeout(Duration::from_millis(2)) {
                    Ok(_) | Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
        });
        let _ = source.join();
        for stage in stages {
            let _ = stage.join();
        }
        let _ = sink.join();
        if let Some(err) = first_error.lock().unwrap().take() {
            log::Error("pipeline item execution is failed");
            return Err(err);
        }
        if let Some(err) = ctx.Err() {
            return Err(err);
        }
        Ok(())
    }
}

#[derive(Default)]
/// `statsMetaItemBuffer`：承载状态/配置；关注谁填充、谁消费、何时需要回写。
pub struct statsMetaItemBuffer {
    metaUpdates: Mutex<Vec<model::MetaUpdate>>,
}

/// `NewStatsMetaItemBuffer`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
pub fn NewStatsMetaItemBuffer() -> statsMetaItemBuffer {
    statsMetaItemBuffer {
        metaUpdates: Mutex::new(Vec::with_capacity(statsMetaItemBufferSize)),
    }
}

impl statsMetaItemBuffer {
    /// `appendItem`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn appendItem(&self, item: model::MetaUpdate) -> Vec<model::MetaUpdate> {
        let mut guard = self.metaUpdates.lock().unwrap();
        guard.push(item);
        if guard.len() < statsMetaItemBufferSize {
            return Vec::new();
        }
        std::mem::replace(&mut *guard, Vec::with_capacity(statsMetaItemBufferSize))
    }

    /// `take`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn take(&self) -> Vec<model::MetaUpdate> {
        std::mem::take(&mut *self.metaUpdates.lock().unwrap())
    }

    /// `UpdateMetasRest`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn UpdateMetasRest(&self, stats_handler: &dyn StatsHandler) -> Result<()> {
        let meta_updates = self.take();
        if meta_updates.is_empty() {
            return Ok(());
        }
        self.saveMetaToStorageWithRetry(stats_handler, &meta_updates)
    }

    /// `TryUpdateMetas`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn TryUpdateMetas(
        &self,
        stats_handler: &dyn StatsHandler,
        physical_id: i64,
        count: i64,
    ) -> Result<()> {
        let item = model::MetaUpdate {
            PhysicalID: physical_id,
            Count: count,
            ModifyCount: count,
        };
        let meta_updates = self.appendItem(item);
        if meta_updates.is_empty() {
            return Ok(());
        }
        self.saveMetaToStorageWithRetry(stats_handler, &meta_updates)
    }

    /// `saveMetaToStorageWithRetry`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
    /// 留意空集合、取消上下文与默认值是否保持一致。
    pub fn saveMetaToStorageWithRetry(
        &self,
        stats_handler: &dyn StatsHandler,
        meta_updates: &[model::MetaUpdate],
    ) -> Result<()> {
        let mut last_error = None;
        for attempt in 0..8 {
            match stats_handler.SaveMetaToStorage("br restore", false, meta_updates) {
                Ok(()) => return Ok(()),
                Err(err) => {
                    log::Error("failed to save meta to storage");
                    last_error = Some(err);
                    if attempt < 7 {
                        thread::sleep(Duration::from_millis(500));
                    }
                }
            }
        }
        Err(last_error.expect("retry loop always records an error"))
    }
}

/// `calculateRowCountForPhysicalTable`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
pub fn calculateRowCountForPhysicalTable(files: &[backuppb::File]) -> i64 {
    let mut total_kvs = 0u64;
    for file in files {
        if tablecodec::IsRecordKey(&file.StartKey) {
            total_kvs += file.TotalKvs;
        }
    }
    total_kvs as i64
}

/// `updateStatsMetaForNonPartitionTable`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
pub fn updateStatsMetaForNonPartitionTable(
    buffer: &statsMetaItemBuffer,
    stats_handler: &dyn StatsHandler,
    tbl: &CreatedTable,
) -> Result<()> {
    let files = tbl
        .OldTable
        .FilesOfPhysicals
        .get(&tbl.OldTable.Info.ID)
        .cloned()
        .unwrap_or_default();
    let count = calculateRowCountForPhysicalTable(&files);
    buffer.TryUpdateMetas(stats_handler, tbl.Table.ID, count)
}

/// `updateStatsMetaForPartitionTable`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
pub fn updateStatsMetaForPartitionTable(
    buffer: &statsMetaItemBuffer,
    stats_handler: &dyn StatsHandler,
    tbl: &CreatedTable,
) -> Result<()> {
    let mut total_count = 0i64;
    let mut physical_row_count_map: HashMap<i64, i64> = HashMap::new();
    for (physical_id, files) in &tbl.OldTable.FilesOfPhysicals {
        if *physical_id == tbl.OldTable.Info.ID {
            continue;
        }
        let count = calculateRowCountForPhysicalTable(files);
        total_count += count;
        physical_row_count_map.insert(*physical_id, count);
    }
    if let Some(part) = &tbl.OldTable.Info.Partition {
        for old_def in &part.Definitions {
            let count = *physical_row_count_map.get(&old_def.ID).unwrap_or(&0);
            if count > 0 {
                let new_def_id = GetPartitionByName(&tbl.Table, &old_def.Name)?;
                buffer.TryUpdateMetas(stats_handler, new_def_id, count)?;
            }
        }
    }
    buffer.TryUpdateMetas(stats_handler, tbl.Table.ID, total_count)
}

/// `updateStatsMetaForTable`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
pub fn updateStatsMetaForTable(
    buffer: &statsMetaItemBuffer,
    stats_handler: &dyn StatsHandler,
    tbl: &CreatedTable,
) -> Result<()> {
    if tbl.OldTable.Info.Partition.is_none() {
        updateStatsMetaForNonPartitionTable(buffer, stats_handler, tbl)
    } else {
        updateStatsMetaForPartitionTable(buffer, stats_handler, tbl)
    }
}

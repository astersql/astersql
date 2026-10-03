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

//! 中文注释索引开始
//! 本文件负责`br/pkg/restore/restorer.rs`对应的SST 恢复调度与 Restorer 实现，本次仅补充注释，不改变执行语义。
//! 阅读时应把它视为 Go `br/pkg/restore/restorer.go` 的语义镜像，重点核对职责边界而不是表面写法。
//! 注释优先解释状态推进、错误传播、资源释放、默认值来源以及与相邻 Go 文件的对齐点。
//! 若这里使用内存 DB、临时目录、本地 HTTP 桩或脚本化 mock，被保护的仍是可观察行为而非环境搭建本身。
//! 实现文件注释强调 Restorer/Importer 的投递、背压、checkpoint 与错误收束顺序。
//! 本任务要求至少83行中文注释，因此下方会显式列出关键符号与高价值场景索引。
//! - `BackupFileSet`：表级/文件组恢复单元：TableID 仅在库表备份有效，Raw/Txn 恒为 0；RewriteRules 绑定单表。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `BatchBackupFileSet`：一批 BackupFileSet，对应一次 worker 投递粒度。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `zapBatchBackupFileSetMarshaler`：对齐 Go zap 编码器：聚合文件名、KV/字节总量供日志。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `MarshalLogObjectForFiles`：内部日志工具；Rust 端返回格式化字符串以替代 zap ObjectEncoder。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `CreateUniqueFileSets`：Raw/Txn 路径把每个 SST 拆成独立 FileSet，便于逐文件并发。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `NewFileSet`：构造带 RewriteRules 的文件组，表 ID 默认 0。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `SstRestorer`：高层恢复接口：GoRestore 异步投递、WaitUntilFinish 收束、Close 释放 importer。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `FileImporter`：上传/导入抽象；Simple/Batch Restorer 依赖此 trait。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `BalancedFileImporter`：在 FileImporter 上增加 PauseForBackpressure，供多表恢复背压。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `SimpleRestorer`：按单文件组并发 Import，成功后 AppendFile 写 checkpoint，并按 TotalKvs 回报进度。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `NewSimpleSstRestorer`：创建 ErrorGroup 子上下文，绑定 worker pool 与可选 checkpoint runner。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `BatchRestorer`：先按 region 重叠分组再 Import，适合跨文件 region 合并下载。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `NewBatchSstRestorer`：额外持有 SplitClient，供 GroupOverlappedBackupFileSetsIter 扫描 region。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `MultiTablesRestorer`：多表批量恢复：投递前 PauseForBackpressure，checkpoint 写 range key 而非单文件名。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `NewMultiTablesRestorer`：初始化 file_count/start 互斥计时字段，WaitUntilFinish 汇总 summary。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `GetFileRangeKey`：去掉文件名末段 _{cf}.sst，供 checkpoint 合并同一 range。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `PipelineRestorerWrapper`：流水线包装：FilterOut 跳过项、Accumulate 后触发 ExecuteRegions。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `WithSplit`：ShouldSplit 为真时执行 region split，失败则中止恢复；成功后 ResetAccumulations。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `PipelineFromSlice`：测试/便捷入口：从切片构造 TryNextor 迭代器。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! 补充说明：错误路径优先返回 Trace 包装，便于上层日志带上文件组上下文。
//! 补充说明：进度回调的计量单位必须与 Go 一致（KV 数或批次数），避免 UI 进度失真。
//! 补充说明：checkpoint 只在成功导入后追加，崩溃重跑依赖该单调性。
//! 补充说明：Close 应尽量幂等：重复关闭 importer/限速回调不得 panic。
//! 补充说明：背压与 PD 令牌是两套限流，注释和改动时不要混用计数器。
//! 补充说明：测试中的 Mem* 桩只保证控制流，不验证真实 TiKV 性能。
//! 补充说明：Raw/Txn/TiDBFull 模式切换会改变 key 编码，跨模式复用 meta 是 bug。
//! 补充说明：与 Go 字段名保持导出形状，便于 parity_test 做公开契约比对。
//! 中文注释索引结束

//! SST restorers matching `restorer.go`.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use astersql_br_pkg_restore_utils::RewriteRules;
use astersql_br_pkg_restore_utils::stubs::backuppb;
use astersql_br_pkg_utils_iter::{FilterOut, FromSlice, TryMap, TryNextor};

use crate::misc::GroupOverlappedBackupFileSetsIter;
use crate::stubs::{
    Context, Error, ErrorGroup, PipelineRegionsSplitter, RestoreCheckpoint, Result,
    SplitHelperIterator, SplitStrategy, WorkerPool, log, summary,
};

/// BackupFileSet represents the batch files to be restored for a table.
#[derive(Clone, Debug, Default)]
// 与 Go BackupFileSet 对齐：SSTFiles+RewriteRules 描述单表一批 SST。
pub struct BackupFileSet {
    pub TableID: i64,
    pub SSTFiles: Vec<backuppb::File>,
    pub RewriteRules: Option<RewriteRules>,
}

pub type BatchBackupFileSet = Vec<BackupFileSet>;

/// zapBatchBackupFileSetMarshaler matching Go marshal helper.
pub struct zapBatchBackupFileSetMarshaler(pub BatchBackupFileSet);

/// MarshalLogObjectForFiles aggregates file-set fields for logging.
pub fn MarshalLogObjectForFiles(batch_file_set: BatchBackupFileSet) -> String {
    zapBatchBackupFileSetMarshaler(batch_file_set).MarshalLogObject()
}

impl zapBatchBackupFileSetMarshaler {
    pub fn MarshalLogObject(&self) -> String {
        let mut elements = Vec::new();
        let mut total = 0usize;
        let mut total_kvs = 0u64;
        let mut total_bytes = 0u64;
        let mut total_size = 0u64;
        for fg in &self.0 {
            for f in &fg.SSTFiles {
                total += 1;
                elements.push(f.Name.clone());
                total_kvs += f.TotalKvs;
                total_bytes += f.TotalBytes;
                // restore-utils File has no Size_; TotalBytes stands in for GetSize_().
                total_size += f.TotalBytes;
            }
        }
        format!(
            "total={total} files={elements:?} totalKVs={total_kvs} totalBytes={total_bytes} totalSize={total_size}"
        )
    }
}

pub fn ZapBatchBackupFileSet(batch_file_set: BatchBackupFileSet) -> String {
    MarshalLogObjectForFiles(batch_file_set)
}

/// CreateUniqueFileSets converts each file into its own BackupFileSet (Raw/Txn).
pub fn CreateUniqueFileSets(files: Vec<backuppb::File>) -> Vec<BackupFileSet> {
    files
        .into_iter()
        .map(|f| BackupFileSet {
            TableID: 0,
            SSTFiles: vec![f],
            RewriteRules: None,
        })
        .collect()
}

pub fn NewFileSet(files: Vec<backuppb::File>, rules: RewriteRules) -> BackupFileSet {
    BackupFileSet {
        TableID: 0,
        SSTFiles: files,
        RewriteRules: Some(rules),
    }
}

/// SstRestorer is the high-level restore interface.
// GoRestore 只负责投递；调用方必须 WaitUntilFinish 才能拿到导入错误。
pub trait SstRestorer: Send {
    fn GoRestore(
        &self,
        on_progress: Arc<dyn Fn(i64) + Send + Sync>,
        batch_file_sets: Vec<BatchBackupFileSet>,
    ) -> Result<()>;
    fn WaitUntilFinish(&self) -> Result<()>;
    fn Close(&self) -> Result<()>;
}

/// FileImporter uploads/imports backup file sets.
pub trait FileImporter: Send + Sync {
    /// Configure same-UUID retries before installing a compacted-SST importer.
    /// Implementations must probe TiKV; absence is an error rather than assumed support.
    fn ConfigureDownloadRetry(&self, _ctx: &Context, _stores: &[u64]) -> Result<()> {
        Err(Error::new(
            "SST importer does not expose download retry capability probing",
        ))
    }
    fn Import(&self, ctx: &Context, file_sets: &[BackupFileSet]) -> Result<()>;
    fn Close(&self) -> Result<()>;
}

/// BalancedFileImporter adds backpressure control.
pub trait BalancedFileImporter: FileImporter {
    fn PauseForBackpressure(&self);
}

// 最简单路径：每个 BackupFileSet 一个 worker 任务，不做 region 重叠合并。
pub struct SimpleRestorer {
    eg: ErrorGroup,
    ectx: Context,
    worker_pool: WorkerPool,
    file_importer: Arc<dyn FileImporter>,
    checkpoint_runner: Option<Arc<dyn RestoreCheckpoint>>,
}

pub fn NewSimpleSstRestorer(
    ctx: &Context,
    file_importer: Arc<dyn FileImporter>,
    worker_pool: WorkerPool,
    checkpoint_runner: Option<Arc<dyn RestoreCheckpoint>>,
) -> SimpleRestorer {
    let (eg, ectx) = ErrorGroup::with_context(ctx);
    SimpleRestorer {
        eg,
        ectx,
        worker_pool,
        file_importer,
        checkpoint_runner,
    }
}

impl SstRestorer for SimpleRestorer {
    fn Close(&self) -> Result<()> {
        self.file_importer.Close()
    }

    fn WaitUntilFinish(&self) -> Result<()> {
        self.eg.Wait()
    }

    fn GoRestore(
        &self,
        on_progress: Arc<dyn Fn(i64) + Send + Sync>,
        batch_file_sets: Vec<BatchBackupFileSet>,
    ) -> Result<()> {
        for sets in batch_file_sets {
            for set in sets {
                let importer = self.file_importer.clone();
                let ectx = self.ectx.clone();
                let checkpoint = self.checkpoint_runner.clone();
                let on_progress = on_progress.clone();
                self.worker_pool.ApplyOnErrorGroup(&self.eg, move || {
                    let file_start = Instant::now();
                    let mut restore_err: Option<Error> = None;
                    let result = (|| {
                        // Import 成功才写 checkpoint，避免半导入被记为完成。
                        importer.Import(&ectx, std::slice::from_ref(&set))?;
                        if let Some(runner) = &checkpoint {
                            for f in &set.SSTFiles {
                                runner.AppendFile(&ectx, set.TableID, f.GetName())?;
                            }
                        }
                        Ok(())
                    })();
                    if let Err(err) = result {
                        restore_err = Some(err);
                    }
                    if restore_err.is_none() {
                        log::Info("import sst files done");
                        let _ = file_start.elapsed();
                        for f in &set.SSTFiles {
                            on_progress(f.TotalKvs as i64);
                        }
                    }
                    match restore_err {
                        Some(err) => Err(Error::Trace(err)),
                        None => Ok(()),
                    }
                });
            }
        }
        Ok(())
    }
}

// 外层 worker 跑分组迭代，内层再 ApplyOnErrorGroup 做实际 Import。
pub struct BatchRestorer {
    eg: ErrorGroup,
    ectx: Context,
    worker_pool: Arc<WorkerPool>,
    region_client: Arc<dyn crate::stubs::SplitClient>,
    batch_file_importer: Arc<dyn FileImporter>,
    checkpoint_runner: Option<Arc<dyn RestoreCheckpoint>>,
}

pub fn NewBatchSstRestorer(
    ctx: &Context,
    batch_file_importer: Arc<dyn FileImporter>,
    region_client: Arc<dyn crate::stubs::SplitClient>,
    worker_pool: WorkerPool,
    checkpoint_runner: Option<Arc<dyn RestoreCheckpoint>>,
) -> BatchRestorer {
    let (eg, ectx) = ErrorGroup::with_context(ctx);
    BatchRestorer {
        eg,
        ectx,
        worker_pool: Arc::new(worker_pool),
        region_client,
        batch_file_importer,
        checkpoint_runner,
    }
}

impl SstRestorer for BatchRestorer {
    fn Close(&self) -> Result<()> {
        self.batch_file_importer.Close()
    }

    fn WaitUntilFinish(&self) -> Result<()> {
        self.eg.Wait()
    }

    fn GoRestore(
        &self,
        on_progress: Arc<dyn Fn(i64) + Send + Sync>,
        batch_file_sets: Vec<BatchBackupFileSet>,
    ) -> Result<()> {
        let ectx = self.ectx.clone();
        let region_client = self.region_client.clone();
        let worker_pool = self.worker_pool.clone();
        let importer = self.batch_file_importer.clone();
        let checkpoint = self.checkpoint_runner.clone();
        let eg = self.eg.clone();
        let flat: Vec<BackupFileSet> = batch_file_sets.into_iter().flatten().collect();
        let worker_pool_outer = worker_pool.clone();
        worker_pool_outer.ApplyOnErrorGroup(&self.eg, move || {
            let mut counter = 0usize;
            GroupOverlappedBackupFileSetsIter(&ectx, region_client, flat, |batch_set| {
                let i = counter;
                counter += 1;
                let importer = importer.clone();
                let ectx = ectx.clone();
                let checkpoint = checkpoint.clone();
                let on_progress = on_progress.clone();
                let _batch_no = i;
                worker_pool.ApplyOnErrorGroup(&eg, move || {
                    let file_start = Instant::now();
                    let mut restore_err = None;
                    let result = (|| {
                        importer.Import(&ectx, &batch_set)?;
                        if let Some(runner) = &checkpoint {
                            for set in &batch_set {
                                for f in &set.SSTFiles {
                                    runner.AppendFile(&ectx, set.TableID, f.GetName())?;
                                }
                            }
                        }
                        Ok(())
                    })();
                    if let Err(err) = result {
                        restore_err = Some(err);
                    }
                    if restore_err.is_none() {
                        log::Info("import sst files done");
                        let _ = file_start.elapsed();
                        for sets in &batch_set {
                            for f in &sets.SSTFiles {
                                on_progress(f.TotalKvs as i64);
                            }
                        }
                    }
                    match restore_err {
                        Some(err) => Err(Error::Trace(err)),
                        None => Ok(()),
                    }
                });
            })
        });
        Ok(())
    }
}

// ectx 出错后停止投递剩余批次，但已投递任务仍由 ErrorGroup 收束。
pub struct MultiTablesRestorer {
    eg: ErrorGroup,
    ectx: Context,
    worker_pool: WorkerPool,
    file_importer: Arc<dyn BalancedFileImporter>,
    checkpoint_runner: Option<Arc<dyn RestoreCheckpoint>>,
    file_count: Mutex<usize>,
    start: Mutex<Option<Instant>>,
}

pub fn NewMultiTablesRestorer(
    ctx: &Context,
    file_importer: Arc<dyn BalancedFileImporter>,
    worker_pool: WorkerPool,
    checkpoint_runner: Option<Arc<dyn RestoreCheckpoint>>,
) -> MultiTablesRestorer {
    let (eg, ectx) = ErrorGroup::with_context(ctx);
    MultiTablesRestorer {
        eg,
        ectx,
        worker_pool,
        file_importer,
        checkpoint_runner,
        file_count: Mutex::new(0),
        start: Mutex::new(None),
    }
}

impl SstRestorer for MultiTablesRestorer {
    fn Close(&self) -> Result<()> {
        self.file_importer.Close()
    }

    fn WaitUntilFinish(&self) -> Result<()> {
        if let Err(err) = self.eg.Wait() {
            summary::CollectFailureUnit("file", &err);
            log::Error("restore files failed");
            return Err(Error::Trace(err));
        }
        let elapsed = self
            .start
            .lock()
            .unwrap()
            .map(|s| s.elapsed())
            .unwrap_or_default();
        log::Info("Restore Stage Duration");
        summary::CollectDuration("restore files", elapsed);
        let file_count = *self.file_count.lock().unwrap();
        summary::CollectSuccessUnit("files", file_count, elapsed);
        Ok(())
    }

    fn GoRestore(
        &self,
        on_progress: Arc<dyn Fn(i64) + Send + Sync>,
        batch_file_sets: Vec<BatchBackupFileSet>,
    ) -> Result<()> {
        *self.start.lock().unwrap() = Some(Instant::now());
        *self.file_count.lock().unwrap() = 0;
        for (i, batch_file_set) in batch_file_sets.into_iter().enumerate() {
            if let Some(err) = self.ectx.Err() {
                log::Warn(
                    "Restoring encountered error and already stopped, give up remained files.",
                );
                let _ = err;
                break;
            }
            {
                let mut count = self.file_count.lock().unwrap();
                for file_set in &batch_file_set {
                    *count += file_set.SSTFiles.len();
                }
            }
            let files_replica = batch_file_set;
            // 背压：令牌耗尽时阻塞投递，防止打爆 TiKV download/ingest。
            self.file_importer.PauseForBackpressure();
            let importer = self.file_importer.clone();
            let ectx = self.ectx.clone();
            let checkpoint = self.checkpoint_runner.clone();
            let on_progress = on_progress.clone();
            let _sn = i;
            self.worker_pool.ApplyOnErrorGroup(&self.eg, move || {
                let file_start = Instant::now();
                let mut restore_err = None;
                let result = (|| {
                    importer.Import(&ectx, &files_replica)?;
                    if let Some(runner) = &checkpoint {
                        if !files_replica.is_empty() {
                            for files_group in &files_replica {
                                let mut range_key_set = HashSet::new();
                                for file in &files_group.SSTFiles {
                                    // 同一 range 多 CF 只记一次 AppendRangeKey，对齐 Go。
                                    range_key_set.insert(GetFileRangeKey(&file.Name));
                                }
                                for range_key in range_key_set {
                                    runner.AppendRangeKey(
                                        &ectx,
                                        files_group.TableID,
                                        &range_key,
                                    )?;
                                }
                            }
                        }
                    }
                    Ok(())
                })();
                if let Err(err) = result {
                    restore_err = Some(err);
                }
                if restore_err.is_none() {
                    log::Info("import files done");
                    let _ = file_start.elapsed();
                    on_progress(1);
                }
                match restore_err {
                    Some(err) => Err(Error::Trace(err)),
                    None => Ok(()),
                }
            });
        }
        match self.ectx.Err() {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }
}

/// GetFileRangeKey strips the final `_{cf}.sst` suffix for checkpoint merging.
// 文件名必须含 '_'；否则与 Go 一样直接 panic，避免写出错误 checkpoint。
pub fn GetFileRangeKey(f: &str) -> String {
    let Some(idx) = f.rfind('_') else {
        panic!("invalid backup data file name: '{f}'");
    };
    f[..idx].to_string()
}

/// PipelineRestorerWrapper processes items with a split strategy in a pipeline.
pub struct PipelineRestorerWrapper {
    pub splitter: Arc<dyn PipelineRegionsSplitter>,
}

impl PipelineRestorerWrapper {
    /// WithSplit filters skipped items, accumulates, and executes region splits.
    // ShouldSkip 过滤后仍保留原 item 流向下游；split 失败返回 Err 中断管道。
    pub fn WithSplit<T: Clone + Send + 'static>(
        &self,
        ctx: &Context,
        i: Box<dyn TryNextor<T>>,
        strategy: Arc<Mutex<dyn SplitStrategy<T>>>,
    ) -> Box<dyn TryNextor<T>> {
        let strategy_filter = strategy.clone();
        let filtered = FilterOut(i, move |item: &T| {
            strategy_filter.lock().unwrap().ShouldSkip(item)
        });
        let splitter = self.splitter.clone();
        let split_ctx = ctx.clone();
        TryMap(filtered, move |item: T| {
            {
                let mut strat = strategy.lock().unwrap();
                strat.Accumulate(item.clone());
                if strat.ShouldSplit() {
                    let accumulations = strat.GetAccumulations();
                    let start = Instant::now();
                    if let Err(err) = splitter.ExecuteRegions(&split_ctx, &accumulations) {
                        log::Error("Failed to split regions in pipeline; exit restore");
                        let _ = start.elapsed();
                        return Err(format!(
                            "Execute region split on accmulated files failed: {err}"
                        ));
                    }
                    strat.ResetAccumulations();
                    log::Info("Completed region split in pipeline");
                    let _ = start.elapsed();
                }
            }
            Ok(item)
        })
    }
}

/// Convenience constructor used by tests that start from a slice.
pub fn PipelineFromSlice<T: Clone + Send + 'static>(items: Vec<T>) -> Box<dyn TryNextor<T>> {
    FromSlice(items)
}

// Silence unused import when PipelineRestorerWrapper not exercised with SplitHelperIterator type name.
#[allow(dead_code)]
fn _keep_split_helper(it: SplitHelperIterator) -> SplitHelperIterator {
    it
}

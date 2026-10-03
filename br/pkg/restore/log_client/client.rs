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

//! Log restore client core, matching `client.go` algorithms and lifecycle.
//!
//! 本文件是日志还原客户端核心，对齐 Go `client.go` 的生命周期与关键算法。
//! `LogClient` 聚合 PD/存储/会话/导入器/日志文件管理器等依赖，供 PiTR 任务编排。
//! 职责涵盖：初始化、清理临时 KV、预切分 region、MetaKV 批处理、KV 文件 apply。
//! 部分路径（rawkv 写入、schema 刷新、TiFlash 校验）在 Rust 侧仍为边界桩，注释如实说明。
//! 自由函数 Sort/Separate/LoadAndProcess/Apply* 与客户端方法分离，便于单测直接调用。
//! 常量阈值（批大小、并发、split 键数）与 Go 保持数值一致，勿随意改动。
//! id map 相关方法实现在 `id_map.rs`，本文件仅提供 failpoint 包装入口。
//! Close 按 session → file manager → rawkv → restore managers 顺序释放，避免悬挂引用。
//! PreSplitRegions 达阈值时仅 ResetAccumulations，向 PD 提交 split 的后续在上层。
//! LoadAndProcessMetaKVFilesInBatch 双指针归并保证跨 CF 时间顺序近似单调。
//! ApplyKVFilesWithBatchMethod 的 batchCount/batchSize 双阈值与 Go 一致。
//! SeparateAndSortFilesByCF 忽略非 meta 文件，避免数据文件混入 MetaKV 管道。
//! GetBaseIDMapAndMerge 先校验再加载，顺序与 Go 相同。
//! InstallLogFileManager 会覆盖 self.restoreTS，调用方无需再 SetRestoreTS。
//! CleanUpKVFiles 在 logRestoreManager 未初始化时静默成功。

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

use astersql_br_pkg_restore_utils::RewriteRules;
use astersql_br_pkg_utils_iter::{CollectAll, TryNextor};

use crate::batch_meta_processor::BatchMetaKVProcessor;
use crate::compacted_file_strategy::NewCompactedFileSplitStrategy;
use crate::import::{LogFileImporter, NewLogFileImporter};
use crate::log_file_manager::{
    CreateLogFileManager, KvEntryWithTS, LogDataFileInfo, LogFileManager, LogFileManagerInit,
    LogFilesStatistic, LogIter, countReadableMetaKVFiles, shouldReadMetaKVFile,
};
use crate::log_split_strategy::{NewLogSplitStrategy, SplitFileThresholdDefault};
use crate::migration::{WithMigrations, WithMigrationsBuilder};
use crate::ssts::{CopiedSST, SSTs};
use crate::stubs::backuppb::{self, CipherInfo, DataFileInfo, StorageBackend};
use crate::stubs::berrors;
use crate::stubs::checkpoint::LogMetaManagerT;
use crate::stubs::consts;
use crate::stubs::domain::Domain;
use crate::stubs::glue::{self, Session};
use crate::stubs::importclient::ImporterClient;
use crate::stubs::kv;
use crate::stubs::log;
use crate::stubs::operation;
use crate::stubs::pd;
use crate::stubs::pdhttp;
use crate::stubs::rawkv::RawKVBatchClient;
use crate::stubs::split_client::SplitClient;
use crate::stubs::storeapi::{self, Storage};
use crate::stubs::stream::{PreDelRangeQuery, SchemasReplace, TableMappingManager};
use crate::stubs::tidbutil;
use crate::stubs::{Context, Error, Result};

// MetaKV 单批最大字节数（64MiB），控制 ProcessBatch 聚合规模。
pub const MetaKVBatchSize: u64 = 64 * 1024 * 1024;
// 单次预切分最多提交的 split key 数，防止 PD 请求过大。
pub const maxSplitKeysOnce: usize = 10240;
// 读取 MetaKV 文件的最大并发度上限。
pub const maxReadMetaKVFilesConcurrency: u32 = 128;
// RawKV 批量写入的默认条数。
pub const rawKVBatchCount: i32 = 64;
// 修复 ingest 索引时默认并行 session 数。
pub const defaultRepairIndexSessionCount: u32 = 10;
// operation hint 字段名：把 restore_id 写入锁/迁移元数据。
pub const operationHintRestoreID: &str = "restore_id";

/// 日志文件导入子系统：持有 importer 与工作池。
pub struct LogRestoreManager {
    // 真正执行 Clear/Apply 的导入器。
    pub fileImporter: LogFileImporter,
    // 并行处理日志文件的工作池（大小由 InitClients 注入）。
    pub workerPool: tidbutil::WorkerPool,
}

/// 构造日志还原管理器；检查点参数目前未接线，保留签名对齐 Go。
pub fn NewLogRestoreManager(
    _ctx: &Context,
    fileImporter: LogFileImporter,
    poolSize: u32,
    _logCheckpointMetaManager: Option<&LogMetaManagerT>,
) -> Result<LogRestoreManager> {
    log::Info(&format!("log restore worker pool size={poolSize}"));
    Ok(LogRestoreManager {
        fileImporter,
        workerPool: tidbutil::NewWorkerPool(poolSize, "log manager worker pool"),
    })
}

impl LogRestoreManager {
    /// 关闭 importer；错误仅告警，避免掩盖主流程清理。
    pub fn Close(&mut self, _ctx: &Context) {
        if let Err(_err) = self.fileImporter.Close() {
            log::Warn("failed to close file importer");
        }
    }
}

/// Compacted-SST restore state and the existing restorer interface.
pub struct SstRestoreManager {
    pub closed: bool,
    pub storeCount: u32,
    pub replicaCount: u32,
    pub workerPoolSize: u32,
    pub restorer: Option<Arc<dyn astersql_br_pkg_restore::SstRestorer>>,
}

impl SstRestoreManager {
    /// 标记已关闭；完整 SST 管线尚未在此文件展开。
    pub fn Close(&mut self, _ctx: &Context) {
        if let Some(restorer) = &self.restorer {
            if restorer.Close().is_err() {
                log::Warn("failed to close SST restorer");
            }
        }
        self.closed = true;
    }
}

#[derive(Default)]
/// 还原过程累计统计（原子字段，供并发更新）。
pub struct restoreStatistics {
    pub restoreSSTKVSize: AtomicU64,
    pub restoreSSTKVCount: AtomicU64,
    pub restoreSSTPhySize: AtomicU64,
    pub restoreSSTTakes: AtomicU64,
}

/// 日志还原客户端：PiTR 任务的中心状态与能力入口。
pub struct LogClient {
    // 元数据/数据文件迭代与下载管理。
    pub LogFileManager: Option<LogFileManager>,
    // 日志 KV 导入管理器。
    pub logRestoreManager: Option<LogRestoreManager>,
    // 压缩 SST 导入管理器（占位）。
    pub sstRestoreManager: Option<SstRestoreManager>,
    // 备份加密信息，Apply 时下发 TiKV。
    pub cipher: Option<CipherInfo>,
    // PD 客户端：cluster id / store 列表等。
    pub pdClient: Arc<dyn pd::Client>,
    // PD HTTP 辅助客户端。
    pub pdHTTPClient: pdhttp::Client,
    // 缓存的集群 ID；0 表示尚未解析。
    pub clusterID: u64,
    // TiDB domain，用于 infoschema / 系统表探测。
    pub dom: Option<Domain>,
    // 导入限速（字节/秒语义与 Go 一致）。
    pub rateLimit: u64,
    // 扫描 region 的并发度。
    pub regionScanConcurrency: u32,
    // MetaKV 写入用的 RawKV 批客户端。
    pub rawKVClient: Option<RawKVBatchClient>,
    // 备份/检查点外部存储。
    pub storage: Option<Arc<dyn Storage>>,
    // 内部 SQL session（系统表读写）。
    pub unsafeSession: Option<Box<dyn Session>>,
    // 当前可读 TS；SetCurrentTS 禁止为 0。
    pub currentTS: u64,
    // 还原目标 TS。
    pub restoreTS: u64,
    // 上游集群 ID，写入 id map 主键。
    pub upstreamClusterID: u64,
    // 本次还原任务 ID，隔离并发 PiTR。
    pub restoreID: u64,
    // 操作上下文：锁与 migration 元数据载体。
    pub operationContext: operation::Context,
    // 是否严格校验备份元数据兼容性。
    pub checkRequirements: bool,
    // 待插入的 GC delete range SQL 缓存。
    pub deleteRangeQuery: Vec<PreDelRangeQuery>,
    // 是否启用检查点存储与进度。
    pub useCheckpoint: bool,
    // 日志文件统计。
    pub logFilesStat: LogFilesStatistic,
    // SST/KV 还原统计。
    pub restoreStat: restoreStatistics,
    // GC 行加载是否已启动（stub 下为布尔标记）。
    pub gcLoaderStarted: bool,
}

/// 最小构造：仅绑定 PD，其余字段置默认，后续 Set*/Init* 注入。
pub fn NewLogClient(pdClient: Arc<dyn pd::Client>, pdHTTPCli: pdhttp::Client) -> LogClient {
    LogClient {
        LogFileManager: None,
        logRestoreManager: None,
        sstRestoreManager: None,
        cipher: None,
        pdClient,
        pdHTTPClient: pdHTTPCli,
        clusterID: 0,
        dom: None,
        rateLimit: 0,
        regionScanConcurrency: 0,
        rawKVClient: None,
        storage: None,
        unsafeSession: None,
        currentTS: 0,
        restoreTS: 0,
        upstreamClusterID: 0,
        restoreID: 0,
        operationContext: operation::Context::default(),
        checkRequirements: true,
        deleteRangeQuery: Vec::new(),
        useCheckpoint: false,
        logFilesStat: LogFilesStatistic::default(),
        restoreStat: restoreStatistics::default(),
        gcLoaderStarted: false,
    }
}

impl LogClient {
    /// 设置 restore_id，并同步到 operation hint。
    pub fn SetRestoreID(&mut self, restoreID: u64) {
        self.restoreID = restoreID;
        self.setOperationContextRestoreID(restoreID);
    }

    /// 替换 operation context，并回写当前 restore_id hint。
    pub fn SetOperationContext(&mut self, operationContext: operation::Context) {
        self.operationContext = operationContext;
        self.setOperationContextRestoreID(self.restoreID);
    }

    /// restore_id=0 时清空 hint，避免残留旧任务标识。
    pub fn setOperationContextRestoreID(&mut self, restoreID: u64) {
        // 空字符串表示无 restore_id hint。
        if restoreID == 0 {
            self.operationContext
                .SetHintField(operationHintRestoreID, "");
            return;
        }
        self.operationContext
            .SetHintField(operationHintRestoreID, &restoreID.to_string());
    }

    /// 控制 id map / backupmeta 兼容性检查是否严格失败。
    pub fn SetCheckRequirements(&mut self, checkRequirements: bool) {
        self.checkRequirements = checkRequirements;
    }

    /// 配置 region 扫描并发。
    pub fn SetRegionScanConcurrency(&mut self, c: u32) {
        self.regionScanConcurrency = c;
    }

    /// 按依赖顺序关闭各子系统，最后打关闭日志。
    pub fn Close(&mut self, ctx: &Context) {
        if let Some(session) = &mut self.unsafeSession {
            session.Close();
        }
        if let Some(manager) = &self.LogFileManager {
            manager.Close();
        }
        if let Some(client) = &mut self.rawKVClient {
            client.Close();
        }
        if let Some(manager) = &mut self.logRestoreManager {
            manager.Close(ctx);
        }
        if let Some(manager) = &mut self.sstRestoreManager {
            manager.Close(ctx);
        }
        // 全部子资源关闭后再记日志，便于排查半关闭状态。
        log::Info("Log client closed");
    }

    /// 若 SST 被改写到其它表，把规则源表 ID 重写到原 TableID；失败则报错。
    pub fn rewriteRulesFor(&self, sst: &dyn SSTs, rules: &RewriteRules) -> Result<RewriteRules> {
        if let Some(r) = sst.as_rewritten() {
            let rewritten = r.RewrittenTo();
            // 仅当改写目标与当前 TableID 不同时才需要 RewriteSourceTableID。
            if rewritten != sst.TableID() {
                let mut rewriteRules = rules.Clone();
                if !rewriteRules.RewriteSourceTableID(rewritten, sst.TableID()) {
                    return Err(Error::Annotatef(
                        berrors::ErrUnknown("rewrite failed"),
                        format!(
                            "table rewritten from a table id ({rewritten}) to ({}) which doesn't exist in the stream",
                            sst.TableID()
                        ),
                    ));
                }
                return Ok(rewriteRules);
            }
        }
        // 无需改写时直接克隆原规则返回。
        Ok(rules.Clone())
    }

    /// 注入 RawKV 批客户端。
    pub fn SetRawKVBatchClient(&mut self, client: RawKVBatchClient) {
        self.rawKVClient = Some(client);
    }

    /// 设置导入限速。
    pub fn SetRateLimit(&mut self, rateLimit: u64) {
        self.rateLimit = rateLimit;
    }

    /// 设置备份解密密钥信息。
    pub fn SetCrypter(&mut self, crypter: CipherInfo) {
        self.cipher = Some(crypter);
    }

    /// 设置上游集群 ID（id map 主键组成部分）。
    pub fn SetUpstreamClusterID(&mut self, upstreamClusterID: u64) {
        self.upstreamClusterID = upstreamClusterID;
    }

    /// 绑定外部存储；当前总是 Ok，签名保留错误通道。
    pub fn SetStorage(&mut self, storage: Arc<dyn Storage>) -> Result<()> {
        self.storage = Some(storage);
        Ok(())
    }

    /// 设置 currentTS；0 非法，与 Go 校验一致。
    pub fn SetCurrentTS(&mut self, ts: u64) -> Result<()> {
        if ts == 0 {
            return Err(Error::new("current ts is 0"));
        }
        self.currentTS = ts;
        Ok(())
    }

    /// 设置还原目标 TS。
    pub fn SetRestoreTS(&mut self, ts: u64) {
        self.restoreTS = ts;
    }

    /// 读取 currentTS。
    pub fn CurrentTS(&self) -> u64 {
        self.currentTS
    }

    /// 优先返回缓存 clusterID，否则问 PD。
    pub fn GetClusterID(&self, ctx: &Context) -> u64 {
        // 缓存命中避免重复 RPC。
        if self.clusterID != 0 {
            return self.clusterID;
        }
        self.pdClient.GetClusterID(ctx)
    }

    /// 借用 domain。
    pub fn GetDomain(&self) -> Option<&Domain> {
        self.dom.as_ref()
    }

    /// 借用内部 session。
    pub fn UnsafeSession(&self) -> Option<&dyn Session> {
        self.unsafeSession.as_deref()
    }

    /// 委托 fileImporter 按前缀清理各 store 临时文件。
    pub fn CleanUpKVFiles(&self, ctx: &Context, prefix: &str) -> Result<()> {
        if let Some(mgr) = &self.logRestoreManager {
            return mgr
                .fileImporter
                .ClearFiles(ctx, self.pdClient.as_ref(), prefix);
        }
        Ok(())
    }

    /// 创建内部 session 并缓存 clusterID，是任务启动的第一步。
    pub fn Init(
        &mut self,
        ctx: &Context,
        g: &dyn glue::Glue,
        store: &dyn kv::Storage,
    ) -> Result<()> {
        self.unsafeSession = Some(g.CreateSession(store)?);
        self.clusterID = self.pdClient.GetClusterID(ctx);
        Ok(())
    }

    /// 装配日志/SST 还原管理器与 importer。
    pub fn InitClients(
        &mut self,
        _ctx: &Context,
        backend: Option<StorageBackend>,
        splitClient: Arc<dyn SplitClient>,
        importClient: Arc<dyn ImporterClient>,
        poolSize: u32,
    ) -> Result<()> {
        let stores: Vec<_> = self
            .pdClient
            .GetAllStores(_ctx)?
            .into_iter()
            .filter(|store| {
                !store.Labels.iter().any(|label| {
                    label.Key == "engine"
                        && (label.Value == "tiflash" || label.Value == "tiflash_compute")
                })
            })
            .collect();
        // poolSize 决定日志还原工作池并行度。
        let importer = NewLogFileImporter(splitClient, importClient, backend);
        self.logRestoreManager = Some(NewLogRestoreManager(_ctx, importer, poolSize, None)?);
        self.sstRestoreManager = Some(SstRestoreManager {
            closed: false,
            storeCount: liveTiKVStoreCount(&stores),
            replicaCount: self.getMaxReplica(_ctx),
            workerPoolSize: 7186 * stores.len() as u32,
            restorer: None,
        });
        Ok(())
    }

    /// 基于存储与时间窗口创建 LogFileManager，并记录 restoreTS。
    pub fn InstallLogFileManager(
        &mut self,
        ctx: &Context,
        startTS: u64,
        restoreTS: u64,
        metadataDownloadBatchSize: u32,
    ) -> Result<()> {
        let storage = self
            .storage
            .clone()
            .ok_or_else(|| Error::new("storage unset"))?;
        // 空迁移列表起步；后续可由外部追加 LockedMigrations。
        let builder = WithMigrationsBuilder::new(startTS, restoreTS);
        let migrations = builder.Build(&[]);
        let mgr = CreateLogFileManager(
            ctx,
            LogFileManagerInit {
                StartTS: startTS,
                RestoreTS: restoreTS,
                Storage: storage,
                MigrationsBuilder: builder,
                Migrations: migrations,
                MetadataDownloadBatchSize: metadataDownloadBatchSize,
                EncryptionManager: None,
            },
        )?;
        self.restoreTS = restoreTS;
        self.LogFileManager = Some(mgr);
        Ok(())
    }

    /// 缓存一条待执行的 delete-range SQL。
    pub fn RecordDeleteRange(&mut self, sql: PreDelRangeQuery) {
        self.deleteRangeQuery.push(sql);
    }

    /// 标记 GC 加载已启动；真实异步加载在 Rust 侧未展开。
    pub fn RunGCRowsLoader(&mut self, _ctx: &Context) {
        self.gcLoaderStarted = true;
    }

    /// 边界：不执行真实 SQL，仅清空已收集查询。
    pub fn InsertGCRows(&mut self, _ctx: &Context) -> Result<()> {
        // Boundary: real SQL insert mocked; drain collected queries.
        self.deleteRangeQuery.clear();
        Ok(())
    }

    /// 只读访问 delete-range 缓存。
    pub fn GetGCRows(&self) -> &[PreDelRangeQuery] {
        &self.deleteRangeQuery
    }

    /// failpoint 包装入口，内部直接转 saveIDMap。
    pub fn SaveIdMapWithFailPoints(
        &self,
        ctx: &Context,
        manager: &TableMappingManager,
        logCheckpointMetaManager: &LogMetaManagerT,
    ) -> Result<()> {
        self.saveIDMap(ctx, manager, logCheckpointMetaManager)
    }

    /// 边界桩：完整 schema 版本重载尚未移植。
    pub fn UpdateSchemaVersionFullReload(&self, _ctx: &Context) -> Result<()> {
        log::Info("UpdateSchemaVersionFullReload");
        Ok(())
    }

    /// 边界桩：按表刷新 meta 尚未移植。
    pub fn RefreshMetaForTables(
        &self,
        _ctx: &Context,
        _schemasReplace: &SchemasReplace,
    ) -> Result<()> {
        log::Info("RefreshMetaForTables");
        Ok(())
    }

    /// 过滤/排序条目后更新统计；真实 rawkv put 仍为桩。
    pub fn RestoreBatchMetaKVFiles(
        &mut self,
        ctx: &Context,
        files: &[DataFileInfo],
        _schemasReplace: &SchemasReplace,
        entries: Vec<KvEntryWithTS>,
        filterTS: u64,
        updateStats: &mut dyn FnMut(u64, u64),
        progressInc: &mut dyn FnMut(),
        _cf: &str,
    ) -> Result<Vec<KvEntryWithTS>> {
        let (cur, filtered) =
            self.filterAndSortKvEntriesFromFiles(ctx, files, entries, filterTS)?;
        if cur.is_empty() {
            return Ok(filtered);
        }
        let mut size = 0u64;
        for e in &cur {
            size += (e.E.Key.len() + e.E.Value.len()) as u64;
        }
        updateStats(cur.len() as u64, size);
        for _ in files {
            progressInc();
        }
        // Boundary: rawkv put of rewritten meta entries is stubbed.
        Ok(filtered)
    }

    /// 无真实文件字节时，仅按 filterTS 重滤结转条目并按 TS/Key 排序。
    pub fn filterAndSortKvEntriesFromFiles(
        &self,
        _ctx: &Context,
        files: &[DataFileInfo],
        mut entries: Vec<KvEntryWithTS>,
        filterTS: u64,
    ) -> Result<(Vec<KvEntryWithTS>, Vec<KvEntryWithTS>)> {
        // Without real file bytes, only re-filter carry-forward entries by TS.
        let _ = files;
        let mut cur = Vec::new();
        let mut filtered = Vec::new();
        for e in entries.drain(..) {
            // 小于 filterTS 的进入当前批，其余结转下批。
            if e.Ts < filterTS {
                cur.push(e);
            } else {
                filtered.push(e);
            }
        }
        // 稳定序：先 TS 再 Key，对齐 Go 处理顺序。
        cur.sort_by(|a, b| a.Ts.cmp(&b.Ts).then_with(|| a.E.Key.cmp(&b.E.Key)));
        Ok((cur, filtered))
    }

    /// 加载基础 id map 并合并进 TableMappingManager。
    pub fn GetBaseIDMapAndMerge(
        &self,
        ctx: &Context,
        restoredTS: u64,
        logCheckpointMetaManager: &LogMetaManagerT,
        manager: &mut TableMappingManager,
    ) -> Result<()> {
        // TiFlash 副本校验在 Rust 侧恒成功（桩）。
        self.validateNoTiFlashReplica()?;
        let dbMaps = self.loadSchemasMap(ctx, restoredTS, logCheckpointMetaManager)?;
        manager.MergeBaseDBReplace(&dbMaps);
        Ok(())
    }

    // 边界桩：Go 会拒绝存在 TiFlash 副本的表。
    fn validateNoTiFlashReplica(&self) -> Result<()> {
        Ok(())
    }

    /// 遍历 DML 日志文件，按 LogSplitStrategy 累计并在阈值处复位，驱动预切分。
    pub fn PreSplitRegions(
        &mut self,
        ctx: &Context,
        rules: HashMap<i64, RewriteRules>,
        logCheckpointMetaManager: Option<&LogMetaManagerT>,
    ) -> Result<()> {
        let mgr = self
            .LogFileManager
            .as_ref()
            .ok_or_else(|| Error::new("log file manager not installed"))?;
        // 迭代器按时间窗口产出待还原日志文件。
        let iter = mgr.LoadDMLFiles(ctx)?;
        let mut strategy = NewLogSplitStrategy(
            ctx,
            self.useCheckpoint,
            logCheckpointMetaManager,
            rules,
            Box::new(|_, _| {}),
            SplitFileThresholdDefault,
        )?;
        let mut iter = iter;
        loop {
            let r = iter.TryNext(&astersql_br_pkg_utils_iter::Context::background());
            if r.FinishedOrError() {
                if let Some(err) = r.Err {
                    return Err(Error::new(err));
                }
                break;
            }
            let file = r.Item.unwrap();
            // 检查点已完成的文件直接跳过。
            if strategy.ShouldSkip(&file) {
                continue;
            }
            // 未跳过则计入 splitter。
            strategy.Accumulate(&file);
            // 达到阈值后 Reset，对应 Go 侧提交 split 的时机。
            if strategy.ShouldSplit() {
                strategy.base.ResetAccumulations();
            }
        }
        Ok(())
    }
}

/// 按 MinTs/MaxTs 排序 MetaKV 文件。
pub fn SortMetaKVFiles(files: &[DataFileInfo]) -> Vec<DataFileInfo> {
    sort_meta_kv_files(files.to_vec())
}

/// 过滤可读 MetaKV，按 default/write CF 分离后再各自排序。
pub fn SeparateAndSortFilesByCF(files: &[DataFileInfo]) -> (Vec<DataFileInfo>, Vec<DataFileInfo>) {
    let mut defaultCF = Vec::new();
    let mut writeCF = Vec::new();
    for f in files {
        // 非 meta 或不可读文件直接丢弃。
        if !shouldReadMetaKVFile(f) {
            continue;
        }
        // 空 CF 视为 default，与 Go 兼容。
        if f.Cf == consts::DefaultCF || f.Cf.is_empty() {
            defaultCF.push(f.clone());
        } else if f.Cf == consts::WriteCF {
            writeCF.push(f.clone());
        }
    }
    (sort_meta_kv_files(defaultCF), sort_meta_kv_files(writeCF))
}

// 内部排序：MinTs 主序，MaxTs 次序。
fn sort_meta_kv_files(mut files: Vec<DataFileInfo>) -> Vec<DataFileInfo> {
    files.sort_by(|a, b| {
        a.MinTs
            .cmp(&b.MinTs)
            .then(a.MaxTs.cmp(&b.MaxTs))
            .then(a.ResolvedTs.cmp(&b.ResolvedTs))
    });
    files
}

/// 按 MinTs 归并 default/write 两个有序流，聚合成不超过 MetaKVBatchSize 的批。
/// 每批先处理 default 再 write；残留 entries 在循环结束后以 filterTS=MAX 冲刷。
pub fn LoadAndProcessMetaKVFilesInBatch(
    ctx: &Context,
    filesInDefaultCF: &[DataFileInfo],
    filesInWriteCF: &[DataFileInfo],
    processor: &mut dyn BatchMetaKVProcessor,
) -> Result<()> {
    const KV_SIZE: usize = 2560;

    let mut range_max = 0u64;
    let mut batch_size = 0u64;
    let mut default_idx = 0usize;
    let mut write_idx = 0usize;
    let mut defaultEntries: Vec<KvEntryWithTS> = Vec::new();
    let mut writeEntries: Vec<KvEntryWithTS> = Vec::new();

    for (i, file) in filesInDefaultCF.iter().enumerate() {
        if i == 0 {
            range_max = file.MaxTs;
            batch_size = file.Length;
        } else if file.MinTs <= range_max && batch_size + file.Length <= MetaKVBatchSize {
            range_max = range_max.max(file.MaxTs);
            batch_size += file.Length;
        } else {
            defaultEntries = processor.ProcessBatch(
                ctx,
                &filesInDefaultCF[default_idx..i],
                defaultEntries,
                file.MinTs,
                consts::DefaultCF,
            )?;
            default_idx = i;
            range_max = file.MaxTs;
            batch_size = (defaultEntries.len() * KV_SIZE) as u64 + file.Length;

            let mut to_write_idx = write_idx;
            while to_write_idx < filesInWriteCF.len()
                && filesInWriteCF[to_write_idx].MinTs < file.MinTs
            {
                to_write_idx += 1;
            }
            writeEntries = processor.ProcessBatch(
                ctx,
                &filesInWriteCF[write_idx..to_write_idx],
                writeEntries,
                file.MinTs,
                consts::WriteCF,
            )?;
            write_idx = to_write_idx;
        }
    }

    processor.ProcessBatch(
        ctx,
        &filesInDefaultCF[default_idx..],
        defaultEntries,
        u64::MAX,
        consts::DefaultCF,
    )?;
    processor.ProcessBatch(
        ctx,
        &filesInWriteCF[write_idx..],
        writeEntries,
        u64::MAX,
        consts::WriteCF,
    )?;
    Ok(())
}

/// 按个数或累计 Length 切批，回调 applyFunc；尾批不为空时再刷一次。
pub fn ApplyKVFilesWithBatchMethod(
    ctx: &Context,
    mut files: LogIter,
    batchCount: usize,
    batchSize: u64,
    applyFunc: &mut dyn FnMut(&Context, Vec<LogDataFileInfo>) -> Result<()>,
) -> Result<()> {
    struct PutBatch {
        table_id: i64,
        cf: String,
        files: Vec<LogDataFileInfo>,
        size: u64,
    }

    let mut put_batches: Vec<PutBatch> = Vec::new();
    let mut delete_files = Vec::new();
    loop {
        let r = files.TryNext(&astersql_br_pkg_utils_iter::Context::background());
        if r.FinishedOrError() {
            if let Some(err) = r.Err {
                return Err(Error::new(err));
            }
            break;
        }
        let f = r.Item.unwrap();
        if f.Type == backuppb::FileType::Delete {
            delete_files.push(f);
            continue;
        }

        // Go bypasses aggregation for a single large put file.
        if f.Length >= batchSize {
            applyFunc(ctx, vec![f])?;
            continue;
        }

        // The Rust file model does not yet expose RegionId, so retain all
        // representable Go grouping dimensions: table and column family.
        let batch = match put_batches
            .iter_mut()
            .find(|batch| batch.table_id == f.TableId && batch.cf == f.Cf)
        {
            Some(batch) => batch,
            None => {
                put_batches.push(PutBatch {
                    table_id: f.TableId,
                    cf: f.Cf.clone(),
                    files: Vec::with_capacity(batchCount),
                    size: 0,
                });
                put_batches.last_mut().unwrap()
            }
        };
        batch.size += f.Length;
        batch.files.push(f);
        if batch.files.len() >= batchCount || batch.size >= batchSize {
            applyFunc(ctx, std::mem::take(&mut batch.files))?;
            batch.size = 0;
        }
    }

    for batch in &mut put_batches {
        if !batch.files.is_empty() {
            applyFunc(ctx, std::mem::take(&mut batch.files))?;
        }
    }

    // Go waits for all put work before submitting deletes. The callback is
    // synchronous here, so reaching this loop is the equivalent barrier.
    let mut delete_batch = Vec::with_capacity(batchCount);
    let mut delete_size = 0u64;
    for file in delete_files {
        delete_size += file.Length;
        delete_batch.push(file);
        if delete_batch.len() >= batchCount || delete_size >= batchSize {
            applyFunc(ctx, std::mem::take(&mut delete_batch))?;
            delete_size = 0;
        }
    }
    if !delete_batch.is_empty() {
        applyFunc(ctx, delete_batch)?;
    }
    Ok(())
}

/// 逐文件回调，不做批聚合。
pub fn ApplyKVFilesWithSingleMethod(
    ctx: &Context,
    mut files: LogIter,
    applyFunc: &mut dyn FnMut(&Context, LogDataFileInfo) -> Result<()>,
) -> Result<()> {
    let mut delete_files = Vec::new();
    loop {
        let r = files.TryNext(&astersql_br_pkg_utils_iter::Context::background());
        if r.FinishedOrError() {
            if let Some(err) = r.Err {
                return Err(Error::new(err));
            }
            break;
        }
        let file = r.Item.unwrap();
        if file.Type == backuppb::FileType::Delete {
            delete_files.push(file);
        } else {
            applyFunc(ctx, file)?;
        }
    }
    // As in Go, delete files are not submitted until every put callback has
    // returned (the synchronous callback provides the wait barrier here).
    for file in delete_files {
        applyFunc(ctx, file)?;
    }
    Ok(())
}

/// RawKV Put 的重试封装入口；当前直接委托客户端 Put。
pub fn PutRawKvWithRetry(
    ctx: &Context,
    client: &RawKVBatchClient,
    key: &[u8],
    value: &[u8],
    originTs: u64,
) -> Result<()> {
    // 重试策略留给 RawKVBatchClient 内部或未来扩充。
    client.Put(ctx, key, value, originTs)
}

/// Test helper matching export_test TEST_NewLogClient.
/// 测试辅助：构造带指定 cluster/restoreTS 的内存 PD 客户端。
pub fn TEST_NewLogClient(clusterID: u64, restoreTS: u64) -> LogClient {
    let mut rc = NewLogClient(
        Arc::new(pd::MemPdClient {
            cluster_id: clusterID,
            stores: vec![],
        }),
        pdhttp::Client::default(),
    );
    rc.clusterID = clusterID;
    rc.restoreTS = restoreTS;
    rc
}

/// 测试辅助：在 TEST_NewLogClient 上再绑定 MemStorage。
pub fn TEST_NewLogClientWithStorage(
    clusterID: u64,
    restoreTS: u64,
    storage: Arc<dyn Storage>,
) -> LogClient {
    let mut rc = TEST_NewLogClient(clusterID, restoreTS);
    rc.storage = Some(storage);
    rc
}

pub fn liveTiKVStoreCount(stores: &[crate::stubs::metapb::Store]) -> u32 {
    stores
        .iter()
        .filter(|store| store.State == crate::stubs::metapb::StoreState::Up)
        .count() as u32
}
pub fn maxReplicaFromReplicateConfig(
    response: Option<&HashMap<String, serde_json::Value>>,
    error: bool,
) -> u32 {
    if error {
        return 3;
    }
    // serde's JSON numbers stand in for Go map[string]any's float64 values.
    response
        .and_then(|r| r.get("max-replicas"))
        .and_then(|v| v.as_f64())
        .filter(|v| *v > 0.0)
        .map(|v| v as u32)
        .unwrap_or(3)
}
impl LogClient {
    fn getMaxReplica(&self, ctx: &Context) -> u32 {
        let Some(http) = self.pdHTTPClient.backend.as_ref() else {
            return 3;
        };
        let mut strategy = astersql_br_pkg_utils::backoff::NewAggressivePDBackoffStrategy();
        while strategy.RemainingAttempts() > 0 {
            if ctx.Err().is_some() {
                return 3;
            }
            match http.GetReplicateConfig(ctx) {
                Ok(response) => return maxReplicaFromReplicateConfig(Some(&response), false),
                Err(error) => {
                    let delay = strategy.NextBackoff(&error);
                    let end = std::time::Instant::now() + delay;
                    while std::time::Instant::now() < end {
                        if ctx.Err().is_some() {
                            return 3;
                        }
                        std::thread::sleep(
                            end.saturating_duration_since(std::time::Instant::now())
                                .min(std::time::Duration::from_millis(10)),
                        );
                    }
                }
            }
        }
        3
    }
    pub fn LoadOrCreateCheckpointMetadataForLogRestore(
        &mut self,
        ctx: &astersql_br_pkg_checkpoint::Context,
        restoreStartTS: u64,
        startTS: u64,
        restoredTS: u64,
        gcRatio: String,
        mut jobs: String,
        items: HashMap<i64, astersql_br_pkg_checkpoint::TiFlashReplicaInfo>,
        manager: &dyn astersql_br_pkg_checkpoint::LogMetaManager,
        snapshotBytes: u64,
    ) -> Result<(String, String, u64)> {
        self.useCheckpoint = true;
        if manager
            .ExistsCheckpointMetadata(ctx)
            .map_err(|e| Error::new(e.to_string()))?
        {
            let metadata = manager
                .LoadCheckpointMetadata(ctx)
                .map_err(|e| Error::new(e.to_string()))?;
            if !metadata.RocksDBMaxBackgroundJobs.is_empty() {
                jobs = metadata.RocksDBMaxBackgroundJobs;
            }
            return Ok((metadata.GcRatio, jobs, metadata.SnapshotRestoreDataSize));
        }
        manager
            .SaveCheckpointMetadata(
                ctx,
                &astersql_br_pkg_checkpoint::CheckpointMetadataForLogRestore {
                    UpstreamClusterID: self.upstreamClusterID,
                    RestoreStartTS: restoreStartTS,
                    StartTS: startTS,
                    RestoredTS: restoredTS,
                    RewriteTS: self.currentTS,
                    GcRatio: gcRatio.clone(),
                    RocksDBMaxBackgroundJobs: jobs.clone(),
                    SnapshotRestoreDataSize: snapshotBytes,
                    TiFlashItems: items,
                },
            )
            .map_err(|e| Error::new(e.to_string()))?;
        Ok((gcRatio, jobs, snapshotBytes))
    }
}

impl LogClient {
    /// Reuse the existing simple SST engine and its real worker pool.
    pub fn InitSSTFileRestorer(
        &mut self,
        ctx: &astersql_br_pkg_restore::stubs::Context,
        importer: Arc<dyn astersql_br_pkg_restore::FileImporter>,
        checkpoint: Option<Arc<dyn astersql_br_pkg_restore::stubs::RestoreCheckpoint>>,
    ) -> Result<()> {
        let manager = self
            .sstRestoreManager
            .as_mut()
            .ok_or_else(|| Error::new("SST restore manager is not initialized"))?;
        let parent = ctx.clone();
        let lookup_context = Context::WithCancellationSource(move || {
            parent.Err().map(|error| Error {
                msg: error.msg,
                code: error.code,
            })
        });
        let stores: Vec<_> = self
            .pdClient
            .GetAllStores(&lookup_context)?
            .into_iter()
            .filter(|s| {
                s.State == crate::stubs::metapb::StoreState::Up
                    && !s.Labels.iter().any(|label| {
                        label.Key == "engine"
                            && (label.Value == "tiflash" || label.Value == "tiflash_compute")
                    })
            })
            .map(|s| s.Id)
            .collect();
        importer
            .ConfigureDownloadRetry(ctx, &stores)
            .map_err(|error| Error {
                msg: error.msg,
                code: error.code,
            })?;
        manager.restorer = Some(Arc::new(astersql_br_pkg_restore::NewSimpleSstRestorer(
            ctx,
            importer,
            astersql_br_pkg_restore::stubs::NewWorkerPool(
                manager.workerPoolSize as u64,
                "sst file",
            ),
            checkpoint,
        )));
        Ok(())
    }
    /// The restore and log crates currently expose distinct context types; the caller
    /// supplies both contexts for the same operation so neither transport loses cancellation.
    pub fn RestoreSSTFileSets(
        &self,
        ctx: &Context,
        restoreCtx: &astersql_br_pkg_restore::stubs::Context,
        sets: astersql_br_pkg_restore::BatchBackupFileSet,
        mode: &mut astersql_br_pkg_restore::import_mode_switcher::ImportModeSwitcher,
        online: bool,
        snapshotBytes: u64,
        checkpointBytes: u64,
        progress: Arc<dyn Fn(i64) + Send + Sync>,
    ) -> Result<()> {
        let begin = std::time::Instant::now();
        if sets.is_empty() {
            return Ok(());
        }
        if let Some(error) = ctx.Err() {
            return Err(error);
        }
        self.adjustTiKVFlowControlForCompactedSSTRestore(
            ctx,
            &sets,
            snapshotBytes,
            checkpointBytes,
        )?;
        // Preserves 02f5e23fb6b3e0f2bc544953a8b433a830abed5d's online replacement.
        if !online {
            mode.GoSwitchToImportMode(restoreCtx)
                .map_err(|e| Error::new(e.to_string()))?;
        }
        let restorer = self
            .sstRestoreManager
            .as_ref()
            .and_then(|m| m.restorer.as_ref())
            .ok_or_else(|| Error::new("SST restorer is not initialized"))?;
        restorer
            .GoRestore(progress, vec![sets.clone()])
            .map_err(|e| Error::new(e.to_string()))?;
        let result = restorer
            .WaitUntilFinish()
            .map_err(|e| Error::new(e.to_string()));
        for file in sets.iter().flat_map(|set| &set.SSTFiles) {
            self.restoreStat
                .restoreSSTKVCount
                .fetch_add(file.TotalKvs, AtomicOrdering::Relaxed);
            self.restoreStat
                .restoreSSTKVSize
                .fetch_add(file.TotalBytes, AtomicOrdering::Relaxed);
            self.restoreStat
                .restoreSSTPhySize
                .fetch_add(file.Size_, AtomicOrdering::Relaxed);
        }
        self.restoreStat
            .restoreSSTTakes
            .fetch_add(begin.elapsed().as_nanos() as u64, AtomicOrdering::Relaxed);
        result
    }
}

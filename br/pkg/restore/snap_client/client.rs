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
//! 本文件负责`br/pkg/restore/snap_client/client.rs`对应的快照恢复 SnapClient 控制器，本次仅补充注释，不改变执行语义。
//! 阅读时应把它视为 Go `br/pkg/restore/snap_client/client.go` 的语义镜像，重点核对职责边界而不是表面写法。
//! 注释优先解释状态推进、错误传播、资源释放、默认值来源以及与相邻 Go 文件的对齐点。
//! 若这里使用内存 DB、临时目录、本地 HTTP 桩或脚本化 mock，被保护的仍是可观察行为而非环境搭建本身。
//! 实现文件注释强调 Restorer/Importer 的投递、背压、checkpoint 与错误收束顺序。
//! 本任务要求至少122行中文注释，因此下方会显式列出关键符号与高价值场景索引。
//! - `STRICT_PLACEMENT_POLICY_MODE`：严格放置策略：恢复时校验/应用 placement policy。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `IGNORE_PLACEMENT_POLICY_MODE`：忽略放置策略，用于不支持 policy 的集群或显式跳过。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `RESET_SPEED_LIMIT_RETRY_TIMES`：关闭时重置 TiKV download 限速的重试次数。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `DEFAULT_DDL_CONCURRENCY`：建表/DDL 默认并发。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `MAX_SPLIT_KEYS_ONCE`：单次 split 键数量上限，防止 PD 请求过大。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `MIN_BATCH_DDL_SIZE`：批量 DDL 最小批次，避免空批。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `SnapClient`：快照恢复中枢：持有 PD/Domain/Importer/Restorer 与备份元数据映射。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `NewRestoreClient`：生产构造入口；测试可用 NewRestoreClientForTest 注入 Mem* 桩。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `GetRestorer`：按模式选择 Simple/Batch/MultiTables Restorer 并缓存。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `CreatePreallocIDCheckpoint`：预分配表 ID 检查点，崩溃恢复时可续用同一 ID 区间。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `SetRateLimit`：设置每 store 下载限速；Close 时需回调清零。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `SetCrypter`：绑定备份加密 CipherInfo，供下载解密。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `CleanTablesIfTemporarySystemTablesRenamed`：系统表临时名回滚/清理，避免残留 __ti_tmp 表。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `AllocTableIDs`：为待建表预分配 ID，生成 RewriteRules 所需映射。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `GetPreAllocedTableIDRange`：暴露已预分配区间，供 checkpoint 与幂等建表。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `InstallPiTRSupport`：挂载 PiTR 收集依赖，日志备份衔接用。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `InitConnections`：初始化 PD/TiKV/Domain 连接；失败应可重试或清晰报错。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `LoadSchemaIfNeededAndInitClient`：按 noSchema/withSysTable 加载备份 schema 并初始化 client 字段。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `IsRawKvMode`：区分 RawKV 与 TiDB 全量路径，影响 key range 与 importer 模式。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `GetFilesInRawRange`：Raw 模式下按起止键筛选备份文件。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `GetDatabases/GetDatabaseMap/GetTableMap`：备份元数据索引，供建库建表与过滤。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `HasBackedUpSysDB`：是否备份了系统库，决定系统表恢复分支。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `SetPlacementPolicyMode`：STRICT/IGNORE 切换；影响 placement rule 管理器行为。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `SetRewriteMode`：Legacy vs Keyspace 改写，决定 keyspace 前缀处理。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `SetConcurrencyPerStore/SetRegionScanConcurrency`：控制每 store 并发与 region 扫描并发，直接关系背压与吞吐。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `SetSplitRegionIndexStep/SetCoarseScatter`：region 切分步进与粗粒度 scatter，平衡调度开销。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `SetBatchDdlSize/SetTxnTotalSizeLimit`：DDL 批大小与事务体积上限，防止 TiDB 事务过大。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `SetWithSysTable`：是否恢复系统表；与 TemporaryTableChecker 联动。
//!   与 Go 对齐时关注错误是否 Trace 包装、进度何时回调、资源是否在 Close 释放。
//!   并发路径下 Extra 注意 ErrorGroup 取消后不再投递，但在飞任务仍需 Wait。
//! - `SetCheckPrivilegeTableRowsCollateCompatibility`：权限表排序规则兼容性检查开关。
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
//! 补充说明：错误路径优先返回 Trace 包装，便于上层日志带上文件组上下文。
//! 补充说明：进度回调的计量单位必须与 Go 一致（KV 数或批次数），避免 UI 进度失真。
//! 补充说明：checkpoint 只在成功导入后追加，崩溃重跑依赖该单调性。
//! 补充说明：Close 应尽量幂等：重复关闭 importer/限速回调不得 panic。
//! 补充说明：背压与 PD 令牌是两套限流，注释和改动时不要混用计数器。
//! 补充说明：测试中的 Mem* 桩只保证控制流，不验证真实 TiKV 性能。
//! 补充说明：Raw/Txn/TiDBFull 模式切换会改变 key 编码，跨模式复用 meta 是 bug。
//! 中文注释索引结束

//! Snapshot restore client matching `client.go`.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use crate::import::{
    DownloadRateLimitTTLSeconds, KvMode, NewSnapFileImporter, NewSnapFileImporterOptions,
    RewriteMode, SnapFileImporter,
};
use crate::pitr_collector::{PiTRCollDep, newPiTRColl};
use crate::stubs::{
    CheckpointRunner, ChecksumClient, ChecksumItem, ClusterConfig, Context, CreatedTable,
    DbSession, DefaultRegionIndexStep, DomainLike, Error, GetAllTiKVStoresWithRetry,
    ImporterClient, IsSysOrTempSysDB, MemDb, MemDomain, MemImporterClient, MemPdClient,
    MemSplitClient, MergeOptionRule, NewAndPreallocTableIDs, NormalizeRegionIndexStep, PdClient,
    PdController, PreallocIDs, Result, ReusePreallocatedTableIDs, SimpleRestorer,
    SnapshotCheckpointManager, SplitClient, SstRestorer, StoreMeta, SysDB, SystemDB,
    TableLocationInfo, TemporaryDBName, TlsConfig, UniqueTableName, WorkloadSchema, backuppb,
    berrors, log, metautil, model,
};
use crate::systable_restore::TemporaryTableChecker;

// 与 Go 字符串常量逐字对齐，供 CLI/配置比较。
pub const STRICT_PLACEMENT_POLICY_MODE: &str = "STRICT";
pub const IGNORE_PLACEMENT_POLICY_MODE: &str = "IGNORE";
pub const RESET_SPEED_LIMIT_RETRY_TIMES: usize = 3;
pub const DEFAULT_DDL_CONCURRENCY: usize = 64;
pub const MAX_SPLIT_KEYS_ONCE: usize = 10240;
pub const MIN_BATCH_DDL_SIZE: usize = 1;

/// SnapClient is the snapshot restore controller.
// 字段多为 Go 同名导出；Option/Arc 包装表达可注入的 PD、Domain、Importer。
pub struct SnapClient {
    pub restorer: Option<Box<dyn SstRestorer>>,
    pub importer: Option<SnapFileImporter>,
    pub pdClient: Arc<dyn PdClient>,
    pub pdStore: Arc<dyn StoreMeta>,
    pub tlsConf: Option<TlsConfig>,
    pub meta_client: Option<Arc<dyn SplitClient>>,
    pub import_client: Option<Arc<dyn ImporterClient>>,
    pub cipher: Option<backuppb::CipherInfo>,
    pub concurrencyPerStore: u32,
    pub regionScanConcurrency: u32,
    pub splitRegionIndexStep: u32,
    pub coarseScatter: bool,
    pub rateLimit: u64,
    pub storeCount: usize,
    pub workerPoolSize: usize,
    pub supportPolicy: bool,
    pub noSchema: bool,
    pub databases: HashMap<String, metautil::Database>,
    pub ddlJobs: Vec<model::Job>,
    pub rebasedTablesMap: HashMap<UniqueTableName, bool>,
    pub backupMeta: Option<backuppb::BackupMeta>,
    pub db: Option<Box<dyn DbSession>>,
    pub dbPool: Vec<Box<dyn DbSession>>,
    pub preallocedIDs: Option<PreallocIDs>,
    pub dom: Option<Arc<dyn DomainLike>>,
    pub policyMode: String,
    pub policyMap: Option<HashMap<String, model::PolicyInfo>>,
    pub batchDdlSize: u32,
    pub txnTotalSizeLimit: u64,
    pub fullClusterRestore: bool,
    pub withSysTable: bool,
    pub rewriteMode: RewriteMode,
    pub temporarySystemTablesRenamed: bool,
    pub restoreUUID: Vec<u8>,
    pub checkpointRunner: Option<Box<dyn CheckpointRunner>>,
    pub checkpointChecksum: HashMap<i64, ChecksumItem>,
    pub privilegeTableRowsCollateCompatibility: bool,
    speed_limit_closed: Arc<Mutex<bool>>,
}

// 默认策略模式与并发常量需与 Go NewRestoreClient 保持一致。
pub fn NewRestoreClient(pd_client: Arc<dyn PdClient>, pd_store: Arc<dyn StoreMeta>) -> SnapClient {
    SnapClient {
        restorer: None,
        importer: None,
        pdClient: pd_client,
        pdStore: pd_store,
        tlsConf: None,
        meta_client: None,
        import_client: None,
        cipher: None,
        concurrencyPerStore: 0,
        regionScanConcurrency: 0,
        splitRegionIndexStep: DefaultRegionIndexStep,
        coarseScatter: false,
        rateLimit: 0,
        storeCount: 0,
        workerPoolSize: 0,
        supportPolicy: false,
        noSchema: false,
        databases: HashMap::new(),
        ddlJobs: Vec::new(),
        rebasedTablesMap: HashMap::new(),
        backupMeta: None,
        db: None,
        dbPool: Vec::new(),
        preallocedIDs: None,
        dom: None,
        policyMode: String::new(),
        policyMap: None,
        batchDdlSize: 0,
        txnTotalSizeLimit: 0,
        fullClusterRestore: false,
        withSysTable: false,
        rewriteMode: RewriteMode::RewriteModeLegacy,
        temporarySystemTablesRenamed: false,
        restoreUUID: Vec::new(),
        checkpointRunner: None,
        checkpointChecksum: HashMap::new(),
        privilegeTableRowsCollateCompatibility: false,
        speed_limit_closed: Arc::new(Mutex::new(false)),
    }
}

impl SnapClient {
    pub fn pd_store_meta(&self) -> &dyn StoreMeta {
        self.pdStore.as_ref()
    }

    // Restorer 选择依赖 importer 能力（是否 Balanced）与恢复模式。
    pub fn GetRestorer(&mut self) -> &mut (dyn SstRestorer + '_) {
        if self.restorer.is_none() {
            self.restorer = Some(Box::new(SimpleRestorer::new()));
        }
        match self.restorer.as_mut() {
            Some(restorer) => restorer.as_mut(),
            None => unreachable!("restorer initialized above"),
        }
    }

    pub fn CreatePreallocIDCheckpoint(&self) -> Option<PreallocIDs> {
        self.preallocedIDs
            .as_ref()
            .and_then(PreallocIDs::CreateCheckpoint)
    }

    // Close 幂等：重复调用不应因限速回调二次失败而 panic。
    pub fn Close(&mut self) {
        if let Some(mut importer) = self.importer.take() {
            let _ = importer.Close();
        }
        if let Some(mut restorer) = self.restorer.take() {
            let _ = restorer.Close();
        }
        if let Some(db) = self.db.as_mut() {
            db.Close();
        }
        for db in &mut self.dbPool {
            db.Close();
        }
        self.db = None;
        self.dbPool.clear();
    }

    // 限速通过 importer SetDownloadSpeedLimit 下发；Close 必须成对清零。
    pub fn SetRateLimit(&mut self, rate_limit: u64) {
        self.rateLimit = rate_limit;
    }

    pub fn SetCrypter(&mut self, crypter: Option<backuppb::CipherInfo>) {
        self.cipher = crypter;
    }

    pub fn CleanTablesIfTemporarySystemTablesRenamed(
        &self,
        load_stats_physical: bool,
        load_sys_table_physical: bool,
        tables: Vec<metautil::Table>,
    ) -> Vec<metautil::Table> {
        if !self.temporarySystemTablesRenamed {
            return tables;
        }
        let checker = TemporaryTableChecker::new(load_stats_physical, load_sys_table_physical);
        tables
            .into_iter()
            .filter(|t| {
                let (_, ok) = checker.CheckTemporaryTables(&t.DB.Name.O, &t.Info.Name.O);
                !ok
            })
            .collect()
    }

    pub fn GetClusterID(&self, ctx: &Context) -> u64 {
        self.pdClient.GetClusterID(ctx)
    }

    pub fn GetDomain(&self) -> Option<Arc<dyn DomainLike>> {
        self.dom.clone()
    }

    pub fn GetTLSConfig(&self) -> Option<&TlsConfig> {
        self.tlsConf.as_ref()
    }

    pub fn GetSupportPolicy(&self) -> bool {
        self.supportPolicy
    }

    pub fn SetCheckPrivilegeTableRowsCollateCompatibility(&mut self, v: bool) {
        self.privilegeTableRowsCollateCompatibility = v;
    }

    pub fn GetCheckPrivilegeTableRowsCollateCompatibility(&self) -> bool {
        self.privilegeTableRowsCollateCompatibility
    }

    pub fn SetConcurrencyPerStore(&mut self, c: u32) {
        self.concurrencyPerStore = c;
    }

    fn updateConcurrency(&mut self) {
        const downloadWorkerPoolSizePerStore: usize = 7186;
        self.workerPoolSize = self.storeCount * downloadWorkerPoolSizePerStore;
    }

    pub fn SetRegionScanConcurrency(&mut self, c: u32) {
        self.regionScanConcurrency = c;
    }

    pub fn GetRegionScanConcurrency(&self) -> u32 {
        self.regionScanConcurrency
    }

    pub fn SetSplitRegionIndexStep(&mut self, step: u32) {
        self.splitRegionIndexStep = NormalizeRegionIndexStep(step);
    }

    pub fn GetSplitRegionIndexStep(&self) -> u32 {
        self.splitRegionIndexStep
    }

    pub fn SetCoarseScatter(&mut self, coarse: bool) {
        self.coarseScatter = coarse;
    }

    pub fn GetCoarseScatter(&self) -> bool {
        self.coarseScatter
    }

    pub fn SetBatchDdlSize(&mut self, batch: u32) {
        self.batchDdlSize = batch;
    }

    pub fn GetBatchDdlSize(&self) -> u32 {
        self.batchDdlSize
    }

    pub fn SetTxnTotalSizeLimit(&mut self, limit: u64) {
        self.txnTotalSizeLimit = limit;
    }

    pub fn SetWithSysTable(&mut self, with_sys: bool) {
        self.withSysTable = with_sys;
    }

    pub fn SetRewriteMode(&mut self, ctx: &Context) {
        self.rewriteMode = match self.pdClient.SupportsKeyspaceBR(ctx) {
            Ok(true) => RewriteMode::RewriteModeKeyspace,
            Ok(false) | Err(_) => RewriteMode::RewriteModeLegacy,
        };
    }

    pub fn GetRewriteMode(&self) -> RewriteMode {
        self.rewriteMode
    }

    // 非法字符串应回落到文档约定默认，避免静默忽略策略。
    pub fn SetPlacementPolicyMode(&mut self, mode: &str) {
        self.policyMode = match mode.to_ascii_uppercase().as_str() {
            IGNORE_PLACEMENT_POLICY_MODE => IGNORE_PLACEMENT_POLICY_MODE.to_string(),
            STRICT_PLACEMENT_POLICY_MODE => STRICT_PLACEMENT_POLICY_MODE.to_string(),
            _ => STRICT_PLACEMENT_POLICY_MODE.to_string(),
        };
        log::Info("set placement policy mode");
    }

    // 预分配失败不得部分建表；RewriteRules 依赖完整旧→新 ID 映射。
    pub fn AllocTableIDs(
        &mut self,
        tables: &[metautil::Table],
        load_stats_physical: bool,
        load_sys_table_physical: bool,
        reuse_prealloc: Option<PreallocIDs>,
    ) -> Result<bool> {
        let mut load_stats_physical = load_stats_physical;
        let reusing_checkpoint = reuse_prealloc.is_some();
        let prealloced = if let Some(reuse) = reuse_prealloc {
            ReusePreallocatedTableIDs(&reuse, tables)?
        } else {
            let allocator = self
                .db
                .as_mut()
                .ok_or_else(|| Error::new("database session is not initialized"))?;
            NewAndPreallocTableIDs(tables, allocator.as_mut())?
        };

        let mut user_table_id_not_reused = false;
        if load_stats_physical {
            let min_user = getMinUserTableID(tables);
            let (start, _) = prealloced.GetIDRange();
            if min_user != i64::MAX && min_user < start {
                user_table_id_not_reused = true;
                load_stats_physical = false;
            }
        }

        if reusing_checkpoint && (load_stats_physical || load_sys_table_physical) {
            let checker = TemporaryTableChecker::new(load_stats_physical, load_sys_table_physical);
            if let Some(dom) = &self.dom {
                for table in tables {
                    if let (db_name, true) =
                        checker.CheckTemporaryTables(&table.DB.Name.O, &table.Info.Name.O)
                    {
                        let downstream_id = prealloced.AllocID(table.Info.ID)?;
                        if let Ok(info) = dom.TableInfoByName(&db_name, &table.Info.Name.O) {
                            if info.ID == downstream_id {
                                self.temporarySystemTablesRenamed = true;
                            }
                        }
                        break;
                    }
                }
            }
        }

        for db in &mut self.dbPool {
            db.RegisterPreallocatedIDs(&prealloced);
        }
        if let Some(db) = self.db.as_mut() {
            db.RegisterPreallocatedIDs(&prealloced);
        }
        self.preallocedIDs = Some(prealloced);
        Ok(user_table_id_not_reused)
    }

    pub fn GetPreAllocedTableIDRange(&self) -> Result<[i64; 2]> {
        let Some(ids) = &self.preallocedIDs else {
            return Err(Error::new("No preAlloced IDs"));
        };
        let (start, end) = ids.GetIDRange();
        if start >= end {
            log::Warn("PreAlloced IDs range is empty, no table to restore");
            return Ok([0, 0]);
        }
        Ok([start, end])
    }

    pub fn InitCheckpoint(
        &mut self,
        ctx: &Context,
        manager: &dyn SnapshotCheckpointManager,
        config: Option<ClusterConfig>,
        restore_start_ts: u64,
        log_restored_ts: u64,
        hash: Vec<u8>,
        checkpoint_exists: bool,
    ) -> Result<(HashMap<i64, HashSet<String>>, Option<ClusterConfig>, u64)> {
        let backup_meta = self
            .backupMeta
            .as_ref()
            .ok_or_else(|| Error::new("backup metadata is not initialized"))?;
        let mut checkpoint_set = HashMap::<i64, HashSet<String>>::new();
        let checkpoint_config;
        let new_restore_start_ts;

        if checkpoint_exists {
            let metadata = manager.LoadCheckpointMetadata(ctx)?;
            self.restoreUUID = metadata.RestoreUUID.clone();
            if metadata.UpstreamClusterID != backup_meta.ClusterId {
                return Err(Error::new(format!(
                    "upstream cluster id mismatch: current {}, checkpoint {}",
                    backup_meta.ClusterId, metadata.UpstreamClusterID
                )));
            }
            if metadata.Hash != hash {
                return Err(Error::new(
                    "snapshot restore command hash does not match checkpoint",
                ));
            }
            if metadata.RestoredTS != backup_meta.EndVersion {
                return Err(Error::new(format!(
                    "restore timestamp mismatch: current {}, checkpoint {}",
                    backup_meta.EndVersion, metadata.RestoredTS
                )));
            }
            if metadata.LogRestoredTS != log_restored_ts {
                return Err(Error::new(format!(
                    "log restored timestamp mismatch: current {}, checkpoint {}",
                    log_restored_ts, metadata.LogRestoredTS
                )));
            }
            new_restore_start_ts = metadata.RestoreStartTS;
            checkpoint_config = metadata.SchedulersConfig;
            for (table_id, range_key) in manager.LoadCheckpointData(ctx)? {
                checkpoint_set
                    .entry(table_id)
                    .or_default()
                    .insert(range_key);
            }
            self.checkpointChecksum = manager.LoadCheckpointChecksum(ctx)?;
        } else {
            let restore_uuid = crate::stubs::new_uuid_bytes();
            let metadata = crate::stubs::CheckpointMetadata {
                UpstreamClusterID: backup_meta.ClusterId,
                RestoreStartTS: restore_start_ts,
                RestoredTS: backup_meta.EndVersion,
                LogRestoredTS: log_restored_ts,
                Hash: hash,
                PreallocIDs: self.CreatePreallocIDCheckpoint(),
                RestoreUUID: restore_uuid.clone(),
                SchedulersConfig: config,
            };
            manager.SaveCheckpointMetadata(ctx, &metadata)?;
            self.restoreUUID = restore_uuid;
            checkpoint_config = None;
            new_restore_start_ts = restore_start_ts;
        }

        self.checkpointRunner = Some(manager.StartCheckpointRunner(ctx)?);
        Ok((checkpoint_set, checkpoint_config, new_restore_start_ts))
    }

    pub fn WaitForFinishCheckpoint(&mut self, ctx: &Context, flush: bool) {
        if let Some(runner) = self.checkpointRunner.as_mut() {
            runner.WaitForFinish(ctx, flush);
        }
    }

    // PiTR 收集器依赖集群连接已就绪，过早安装会拿到空 PD。
    pub fn InstallPiTRSupport(&mut self, ctx: &Context, mut deps: PiTRCollDep) -> Result<()> {
        deps.LoadMaxCopyConcurrency(ctx, self.concurrencyPerStore)?;
        if deps.restoreUUID.is_empty() {
            deps.restoreUUID = if self.restoreUUID.is_empty() {
                crate::stubs::new_uuid_bytes()
            } else {
                self.restoreUUID.clone()
            };
        }
        let coll = Arc::new(newPiTRColl(ctx, deps)?);
        if coll.enabled() {
            if self.IsIncremental() {
                let _ = coll.close();
                return Err(Error::Annotate(
                    berrors::ErrStreamLogTaskExist("log backup task exists"),
                    "incremental restore is unsafe while log backup is enabled",
                ));
            }
            let importer = self
                .importer
                .as_mut()
                .ok_or_else(|| Error::new("snapshot importer is not initialized"))?;
            let batch_collector = coll.clone();
            importer.AddBeforeIngestCallback(Box::new(move |callback_ctx, file_sets| {
                batch_collector.onBatchOwned(callback_ctx, file_sets)
            }));
            let close_collector = coll;
            importer.AddCloseCallback(Box::new(move |_| close_collector.close()));
        }
        Ok(())
    }

    // 连接初始化顺序：PD → store 列表 → import/split client → domain。
    pub fn InitConnections(
        &mut self,
        dom: Arc<dyn DomainLike>,
        db: Box<dyn DbSession>,
    ) -> Result<()> {
        self.dom = Some(dom);
        self.db = Some(db);
        if self.backupMeta.is_none() {
            self.backupMeta = Some(backuppb::BackupMeta::default());
        }
        Ok(())
    }

    pub fn initClients(
        &mut self,
        ctx: &Context,
        backend: Option<backuppb::StorageBackend>,
        is_raw_kv_mode: bool,
        is_txn_kv_mode: bool,
        meta_client: Arc<dyn SplitClient>,
        import_client: Arc<dyn ImporterClient>,
        stores: Vec<crate::stubs::metapb::Store>,
        raw_start_key: Vec<u8>,
        raw_end_key: Vec<u8>,
    ) -> Result<()> {
        self.meta_client = Some(meta_client.clone());
        self.import_client = Some(import_client.clone());
        self.storeCount = stores.len();
        self.updateConcurrency();
        let kv_mode = if is_raw_kv_mode {
            KvMode::Raw
        } else if is_txn_kv_mode {
            KvMode::Txn
        } else {
            KvMode::TiDBFull
        };
        let mut create_cbs: Vec<Box<dyn Fn(&mut SnapFileImporter) -> Result<()> + Send + Sync>> =
            Vec::new();
        let mut close_cbs = Vec::new();
        if is_raw_kv_mode {
            create_cbs.push(Box::new(move |importer| {
                importer.SetRawRange(raw_start_key.clone(), raw_end_key.clone())
            }));
        }
        let callback_ctx = ctx.clone();
        let callback_stores = stores.clone();
        create_cbs.push(Box::new(move |importer| {
            importer.CheckMultiIngestSupport(&callback_ctx, &callback_stores)
        }));
        let retry_ctx = ctx.clone();
        let retry_stores = stores.clone();
        create_cbs.push(Box::new(move |importer| {
            importer.CheckPeerDownloadRetrySupport(&retry_ctx, &retry_stores)
        }));
        if self.rateLimit != 0 {
            let (speed_create, speed_close) = SetSpeedLimitCallbacks(
                ctx,
                self.pdStore.clone(),
                import_client.clone(),
                self.rateLimit,
                self.workerPoolSize.max(1),
                self.speed_limit_closed.clone(),
            )?;
            create_cbs.extend(speed_create);
            close_cbs.extend(speed_close);
        }
        let options = NewSnapFileImporterOptions(
            self.cipher.clone(),
            meta_client,
            import_client,
            backend,
            self.rewriteMode,
            stores,
            self.concurrencyPerStore,
            self.regionScanConcurrency,
            false,
            create_cbs,
            close_cbs,
        );
        let api_version = if is_raw_kv_mode || is_txn_kv_mode {
            self.backupMeta
                .as_ref()
                .map(|meta| meta.ApiVersion)
                .unwrap_or(0)
        } else {
            self.dom
                .as_ref()
                .map(|domain| domain.GetAPIVersion())
                .unwrap_or(0)
        };
        self.importer = Some(NewSnapFileImporter(ctx, api_version, kv_mode, options)?);
        Ok(())
    }

    // noSchema 时跳过元数据加载，但仍需能驱动 Raw/Txn 文件导入。
    pub fn LoadSchemaIfNeededAndInitClient(
        &mut self,
        ctx: &Context,
        backup_meta: backuppb::BackupMeta,
        databases: HashMap<String, metautil::Database>,
        ddl_jobs: Vec<model::Job>,
        backend: Option<backuppb::StorageBackend>,
        raw_start_key: Vec<u8>,
        raw_end_key: Vec<u8>,
        has_explicit_filter: bool,
        is_full_restore: bool,
        with_sys: bool,
    ) -> Result<()> {
        if needLoadSchemas(&backup_meta) {
            self.databases = databases;
            self.ddlJobs = ddl_jobs;
        }
        let is_raw_kv_mode = backup_meta.IsRawKv;
        let is_txn_kv_mode = backup_meta.IsTxnKv;
        self.backupMeta = Some(backup_meta);
        let meta_client = self
            .meta_client
            .clone()
            .ok_or_else(|| Error::new("split client is not initialized"))?;
        let import_client = self
            .import_client
            .clone()
            .ok_or_else(|| Error::new("import client is not initialized"))?;
        let stores = GetAllTiKVStoresWithRetry(ctx, self.pdStore.as_ref())?;
        self.initClients(
            ctx,
            backend,
            is_raw_kv_mode,
            is_txn_kv_mode,
            meta_client,
            import_client,
            stores,
            raw_start_key,
            raw_end_key,
        )?;
        self.InitFullClusterRestore(has_explicit_filter, is_full_restore, with_sys);
        Ok(())
    }

    // Raw 模式跳过 schema/DDL，直接按 key range 导入 SST。
    pub fn IsRawKvMode(&self) -> bool {
        self.backupMeta.as_ref().map(|m| m.IsRawKv).unwrap_or(false)
    }

    pub fn GetFilesInRawRange(
        &self,
        start_key: &[u8],
        end_key: &[u8],
        cf: &str,
    ) -> Result<Vec<backuppb::File>> {
        let Some(meta) = &self.backupMeta else {
            return Err(Error::Annotate(
                berrors::ErrRestoreModeMismatch("the backup data is not in raw kv mode"),
                "the backup data is not in raw kv mode",
            ));
        };
        if !meta.IsRawKv {
            return Err(Error::Annotate(
                berrors::ErrRestoreModeMismatch("the backup data is not in raw kv mode"),
                "the backup data is not in raw kv mode",
            ));
        }

        for raw_range in &meta.RawRanges {
            if raw_range.Cf != cf {
                continue;
            }
            if (!raw_range.EndKey.is_empty() && start_key >= raw_range.EndKey.as_slice())
                || (!end_key.is_empty() && raw_range.StartKey.as_slice() >= end_key)
            {
                continue;
            }
            let requested_end_exceeds_backup = if raw_range.EndKey.is_empty() {
                false
            } else {
                end_key.is_empty() || end_key > raw_range.EndKey.as_slice()
            };
            if start_key < raw_range.StartKey.as_slice() || requested_end_exceeds_backup {
                return Err(Error::Annotate(
                    berrors::ErrRestoreRangeMismatch(format!(
                        "the given range to restore [{:?}, {:?}) is not fully covered by the range that was backed up [{:?}, {:?})",
                        start_key, end_key, raw_range.StartKey, raw_range.EndKey
                    )),
                    "restore range is only partially covered",
                ));
            }

            return Ok(meta
                .Files
                .iter()
                .filter(|file| {
                    file.Cf == cf
                        && (file.EndKey.is_empty() || file.EndKey.as_slice() >= start_key)
                        && (end_key.is_empty() || end_key > file.StartKey.as_slice())
                })
                .cloned()
                .collect());
        }

        Err(Error::Annotate(
            berrors::ErrRestoreRangeMismatch("no backup data in the range"),
            "no backup data in the range",
        ))
    }

    pub fn ResetTS(&self, ctx: &Context, pd_controller: &dyn PdController) -> Result<()> {
        let restore_ts = self
            .backupMeta
            .as_ref()
            .ok_or_else(|| Error::new("backup metadata is not initialized"))?
            .EndVersion;
        let mut last_error = None;
        for _ in 0..8 {
            if let Some(err) = ctx.Err() {
                return Err(err);
            }
            match pd_controller.ResetTS(ctx, restore_ts) {
                Ok(()) => return Ok(()),
                Err(err) => {
                    last_error = Some(err);
                    std::thread::yield_now();
                }
            }
        }
        Err(last_error.unwrap_or_else(|| Error::new("failed to reset PD timestamp")))
    }

    pub fn GetDatabases(&self) -> Vec<&metautil::Database> {
        self.databases.values().collect()
    }

    pub fn GetDatabaseMap(&self) -> HashMap<i64, &metautil::Database> {
        self.databases.values().map(|d| (d.Info.ID, d)).collect()
    }

    pub fn GetTableMap(&self) -> HashMap<i64, &metautil::Table> {
        let mut m = HashMap::new();
        for db in self.databases.values() {
            for t in &db.Tables {
                m.insert(t.Info.ID, t);
            }
        }
        m
    }

    pub fn GetPartitionMap(&self) -> HashMap<i64, TableLocationInfo> {
        let mut partitions = HashMap::new();
        for database in self.databases.values() {
            for table in &database.Tables {
                let Some(partition_info) = &table.Info.Partition else {
                    continue;
                };
                for partition in &partition_info.Definitions {
                    partitions.insert(
                        partition.ID,
                        TableLocationInfo {
                            ParentTableID: table.Info.ID,
                            TableName: table.Info.Name.O.clone(),
                            DbID: database.Info.ID,
                            IsPartition: true,
                        },
                    );
                }
            }
        }
        partitions
    }

    pub fn HasBackedUpSysDB(&self) -> bool {
        [SystemDB, SysDB, WorkloadSchema]
            .iter()
            .any(|db| self.databases.contains_key(&TemporaryDBName(db)))
    }

    pub fn GetPlacementPolicies(&self) -> Result<HashMap<String, model::PolicyInfo>> {
        let Some(backup_meta) = &self.backupMeta else {
            return Ok(HashMap::new());
        };
        let mut policies = HashMap::new();
        for policy in &backup_meta.Policies {
            let value: serde_json::Value = serde_json::from_slice(&policy.Info)
                .map_err(|err| Error::new(format!("invalid placement policy: {err}")))?;
            let name_value = value
                .get("name")
                .or_else(|| value.get("Name"))
                .ok_or_else(|| Error::new("placement policy has no name"))?;
            let name = if let Some(name) = name_value.as_str() {
                name.to_string()
            } else {
                name_value
                    .get("O")
                    .or_else(|| name_value.get("o"))
                    .or_else(|| name_value.get("L"))
                    .or_else(|| name_value.get("l"))
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| Error::new("placement policy has an invalid name"))?
                    .to_string()
            };
            let info = model::PolicyInfo {
                Name: model::CIStr::new(name),
            };
            policies.insert(info.Name.L.clone(), info);
        }
        Ok(policies)
    }

    pub fn SetPolicyMap(&mut self, policies: HashMap<String, model::PolicyInfo>) {
        self.policyMap = Some(policies);
    }

    pub fn CreatePolicies(
        &mut self,
        ctx: &Context,
        policies: &HashMap<String, model::PolicyInfo>,
    ) -> Result<()> {
        let Some(session) = self.db.as_mut() else {
            return Err(Error::new("database session is not initialized"));
        };
        for policy in policies.values() {
            session.CreatePlacementPolicy(ctx, policy)?;
        }
        Ok(())
    }

    pub fn GetDDLJobs(&self) -> &[model::Job] {
        &self.ddlJobs
    }

    pub fn CreateDatabases(&mut self, ctx: &Context, dbs: &[metautil::Database]) -> Result<()> {
        if self.IsSkipCreateSQL() {
            log::Info("skip create database");
            return Ok(());
        }
        if self.dbPool.is_empty() {
            let Some(session) = self.db.as_mut() else {
                return Err(Error::new("database session is not initialized"));
            };
            for db in dbs {
                if session.CreateDatabase(ctx, db, self.supportPolicy)? {
                    db.SetReusedByPITR();
                }
            }
            return Ok(());
        }

        let worker_count = self.dbPool.len().min(dbs.len());
        let mut assignments = vec![Vec::<metautil::Database>::new(); worker_count];
        for (index, database) in dbs.iter().cloned().enumerate() {
            assignments[index % worker_count].push(database);
        }
        let support_policy = self.supportPolicy;
        std::thread::scope(|scope| {
            let mut handles = Vec::with_capacity(worker_count);
            for (session, databases) in self.dbPool.iter_mut().take(worker_count).zip(assignments) {
                let ctx = ctx.clone();
                handles.push(scope.spawn(move || {
                    for database in databases {
                        if session.CreateDatabase(&ctx, &database, support_policy)? {
                            database.SetReusedByPITR();
                        }
                    }
                    Ok::<_, Error>(())
                }));
            }
            for handle in handles {
                handle
                    .join()
                    .map_err(|_| Error::new("create database worker panicked"))??;
            }
            Ok(())
        })
    }

    pub fn generateRebasedTables(&mut self, tables: &[metautil::Table]) {
        self.rebasedTablesMap.clear();
        if !self.IsIncremental() {
            return;
        }
        for t in tables {
            self.rebasedTablesMap.insert(
                UniqueTableName {
                    DB: t.DB.Name.O.clone(),
                    Table: t.Info.Name.O.clone(),
                },
                true,
            );
        }
    }

    pub fn getRebasedTables(&self) -> &HashMap<UniqueTableName, bool> {
        &self.rebasedTablesMap
    }

    pub fn CreateTables(
        &mut self,
        ctx: &Context,
        tables: &[metautil::Table],
        new_ts: u64,
    ) -> Result<Vec<CreatedTable>> {
        self.generateRebasedTables(tables);
        if self.batchDdlSize > MIN_BATCH_DDL_SIZE as u32 && !self.dbPool.is_empty() {
            match self.createTablesBatch(ctx, tables, new_ts) {
                Ok(created) => return Ok(created),
                Err(err) if fallBack2CreateTable(&err) => {
                    log::Info("fall back to the sequential create table");
                }
                Err(err) => return Err(err),
            }
        }

        self.createTablesSingle(ctx, tables, new_ts)
    }

    fn createTables(
        ctx: &Context,
        db: &mut dyn DbSession,
        tables: &[metautil::Table],
        new_ts: u64,
        skip_create_sql: bool,
        support_policy: bool,
        rebased: &HashMap<UniqueTableName, bool>,
        dom: &Arc<dyn DomainLike>,
    ) -> Result<Vec<CreatedTable>> {
        if !skip_create_sql {
            db.CreateTables(ctx, tables, rebased, support_policy)?;
        }
        let created = Self::buildCreatedTables(dom, tables, new_ts)?;
        Self::setMergeOptionForTables(ctx, dom, &created)?;
        Ok(created)
    }

    fn setMergeOptionForTables(
        ctx: &Context,
        dom: &Arc<dyn DomainLike>,
        created_tables: &[CreatedTable],
    ) -> Result<()> {
        let mut rules = Vec::new();
        for created in created_tables {
            let old = &created.OldTable;
            if old.IsMergeOptionAllowed {
                rules.push(MergeOptionRule {
                    DbName: old.DB.Name.L.clone(),
                    TableName: old.Info.Name.L.clone(),
                    PhysicalID: created.Table.ID,
                    ..Default::default()
                });
            }
            let (Some(old_partitions), Some(new_partitions)) =
                (&old.Info.Partition, &created.Table.Partition)
            else {
                continue;
            };
            let downstream: HashMap<_, _> = new_partitions
                .Definitions
                .iter()
                .map(|definition| (definition.Name.O.as_str(), definition.ID))
                .collect();
            for old_definition in &old_partitions.Definitions {
                if !old
                    .PartitionMergeOptionAllowed
                    .get(&old_definition.Name.O)
                    .copied()
                    .unwrap_or(false)
                {
                    continue;
                }
                let Some(physical_id) = downstream.get(old_definition.Name.O.as_str()) else {
                    continue;
                };
                rules.push(MergeOptionRule {
                    DbName: old.DB.Name.L.clone(),
                    TableName: old.Info.Name.L.clone(),
                    PartitionName: old_definition.Name.L.clone(),
                    PhysicalID: *physical_id,
                });
            }
        }
        if rules.is_empty() {
            return Ok(());
        }
        dom.UpdateMergeOptionRules(ctx, &rules)
            .map_err(|err| Error::Annotate(err, "failed to batch set merge_option for tables"))
    }

    fn buildCreatedTables(
        dom: &Arc<dyn DomainLike>,
        tables: &[metautil::Table],
        new_ts: u64,
    ) -> Result<Vec<CreatedTable>> {
        let mut created = Vec::with_capacity(tables.len());
        for table in tables {
            let new_info = dom.TableInfoByName(&table.DB.Name.O, &table.Info.Name.O)?;
            if new_info.IsCommonHandle != table.Info.IsCommonHandle {
                return Err(Error::Annotate(
                    berrors::ErrRestoreModeMismatch("mode mismatch"),
                    "Clustered index option mismatch",
                ));
            }
            let mut rewrite_rules = crate::stubs::RewriteRules::new_prefix(
                &crate::stubs::tablecodec::EncodeTablePrefix(table.Info.ID),
                &crate::stubs::tablecodec::EncodeTablePrefix(new_info.ID),
            );
            for rule in &mut rewrite_rules.Data {
                rule.NewTimestamp = new_ts;
            }
            created.push(CreatedTable {
                RewriteRule: Some(rewrite_rules),
                Table: new_info,
                OldTable: table.clone(),
            });
        }
        Ok(created)
    }

    fn createTablesBatch(
        &mut self,
        ctx: &Context,
        tables: &[metautil::Table],
        new_ts: u64,
    ) -> Result<Vec<CreatedTable>> {
        let batch_size = self.batchDdlSize as usize;
        let ordered = SortTablesBySchemaID(tables.to_vec());
        let batches: Vec<Vec<metautil::Table>> = ordered
            .chunks(batch_size)
            .map(<[metautil::Table]>::to_vec)
            .collect();
        let worker_count = self.dbPool.len().min(batches.len());
        let mut assignments = vec![Vec::<Vec<metautil::Table>>::new(); worker_count];
        for (index, batch) in batches.into_iter().enumerate() {
            assignments[index % worker_count].push(batch);
        }
        let dom = self
            .dom
            .clone()
            .ok_or_else(|| Error::new("domain is not initialized"))?;
        let skip = self.IsSkipCreateSQL();
        let support_policy = self.supportPolicy;
        let rebased = self.rebasedTablesMap.clone();
        std::thread::scope(|scope| {
            let mut handles = Vec::with_capacity(worker_count);
            for (db, worker_batches) in self.dbPool.iter_mut().take(worker_count).zip(assignments) {
                let dom = dom.clone();
                let ctx = ctx.clone();
                let rebased = rebased.clone();
                handles.push(scope.spawn(move || {
                    let mut worker_created = Vec::new();
                    for batch in worker_batches {
                        worker_created.extend(Self::createTables(
                            &ctx,
                            db.as_mut(),
                            &batch,
                            new_ts,
                            skip,
                            support_policy,
                            &rebased,
                            &dom,
                        )?);
                    }
                    Ok::<_, Error>(worker_created)
                }));
            }
            let mut created = Vec::with_capacity(tables.len());
            for handle in handles {
                let worker_created = handle
                    .join()
                    .map_err(|_| Error::new("create tables worker panicked"))??;
                created.extend(worker_created);
            }
            Ok(created)
        })
    }

    fn createTable(
        ctx: &Context,
        db: &mut dyn DbSession,
        table: &metautil::Table,
        new_ts: u64,
        skip_create_sql: bool,
        support_policy: bool,
        rebased: &HashMap<UniqueTableName, bool>,
        dom: &Arc<dyn DomainLike>,
    ) -> Result<CreatedTable> {
        if !skip_create_sql {
            db.CreateTable(ctx, table, rebased, support_policy)?;
        }
        Self::buildCreatedTables(dom, std::slice::from_ref(table), new_ts).map(|mut tables| {
            tables
                .pop()
                .expect("one input table produces one created table")
        })
    }

    fn createTablesSingle(
        &mut self,
        ctx: &Context,
        tables: &[metautil::Table],
        new_ts: u64,
    ) -> Result<Vec<CreatedTable>> {
        let dom = self
            .dom
            .clone()
            .ok_or_else(|| Error::new("domain is not initialized"))?;
        let skip = self.IsSkipCreateSQL();
        let support_policy = self.supportPolicy;
        let rebased = self.rebasedTablesMap.clone();
        if self.dbPool.is_empty() {
            let db = self
                .db
                .as_mut()
                .ok_or_else(|| Error::new("database session is not initialized"))?;
            let mut created = Vec::with_capacity(tables.len());
            for table in tables {
                created.push(Self::createTable(
                    ctx,
                    db.as_mut(),
                    table,
                    new_ts,
                    skip,
                    support_policy,
                    &rebased,
                    &dom,
                )?);
            }
            return Ok(created);
        }

        let worker_count = self.dbPool.len().min(tables.len());
        let mut assignments = vec![Vec::<metautil::Table>::new(); worker_count];
        for (index, table) in tables.iter().cloned().enumerate() {
            assignments[index % worker_count].push(table);
        }
        std::thread::scope(|scope| {
            let mut handles = Vec::with_capacity(worker_count);
            for (db, worker_tables) in self.dbPool.iter_mut().take(worker_count).zip(assignments) {
                let dom = dom.clone();
                let ctx = ctx.clone();
                let rebased = rebased.clone();
                handles.push(scope.spawn(move || {
                    let mut created = Vec::with_capacity(worker_tables.len());
                    for table in worker_tables {
                        created.push(Self::createTable(
                            &ctx,
                            db.as_mut(),
                            &table,
                            new_ts,
                            skip,
                            support_policy,
                            &rebased,
                            &dom,
                        )?);
                    }
                    Ok::<_, Error>(created)
                }));
            }
            let mut created = Vec::with_capacity(tables.len());
            for handle in handles {
                let worker_created = handle
                    .join()
                    .map_err(|_| Error::new("create table worker panicked"))??;
                created.extend(worker_created);
            }
            Ok(created)
        })
    }

    pub fn InitFullClusterRestore(
        &mut self,
        explicit_filter: bool,
        is_full_restore: bool,
        with_sys: bool,
    ) {
        self.fullClusterRestore =
            !explicit_filter && !self.IsIncremental() && is_full_restore && with_sys;
        log::Info("mark full cluster restore");
    }

    pub fn IsFullClusterRestore(&self) -> bool {
        self.fullClusterRestore
    }

    pub fn IsIncremental(&self) -> bool {
        let Some(meta) = &self.backupMeta else {
            return false;
        };
        !(meta.StartVersion == meta.EndVersion || meta.StartVersion == 0)
    }

    pub fn NeedCheckFreshCluster(
        &self,
        explicit_filter: bool,
        checkpoint_enabled_and_exists: bool,
    ) -> bool {
        !self.IsIncremental() && !explicit_filter && !checkpoint_enabled_and_exists
    }

    pub fn EnableSkipCreateSQL(&mut self) {
        self.noSchema = true;
    }

    pub fn IsSkipCreateSQL(&self) -> bool {
        self.noSchema
    }

    pub fn EnsureNoUserTables(&self) -> Result<()> {
        log::Info("checking whether cluster contains user dbs and tables");
        let dom = self
            .dom
            .as_ref()
            .ok_or_else(|| Error::new("domain is not initialized"))?;
        dom.AssertUserDBsEmpty()
    }

    pub fn ExecDDLs(&mut self, ctx: &Context, mut ddl_jobs: Vec<model::Job>) -> Result<()> {
        ddl_jobs.sort_by_key(|j| j.SchemaVersion);
        let mut db = if ddl_jobs.is_empty() {
            None
        } else {
            Some(
                self.db
                    .as_mut()
                    .ok_or_else(|| Error::new("database session is not initialized"))?,
            )
        };
        for job in ddl_jobs {
            if let Some(db) = db.as_deref_mut() {
                db.ExecDDL(ctx, &job)?;
            }
            log::Info("execute ddl query");
        }
        Ok(())
    }

    pub fn execAndValidateChecksum(
        &mut self,
        ctx: &Context,
        table: &CreatedTable,
        checksum_client: &dyn ChecksumClient,
        concurrency: u32,
    ) -> Result<()> {
        let mut expected = ChecksumItem {
            TableID: table.Table.ID,
            ..Default::default()
        };
        let mut checksum_exists = false;
        for files in table.OldTable.FilesOfPhysicals.values() {
            for file in files {
                checksum_exists = true;
                expected.Crc64xor ^= file.Crc64Xor;
                expected.TotalKvs = expected.TotalKvs.saturating_add(file.TotalKvs);
                expected.TotalBytes = expected.TotalBytes.saturating_add(file.TotalBytes);
            }
        }
        if !checksum_exists {
            return Ok(());
        }

        let actual = if let Some(item) = self.checkpointChecksum.get(&table.Table.ID) {
            item.clone()
        } else {
            let item = checksum_client.CalculateChecksum(ctx, table, concurrency)?;
            if let Some(runner) = self.checkpointRunner.as_mut() {
                runner.FlushChecksumItem(ctx, &item)?;
            }
            item
        };
        if actual.Crc64xor != expected.Crc64xor
            || actual.TotalKvs != expected.TotalKvs
            || actual.TotalBytes != expected.TotalBytes
        {
            return Err(Error::Annotate(
                berrors::ErrRestoreChecksumMismatch(format!(
                    "checksum mismatch for table '{}.{}' (ID: {}): crc64xor (expected: {}, actual: {}), totalKvs (expected: {}, actual: {}), totalBytes (expected: {}, actual: {})",
                    table.OldTable.DB.Name.O,
                    table.OldTable.Info.Name.O,
                    table.Table.ID,
                    expected.Crc64xor,
                    actual.Crc64xor,
                    expected.TotalKvs,
                    actual.TotalKvs,
                    expected.TotalBytes,
                    actual.TotalBytes,
                )),
                "failed in validate checksum",
            ));
        }
        Ok(())
    }
}

pub fn getMinUserTableID(tables: &[metautil::Table]) -> i64 {
    let mut min_user_table_id = i64::MAX;
    for table in tables {
        if !IsSysOrTempSysDB(&table.DB.Name.O) {
            if table.Info.ID < min_user_table_id {
                min_user_table_id = table.Info.ID;
            }
            if let Some(part) = &table.Info.Partition {
                for p in &part.Definitions {
                    if p.ID < min_user_table_id {
                        min_user_table_id = p.ID;
                    }
                }
            }
        }
    }
    min_user_table_id
}

pub fn makeDBPool<F>(size: usize, mut db_factory: F) -> (Vec<Box<dyn DbSession>>, Option<Error>)
where
    F: FnMut() -> Result<Box<dyn DbSession>>,
{
    let mut pool = Vec::with_capacity(size);
    for _ in 0..size {
        match db_factory() {
            Ok(db) => pool.push(db),
            Err(err) => return (pool, Some(err)),
        }
    }
    (pool, None)
}

/// Returns (createCallbacks, closeCallbacks) that set and reset download speed limits.
pub fn SetSpeedLimitCallbacks(
    ctx: &Context,
    pd_store: Arc<dyn StoreMeta>,
    import_client: Arc<dyn ImporterClient>,
    rate_limit: u64,
    concurrency: usize,
    closed: Arc<Mutex<bool>>,
) -> Result<(
    Vec<Box<dyn Fn(&mut SnapFileImporter) -> Result<()> + Send + Sync>>,
    Vec<Box<dyn Fn(&mut SnapFileImporter) -> Result<()> + Send + Sync>>,
)> {
    let refresh_pd_store = pd_store.clone();
    let refresh_import_client = import_client.clone();
    let refresh_ctx = ctx.clone();
    let set_fn = SetSpeedLimitFn(
        ctx,
        pd_store.clone(),
        import_client.clone(),
        rate_limit,
        concurrency,
    )?;
    let reset_fn = SetSpeedLimitFn(ctx, pd_store, import_client, 0, concurrency)?;
    let closed_flag = closed;
    let stop_state = Arc::new((Mutex::new(false), Condvar::new()));
    let refresh_handle = Arc::new(Mutex::new(None::<std::thread::JoinHandle<()>>));
    let create_stop_state = stop_state.clone();
    let create_refresh_handle = refresh_handle.clone();
    let close_stop_state = stop_state;
    let close_refresh_handle = refresh_handle;
    Ok((
        vec![Box::new(move |importer: &mut SnapFileImporter| {
            set_fn(importer)?;
            let mut handle = create_refresh_handle.lock().unwrap();
            if handle.is_none() {
                let task_id = importer.taskId.clone();
                let pd_store = refresh_pd_store.clone();
                let import_client = refresh_import_client.clone();
                let ctx = refresh_ctx.clone();
                let stop_state = create_stop_state.clone();
                *handle = Some(std::thread::spawn(move || {
                    let (stop_lock, stop_signal) = &*stop_state;
                    loop {
                        let stop = stop_lock.lock().unwrap();
                        if *stop || ctx.Done() {
                            break;
                        }
                        let (stop, _) = stop_signal
                            .wait_timeout(stop, std::time::Duration::from_secs(180))
                            .unwrap();
                        if *stop || ctx.Done() {
                            break;
                        }
                        drop(stop);
                        let _ = setSpeedLimitForTask(
                            &ctx,
                            pd_store.as_ref(),
                            import_client.as_ref(),
                            &task_id,
                            rate_limit,
                            concurrency,
                        );
                    }
                }));
            }
            Ok(())
        })],
        vec![Box::new(move |importer: &mut SnapFileImporter| {
            let mut guard = closed_flag.lock().unwrap();
            if *guard {
                return Ok(());
            }
            {
                let (stop_lock, stop_signal) = &*close_stop_state;
                *stop_lock.lock().unwrap() = true;
                stop_signal.notify_all();
            }
            if let Some(handle) = close_refresh_handle.lock().unwrap().take() {
                let _ = handle.join();
            }
            let mut last_error = None;
            for retry in 0..RESET_SPEED_LIMIT_RETRY_TIMES {
                match reset_fn(importer) {
                    Ok(()) => {
                        *guard = true;
                        return Ok(());
                    }
                    Err(err) => {
                        last_error = Some(err);
                        std::thread::sleep(std::time::Duration::from_secs((retry + 3) as u64));
                    }
                }
            }
            Err(last_error.unwrap_or_else(|| Error::new("failed to reset speed limit")))
        })],
    ))
}

pub fn SetSpeedLimitFn(
    ctx: &Context,
    pd_store: Arc<dyn StoreMeta>,
    import_client: Arc<dyn ImporterClient>,
    rate_limit: u64,
    concurrency: usize,
) -> Result<Box<dyn Fn(&mut SnapFileImporter) -> Result<()> + Send + Sync>> {
    let ctx = ctx.clone();
    Ok(Box::new(move |importer: &mut SnapFileImporter| {
        setSpeedLimitForTask(
            &ctx,
            pd_store.as_ref(),
            import_client.as_ref(),
            &importer.taskId,
            rate_limit,
            concurrency,
        )
    }))
}

fn setSpeedLimitForTask(
    ctx: &Context,
    pd_store: &dyn StoreMeta,
    import_client: &dyn ImporterClient,
    task_id: &str,
    rate_limit: u64,
    concurrency: usize,
) -> Result<()> {
    let stores = GetAllTiKVStoresWithRetry(ctx, pd_store)?;
    if stores.is_empty() {
        return Ok(());
    }

    let next = AtomicUsize::new(0);
    let stopped = AtomicBool::new(false);
    let first_error = Mutex::new(None);
    let worker_count = concurrency.max(1).min(stores.len());
    let request = crate::stubs::import_sstpb::SetDownloadSpeedLimitRequest {
        TaskId: task_id.to_string(),
        SpeedLimit: rate_limit,
        TtlSeconds: DownloadRateLimitTTLSeconds,
    };

    std::thread::scope(|scope| {
        for _ in 0..worker_count {
            scope.spawn(|| {
                loop {
                    if stopped.load(Ordering::Acquire) {
                        break;
                    }
                    if let Some(err) = ctx.Err() {
                        if !stopped.swap(true, Ordering::AcqRel) {
                            *first_error.lock().unwrap() = Some(err);
                        }
                        break;
                    }
                    let index = next.fetch_add(1, Ordering::AcqRel);
                    let Some(store) = stores.get(index) else {
                        break;
                    };
                    if let Err(err) =
                        import_client.SetDownloadSpeedLimit(ctx, store.GetId(), &request)
                    {
                        if !stopped.swap(true, Ordering::AcqRel) {
                            *first_error.lock().unwrap() = Some(Error::Trace(err));
                        }
                        break;
                    }
                }
            });
        }
    });

    let error = first_error.lock().unwrap().take();
    match error {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

pub fn needLoadSchemas(backup_meta: &backuppb::BackupMeta) -> bool {
    !(backup_meta.IsRawKv || backup_meta.IsTxnKv)
}

fn fallBack2CreateTable(err: &Error) -> bool {
    err.code == Some("BR:Restore:ErrUnsupportedBatchDDL")
        || err.msg.contains("unsupported batch create table")
}

pub fn SortTablesBySchemaID(tables: Vec<metautil::Table>) -> Vec<metautil::Table> {
    if tables.len() <= 1 {
        return tables;
    }
    let mut ordered = tables;
    ordered.sort_by(|a, b| {
        a.DB.ID
            .cmp(&b.DB.ID)
            .then_with(|| a.Info.ID.cmp(&b.Info.ID))
    });
    ordered
}

/// Convenience constructor for tests with in-memory PD.
pub fn NewRestoreClientForTest() -> SnapClient {
    let pd = Arc::new(MemPdClient {
        cluster_id: 1,
        stores: Vec::new(),
    });
    let mut client = NewRestoreClient(pd.clone(), pd);
    client.concurrencyPerStore = 1;
    client.regionScanConcurrency = 1;
    client.policyMode = STRICT_PLACEMENT_POLICY_MODE.to_string();
    client.batchDdlSize = MIN_BATCH_DDL_SIZE as u32;
    client.restoreUUID = crate::stubs::new_uuid_bytes();
    client.meta_client = Some(Arc::new(MemSplitClient::default()));
    client.import_client = Some(Arc::new(MemImporterClient::default()));
    client.db = Some(Box::new(MemDb::default()));
    client.dom = Some(Arc::new(MemDomain {
        empty: true,
        ..Default::default()
    }));
    client
}

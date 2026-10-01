// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// Go-compatible InfoSchema reload, lease recovery and metadata-lock barriers.

use crate::{
    Filter, Loader, MDLCheckTableInfo, RelatedSchemaChange, SchemaInfo, SchemaStore, SyncError,
};
use astersql_ddl_schemaver::{Context, Syncer as VersionSyncer};
use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// System SQL boundary. Implementations borrow the existing session pool,
/// execute rollback followed by the MDL query, and return/destroy the resource.
pub trait MDLSessionPool: Send + Sync {
    fn ReadMDLRows(
        &self,
        min_job: i64,
        version: i64,
    ) -> Result<HashMap<i64, crate::JobMDL>, SyncError>;
}
/// Coordinator operates on remaining jobs, not on transaction-held locks.
pub trait InfoSchemaCoordinator: Send + Sync {
    fn CheckOldRunningTxn(&self, jobs: &mut HashMap<i64, crate::JobMDL>);
    fn KillNonFlashbackClusterConn(&self);
}
type CoordinatorGetter = Arc<dyn Fn() -> Option<Arc<dyn InfoSchemaCoordinator>> + Send + Sync>;
#[derive(Default)]
struct MDLProgress {
    last_version: i64,
    pending: bool,
    published: HashMap<i64, i64>,
}

/// Schema 版本切换后的校验器接口：更新租约相关状态或重置。
pub trait SchemaValidator: Send + Sync {
    /// 每次重载均更新租约时间戳，schema 变化时附带相关表变更。
    fn Update(&self, timestamp: u64, old: i64, new: i64, change: Option<&RelatedSchemaChange>);
    /// 重置校验器内部状态。
    fn Reset(&self);
    fn Stop(&self);
    fn Restart(&self, version: i64);
}

/// Syncer is the main structure for syncing the info schema.
///
/// InfoSchema 同步器：持有 Loader、可选 SchemaValidator、MDL 检查表与跨 KS 标志。
pub struct Syncer {
    /// 实际执行加载的 Loader。
    pub(crate) loader: Loader,
    reload_lock: Mutex<()>,
    schemaLease: Duration,
    sysSessionPool: Option<Arc<dyn MDLSessionPool>>,
    coordinator: Option<CoordinatorGetter>,
    versionSyncer: Option<Arc<dyn VersionSyncer>>,
    minJobIDRefresher: Option<Arc<astersql_ddl_systable::MinJobIdRefresher>>,
    mdl_progress: Mutex<MDLProgress>,
    mdl_wake: (Mutex<bool>, Condvar),
    deferFn: Arc<crate::DeferFn>,
    /// 校验器在每次重载时续租，网络租约丢失时停止，重新加载后启动。
    schemaValidator: Option<Arc<dyn SchemaValidator>>,
    /// MDL 检查涉及的表 ID 集合状态。
    mdlCheckTableInfo: MDLCheckTableInfo,
    /// crossKS mirrors Go's `Syncer.crossKS`: true only for syncers created
    /// via `NewCrossKSSyncer`.
    ///
    /// 是否为跨 keyspace Syncer（仅 `NewCrossKSSyncer` 为 true）。
    crossKS: bool,
}

/// New creates a new Syncer instance. Mirrors the Go signature
/// `New(store, infoCache, schemaLease, sysSessionPool, isValidator, filter)`;
/// The cache and system pool are shared production resources. The schema
/// lease is measured in milliseconds; zero is accepted for construction tests.
///
/// 构造普通 Syncer，接入共享缓存、租约和系统会话池。
pub fn New(
    store: Option<Arc<dyn SchemaStore>>,
    infoCache: Option<Arc<crate::InfoCache>>,
    schemaLease: u64,
    sysSessionPool: Option<Arc<dyn MDLSessionPool>>,
    isValidator: Option<Arc<dyn SchemaValidator>>,
    filter: Option<Arc<dyn Filter>>,
) -> Syncer {
    newSyncer(
        store,
        infoCache,
        schemaLease,
        sysSessionPool,
        isValidator,
        filter,
        false,
    )
}

/// NewCrossKSSyncer creates a new Syncer instance for cross keyspace. Mirrors
/// Go's `NewCrossKSSyncer(store, infoCache, schemaLease, sysSessionPool,
/// isValidator, targetKS)`; like Go, the loader is created without a Filter.
///
/// 构造跨 keyspace Syncer：Loader 不带 Filter，且 `crossKS = true`。
pub fn NewCrossKSSyncer(
    store: Option<Arc<dyn SchemaStore>>,
    infoCache: Option<Arc<crate::InfoCache>>,
    schemaLease: u64,
    sysSessionPool: Option<Arc<dyn MDLSessionPool>>,
    isValidator: Option<Arc<dyn SchemaValidator>>,
    _targetKS: &str,
) -> Syncer {
    newSyncer(
        store,
        infoCache,
        schemaLease,
        sysSessionPool,
        isValidator,
        None,
        true,
    )
}

/// 按是否跨 KS 选择 Loader 构造并组装 Syncer。
fn newSyncer(
    store: Option<Arc<dyn SchemaStore>>,
    infoCache: Option<Arc<crate::InfoCache>>,
    schemaLease: u64,
    sysSessionPool: Option<Arc<dyn MDLSessionPool>>,
    isValidator: Option<Arc<dyn SchemaValidator>>,
    filter: Option<Arc<dyn Filter>>,
    crossKS: bool,
) -> Syncer {
    let defer_fn = Arc::new(crate::DeferFn::default());
    let loader = if crossKS {
        match store {
            Some(store) => crate::NewLoaderForCrossKS(store, infoCache),
            None => crate::newLoader(None, infoCache, None, None),
        }
    } else {
        crate::newLoader(store, infoCache, Some(defer_fn.clone()), filter)
    };
    Syncer {
        loader,
        reload_lock: Mutex::new(()),
        schemaLease: Duration::from_millis(schemaLease),
        sysSessionPool,
        coordinator: None,
        versionSyncer: None,
        minJobIDRefresher: None,
        mdl_progress: Mutex::new(MDLProgress::default()),
        mdl_wake: (Mutex::new(false), Condvar::new()),
        deferFn: defer_fn,
        schemaValidator: isValidator,
        mdlCheckTableInfo: MDLCheckTableInfo::default(),
        crossKS,
    }
}

impl Syncer {
    /// Wire the production coordinator and schema-version protocol before loops start.
    pub fn InitRequiredFields(
        &mut self,
        coordinator: CoordinatorGetter,
        syncer: Arc<dyn VersionSyncer>,
    ) {
        self.configure_protocol();
        self.coordinator = Some(coordinator);
        self.versionSyncer = Some(syncer);
    }
    pub fn SetMinJobIDRefresher(
        &mut self,
        refresher: Arc<astersql_ddl_systable::MinJobIdRefresher>,
    ) {
        self.minJobIDRefresher = Some(refresher);
    }
    fn configure_protocol(&self) {
        astersql_ddl_schemaver::SetMDLEnabled(astersql_sessionctx_vardef::IsMDLEnabled());
        astersql_ddl_schemaver::SetNextGen(astersql_config_kerneltype::IsNextGen());
    }
    fn version_syncer(&self) -> Result<&Arc<dyn VersionSyncer>, SyncError> {
        self.versionSyncer
            .as_ref()
            .ok_or_else(|| SyncError("schema version syncer is not initialized".into()))
    }
    pub fn RefreshMDLFromSQL(&self) -> Result<(), SyncError> {
        let version = self
            .InfoSchema()
            .ok_or_else(|| SyncError("schema is not loaded".into()))?
            .Version;
        let pool = self
            .sysSessionPool
            .as_ref()
            .ok_or_else(|| SyncError("MDL session pool is not initialized".into()))?;
        let min = self
            .minJobIDRefresher
            .as_ref()
            .ok_or_else(|| SyncError("minimum job ID refresher is not initialized".into()))?
            .current_min_job_id();
        let jobs = pool.ReadMDLRows(min, version)?;
        self.refreshMDLCheckTableInfoWithJobs(jobs, version);
        Ok(())
    }
    /// One iteration, shared by the loop and deterministic network-boundary tests.
    pub fn CheckMDL(&self) -> Result<(), SyncError> {
        self.check_mdl_with_context(Context::Background())
    }
    fn check_mdl_with_context(&self, context: Context) -> Result<(), SyncError> {
        self.configure_protocol();
        let (version, mut jobs) = self.mdlCheckSnapshot();
        let mut progress = self.mdl_progress.lock().unwrap();
        if version <= progress.last_version && !progress.pending {
            return Ok(());
        }
        progress.last_version = version;
        let count = jobs.len();
        if count == 0 {
            progress.pending = false;
            return Ok(());
        }
        if let Some(coordinator) = self.coordinator.as_ref().and_then(|getter| getter()) {
            coordinator.CheckOldRunningTxn(&mut jobs);
        }
        progress.pending = jobs.len() != count;
        if progress.published.len() > 1000 {
            progress.published.clear();
        }
        let mut publication_error = None;
        for (id, job) in jobs {
            if progress
                .published
                .get(&id)
                .is_some_and(|version| *version >= job.Ver)
            {
                continue;
            }
            match self
                .version_syncer()?
                .UpdateSelfVersion(context.clone(), id, job.Ver)
            {
                Ok(()) => {
                    progress.published.insert(id, job.Ver);
                }
                Err(error) => {
                    progress.pending = true;
                    publication_error = Some(SyncError(error.to_string()));
                }
            }
        }
        publication_error.map_or(Ok(()), Err)
    }
    pub fn MDLCheckLoop(&self, context: Context) -> Result<(), SyncError> {
        self.version_syncer()?;
        while !context.Done() {
            let (pending, wake) = &self.mdl_wake;
            let mut pending = pending.lock().unwrap();
            if !*pending {
                pending = wake
                    .wait_timeout(pending, Duration::from_millis(50))
                    .unwrap()
                    .0;
            }
            *pending = false;
            drop(pending);
            if context.Done() {
                break;
            }
            if astersql_sessionctx_vardef::IsMDLEnabled() {
                if let Err(error) = self.check_mdl_with_context(context.clone()) {
                    eprintln!("MDL version update failed: {error}");
                }
            }
        }
        Ok(())
    }
    pub fn SyncLoop(&self, context: Context) -> Result<(), SyncError> {
        let syncer = self.version_syncer()?;
        if self.schemaLease.is_zero() {
            return Err(SyncError("schema lease must be positive".into()));
        }
        let mut watch = syncer.GlobalVersionCh();
        let interval = self.schemaLease / 2;
        let mut next = Instant::now() + interval;
        while !context.Done() {
            if syncer.Done().Done() {
                let validator = self
                    .schemaValidator
                    .as_ref()
                    .ok_or_else(|| SyncError("schema validator is not initialized".into()))?;
                validator.Stop();
                while syncer.Restart(context.clone()).is_err() {
                    if !context.Wait(Duration::from_secs(1)) {
                        return Ok(());
                    }
                }
                while self.ReloadWithContext(context.clone()).is_err() {
                    if !context.Wait(Duration::from_millis(200)) {
                        return Ok(());
                    }
                }
                validator.Restart(self.InfoSchema().unwrap().Version);
                watch = syncer.GlobalVersionCh();
            } else {
                let timeout = next
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(50));
                let reload = match watch.RecvTimeout(timeout) {
                    Ok(_) => true,
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        syncer.WatchGlobalSchemaVer(context.clone());
                        watch = syncer.GlobalVersionCh();
                        true
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Instant::now() >= next,
                };
                if !reload {
                    continue;
                }
                if let Err(error) = self.ReloadWithContext(context.clone()) {
                    eprintln!("schema reload failed: {error}");
                }
            }
            if Instant::now() >= next {
                self.deferFn.check();
                next = Instant::now() + interval;
            }
            if let Err(error) = self.RefreshMDLFromSQL() {
                eprintln!("MDL refresh failed: {error}");
            }
            *self.mdl_wake.0.lock().unwrap() = true;
            self.mdl_wake.1.notify_one();
        }
        Ok(())
    }

    /// 根据不含表 ID 的作业映射刷新 MDL 版本；完整 SQL 行使用 WithJobs。
    pub fn refreshMDLCheckTableInfo(&self, jobs: HashMap<i64, i64>, newestVer: i64) {
        let jobs = jobs
            .into_iter()
            .map(|(jobID, ver)| {
                (
                    jobID,
                    crate::JobMDL {
                        Ver: ver,
                        TableIDs: Default::default(),
                    },
                )
            })
            .collect();
        self.mdlCheckTableInfo.replace(newestVer, jobs);
    }

    /// Refresh the MDL rows while applying the cross-keyspace table filter.
    pub fn refreshMDLCheckTableInfoWithJobs(
        &self,
        jobs: HashMap<i64, crate::JobMDL>,
        newestVer: i64,
    ) {
        let jobs = jobs
            .into_iter()
            .filter(|(_, job)| !self.skipMDLCheck(&job.TableIDs))
            .collect();
        self.mdlCheckTableInfo.replace(newestVer, jobs);
    }

    /// Return the latest MDL snapshot for callers driving a check loop.
    pub fn mdlCheckSnapshot(&self) -> (i64, HashMap<i64, crate::JobMDL>) {
        self.mdlCheckTableInfo.snapshot()
    }

    /// Check whether a table is covered by a current MDL job.
    pub fn mdlCheckContains(&self, tableID: i64) -> bool {
        self.mdlCheckTableInfo.contains(tableID)
    }

    /// skipMDLCheck mirrors Go's `Syncer.skipMDLCheck`:
    ///   - a regular (non-crossKS) syncer never skips the MDL check.
    ///   - a crossKS syncer only cares about system tables, so it skips the
    ///     check unless one of the given table IDs is a reserved (system)
    ///     table ID.
    ///
    /// 是否跳过 MDL 检查：普通 Syncer 永不跳过；跨 KS 仅当涉及保留（系统）表 ID 时才不跳过。
    pub fn skipMDLCheck(&self, tableIDs: &std::collections::HashSet<i64>) -> bool {
        if !self.crossKS {
            return false;
        }
        // 任一表为保留 ID 则仍需做 MDL 检查。
        for &id in tableIDs {
            if metadef::IsReservedID(id) {
                return false;
            }
        }
        true
    }

    /// 委托 Loader 按 TS 加载 InfoSchema。
    pub fn LoadWithTS(
        &self,
        startTS: u64,
        isSnapshot: bool,
    ) -> Result<(SchemaInfo, bool, i64, Option<RelatedSchemaChange>), SyncError> {
        self.loader.LoadWithTS(startTS, isSnapshot)
    }

    /// Reload reloads InfoSchema.
    ///
    /// 重载 InfoSchema：未命中缓存时通过校验器 `Update` 通知版本变更。
    pub fn Reload(&self) -> Result<(), SyncError> {
        self.ReloadWithContext(Context::Background())
    }
    /// Service-owned reloads must cancel version publication with their loops.
    pub fn ReloadWithContext(&self, context: Context) -> Result<(), SyncError> {
        if let Some(error) = context.Err() {
            return Err(SyncError(error.to_string()));
        }
        self.configure_protocol();
        let _reload = self.reload_lock.lock().unwrap();
        let started = Instant::now();
        let mut timestamp = self.loader.currentVersion()? as u64;
        let loaded = match self.LoadWithTS(timestamp, false) {
            Ok(loaded) => loaded,
            Err(error) => {
                let flashback = getFlashbackStartTSFromErrorMsg(&error.0);
                if flashback == 0 {
                    return Err(error);
                }
                timestamp = flashback - 1;
                self.LoadWithTS(timestamp, false)?
            }
        };
        let (schema, hit, old, change) = loaded;
        if !hit {
            if old < schema.Version {
                if let Some(protocol) = &self.versionSyncer {
                    if let Err(error) =
                        protocol.UpdateSelfVersion(context.clone(), 0, schema.Version)
                    {
                        eprintln!("schema version publication failed: {error}");
                    }
                }
            }
            if change.is_none() {
                if let Some(validator) = &self.schemaValidator {
                    validator.Reset();
                }
            }
        }
        // A slow load can renew using a newer storage timestamp only if the
        // schema did not change in the meantime (Go Reload's lease recovery).
        if !self.schemaLease.is_zero() && started.elapsed() > self.schemaLease / 2 {
            if let Ok(latest) = self.loader.currentVersion() {
                if self
                    .loader
                    .schema_version_at(latest as u64)
                    .is_ok_and(|v| v == schema.Version)
                {
                    timestamp = latest as u64;
                }
            }
        }
        if let Some(validator) = &self.schemaValidator {
            validator.Update(timestamp, old, schema.Version, change.as_ref());
        }
        self.postReload(old, schema.Version, change.as_ref());
        Ok(())
    }
    fn postReload(&self, old: i64, new: i64, change: Option<&RelatedSchemaChange>) {
        if old == new {
            return;
        }
        let Some(change) = change else {
            return;
        };
        for (id, action) in change.PhyTblIDS.iter().zip(&change.ActionTypes) {
            if action.code() == 28 {
                self.loader.delete_cached_table(*id);
            }
            if action.code() == 62 {
                if let Some(coordinator) = self.coordinator.as_ref().and_then(|get| get()) {
                    coordinator.KillNonFlashbackClusterConn();
                }
            }
        }
    }

    /// InfoSchema gets the latest information schema loaded so far.
    ///
    /// 返回目前已加载的最新 Information Schema。
    pub fn InfoSchema(&self) -> Option<SchemaInfo> {
        self.loader.latest()
    }

    /// GetSchemaValidator returns the schema validator.
    ///
    /// 返回已绑定的 SchemaValidator（若有）。
    pub fn GetSchemaValidator(&self) -> Option<Arc<dyn SchemaValidator>> {
        self.schemaValidator.clone()
    }

    /// ChangeSchemaCacheSize changes the schema cache size.
    ///
    /// 调整底层 Loader 的 schema 缓存容量。
    pub fn ChangeSchemaCacheSize(&self, size: u64) {
        self.loader.changeSchemaCacheSize(size)
    }
}

/// getFlashbackStartTSFromErrorMsg extracts the flashback start TS embedded
/// in an error message, mirroring the Go helper of the same name.
///
/// 从 flashback（闪回）进行中错误消息里解析 `FlashbackStartTS`；解析失败返回 0。
pub fn getFlashbackStartTSFromErrorMsg(message: &str) -> u64 {
    let marker = "is in flashback progress, FlashbackStartTS is ";
    let mut parts = message.split(marker);
    match (parts.next(), parts.next(), parts.next()) {
        (Some(_), Some(version), None) => version.parse().unwrap_or(0),
        _ => 0,
    }
}

impl SchemaValidator for astersql_infoschema_isvalidator::Validator {
    fn Update(&self, ts: u64, old: i64, new: i64, change: Option<&RelatedSchemaChange>) {
        let change = change.map(|c| astersql_infoschema_isvalidator::RelatedSchemaChange {
            phy_tbl_ids: c.PhyTblIDS.clone(),
            action_types: c.ActionTypes.iter().map(|a| a.code() as u64).collect(),
        });
        self.update(ts, old, new, change.as_ref());
    }
    fn Reset(&self) {
        self.reset();
    }
    fn Stop(&self) {
        self.stop();
    }
    fn Restart(&self, version: i64) {
        self.restart(version);
    }
}

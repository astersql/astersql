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

// Ported from pkg/infoschema/issyncer/syncer.go. See loader.rs's module doc
// comment for why the store/infoCache/sysSessionPool plumbing is simplified
// to Option-typed placeholders here: none of it is exercised by the ported
// tests (deferfn_test.rs / syncer_test.rs / loader_test.rs), only
// `skipMDLCheck` and `New`/`NewCrossKSSyncer` construction are.

// InfoSchema Syncer：封装 Loader、SchemaValidator 与 MDL 检查表信息。
//
// 对应 Go `issyncer.Syncer`。负责触发加载、重载后通知校验器，以及
// 跨 keyspace 场景下是否跳过 MDL（Metadata Lock，元数据锁）检查。
// store / infoCache / sysSessionPool 等在本移植中为占位，测试主要覆盖
// `skipMDLCheck` 与构造路径。

use crate::{
    Filter, Loader, MDLCheckTableInfo, RelatedSchemaChange, SchemaInfo, SchemaStore, SyncError,
};
use std::collections::HashMap;
use std::sync::Arc;

/// Schema 版本切换后的校验器接口：更新租约相关状态或重置。
pub trait SchemaValidator: Send + Sync {
    /// 在从旧版本加载到新版本后更新校验器（可附带相关表变更）。
    fn Update(&self, old: i64, new: i64, change: Option<&RelatedSchemaChange>);
    /// 重置校验器内部状态。
    fn Reset(&self);
}

/// Syncer is the main structure for syncing the info schema.
///
/// InfoSchema 同步器：持有 Loader、可选 SchemaValidator、MDL 检查表与跨 KS 标志。
pub struct Syncer {
    /// 实际执行加载的 Loader。
    pub(crate) loader: Loader,
    /// 可选的 schema 校验器；重载未命中缓存时调用 Update。
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
/// `infoCache`/`schemaLease`/`sysSessionPool` have no Rust-side equivalent
/// yet and are accepted only so call sites can pass `None`/`0` like Go's
/// `nil`/`0`, matching `New(nil, nil, 0, nil, nil, nil)` in the Go test.
///
/// 构造普通 Syncer；未使用的参数保留以对齐 Go 调用约定。
pub fn New(
    store: Option<Arc<dyn SchemaStore>>,
    _infoCache: Option<()>,
    _schemaLease: u64,
    _sysSessionPool: Option<()>,
    isValidator: Option<Arc<dyn SchemaValidator>>,
    filter: Option<Arc<dyn Filter>>,
) -> Syncer {
    newSyncer(store, isValidator, filter, false)
}

/// NewCrossKSSyncer creates a new Syncer instance for cross keyspace. Mirrors
/// Go's `NewCrossKSSyncer(store, infoCache, schemaLease, sysSessionPool,
/// isValidator, targetKS)`; like Go, the loader is created without a Filter.
///
/// 构造跨 keyspace Syncer：Loader 不带 Filter，且 `crossKS = true`。
pub fn NewCrossKSSyncer(
    store: Option<Arc<dyn SchemaStore>>,
    _infoCache: Option<()>,
    _schemaLease: u64,
    _sysSessionPool: Option<()>,
    isValidator: Option<Arc<dyn SchemaValidator>>,
    _targetKS: &str,
) -> Syncer {
    newSyncer(store, isValidator, None, true)
}

/// 按是否跨 KS 选择 Loader 构造并组装 Syncer。
fn newSyncer(
    store: Option<Arc<dyn SchemaStore>>,
    isValidator: Option<Arc<dyn SchemaValidator>>,
    filter: Option<Arc<dyn Filter>>,
    crossKS: bool,
) -> Syncer {
    let loader = if crossKS {
        match store {
            Some(store) => crate::NewLoaderForCrossKS(store, None),
            None => crate::newLoader(None, None, None, None),
        }
    } else {
        crate::newLoader(store, None, None, filter)
    };
    Syncer {
        loader,
        schemaValidator: isValidator,
        mdlCheckTableInfo: MDLCheckTableInfo::default(),
        crossKS,
    }
}

impl Syncer {
    /// InitRequiredFields initializes some fields of the Syncer. Kept as a
    /// no-op stub, see `Loader::initFields`.
    ///
    /// 初始化必需字段的占位，转发到 `Loader::initFields`。
    pub fn InitRequiredFields(&mut self) {
        self.loader.initFields();
    }

    /// 根据 DDL 作业映射与最新版本刷新 MDL 检查表信息（当前为占位）。
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
        let old = self
            .InfoSchema()
            .map(|is| is.SchemaMetaVersion())
            .unwrap_or(0);
        let startTS = self.loader.currentVersion()? as u64;
        let (is, hitCache, _, change) = self.LoadWithTS(startTS, false)?;
        if !hitCache {
            if change.is_none() {
                if let Some(validator) = &self.schemaValidator {
                    validator.Reset();
                }
            }
            self.postReload(old, is.SchemaMetaVersion(), change.as_ref());
        }
        if let Some(validator) = &self.schemaValidator {
            validator.Update(old, is.SchemaMetaVersion(), change.as_ref());
        }
        Ok(())
    }

    /// 重载成功后通知 SchemaValidator。
    fn postReload(&self, old: i64, new: i64, change: Option<&RelatedSchemaChange>) {
        if old == new || change.is_none() {
            return;
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

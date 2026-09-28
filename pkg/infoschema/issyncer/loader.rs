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

// Ported from pkg/infoschema/issyncer/loader.go. The full Go implementation
// depends on kv.Storage/meta.Reader/meta.Mutator and the infoschema.Builder,
// none of which have a compiling, network-independent Rust port yet. This
// file instead defines a `SchemaStore` trait that captures the same shape of
// operations Loader needs from the storage layer (CurrentVersion, schema
// diffs, database/table listing), so production code can plug in a real
// store later while tests use an in-memory implementation
// (see `loader_test.rs`). All the branch logic that Go tests actually
// exercise (full load vs. diff load, crossKS system-table-only restriction,
// Filter-based skipping) is preserved faithfully.

// InfoSchema 加载器：从 SchemaStore 全量或按 SchemaDiff 增量构建本地缓存。
//
// 对应 Go `issyncer.Loader`。完整实现依赖 kv.Storage / meta.Reader /
// infoschema.Builder，Rust 侧尚无独立可编译移植，故用 `SchemaStore` trait
// 抽象存储层操作；测试用内存实现。保留 Go 测试覆盖的分支：全量 vs 增量、
// 跨 keyspace（多租户命名空间）仅加载系统表、以及 Filter 过滤。

use crate::{
    ActionType, DBInfo, Filter, RelatedSchemaChange, SchemaDiff, SchemaInfo, SyncError, TableInfo,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// LoadSchemaDiffVersionGapThreshold is the threshold for version gap to
/// reload domain by loading schema diffs.
///
/// 版本间隙阈值：当前与目标版本差小于该值时优先走增量 diff 加载，否则全量。
const LoadSchemaDiffVersionGapThreshold: i64 = 10000;

/// memory schema IDs used only to identify the two always-present in-memory
/// databases (`information_schema`, `metrics_schema`); they never collide
/// with real schema IDs which are always >= 1 in Go.
///
/// 内存虚拟库 `information_schema` 的固定负 ID，避免与真实库 ID（≥1）冲突。
const InformationSchemaID: i64 = -1;
/// 内存虚拟库 `metrics_schema` 的固定负 ID。
const MetricsSchemaID: i64 = -2;

/// SchemaStore is the storage abstraction Loader depends on. It mirrors the
/// combination of `kv.Storage` + `meta.Reader` that Go's Loader uses:
///   - `CurrentVersion`/`MaxDiffVersion` stand in for
///     `kv.Storage.CurrentVersion` + `meta.Reader.GetSchemaVersionWithNonEmptyDiff`.
///   - `GetSchemaDiff`/`GetDatabase`/`ListDatabases`/`ListTables`/`GetTable`
///     stand in for the corresponding `meta.Reader` methods.
///
/// Default method bodies return "empty" results so that a store only used to
/// exercise `crossKS`/`GetKeyspace` (as in `testStoreWithKS` in Go) doesn't
/// need to implement the full surface.
///
/// Loader 依赖的存储抽象：对应 Go 侧 `kv.Storage` + `meta.Reader` 的组合能力。
/// 默认方法返回空结果，便于只测 keyspace / crossKS 的轻量 stub。
pub trait SchemaStore: Send + Sync {
    /// 返回当前存储所属 keyspace 名；默认空串。
    fn GetKeyspace(&self) -> String {
        String::new()
    }
    /// 当前存储版本（原始写版本计数）；默认 0。
    fn CurrentVersion(&self) -> Result<i64, SyncError> {
        Ok(0)
    }
    /// Mirrors `meta.Reader.GetSchemaVersionWithNonEmptyDiff`: the newest
    /// schema version that has a non-empty diff recorded, 0 before bootstrap.
    ///
    /// 已记录非空 SchemaDiff 的最新 schema 版本；引导完成前为 0。
    fn MaxDiffVersion(&self) -> i64 {
        0
    }
    /// 按 schema 版本号取对应 SchemaDiff；无则 `None`（可安全跳过空 diff）。
    fn GetSchemaDiff(&self, _version: i64) -> Option<SchemaDiff> {
        None
    }
    /// 按数据库 ID 取库元信息。
    fn GetDatabase(&self, _id: i64) -> Option<DBInfo> {
        None
    }
    /// 列出全部数据库。
    fn ListDatabases(&self) -> Vec<DBInfo> {
        Vec::new()
    }
    /// 列出指定库下的全部表。
    fn ListTables(&self, _schemaID: i64) -> Vec<TableInfo> {
        Vec::new()
    }
    /// 按库 ID + 表 ID 取单表元信息。
    fn GetTable(&self, _schemaID: i64, _tableID: i64) -> Option<TableInfo> {
        None
    }
}

/// Loader 内部缓存：最近一次加载结果，以及按版本索引的历史快照。
#[derive(Clone, Debug, Default)]
struct LoaderCache {
    /// 最近一次成功加载的 SchemaInfo。
    latest: Option<SchemaInfo>,
    /// 按目标 schema 版本缓存的 SchemaInfo。
    byVersion: HashMap<i64, SchemaInfo>,
}

/// Loader is the main structure for syncing the info schema. See the module
/// doc comment for how this differs from Go's `Loader`.
///
/// InfoSchema 加载器主体：持有存储、可选 Filter、跨 KS 标志与版本缓存。
pub struct Loader {
    /// 底层 SchemaStore；未设置时 LoadWithTS 会报错。
    store: Option<Arc<dyn SchemaStore>>,
    /// 可选加载过滤器（如 BR 场景只加载系统/临时库）。
    pub(crate) filter: Option<Arc<dyn Filter>>,
    /// if true, it means the loader is used for cross keyspace, we only allow
    /// loading system tables.
    ///
    /// 为 true 表示跨 keyspace 加载器：仅允许加载系统表相关 diff / 库。
    crossKS: bool,
    /// 已加载 schema 的互斥缓存。
    cache: Mutex<LoaderCache>,
}

/// 构造普通（非跨 KS）Loader；`infoCache`/`deferFn` 占位以对齐 Go 签名。
pub fn newLoader(
    store: Option<Arc<dyn SchemaStore>>,
    _infoCache: Option<()>,
    _deferFn: Option<()>,
    filter: Option<Arc<dyn Filter>>,
) -> Loader {
    Loader {
        store,
        filter,
        crossKS: false,
        cache: Mutex::new(LoaderCache::default()),
    }
}

/// NewLoaderForCrossKS creates a new Loader instance.
///
/// 构造跨 keyspace 的 Loader：强制 `crossKS = true`，且不带 Filter。
pub fn NewLoaderForCrossKS(store: Arc<dyn SchemaStore>, _infoCache: Option<()>) -> Loader {
    Loader {
        store: Some(store),
        filter: None,
        crossKS: true,
        cache: Mutex::new(LoaderCache::default()),
    }
}

impl Loader {
    /// initFields initializes some fields of the Loader. Kept as a no-op stub:
    /// the autoid/sysExecutorFactory wiring it configures in Go is not needed
    /// by any currently-ported test.
    ///
    /// 初始化 Loader 字段的占位；Go 侧 autoid / 系统执行器工厂接线此处不需要。
    pub fn initFields(&mut self) {}

    /// Return the storage timestamp used by `Syncer::Reload`.
    pub fn currentVersion(&self) -> Result<i64, SyncError> {
        self.store
            .as_ref()
            .ok_or_else(|| SyncError("loader has no backing store".to_string()))?
            .CurrentVersion()
    }

    /// LoadWithTS loads info schema at startTS. It returns:
    /// 1. the needed info schema
    /// 2. cache hit indicator
    /// 3. currentSchemaVersion (before loading)
    /// 4. the changed table IDs if it is not a full load
    ///
    /// 按时间戳/当前存储状态加载 InfoSchema。返回值依次为：
    /// 目标 schema、是否命中缓存、加载前的当前版本、增量变更（全量时为 None）。
    /// `startTS` 在本移植中尚未驱动读快照，仅保留签名对齐。
    pub fn LoadWithTS(
        &self,
        _startTS: u64,
        isSnapshot: bool,
    ) -> Result<(SchemaInfo, bool, i64, Option<RelatedSchemaChange>), SyncError> {
        let store = self
            .store
            .as_ref()
            .ok_or_else(|| SyncError("loader has no backing store".to_string()))?;

        // 需要加载到的目标 schema 版本 = 已有非空 diff 的最新版本。
        let neededSchemaVersion = store.MaxDiffVersion();

        // 读取缓存：当前版本、是否已有该目标版本快照、旧的 latest。
        let (currentSchemaVersion, cacheHit, oldLatest) = {
            let cache = self.cache.lock().unwrap();
            let current = cache.latest.as_ref().map(|is| is.Version).unwrap_or(0);
            let hit = cache.byVersion.get(&neededSchemaVersion).cloned();
            (current, hit, cache.latest.clone())
        };

        // 缓存命中：刷新 latest 并直接返回。
        if let Some(hit) = cacheHit {
            let mut cache = self.cache.lock().unwrap();
            cache.latest = Some(hit.clone());
            return Ok((hit, true, 0, None));
        }

        // 非快照、已有基线、且版本间隙小于阈值时，尝试只应用 SchemaDiff。
        if !isSnapshot
            && currentSchemaVersion != 0
            && neededSchemaVersion > currentSchemaVersion
            && neededSchemaVersion - currentSchemaVersion < LoadSchemaDiffVersionGapThreshold
        {
            if let Some(old) = oldLatest {
                if let Ok((is, change)) = self.tryLoadSchemaDiffs(
                    store.as_ref(),
                    &old,
                    currentSchemaVersion,
                    neededSchemaVersion,
                ) {
                    let mut cache = self.cache.lock().unwrap();
                    cache.byVersion.insert(neededSchemaVersion, is.clone());
                    cache.latest = Some(is.clone());
                    return Ok((is, false, currentSchemaVersion, Some(change)));
                }
                // We can fall back to full load, don't need to return the error.
                // 增量失败则回退全量，不向上抛错。
            }
        }

        // 全量加载：枚举库表并注入内存虚拟库（非 crossKS）。
        let is = self.fetchAllSchemasWithTables(store.as_ref(), neededSchemaVersion)?;
        let mut cache = self.cache.lock().unwrap();
        cache.byVersion.insert(neededSchemaVersion, is.clone());
        cache.latest = Some(is.clone());
        Ok((is, false, currentSchemaVersion, None))
    }

    /// skipLoadingDiff mirrors Go's `Loader.skipLoadingDiff`: a caller-supplied
    /// Filter gets first say, then crossKS loaders only accept diffs related
    /// to reserved (system) table IDs.
    ///
    /// 是否跳过该 SchemaDiff：先问 Filter，再对跨 KS 仅保留系统（保留 ID）表相关 diff。
    pub fn skipLoadingDiff(&self, diff: &SchemaDiff) -> bool {
        let latest = self.cache.lock().unwrap().latest.clone();
        self.skipLoadingDiffWithLatest(diff, latest.as_ref())
    }

    /// 在已知 latest SchemaInfo 时判断是否跳过 diff（避免重复加锁）。
    fn skipLoadingDiffWithLatest(&self, diff: &SchemaDiff, latest: Option<&SchemaInfo>) -> bool {
        if let Some(filter) = &self.filter {
            if filter.SkipLoadDiff(diff, latest) {
                return true;
            }
        }

        if !self.crossKS {
            return false;
        }

        // for cross keyspace loader, we only load diff related to system tables.
        // 跨 KS：若新/旧表 ID 都不是保留（系统）ID，则跳过。
        let isRelatedToSystemTables =
            metadef::IsReservedID(diff.TableID) || metadef::IsReservedID(diff.OldTableID);
        !isRelatedToSystemTables
    }

    /// tryLoadSchemaDiffs tries to only load latest schema changes, applying
    /// each diff on top of `old` (the currently cached schema). Returns the
    /// new schema plus the set of changed physical table IDs.
    ///
    /// 在旧 SchemaInfo 上依次应用 `(usedVersion, newVersion]` 的 SchemaDiff，
    /// 返回新 schema 与相关物理表变更集合。
    fn tryLoadSchemaDiffs(
        &self,
        store: &dyn SchemaStore,
        old: &SchemaInfo,
        usedVersion: i64,
        newVersion: i64,
    ) -> Result<(SchemaInfo, RelatedSchemaChange), SyncError> {
        let mut databases = old.Databases.clone();
        let mut tables = old.Tables.clone();
        let mut change = RelatedSchemaChange::default();

        let mut version = usedVersion;
        while version < newVersion {
            version += 1;
            let diff = match store.GetSchemaDiff(version) {
                Some(diff) => diff,
                // Empty diff means the txn of generating schema version is
                // committed, but the txn of `runDDLJob` is not or failed. It
                // is safe to skip it.
                // 空 diff：版本号已提交但 DDL 作业事务未成功，可安全跳过。
                None => continue,
            };

            if self.skipLoadingDiffWithLatest(&diff, Some(old)) {
                // we still conceptually need to set the schema version even
                // when skipping, which we do unconditionally below via
                // `newVersion`.
                // 跳过仍推进版本号（最终 SchemaInfo.Version = newVersion）。
                continue;
            }

            match diff.Type {
                ActionType::CreateTable => {
                    // 替换同库同 ID 表项，并记录物理表变更。
                    if let Some(table) = store.GetTable(diff.SchemaID, diff.TableID) {
                        tables.retain(|t| !(t.SchemaID == diff.SchemaID && t.ID == diff.TableID));
                        tables.push(table);
                    }
                    change.PhyTblIDS.push(diff.TableID);
                    change.ActionTypes.push(diff.Type);
                }
                ActionType::CreateSchema => {
                    // 替换同 ID 库项。
                    if let Some(db) = store.GetDatabase(diff.SchemaID) {
                        databases.retain(|d| d.ID != diff.SchemaID);
                        databases.push(db);
                    }
                }
                // Placement Policy 等动作在本精简枚举中无额外状态更新。
                ActionType::None
                | ActionType::CreatePlacementPolicy
                | ActionType::AlterPlacementPolicy
                | ActionType::DropPlacementPolicy
                | ActionType::CreateResourceGroup
                | ActionType::DropResourceGroup
                | ActionType::AlterResourceGroup => {}
            }
        }

        Ok((
            SchemaInfo {
                Version: newVersion,
                Databases: databases,
                Tables: tables,
            },
            change,
        ))
    }

    /// fetchAllSchemasWithTables fetches all schemas with their tables,
    /// mirroring Go's `Loader.fetchAllSchemasWithTables` plus the memory
    /// schema injection Go's `infoschema.Builder` performs for non-crossKS
    /// loads.
    ///
    /// 全量拉取：crossKS 仅取系统库；否则枚举全部库（可经 Filter），
    /// 并始终注入 `information_schema` / `metrics_schema` 虚拟库。
    fn fetchAllSchemasWithTables(
        &self,
        store: &dyn SchemaStore,
        neededSchemaVersion: i64,
    ) -> Result<SchemaInfo, SyncError> {
        let mut databases = Vec::new();
        let mut tables = Vec::new();

        if self.crossKS {
            // 跨 KS：必须存在系统库，且只加载其下表。
            let db = store
                .GetDatabase(metadef::SystemDatabaseID)
                .ok_or_else(|| SyncError("system database not found".to_string()))?;
            tables.extend(store.ListTables(db.ID));
            databases.push(db);
        } else {
            let mut dbs = store.ListDatabases();
            // Filter 返回 true 表示跳过该库。
            if let Some(filter) = &self.filter {
                dbs.retain(|db| !filter.SkipLoadSchema(Some(db)));
            }
            for db in &dbs {
                tables.extend(store.ListTables(db.ID));
            }
            databases.extend(dbs);
            // Memory-only schemas are always present regardless of Filter,
            // matching Go's infoschema.Builder behaviour for non-crossKS
            // loads.
            // 内存虚拟库不受 Filter 影响，始终注入。
            databases.push(DBInfo::new(InformationSchemaID, "information_schema"));
            databases.push(DBInfo::new(MetricsSchemaID, "metrics_schema"));
        }

        Ok(SchemaInfo {
            Version: neededSchemaVersion,
            Databases: databases,
            Tables: tables,
        })
    }

    /// latest returns the most recently loaded schema, if any.
    ///
    /// 返回最近一次加载的 SchemaInfo（若有）。
    pub fn latest(&self) -> Option<SchemaInfo> {
        self.cache.lock().unwrap().latest.clone()
    }

    /// changeSchemaCacheSize changes the schema cache size. Kept as a no-op
    /// stub: schema cache size (infoschema v2) is out of scope here.
    ///
    /// 调整 schema 缓存容量的占位；infoschema v2 缓存策略不在此范围。
    pub fn changeSchemaCacheSize(&self, _size: u64) {}
}

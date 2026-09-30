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

// Go-compatible schema loading uses one immutable timestamp-bound reader per
// load, preserves complete table metadata, and delegates diffs to the shared
// InfoSchema Builder. Cross-keyspace loading remains system-table-only.

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
pub trait SchemaReader {
    /// Latest schema version whose diff is committed in this snapshot.
    fn MaxDiffVersion(&self) -> Result<i64, SyncError> {
        Ok(0)
    }
    /// Read one committed diff; missing and IO/decoding errors remain distinct.
    fn GetSchemaDiff(&self, _version: i64) -> Result<Option<SchemaDiff>, SyncError> {
        Ok(None)
    }
    /// Read the complete database metadata by ID.
    fn GetDatabase(&self, _id: i64) -> Result<Option<DBInfo>, SyncError> {
        Ok(None)
    }
    /// Enumerate database metadata in this snapshot.
    fn ListDatabases(&self) -> Result<Vec<DBInfo>, SyncError> {
        Ok(Vec::new())
    }
    /// Enumerate complete tables belonging to one database.
    fn ListTables(&self, _schema_id: i64) -> Result<Vec<TableInfo>, SyncError> {
        Ok(Vec::new())
    }
    /// Read the complete table model by database and table ID.
    fn GetTable(&self, _schema_id: i64, _table_id: i64) -> Result<Option<TableInfo>, SyncError> {
        Ok(None)
    }
}

/// Storage and immutable per-load reader. Snapshots are consumed on the loading
/// thread; a KV snapshot need not implement Send/Sync.
pub trait SchemaStore: SchemaReader + Send + Sync {
    fn GetKeyspace(&self) -> String {
        String::new()
    }
    fn CurrentVersion(&self) -> Result<i64, SyncError> {
        Ok(0)
    }
    /// In-memory test stores may read themselves; production adapters return
    /// an immutable reader at exactly start_ts.
    fn Snapshot(&self, _start_ts: u64) -> Result<Option<Box<dyn SchemaReader>>, SyncError> {
        Ok(None)
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
    /// 同次加载的版本、diff 与库表读取使用 startTS 的同一个快照。
    pub fn LoadWithTS(
        &self,
        startTS: u64,
        isSnapshot: bool,
    ) -> Result<(SchemaInfo, bool, i64, Option<RelatedSchemaChange>), SyncError> {
        let store = self
            .store
            .as_ref()
            .ok_or_else(|| SyncError("loader has no backing store".to_string()))?;

        // 需要加载到的目标 schema 版本 = 已有非空 diff 的最新版本。
        let snapshot = store.Snapshot(startTS)?;
        let reader: &dyn SchemaReader = snapshot.as_deref().unwrap_or(store.as_ref());
        let neededSchemaVersion = reader.MaxDiffVersion()?;

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
                if let Ok((is, change)) =
                    self.tryLoadSchemaDiffs(reader, &old, currentSchemaVersion, neededSchemaVersion)
                {
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
        let is = self.fetchAllSchemasWithTables(reader, neededSchemaVersion)?;
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
        store: &dyn SchemaReader,
        old: &SchemaInfo,
        usedVersion: i64,
        newVersion: i64,
    ) -> Result<(SchemaInfo, RelatedSchemaChange), SyncError> {
        let mut builder = astersql_infoschema::builder::Builder::new(
            astersql_infoschema::infoschema_v2::NewData(),
            false,
        )
        .WithCrossKS(self.crossKS);
        let mut databases = old.builder_databases();
        builder.InitWithDBInfos(&mut databases, vec![], vec![], usedVersion);
        let metadata = BuilderReader(store);
        let mut change = RelatedSchemaChange::default();
        for version in (usedVersion + 1)..=newVersion {
            let Some(diff) = store.GetSchemaDiff(version)? else {
                continue;
            };
            if self.skipLoadingDiffWithLatest(&diff, Some(old)) {
                builder.SetSchemaVersion(version);
                continue;
            }
            if diff.RegenerateSchemaMap {
                return Err(SyncError(
                    "schema diff requires regenerating schema map".into(),
                ));
            }
            let ids = builder
                .ApplyDiff(&metadata, &diff.builder_diff())
                .map_err(SyncError)?;
            if !matches!(diff.Type.code(), 30 | 31) {
                change
                    .ActionTypes
                    .extend(std::iter::repeat_n(diff.Type, ids.len()));
                change.PhyTblIDS.extend(ids);
            }
        }
        builder.SetSchemaVersion(newVersion);
        let schema = builder.Build(0);
        let mut loaded = SchemaInfo::from_schema(schema.as_ref());
        for db in &mut loaded.Databases {
            if let Some(metadata) = store.GetDatabase(db.ID)? {
                db.Model = metadata.Model;
            }
        }
        Ok((loaded, change))
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
        store: &dyn SchemaReader,
        neededSchemaVersion: i64,
    ) -> Result<SchemaInfo, SyncError> {
        let mut databases = Vec::new();
        let mut tables = Vec::new();

        if self.crossKS {
            // 跨 KS：必须存在系统库，且只加载其下表。
            let db = store
                .GetDatabase(metadef::SystemDatabaseID)?
                .ok_or_else(|| SyncError("system database not found".to_string()))?;
            tables.extend(store.ListTables(db.ID)?);
            databases.push(db);
        } else {
            let mut dbs = store.ListDatabases()?;
            // Filter 返回 true 表示跳过该库。
            if let Some(filter) = &self.filter {
                dbs.retain(|db| !filter.SkipLoadSchema(Some(db)));
            }
            for db in &dbs {
                tables.extend(store.ListTables(db.ID)?);
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

struct BuilderReader<'a>(&'a dyn SchemaReader);
impl astersql_infoschema::builder::MetadataReader for BuilderReader<'_> {
    fn database(&self, id: i64) -> Result<Option<astersql_infoschema::DBInfo>, String> {
        self.0
            .GetDatabase(id)
            .map(|v| v.map(|db| db.builder_database()))
            .map_err(|e| e.to_string())
    }
    fn table(&self, db: i64, id: i64) -> Result<Option<astersql_infoschema::TableInfo>, String> {
        self.0
            .GetTable(db, id)
            .map(|v| v.map(|t| t.builder_table()))
            .map_err(|e| e.to_string())
    }
}

/// A Go meta reader bound to one real MVCC snapshot. No private catalog is read.
pub struct KvMetaReader {
    snapshot: Box<dyn astersql_kv::Snapshot>,
}
impl KvMetaReader {
    /// Mark a real immutable KV snapshot as an internal meta read.
    pub fn new(mut snapshot: Box<dyn astersql_kv::Snapshot>) -> Self {
        snapshot.SetOption(astersql_kv::RequestSourceInternal, Some(Box::new(true)));
        snapshot.SetOption(
            astersql_kv::RequestSourceType,
            Some(Box::new(astersql_kv::InternalTxnMeta.to_string())),
        );
        snapshot.SetOption(astersql_kv::TiKVClientReadTimeout, Some(Box::new(3000_u64)));
        Self { snapshot }
    }
    fn string_key(key: &[u8]) -> astersql_kv::Key {
        astersql_kv::Key(astersql_util_codec::EncodeUint(
            astersql_util_codec::EncodeBytes(b"m".to_vec(), key),
            b's' as u64,
        ))
    }
    fn hash_prefix(key: &[u8]) -> astersql_kv::Key {
        astersql_kv::Key(astersql_util_codec::EncodeUint(
            astersql_util_codec::EncodeBytes(b"m".to_vec(), key),
            b'h' as u64,
        ))
    }
    fn get(&self, key: astersql_kv::Key) -> Result<Option<Vec<u8>>, SyncError> {
        match self
            .snapshot
            .Get(&astersql_kv::Context::default(), key, &[])
        {
            Ok(value) => Ok(Some(value.Value)),
            Err(error) if astersql_kv::IsErrNotFound(&error) => Ok(None),
            Err(error) => Err(SyncError(error.to_string())),
        }
    }
    fn hash_get(&self, hash: &[u8], field: &[u8]) -> Result<Option<Vec<u8>>, SyncError> {
        self.get(astersql_kv::Key(astersql_util_codec::EncodeBytes(
            Self::hash_prefix(hash).0,
            field,
        )))
    }
    fn scan(&self, hash: &[u8], field_prefix: &[u8]) -> Result<Vec<Vec<u8>>, SyncError> {
        let prefix = Self::hash_prefix(hash);
        let mut iter = self
            .snapshot
            .Iter(prefix.clone(), Some(prefix.PrefixNext()))
            .map_err(|e| SyncError(e.to_string()))?;
        let result = (|| {
            let mut values = Vec::new();
            while iter.Valid() {
                let (_, field) =
                    astersql_util_codec::DecodeBytes(&iter.Key().0[prefix.0.len()..], None)
                        .map_err(|e| SyncError(e.to_string()))?;
                if field.starts_with(field_prefix) {
                    values.push(iter.Value());
                }
                iter.Next().map_err(|e| SyncError(e.to_string()))?;
            }
            Ok(values)
        })();
        iter.Close();
        result
    }
}
#[derive(serde::Deserialize, Default)]
#[serde(default)]
struct GoDiff {
    version: i64,
    #[serde(rename = "type")]
    action: u8,
    schema_id: i64,
    table_id: i64,
    old_schema_id: i64,
    old_table_id: i64,
    regenerate_schema_map: bool,
    read_table_from_meta: bool,
    sub_action_types: Option<Vec<u8>>,
    affected_options: Option<Vec<GoAffected>>,
}
#[derive(serde::Deserialize)]
struct GoAffected {
    schema_id: i64,
    #[serde(default)]
    old_schema_id: i64,
    table_id: i64,
    #[serde(default)]
    old_table_id: i64,
}
impl SchemaReader for KvMetaReader {
    fn MaxDiffVersion(&self) -> Result<i64, SyncError> {
        let version = self
            .get(Self::string_key(b"SchemaVersionKey"))?
            .map(|v| {
                String::from_utf8(v)
                    .map_err(|e| SyncError(e.to_string()))
                    .and_then(|v| v.parse::<i64>().map_err(|e| SyncError(e.to_string())))
            })
            .transpose()?
            .unwrap_or(0);
        Ok(if version > 0 && self.GetSchemaDiff(version)?.is_none() {
            version - 1
        } else {
            version
        })
    }
    fn GetSchemaDiff(&self, version: i64) -> Result<Option<SchemaDiff>, SyncError> {
        self.get(Self::string_key(format!("Diff:{version}").as_bytes()))?
            .map(|bytes| {
                let diff: GoDiff = serde_json::from_slice(&bytes)
                    .map_err(|e| SyncError(format!("decode schema diff {version}: {e}")))?;
                Ok(SchemaDiff {
                    Version: diff.version,
                    Type: ActionType::from_code(diff.action),
                    SchemaID: diff.schema_id,
                    TableID: diff.table_id,
                    OldSchemaID: diff.old_schema_id,
                    OldTableID: diff.old_table_id,
                    RegenerateSchemaMap: diff.regenerate_schema_map,
                    ReadTableFromMeta: diff.read_table_from_meta,
                    SubActionTypes: diff.sub_action_types.unwrap_or_default(),
                    AffectedOptions: diff
                        .affected_options
                        .unwrap_or_default()
                        .into_iter()
                        .map(|o| astersql_infoschema::builder::AffectedOption {
                            schema_id: o.schema_id,
                            table_id: o.table_id,
                            old_schema_id: o.old_schema_id,
                            old_table_id: o.old_table_id,
                        })
                        .collect(),
                })
            })
            .transpose()
    }
    fn GetDatabase(&self, id: i64) -> Result<Option<DBInfo>, SyncError> {
        self.hash_get(b"DBs", format!("DB:{id}").as_bytes())?
            .map(|bytes| {
                astersql_meta_model::DecodeDBInfo(&bytes)
                    .map(DBInfo::from_model)
                    .map_err(SyncError)
            })
            .transpose()
    }
    fn ListDatabases(&self) -> Result<Vec<DBInfo>, SyncError> {
        self.scan(b"DBs", b"DB:")?
            .iter()
            .map(|bytes| {
                astersql_meta_model::DecodeDBInfo(bytes)
                    .map(DBInfo::from_model)
                    .map_err(SyncError)
            })
            .collect()
    }
    fn ListTables(&self, db: i64) -> Result<Vec<TableInfo>, SyncError> {
        if self.GetDatabase(db)?.is_none() {
            return Err(SyncError(format!("database {db} not found")));
        }
        self.scan(format!("DB:{db}").as_bytes(), b"Table:")?
            .iter()
            .map(|bytes| {
                astersql_meta_model::DecodeTableInfo(bytes)
                    .map(|t| TableInfo::from_model(t, db))
                    .map_err(SyncError)
            })
            .collect()
    }
    fn GetTable(&self, db: i64, id: i64) -> Result<Option<TableInfo>, SyncError> {
        if self.GetDatabase(db)?.is_none() {
            return Err(SyncError(format!("database {db} not found")));
        }
        self.hash_get(
            format!("DB:{db}").as_bytes(),
            format!("Table:{id}").as_bytes(),
        )?
        .map(|bytes| {
            astersql_meta_model::DecodeTableInfo(&bytes)
                .map(|t| TableInfo::from_model(t, db))
                .map_err(SyncError)
        })
        .transpose()
    }
}

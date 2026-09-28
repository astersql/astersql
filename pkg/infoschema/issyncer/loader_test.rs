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

// Ported from pkg/infoschema/issyncer/loader_test.go.
//
// Go's version exercises `Loader` against a real `mockstore`/`meta.Mutator`
// and `infoschema.InfoCache`. None of those have a compiling, network-free
// Rust port available (mockstore pulls in the real TiKV/raft dependency
// stack, which this sandbox cannot reliably fetch). Instead this file
// implements a small in-memory `SchemaStore` (`InMemoryStore`) that supports
// the same operations the Go tests use (CreateDatabase, CreateTableOrView,
// GenSchemaVersion, SetSchemaDiff, CurrentVersion) so every test case and
// assertion below matches the Go original branch-for-branch: initial full
// load, cache hit on repeat load, diff-based incremental load, crossKS
// system-table-only restriction, and BR/Filter-based schema selection.

// Loader 单元测试：用内存 SchemaStore 复现 Go `loader_test.go` 各分支。
//
// 覆盖：首次全量加载、缓存命中、SchemaDiff 增量加载、跨 keyspace 仅系统表、
// 以及 BR（Backup & Restore）Filter 按库名筛选。无真实 mockstore / TiKV 依赖。

use crate::{
    ActionType, DBInfo, Filter, New, NewLoaderForCrossKS, SchemaDiff, SchemaInfo, SchemaStore,
    SyncError, TableInfo, newLoader,
};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

// mysql.SystemDB in Go is the literal "mysql"; hardcoded here to avoid
// pulling in the parser crates just for this one constant.
/// 系统库名字面量（Go `mysql.SystemDB`）。
const SYSTEM_DB: &str = "mysql";

/// 内存存储内部可变状态。
#[derive(Default)]
struct StoreState {
    /// 原始写版本计数（每次变更 +1）。
    rawVer: i64,
    /// 已生成的 schema 元版本（GenSchemaVersion）。
    schemaVer: i64,
    /// 库 ID → 库元信息。
    databases: HashMap<i64, DBInfo>,
    /// 库 ID →（表 ID → 表元信息）。
    tables: HashMap<i64, HashMap<i64, TableInfo>>,
    /// schema 版本 → SchemaDiff。
    diffs: HashMap<i64, SchemaDiff>,
}

/// InMemoryStore is a minimal `SchemaStore` used only by tests, mirroring the
/// subset of `mockstore` + `meta.Mutator` behaviour the Go tests rely on.
///
/// 测试专用内存 SchemaStore，模拟 Go mockstore/meta.Mutator 的最小子集。
#[derive(Default)]
struct InMemoryStore {
    state: Mutex<StoreState>,
}

impl InMemoryStore {
    /// 构造共享的空内存存储。
    fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// 创建数据库并递增 rawVer。
    fn createDatabase(&self, db: DBInfo) {
        let mut s = self.state.lock().unwrap();
        s.tables.entry(db.ID).or_default();
        s.databases.insert(db.ID, db);
        s.rawVer += 1;
    }

    /// 在指定库下创建表/视图并递增 rawVer。
    fn createTableOrView(&self, schemaID: i64, table: TableInfo) {
        let mut s = self.state.lock().unwrap();
        s.tables
            .entry(schemaID)
            .or_default()
            .insert(table.ID, table);
        s.rawVer += 1;
    }

    /// 分配新的 schema 元版本（同时递增 rawVer），对应 Go GenSchemaVersion。
    fn genSchemaVersion(&self) -> i64 {
        let mut s = self.state.lock().unwrap();
        s.schemaVer += 1;
        s.rawVer += 1;
        s.schemaVer
    }

    /// 写入指定版本的 SchemaDiff。
    fn setSchemaDiff(&self, diff: SchemaDiff) {
        let mut s = self.state.lock().unwrap();
        s.diffs.insert(diff.Version, diff);
        s.rawVer += 1;
    }

    /// 返回当前 rawVer（作为 LoadWithTS 的“时间戳/版本”输入）。
    fn currentVersion(&self) -> i64 {
        self.state.lock().unwrap().rawVer
    }
}

impl SchemaStore for InMemoryStore {
    fn CurrentVersion(&self) -> Result<i64, SyncError> {
        Ok(self.currentVersion())
    }
    fn MaxDiffVersion(&self) -> i64 {
        self.state.lock().unwrap().schemaVer
    }
    fn GetSchemaDiff(&self, version: i64) -> Option<SchemaDiff> {
        self.state.lock().unwrap().diffs.get(&version).cloned()
    }
    fn GetDatabase(&self, id: i64) -> Option<DBInfo> {
        self.state.lock().unwrap().databases.get(&id).cloned()
    }
    fn ListDatabases(&self) -> Vec<DBInfo> {
        self.state
            .lock()
            .unwrap()
            .databases
            .values()
            .cloned()
            .collect()
    }
    fn ListTables(&self, schemaID: i64) -> Vec<TableInfo> {
        self.state
            .lock()
            .unwrap()
            .tables
            .get(&schemaID)
            .map(|m| m.values().cloned().collect())
            .unwrap_or_default()
    }
    fn GetTable(&self, schemaID: i64, tableID: i64) -> Option<TableInfo> {
        self.state
            .lock()
            .unwrap()
            .tables
            .get(&schemaID)?
            .get(&tableID)
            .cloned()
    }
}

/// testStoreWithKS mirrors Go's `testStoreWithKS`: only `GetKeyspace` matters,
/// every other `SchemaStore` method keeps the trait's empty default.
///
/// 仅提供 keyspace 名的轻量 SchemaStore stub。
#[derive(Default)]
struct TestStoreWithKS;
impl SchemaStore for TestStoreWithKS {
    fn GetKeyspace(&self) -> String {
        "test_ks".to_string()
    }
}

/// testNameFilter mirrors Go's `testNameFilter`: a lightweight Filter
/// implementation for tests, mimicking BR behaviour without importing BR
/// packages to avoid cycles.
///
/// 按库名回调决定是否加载的测试 Filter，模拟 BR 过滤且避免引入 BR 包。
struct TestNameFilter {
    /// 返回 true 表示允许加载该库。
    allow: fn(&DBInfo) -> bool,
}

/// 由可选 allow 回调构造 Filter；`None` 表示不加 Filter。
fn newTestNameFilter(allow: Option<fn(&DBInfo) -> bool>) -> Option<Arc<dyn Filter>> {
    allow.map(|allow| Arc::new(TestNameFilter { allow }) as Arc<dyn Filter>)
}

impl Filter for TestNameFilter {
    fn SkipLoadDiff(&self, diff: &SchemaDiff, latestIS: Option<&SchemaInfo>) -> bool {
        // CREATE DATABASE / Placement Policy 始终放行，与 Go 测试一致。
        if matches!(
            diff.Type,
            ActionType::CreateSchema
                | ActionType::CreatePlacementPolicy
                | ActionType::AlterPlacementPolicy
                | ActionType::DropPlacementPolicy
                | ActionType::CreateResourceGroup
                | ActionType::DropResourceGroup
                | ActionType::AlterResourceGroup
        ) {
            return false;
        }
        if diff.SchemaID == 0 {
            return false;
        }
        let Some(latestIS) = latestIS else {
            // 尚无缓存 schema 时无法按名判断，默认跳过。
            return true;
        };
        match latestIS.SchemaByID(diff.SchemaID) {
            Some(schema) => !(self.allow)(schema),
            None => true,
        }
    }

    fn SkipLoadSchema(&self, dbInfo: Option<&DBInfo>) -> bool {
        match dbInfo {
            None => false,
            Some(db) => !(self.allow)(db),
        }
    }
}

/// 仅填充 TableID / OldTableID 的 diff 辅助构造。
fn diffWithIds(tableID: i64, oldTableID: i64) -> SchemaDiff {
    SchemaDiff {
        TableID: tableID,
        OldTableID: oldTableID,
        ..Default::default()
    }
}

/// 仅填充 SchemaID / TableID 的 diff 辅助构造。
fn diffForSchema(schemaID: i64, tableID: i64) -> SchemaDiff {
    SchemaDiff {
        SchemaID: schemaID,
        TableID: tableID,
        ..Default::default()
    }
}

/// 填充 OldSchemaID / SchemaID / TableID 的 diff 辅助构造。
fn diffWithOldSchema(oldSchemaID: i64, schemaID: i64, tableID: i64) -> SchemaDiff {
    SchemaDiff {
        OldSchemaID: oldSchemaID,
        SchemaID: schemaID,
        TableID: tableID,
        ..Default::default()
    }
}

/// 收集 SchemaInfo 中全部库的小写名集合。
fn schemaNameSet(is: &SchemaInfo) -> HashSet<String> {
    is.AllSchemas().iter().map(|s| s.NameL.clone()).collect()
}

/// test_load_from_ts is the port of Go's `TestLoadFromTS`.
///
/// 验证：空库全量仅两虚拟库；二次命中缓存；建库建表后全量；再增量加载新表。
#[test]
fn test_load_from_ts() {
    let store = InMemoryStore::new();
    let loader = newLoader(
        Some(store.clone() as Arc<dyn SchemaStore>),
        None,
        None,
        None,
    );
    let ver = store.currentVersion();
    let (is, hitCache, oldSchemaVersion, changes) =
        loader.LoadWithTS(ver as u64, false).expect("LoadWithTS");
    let allSchemas = is.AllSchemas();
    // only 2 memory schemas are there
    // 初始仅有 information_schema / metrics_schema。
    assert_eq!(allSchemas.len(), 2);
    assert!(allSchemas.iter().any(|s| s.NameL == "information_schema"));
    assert!(allSchemas.iter().any(|s| s.NameL == "metrics_schema"));
    assert!(!hitCache);
    assert_eq!(oldSchemaVersion, 0);
    assert!(changes.is_none());

    // hit cache
    // 相同目标版本应命中 byVersion 缓存。
    let (_, hitCache, _, changes) = loader.LoadWithTS(ver as u64, false).expect("LoadWithTS");
    assert!(hitCache);
    assert!(changes.is_none());

    // 写入用户库与表，并记录 CreateTable diff。
    store.createDatabase(DBInfo::new(1, "test"));
    store.createTableOrView(1, TableInfo::new(1, 1, "t"));
    let schVer = store.genSchemaVersion();
    store.setSchemaDiff(SchemaDiff {
        Version: schVer,
        Type: ActionType::CreateTable,
        SchemaID: 1,
        TableID: 1,
        ..Default::default()
    });

    let ver = store.currentVersion();
    let loader = newLoader(
        Some(store.clone() as Arc<dyn SchemaStore>),
        None,
        None,
        None,
    );
    let (is, hitCache, oldSchemaVersion, changes) =
        loader.LoadWithTS(ver as u64, false).expect("LoadWithTS");
    let allSchemas = is.AllSchemas();
    assert_eq!(is.SchemaMetaVersion(), 1);
    assert_eq!(allSchemas.len(), 3);
    assert!(allSchemas.iter().any(|s| s.NameL == "information_schema"));
    assert!(allSchemas.iter().any(|s| s.NameL == "metrics_schema"));
    assert!(allSchemas.iter().any(|s| s.NameL == "test"));
    assert!(!hitCache);
    assert_eq!(oldSchemaVersion, 0);
    assert!(changes.is_none());
    let tbls = is.SchemaTableInfos("test");
    assert_eq!(tbls.len(), 1);
    assert_eq!(tbls[0].NameL, "t");

    // load from diff
    // 在已有 latest 上再加表，应走增量路径并回报 PhyTblIDS。
    store.createTableOrView(1, TableInfo::new(2, 1, "t1"));
    let schVer = store.genSchemaVersion();
    store.setSchemaDiff(SchemaDiff {
        Version: schVer,
        Type: ActionType::CreateTable,
        SchemaID: 1,
        TableID: 2,
        ..Default::default()
    });

    let ver = store.currentVersion();
    let (is, hitCache, oldSchemaVersion, changes) =
        loader.LoadWithTS(ver as u64, false).expect("LoadWithTS");
    let allSchemas = is.AllSchemas();
    assert_eq!(is.SchemaMetaVersion(), 2);
    assert_eq!(allSchemas.len(), 3);
    assert!(allSchemas.iter().any(|s| s.NameL == "information_schema"));
    assert!(allSchemas.iter().any(|s| s.NameL == "metrics_schema"));
    assert!(allSchemas.iter().any(|s| s.NameL == "test"));
    assert!(!hitCache);
    assert_eq!(oldSchemaVersion, 1);
    assert_eq!(changes.expect("changes").PhyTblIDS.len(), 1);
    let tbls = is.SchemaTableInfos("test");
    assert_eq!(tbls.len(), 2);
    assert!(tbls.iter().any(|t| t.NameL == "t"));
    assert!(tbls.iter().any(|t| t.NameL == "t1"));
}

/// test_load_from_ts_for_cross_ks is the port of Go's `TestLoadFromTSForCrossKS`.
///
/// 跨 KS：无系统库时报错；仅加载系统库；增量时跳过非保留表 ID 的 diff。
#[test]
fn test_load_from_ts_for_cross_ks() {
    let store = InMemoryStore::new();
    let loader = NewLoaderForCrossKS(store.clone() as Arc<dyn SchemaStore>, None);
    let ver = store.currentVersion();
    let err = loader
        .LoadWithTS(ver as u64, false)
        .expect_err("expected system database not found");
    assert!(err.0.contains("system database not found"));

    let systemDBID = metadef::SystemDatabaseID;
    store.createDatabase(DBInfo::new(systemDBID, SYSTEM_DB));
    let testTblID = metadef::ReservedGlobalIDUpperBound - 1;
    store.createTableOrView(systemDBID, TableInfo::new(testTblID, systemDBID, "t"));
    let schVer = store.genSchemaVersion();
    store.setSchemaDiff(SchemaDiff {
        Version: schVer,
        Type: ActionType::CreateTable,
        SchemaID: systemDBID,
        TableID: testTblID,
        ..Default::default()
    });

    let ver = store.currentVersion();
    let (is, hitCache, oldSchemaVersion, changes) =
        loader.LoadWithTS(ver as u64, false).expect("LoadWithTS");
    let allSchemas = is.AllSchemas();
    assert_eq!(is.SchemaMetaVersion(), 1);
    assert_eq!(allSchemas.len(), 1);
    assert_eq!(allSchemas[0].NameL, SYSTEM_DB);
    assert_eq!(allSchemas[0].ID, systemDBID);
    assert!(!hitCache);
    assert_eq!(oldSchemaVersion, 0);
    assert!(changes.is_none());
    let tbls = is.SchemaTableInfos(SYSTEM_DB);
    assert_eq!(tbls.len(), 1);
    assert_eq!(tbls[0].NameL, "t");

    // load from diff, diff of non-reserved table ID is not loaded
    // 保留表 ID 的 diff 会加载；普通表 ID=100 的 diff 被 skip。
    let reservedTblID = metadef::ReservedGlobalIDUpperBound - 2;
    store.createTableOrView(systemDBID, TableInfo::new(reservedTblID, systemDBID, "t1"));
    let schVer = store.genSchemaVersion();
    store.setSchemaDiff(SchemaDiff {
        Version: schVer,
        Type: ActionType::CreateTable,
        SchemaID: systemDBID,
        TableID: reservedTblID,
        ..Default::default()
    });

    store.createTableOrView(systemDBID, TableInfo::new(100, systemDBID, "t100"));
    let schVer = store.genSchemaVersion();
    store.setSchemaDiff(SchemaDiff {
        Version: schVer,
        Type: ActionType::CreateTable,
        SchemaID: systemDBID,
        TableID: 100,
        ..Default::default()
    });

    let ver = store.currentVersion();
    let (is, hitCache, oldSchemaVersion, changes) =
        loader.LoadWithTS(ver as u64, false).expect("LoadWithTS");
    let allSchemas = is.AllSchemas();
    assert_eq!(is.SchemaMetaVersion(), 3);
    assert_eq!(allSchemas.len(), 1);
    assert_eq!(allSchemas[0].NameL, SYSTEM_DB);
    assert_eq!(allSchemas[0].ID, systemDBID);
    assert!(!hitCache);
    assert_eq!(oldSchemaVersion, 1);
    assert_eq!(changes.expect("changes").PhyTblIDS.len(), 1);
    let tbls = is.SchemaTableInfos(SYSTEM_DB);
    assert_eq!(tbls.len(), 2);
    assert!(tbls.iter().any(|t| t.NameL == "t"));
    assert!(tbls.iter().any(|t| t.NameL == "t1"));
}

/// test_loader_skip_loading_diff is the port of Go's `TestLoaderSkipLoadingDiff`.
///
/// 普通 Loader 不跳过；跨 KS Loader 仅放行涉及保留表 ID 的 diff。
#[test]
fn test_loader_skip_loading_diff() {
    let syncer = New(None, None, 0, None, None, None);
    assert!(!syncer.loader.skipLoadingDiff(&SchemaDiff::default()));
    assert!(!syncer.loader.skipLoadingDiff(&diffWithIds(100, 0)));
    assert!(!syncer.loader.skipLoadingDiff(&diffWithIds(0, 100)));
    assert!(!syncer.loader.skipLoadingDiff(&diffWithIds(100, 100)));
    assert!(
        !syncer
            .loader
            .skipLoadingDiff(&diffWithIds(metadef::ReservedGlobalIDUpperBound, 0))
    );
    assert!(
        !syncer
            .loader
            .skipLoadingDiff(&diffWithIds(0, metadef::ReservedGlobalIDUpperBound))
    );
    assert!(!syncer.loader.skipLoadingDiff(&diffWithIds(
        metadef::ReservedGlobalIDUpperBound,
        metadef::ReservedGlobalIDUpperBound
    )));

    let loaderForCrossKS =
        NewLoaderForCrossKS(Arc::new(TestStoreWithKS) as Arc<dyn SchemaStore>, None);
    assert!(loaderForCrossKS.skipLoadingDiff(&SchemaDiff::default()));
    assert!(loaderForCrossKS.skipLoadingDiff(&diffWithIds(100, 0)));
    assert!(loaderForCrossKS.skipLoadingDiff(&diffWithIds(0, 100)));
    assert!(loaderForCrossKS.skipLoadingDiff(&diffWithIds(100, 100)));
    assert!(
        !loaderForCrossKS.skipLoadingDiff(&diffWithIds(metadef::ReservedGlobalIDUpperBound, 0))
    );
    assert!(
        !loaderForCrossKS.skipLoadingDiff(&diffWithIds(0, metadef::ReservedGlobalIDUpperBound))
    );
    assert!(!loaderForCrossKS.skipLoadingDiff(&diffWithIds(
        metadef::ReservedGlobalIDUpperBound,
        metadef::ReservedGlobalIDUpperBound
    )));
}

/// test_loader_skip_loading_diff_for_br is the port of Go's
/// `TestLoaderSkipLoadingDiffForBR`.
///
/// BR Filter：系统库 / BR 临时库相关 diff 不跳过；用户库与未知库跳过。
#[test]
fn test_loader_skip_loading_diff_for_br() {
    let store = InMemoryStore::new();
    // Create databases: system database, BR-related database, and user database.
    // 系统库、BR 临时库、用户库各一。
    store.createDatabase(DBInfo::new(1, SYSTEM_DB));
    store.createDatabase(DBInfo::new(2, "__TiDB_BR_Temporary_test"));
    store.createDatabase(DBInfo::new(3, "userdb"));

    let ver = store.currentVersion();

    let brFilter = newTestNameFilter(Some(|db: &DBInfo| {
        metadef::IsSystemDB(&db.NameL) || metadef::IsBRRelatedDB(&db.NameO)
    }));
    let loaderForBR = newLoader(
        Some(store.clone() as Arc<dyn SchemaStore>),
        None,
        None,
        brFilter,
    );
    assert!(loaderForBR.filter.is_some());

    // Load initial schema to populate the cache.
    // 先全量加载以填充 Filter 所需的 latest 缓存。
    let _ = loaderForBR
        .LoadWithTS(ver as u64, false)
        .expect("LoadWithTS");

    // Schema diff for system database - should NOT skip.
    assert!(
        !loaderForBR.skipLoadingDiff(&diffForSchema(1, 10)),
        "should NOT skip diff for system database"
    );
    // CREATE DATABASE diff should always pass through the filter.
    assert!(
        !loaderForBR.skipLoadingDiff(&SchemaDiff {
            Type: ActionType::CreateSchema,
            SchemaID: 4,
            ..Default::default()
        }),
        "should NOT skip CREATE DATABASE diffs even when the schema name is unknown"
    );
    for action in [
        ActionType::CreatePlacementPolicy,
        ActionType::AlterPlacementPolicy,
        ActionType::DropPlacementPolicy,
        ActionType::CreateResourceGroup,
        ActionType::DropResourceGroup,
        ActionType::AlterResourceGroup,
    ] {
        assert!(
            !loaderForBR.skipLoadingDiff(&SchemaDiff {
                Type: action,
                SchemaID: 999,
                ..Default::default()
            }),
            "global policy/resource-group actions must pass through the filter"
        );
    }
    // Schema diff for BR-related database - should NOT skip.
    assert!(
        !loaderForBR.skipLoadingDiff(&diffForSchema(2, 20)),
        "should NOT skip diff for BR-related database"
    );
    // Schema diff for user database - should skip.
    assert!(
        loaderForBR.skipLoadingDiff(&diffForSchema(3, 30)),
        "should skip diff for user database"
    );
    // OldSchemaID for system database - still skipped because selection is based on SchemaID.
    assert!(
        loaderForBR.skipLoadingDiff(&diffWithOldSchema(1, 3, 40)),
        "should skip diff when SchemaID is filtered out even if OldSchemaID is system database"
    );
    // OldSchemaID for BR-related database - still skipped because SchemaID is filtered out.
    assert!(
        loaderForBR.skipLoadingDiff(&diffWithOldSchema(2, 3, 50)),
        "should skip diff when SchemaID is filtered out even if OldSchemaID is BR-related database"
    );
    // Both SchemaID and OldSchemaID as user databases - should skip.
    assert!(
        loaderForBR.skipLoadingDiff(&diffWithOldSchema(3, 3, 60)),
        "should skip diff when both SchemaID and OldSchemaID are user databases"
    );
    // Non-existent SchemaID - should skip.
    assert!(
        loaderForBR.skipLoadingDiff(&diffForSchema(999, 70)),
        "should skip diff for non-existent database"
    );
}

/// test_load_for_br is the port of Go's `TestLoadForBR`.
///
/// 全量加载时 BR Filter 只保留系统/BR 临时库（虚拟库仍注入）；无 Filter 则全保留。
#[test]
fn test_load_for_br() {
    let store = InMemoryStore::new();
    store.createDatabase(DBInfo::new(1, SYSTEM_DB));
    store.createTableOrView(1, TableInfo::new(1, 1, "t1"));
    store.createDatabase(DBInfo::new(2, "__TiDB_BR_Temporary_test"));
    store.createTableOrView(2, TableInfo::new(2, 2, "t2"));
    store.createDatabase(DBInfo::new(3, "userdb"));
    store.createTableOrView(3, TableInfo::new(3, 3, "t3"));
    let schVer = store.genSchemaVersion();
    store.setSchemaDiff(SchemaDiff {
        Version: schVer,
        Type: ActionType::CreateTable,
        SchemaID: 3,
        TableID: 3,
        ..Default::default()
    });

    let ver = store.currentVersion();

    // br_filter_only: only load system and BR-related databases.
    // 带 BR Filter：不应加载 userdb，但应有虚拟库。
    let brFilter = newTestNameFilter(Some(|db: &DBInfo| {
        metadef::IsSystemDB(&db.NameL) || metadef::IsBRRelatedDB(&db.NameO)
    }));
    let loaderForBR = newLoader(
        Some(store.clone() as Arc<dyn SchemaStore>),
        None,
        None,
        brFilter,
    );
    let (is, hitCache, oldSchemaVersion, changes) = loaderForBR
        .LoadWithTS(ver as u64, false)
        .expect("LoadWithTS");
    assert!(!hitCache);
    assert_eq!(oldSchemaVersion, 0);
    assert!(changes.is_none());

    let schemaNames = schemaNameSet(&is);
    assert!(
        schemaNames.contains(SYSTEM_DB),
        "system database should be loaded"
    );
    assert!(
        schemaNames.contains("__tidb_br_temporary_test"),
        "BR-related database should be loaded"
    );
    assert!(
        !schemaNames.contains("userdb"),
        "user database should NOT be loaded for BR"
    );
    assert!(
        schemaNames.contains("information_schema"),
        "information_schema should be loaded"
    );
    assert!(
        schemaNames.contains("metrics_schema"),
        "metrics_schema should be loaded"
    );

    let tbls = is.SchemaTableInfos(SYSTEM_DB);
    assert_eq!(tbls.len(), 1);
    assert_eq!(tbls[0].NameL, "t1");
    let tbls = is.SchemaTableInfos("__TiDB_BR_Temporary_test");
    assert_eq!(tbls.len(), 1);
    assert_eq!(tbls[0].NameL, "t2");
    let tbls = is.SchemaTableInfos("userdb");
    assert_eq!(tbls.len(), 0);

    // no_filter_all_dbs: load all databases when no filter is used.
    // 无 Filter：用户库也应加载。
    let loaderNormal = newLoader(
        Some(store.clone() as Arc<dyn SchemaStore>),
        None,
        None,
        None,
    );
    let (isNormal, hitCache, oldSchemaVersion, changes) = loaderNormal
        .LoadWithTS(ver as u64, false)
        .expect("LoadWithTS");
    assert!(!hitCache);
    assert_eq!(oldSchemaVersion, 0);
    assert!(changes.is_none());

    let schemaNamesNormal = schemaNameSet(&isNormal);
    assert!(
        schemaNamesNormal.contains(SYSTEM_DB),
        "system database should be loaded"
    );
    assert!(
        schemaNamesNormal.contains("__tidb_br_temporary_test"),
        "BR-related database should be loaded"
    );
    assert!(
        schemaNamesNormal.contains("userdb"),
        "user database should be loaded"
    );
    assert!(
        schemaNamesNormal.contains("information_schema"),
        "information_schema should be loaded"
    );
    assert!(
        schemaNamesNormal.contains("metrics_schema"),
        "metrics_schema should be loaded"
    );
}

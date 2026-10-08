// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

//! Real compiling tests for slim `precheck_impl` checkers vs Go `precheck_impl_test.go`.

use crate::*;
use astersql_lightning_pkg_checkpoints as checkpoints;
use astersql_lightning_pkg_precheck as precheck;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Configurable target used to drive GetStorageInfo / IsTableEmpty for checker tests.
// 语义说明：`StubTarget` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
// 语义说明：当实现继续演进时，应优先保持这一层公开语义与 Go 对照一致。
// 回归说明：`StubTarget` 这一层额外强调测试场景与 Go 对照的对应关系，便于后续回归定位。
// 场景说明：`StubTarget` 这一层补充记录测试意图、夹具作用或断言边界，防止后续误改契约。
struct StubTarget {
    storage: Mutex<StoresInfo>,
    empty_map: Mutex<HashMap<(String, String), bool>>,
    db: Option<sql::DB>,
    version_error: Mutex<Option<String>>,
}

// 语义说明：`StubTarget` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
// 语义说明：当实现继续演进时，应优先保持这一层公开语义与 Go 对照一致。
// 回归说明：`StubTarget` 这一层额外强调测试场景与 Go 对照的对应关系，便于后续回归定位。
// 场景说明：`StubTarget` 这一层补充记录测试意图、夹具作用或断言边界，防止后续误改契约。
impl StubTarget {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            storage: Mutex::new(StoresInfo::default()),
            empty_map: Mutex::new(HashMap::new()),
            db: None,
            version_error: Mutex::new(None),
        })
    }

    fn with_db(db: sql::DB) -> Arc<Self> {
        Arc::new(Self {
            storage: Mutex::new(StoresInfo::default()),
            empty_map: Mutex::new(HashMap::new()),
            db: Some(db),
            version_error: Mutex::new(None),
        })
    }

    fn set_stores(&self, stores: Vec<StoreInfo>) {
        let mut g = self.storage.lock().unwrap();
        g.Count = stores.len() as i32;
        g.Stores = stores;
    }

    fn set_table_empty(&self, db: &str, tbl: &str, empty: bool) {
        self.empty_map
            .lock()
            .unwrap()
            .insert((db.to_string(), tbl.to_string()), empty);
    }

    fn set_version_error(&self, message: &str) {
        *self.version_error.lock().unwrap() = Some(message.to_string());
    }
}

// 语义说明：`StubTarget` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
// 语义说明：当实现继续演进时，应优先保持这一层公开语义与 Go 对照一致。
// 回归说明：`StubTarget` 这一层额外强调测试场景与 Go 对照的对应关系，便于后续回归定位。
// 场景说明：`StubTarget` 这一层补充记录测试意图、夹具作用或断言边界，防止后续误改契约。
impl TargetInfoGetter for StubTarget {
    fn FetchRemoteDBModels(&self, _ctx: context::Context) -> Result<Vec<model::DBInfo>> {
        Ok(vec![])
    }
    fn FetchRemoteTableModels(
        &self,
        _ctx: context::Context,
        _schemaName: &str,
    ) -> Result<Vec<model::TableInfo>> {
        Ok(vec![])
    }
    fn CheckVersionRequirements(&self, _ctx: context::Context) -> Result<()> {
        match self.version_error.lock().unwrap().clone() {
            Some(message) => Err(Error::new(message)),
            None => Ok(()),
        }
    }
    fn IsTableEmpty(
        &self,
        ctx: context::Context,
        schemaName: &str,
        tableName: &str,
    ) -> Result<Option<bool>> {
        if let Some(v) = self
            .empty_map
            .lock()
            .unwrap()
            .get(&(schemaName.to_string(), tableName.to_string()))
        {
            return Ok(Some(*v));
        }
        if let Some(db) = &self.db {
            return NewTargetInfoGetterImpl(&config::Config::NewConfig(), db.clone(), None)
                .unwrap()
                .IsTableEmpty(ctx, schemaName, tableName);
        }
        Ok(Some(true))
    }
    fn GetTargetSysVariablesForImport(
        &self,
        _ctx: context::Context,
        _opts: &[astersql_lightning_pkg_importer_opts::GetPreInfoOption],
    ) -> HashMap<String, String> {
        HashMap::new()
    }
    fn GetMaxReplica(&self, _ctx: context::Context) -> Result<u64> {
        Ok(3)
    }
    fn GetStorageInfo(&self, _ctx: context::Context) -> Result<StoresInfo> {
        Ok(self.storage.lock().unwrap().clone())
    }
    fn GetEmptyRegionsInfo(&self, _ctx: context::Context) -> Result<RegionsInfo> {
        Ok(RegionsInfo::default())
    }
}

// 语义说明：`make_getter` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
// 语义说明：当实现继续演进时，应优先保持这一层公开语义与 Go 对照一致。
// 回归说明：`make_getter` 这一层额外强调测试场景与 Go 对照的对应关系，便于后续回归定位。
// 场景说明：`make_getter` 这一层补充记录测试意图、夹具作用或断言边界，防止后续误改契约。
fn make_getter(
    cfg: &config::Config,
    db_metas: Vec<mydump::MDDatabaseMeta>,
    target: Arc<dyn TargetInfoGetter>,
) -> Arc<dyn PreImportInfoGetter> {
    let storage = storeapi::Storage::new("file:///tmp");
    for db in &db_metas {
        for table in &db.Tables {
            if let Some(schema) = &table.SchemaFile {
                storage.Put(
                    &schema.Path,
                    format!("CREATE TABLE `{}` (`id` BIGINT)", table.Name).into_bytes(),
                );
            }
        }
    }
    NewPreImportInfoGetter(cfg, db_metas, storage, target, None, None, vec![]).unwrap()
}

// 语义说明：`db_meta_with_csv` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
// 语义说明：当实现继续演进时，应优先保持这一层公开语义与 Go 对照一致。
// 回归说明：`db_meta_with_csv` 这一层额外强调测试场景与 Go 对照的对应关系，便于后续回归定位。
// 场景说明：`db_meta_with_csv` 这一层补充记录测试意图、夹具作用或断言边界，防止后续误改契约。
fn db_meta_with_csv(
    db: &str,
    tbl: &str,
    file_size: i64,
    total_size: i64,
) -> mydump::MDDatabaseMeta {
    mydump::MDDatabaseMeta {
        Name: db.to_string(),
        Tables: vec![mydump::MDTableMeta {
            DB: db.to_string(),
            Name: tbl.to_string(),
            TotalSize: total_size,
            DataFiles: vec![mydump::FileInfo {
                TableName: format!("`{}`.`{}`", db, tbl),
                FileMeta: mydump::SourceFileMeta {
                    Path: format!("{}/{}/data.1.csv", db, tbl),
                    Type: mydump::SourceTypeCSV,
                    FileSize: file_size,
                    ..Default::default()
                },
            }],
            SchemaFile: Some(mydump::SourceFileMeta {
                Path: format!("{}/{}/{}.schema.sql", db, tbl, tbl),
                ..Default::default()
            }),
        }],
    }
}

// 语义说明：`assert_result` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
// 语义说明：当实现继续演进时，应优先保持这一层公开语义与 Go 对照一致。
// 回归说明：`assert_result` 这一层额外强调测试场景与 Go 对照的对应关系，便于后续回归定位。
// 场景说明：`assert_result` 这一层补充记录测试意图、夹具作用或断言边界，防止后续误改契约。
fn assert_result(
    res: Option<precheck::CheckResult>,
    id: precheck::CheckItemID,
    severity: precheck::CheckType,
    passed: bool,
) -> precheck::CheckResult {
    let r = res.expect("expected Some(CheckResult)");
    assert_eq!(r.Item, id);
    assert_eq!(r.Severity, severity);
    assert_eq!(r.Passed, passed, "message={}", r.Message);
    r
}

#[test]
// 语义说明：`test_cluster_resource_check_basic` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
// 语义说明：当实现继续演进时，应优先保持这一层公开语义与 Go 对照一致。
// 回归说明：`test_cluster_resource_check_basic` 这一层额外强调测试场景与 Go 对照的对应关系，便于后续回归定位。
// 场景说明：`test_cluster_resource_check_basic` 这一层补充记录测试意图、夹具作用或断言边界，防止后续误改契约。
fn test_cluster_resource_check_basic() {
    let mut cfg = config::Config::NewConfig();
    cfg.TikvImporter.Backend = config::BackendLocal.into();
    let target = StubTarget::new();
    let getter = make_getter(&cfg, vec![], target.clone());
    let mut ci = NewClusterResourceCheckItem(getter);
    assert_eq!(ci.GetCheckItemID(), precheck::CheckTargetClusterSize);
    // Go treats missing capacity as insufficient only when source data needs space.
    let res = ci.Check(precheck::context::Background()).unwrap();
    assert_result(res, precheck::CheckTargetClusterSize, precheck::Warn, true);

    let metas = vec![db_meta_with_csv("db1", "tbl1", 100, 100)];
    let getter = make_getter(&cfg, metas, target.clone());
    let mut ci = NewClusterResourceCheckItem(getter);
    // Local backend estimates 33 bytes and applies the target replica count (3),
    // so 50 bytes is insufficient for the resulting 99-byte TiKV requirement.
    target.set_stores(vec![StoreInfo {
        Store: StoreMeta {
            Id: 1,
            Address: "s1".into(),
        },
        Status: StoreStatus {
            Capacity: 1000,
            Available: 50,
            RegionCount: 0,
            EmptyRegionCount: 0,
        },
    }]);
    let res = ci.Check(precheck::context::Background()).unwrap();
    assert_result(res, precheck::CheckTargetClusterSize, precheck::Warn, false);

    // avail=900 >= need → pass
    target.set_stores(vec![StoreInfo {
        Store: StoreMeta {
            Id: 1,
            Address: "s1".into(),
        },
        Status: StoreStatus {
            Capacity: 1000,
            Available: 900,
            RegionCount: 0,
            EmptyRegionCount: 0,
        },
    }]);
    let res = ci.Check(precheck::context::Background()).unwrap();
    assert_result(res, precheck::CheckTargetClusterSize, precheck::Warn, true);
}

#[test]
// 语义说明：`test_cluster_version_check_basic` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
// 语义说明：当实现继续演进时，应优先保持这一层公开语义与 Go 对照一致。
// 回归说明：`test_cluster_version_check_basic` 这一层额外强调测试场景与 Go 对照的对应关系，便于后续回归定位。
// 场景说明：`test_cluster_version_check_basic` 这一层补充记录测试意图、夹具作用或断言边界，防止后续误改契约。
fn test_cluster_version_check_basic() {
    let mut cfg = config::Config::NewConfig();
    cfg.TikvImporter.Backend = config::BackendLocal.into();
    let target = StubTarget::new();
    let getter = make_getter(&cfg, vec![], target.clone());
    let mut ci = NewClusterVersionCheckItem(getter, &[]);
    assert_eq!(ci.GetCheckItemID(), precheck::CheckTargetClusterVersion);
    let res = ci.Check(precheck::context::Background()).unwrap();
    let r = assert_result(
        res,
        precheck::CheckTargetClusterVersion,
        precheck::Critical,
        true,
    );
    assert!(r.Message.contains("cluster version check passed"));

    target.set_version_error("unsupported target version");
    let res = ci.Check(precheck::context::Background()).unwrap();
    let r = assert_result(
        res,
        precheck::CheckTargetClusterVersion,
        precheck::Critical,
        false,
    );
    assert!(r.Message.contains("unsupported target version"));
}

#[test]
// 语义说明：`test_empty_region_check_basic` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
// 语义说明：当实现继续演进时，应优先保持这一层公开语义与 Go 对照一致。
// 回归说明：`test_empty_region_check_basic` 这一层额外强调测试场景与 Go 对照的对应关系，便于后续回归定位。
// 场景说明：`test_empty_region_check_basic` 这一层补充记录测试意图、夹具作用或断言边界，防止后续误改契约。
fn test_empty_region_check_basic() {
    let mut cfg = config::Config::NewConfig();
    cfg.TikvImporter.Backend = config::BackendLocal.into();
    let target = StubTarget::new();
    let getter = make_getter(&cfg, vec![], target.clone());
    let mut ci = NewEmptyRegionCheckItem(getter, &[]);
    assert_eq!(ci.GetCheckItemID(), precheck::CheckTargetClusterEmptyRegion);

    // No stores → pass (Warn severity, passed=true)
    let res = ci.Check(precheck::context::Background()).unwrap();
    assert_result(
        res,
        precheck::CheckTargetClusterEmptyRegion,
        precheck::Warn,
        true,
    );

    // 5000 empty regions exceeds the error threshold, while the checker item
    // retains Go's Warn severity and reports Passed=false.
    target.set_stores(vec![
        StoreInfo {
            Store: StoreMeta {
                Id: 1,
                Address: "s1".into(),
            },
            Status: StoreStatus {
                Capacity: 1000,
                Available: 900,
                RegionCount: 0,
                EmptyRegionCount: 5000,
            },
        },
        StoreInfo {
            Store: StoreMeta {
                Id: 2,
                Address: "s2".into(),
            },
            Status: StoreStatus {
                Capacity: 1000,
                Available: 900,
                RegionCount: 0,
                EmptyRegionCount: 0,
            },
        },
    ]);
    let res = ci.Check(precheck::context::Background()).unwrap();
    assert_result(
        res,
        precheck::CheckTargetClusterEmptyRegion,
        precheck::Warn,
        false,
    );

    // Go only fails the check above the error threshold; a warning does not
    // make the check fail, and equality with the warning threshold is allowed.
    target.set_stores(vec![StoreInfo {
        Status: StoreStatus {
            EmptyRegionCount: WARN_EMPTY_REGION_CNT_PER_STORE,
            ..Default::default()
        },
        ..Default::default()
    }]);
    let res = ci.Check(precheck::context::Background()).unwrap();
    assert_result(
        res,
        precheck::CheckTargetClusterEmptyRegion,
        precheck::Warn,
        true,
    );
}

#[test]
// 语义说明：`test_region_distribution_check_basic` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
// 语义说明：当实现继续演进时，应优先保持这一层公开语义与 Go 对照一致。
// 回归说明：`test_region_distribution_check_basic` 这一层额外强调测试场景与 Go 对照的对应关系，便于后续回归定位。
// 场景说明：`test_region_distribution_check_basic` 这一层补充记录测试意图、夹具作用或断言边界，防止后续误改契约。
fn test_region_distribution_check_basic() {
    let mut cfg = config::Config::NewConfig();
    cfg.TikvImporter.Backend = config::BackendLocal.into();
    let target = StubTarget::new();
    let getter = make_getter(&cfg, vec![], target.clone());
    let mut ci = NewRegionDistributionCheckItem(getter, &[]);
    assert_eq!(ci.GetCheckItemID(), precheck::CheckTargetClusterRegionDist);

    // Empty / below threshold → skipped pass
    let res = ci.Check(precheck::context::Background()).unwrap();
    assert_result(
        res,
        precheck::CheckTargetClusterRegionDist,
        precheck::Warn,
        true,
    );

    // 5000 vs 500 gives a ratio below the error threshold: Warn item, failed.
    target.set_stores(vec![
        StoreInfo {
            Store: StoreMeta {
                Id: 1,
                Address: "s1".into(),
            },
            Status: StoreStatus {
                Capacity: 1000,
                Available: 900,
                RegionCount: 5000,
                EmptyRegionCount: 0,
            },
        },
        StoreInfo {
            Store: StoreMeta {
                Id: 2,
                Address: "s2".into(),
            },
            Status: StoreStatus {
                Capacity: 1000,
                Available: 900,
                RegionCount: 500,
                EmptyRegionCount: 0,
            },
        },
    ]);
    let res = ci.Check(precheck::context::Background()).unwrap();
    assert_result(
        res,
        precheck::CheckTargetClusterRegionDist,
        precheck::Warn,
        false,
    );

    // A warning-level skew keeps Passed=true in Go.
    target.set_stores(vec![
        StoreInfo {
            Status: StoreStatus {
                RegionCount: 1000,
                ..Default::default()
            },
            ..Default::default()
        },
        StoreInfo {
            Status: StoreStatus {
                RegionCount: 700,
                ..Default::default()
            },
            ..Default::default()
        },
    ]);
    let res = ci.Check(precheck::context::Background()).unwrap();
    assert_result(
        res,
        precheck::CheckTargetClusterRegionDist,
        precheck::Warn,
        true,
    );
}

#[test]
// 语义说明：`test_storage_permission_check_basic` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
// 语义说明：当实现继续演进时，应优先保持这一层公开语义与 Go 对照一致。
// 回归说明：`test_storage_permission_check_basic` 这一层额外强调测试场景与 Go 对照的对应关系，便于后续回归定位。
// 场景说明：`test_storage_permission_check_basic` 这一层补充记录测试意图、夹具作用或断言边界，防止后续误改契约。
fn test_storage_permission_check_basic() {
    let mut cfg = config::Config::NewConfig();
    cfg.Mydumper.SourceDir = "file:///tmp".into();
    let mut ci = NewStoragePermissionCheckItem(&cfg);
    assert_eq!(ci.GetCheckItemID(), precheck::CheckSourcePermission);
    let res = ci.Check(precheck::context::Background()).unwrap();
    assert_result(
        res,
        precheck::CheckSourcePermission,
        precheck::Critical,
        true,
    );

    // Slim only rejects empty SourceDir (not unreachable S3).
    cfg.Mydumper.SourceDir.clear();
    let mut ci = NewStoragePermissionCheckItem(&cfg);
    let res = ci.Check(precheck::context::Background()).unwrap();
    assert_result(
        res,
        precheck::CheckSourcePermission,
        precheck::Critical,
        false,
    );
}

#[test]
// 语义说明：`test_large_file_check_basic` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
// 语义说明：当实现继续演进时，应优先保持这一层公开语义与 Go 对照一致。
// 回归说明：`test_large_file_check_basic` 这一层额外强调测试场景与 Go 对照的对应关系，便于后续回归定位。
// 场景说明：`test_large_file_check_basic` 这一层补充记录测试意图、夹具作用或断言边界，防止后续误改契约。
fn test_large_file_check_basic() {
    let mut cfg = config::Config::NewConfig();
    cfg.Mydumper.StrictFormat = false;
    let mut ci = NewLargeFileCheckItem(&cfg, &[]);
    assert_eq!(ci.GetCheckItemID(), precheck::CheckLargeDataFile);
    let res = ci.Check(precheck::context::Background()).unwrap();
    assert_result(res, precheck::CheckLargeDataFile, precheck::Warn, true);

    let metas = vec![db_meta_with_csv(
        "db1",
        "tbl1",
        (DEFAULT_CSV_SIZE + 1) as i64,
        (DEFAULT_CSV_SIZE + 1) as i64,
    )];
    let mut ci = NewLargeFileCheckItem(&cfg, &metas);
    let res = ci.Check(precheck::context::Background()).unwrap();
    assert_result(res, precheck::CheckLargeDataFile, precheck::Warn, false);

    cfg.Mydumper.StrictFormat = true;
    let mut ci = NewLargeFileCheckItem(&cfg, &metas);
    let res = ci.Check(precheck::context::Background()).unwrap();
    assert_result(res, precheck::CheckLargeDataFile, precheck::Warn, true);
}

#[test]
// 语义说明：`test_local_disk_placement_check_basic` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
// 语义说明：当实现继续演进时，应优先保持这一层公开语义与 Go 对照一致。
// 回归说明：`test_local_disk_placement_check_basic` 这一层额外强调测试场景与 Go 对照的对应关系，便于后续回归定位。
// 场景说明：`test_local_disk_placement_check_basic` 这一层补充记录测试意图、夹具作用或断言边界，防止后续误改契约。
fn test_local_disk_placement_check_basic() {
    let mut cfg = config::Config::NewConfig();
    cfg.Mydumper.SourceDir = "file:///dev/".into();
    cfg.TikvImporter.SortedKVDir = "/tmp/".into();
    let mut ci = NewLocalDiskPlacementCheckItem(&cfg);
    assert_eq!(ci.GetCheckItemID(), precheck::CheckLocalDiskPlacement);
    let res = ci.Check(precheck::context::Background()).unwrap();
    assert_result(res, precheck::CheckLocalDiskPlacement, precheck::Warn, true);

    cfg.Mydumper.SourceDir = "/tmp/".into();
    cfg.TikvImporter.SortedKVDir = "/tmp/".into();
    let mut ci = NewLocalDiskPlacementCheckItem(&cfg);
    let res = ci.Check(precheck::context::Background()).unwrap();
    assert_result(
        res,
        precheck::CheckLocalDiskPlacement,
        precheck::Warn,
        false,
    );
}

#[test]
// 语义说明：`test_local_temp_kv_dir_check_basic` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
// 语义说明：当实现继续演进时，应优先保持这一层公开语义与 Go 对照一致。
// 回归说明：`test_local_temp_kv_dir_check_basic` 这一层额外强调测试场景与 Go 对照的对应关系，便于后续回归定位。
// 场景说明：`test_local_temp_kv_dir_check_basic` 这一层补充记录测试意图、夹具作用或断言边界，防止后续误改契约。
fn test_local_temp_kv_dir_check_basic() {
    let mut cfg = config::Config::NewConfig();
    cfg.TikvImporter.Backend = config::BackendLocal.into();
    cfg.TikvImporter.SortedKVDir = "/tmp/".into();
    let target = StubTarget::new();
    let getter = make_getter(&cfg, vec![], target.clone());
    let mut ci = NewLocalTempKVDirCheckItem(&cfg, getter, &[]);
    assert_eq!(ci.GetCheckItemID(), precheck::CheckLocalTempKVDir);
    let res = ci.Check(precheck::context::Background()).unwrap();
    assert_result(res, precheck::CheckLocalTempKVDir, precheck::Critical, true);

    // Slim only checks SortedKVDir empty/non-empty (ignores data size).
    cfg.TikvImporter.SortedKVDir.clear();
    let getter = make_getter(&cfg, vec![], target);
    let mut ci = NewLocalTempKVDirCheckItem(&cfg, getter, &[]);
    let res = ci.Check(precheck::context::Background()).unwrap();
    let r = assert_result(
        res,
        precheck::CheckLocalTempKVDir,
        precheck::Critical,
        false,
    );
    assert!(r.Message.contains("sorted-kv-dir"));

    // Non-local → None
    cfg.TikvImporter.Backend = config::BackendTiDB.into();
    cfg.TikvImporter.SortedKVDir = "/tmp/".into();
    let target = StubTarget::new();
    let getter = make_getter(&cfg, vec![], target);
    let mut ci = NewLocalTempKVDirCheckItem(&cfg, getter, &[]);
    let res = ci.Check(precheck::context::Background()).unwrap();
    assert!(res.is_none());
}

#[test]
// 语义说明：`test_checkpoint_check_basic` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
// 语义说明：当实现继续演进时，应优先保持这一层公开语义与 Go 对照一致。
// 回归说明：`test_checkpoint_check_basic` 这一层额外强调测试场景与 Go 对照的对应关系，便于后续回归定位。
// 场景说明：`test_checkpoint_check_basic` 这一层补充记录测试意图、夹具作用或断言边界，防止后续误改契约。
fn test_checkpoint_check_basic() {
    let mut cfg = config::Config::NewConfig();
    cfg.Checkpoint.Enable = true;
    let target = StubTarget::new();
    let getter = make_getter(&cfg, vec![], target);
    let cpdb: Arc<dyn checkpoints::DB> = Arc::new(checkpoints::NewNullCheckpointsDB());
    let mut ci = NewCheckpointCheckItem(&cfg, getter.clone(), &[], Some(cpdb));
    assert_eq!(ci.GetCheckItemID(), precheck::CheckCheckpoints);
    let res = ci.Check(precheck::context::Background()).unwrap();
    assert_result(res, precheck::CheckCheckpoints, precheck::Critical, true);

    let mut ci = NewCheckpointCheckItem(&cfg, getter, &[], None);
    let res = ci.Check(precheck::context::Background()).unwrap();
    assert_result(res, precheck::CheckCheckpoints, precheck::Critical, false);

    cfg.Checkpoint.Enable = false;
    let target = StubTarget::new();
    let getter = make_getter(&cfg, vec![], target);
    let mut ci = NewCheckpointCheckItem(&cfg, getter, &[], None);
    assert!(ci.Check(precheck::context::Background()).unwrap().is_none());
}

#[test]
// 语义说明：`test_schema_check_basic` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
// 语义说明：当实现继续演进时，应优先保持这一层公开语义与 Go 对照一致。
// 回归说明：`test_schema_check_basic` 这一层额外强调测试场景与 Go 对照的对应关系，便于后续回归定位。
// 场景说明：`test_schema_check_basic` 这一层补充记录测试意图、夹具作用或断言边界，防止后续误改契约。
fn test_schema_check_basic() {
    let mut cfg = config::Config::NewConfig();
    cfg.TikvImporter.Backend = config::BackendLocal.into();
    cfg.Mydumper.CSV.Header = true;
    let metas = vec![db_meta_with_csv("db1", "tbl1", 100, 100)];
    let target = StubTarget::new();
    let getter = make_getter(&cfg, metas.clone(), target);
    let mut ci = NewSchemaCheckItem(&cfg, getter, &metas, None);
    assert_eq!(ci.GetCheckItemID(), precheck::CheckSourceSchemaValid);
    // Slim loads stub table structures from SchemaFile → always valid when metas present.
    let res = ci.Check(precheck::context::Background()).unwrap();
    let r = assert_result(
        res,
        precheck::CheckSourceSchemaValid,
        precheck::Critical,
        true,
    );
    assert!(r.Message.contains("source schema valid"));
}

#[test]
// 语义说明：`test_csv_header_check_basic` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
// 语义说明：当实现继续演进时，应优先保持这一层公开语义与 Go 对照一致。
// 回归说明：`test_csv_header_check_basic` 这一层额外强调测试场景与 Go 对照的对应关系，便于后续回归定位。
// 场景说明：`test_csv_header_check_basic` 这一层补充记录测试意图、夹具作用或断言边界，防止后续误改契约。
fn test_csv_header_check_basic() {
    let mut cfg = config::Config::NewConfig();
    cfg.Mydumper.CSV.Header = false;
    let metas = vec![db_meta_with_csv("db1", "tbl1", 100, 100)];
    let target = StubTarget::new();
    let getter = make_getter(&cfg, metas.clone(), target);
    let mut ci = NewCSVHeaderCheckItem(&cfg, getter.clone(), &metas);
    assert_eq!(ci.GetCheckItemID(), precheck::CheckCSVHeader);
    let res = ci.Check(precheck::context::Background()).unwrap();
    let r = assert_result(res, precheck::CheckCSVHeader, precheck::Critical, true);
    assert_eq!(
        r.Message,
        "the config [mydumper.csv.header] is set to false, and CSV header lines are really not detected in the data files"
    );

    cfg.Mydumper.CSV.Header = true;
    let mut ci = NewCSVHeaderCheckItem(&cfg, getter, &metas);
    assert!(ci.Check(precheck::context::Background()).unwrap().is_none());
}

#[test]
fn test_csv_header_check_skips_when_header_enabled() {
    let target = StubTarget::new();
    let mut cfg = config::Config::NewConfig();
    cfg.Mydumper.CSV.Header = true;
    let getter = make_getter(&cfg, vec![], target);
    let mut checker = NewCSVHeaderCheckItem(&cfg, getter, &[]);
    let result = checker
        .Check(precheck::context::Background())
        .expect("header-enabled check");
    assert!(result.is_none());
}

#[test]
// 语义说明：`test_table_empty_check_basic` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
// 语义说明：当实现继续演进时，应优先保持这一层公开语义与 Go 对照一致。
// 回归说明：`test_table_empty_check_basic` 这一层额外强调测试场景与 Go 对照的对应关系，便于后续回归定位。
// 场景说明：`test_table_empty_check_basic` 这一层补充记录测试意图、夹具作用或断言边界，防止后续误改契约。
fn test_table_empty_check_basic() {
    let mut cfg = config::Config::NewConfig();
    cfg.TikvImporter.Backend = config::BackendLocal.into();
    let metas = vec![db_meta_with_csv("db1", "tbl1", 100, 100)];
    let target = StubTarget::new();
    target.set_table_empty("db1", "tbl1", true);
    let getter = make_getter(&cfg, metas.clone(), target.clone());
    let mut ci = NewTableEmptyCheckItem(&cfg, getter, &metas, None);
    assert_eq!(ci.GetCheckItemID(), precheck::CheckTargetTableEmpty);
    let res = ci.Check(precheck::context::Background()).unwrap();
    assert_result(
        res,
        precheck::CheckTargetTableEmpty,
        precheck::Critical,
        true,
    );

    target.set_table_empty("db1", "tbl1", false);
    let getter = make_getter(&cfg, metas.clone(), target);
    let mut ci = NewTableEmptyCheckItem(&cfg, getter, &metas, None);
    let res = ci.Check(precheck::context::Background()).unwrap();
    let r = assert_result(
        res,
        precheck::CheckTargetTableEmpty,
        precheck::Critical,
        false,
    );
    assert!(r.Message.contains("db1.tbl1"));
}

#[test]
// 语义说明：`test_cdc_pitr_check_item` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
// 语义说明：当实现继续演进时，应优先保持这一层公开语义与 Go 对照一致。
// 回归说明：`test_cdc_pitr_check_item` 这一层额外强调测试场景与 Go 对照的对应关系，便于后续回归定位。
// 场景说明：`test_cdc_pitr_check_item` 这一层补充记录测试意图、夹具作用或断言边界，防止后续误改契约。
fn test_cdc_pitr_check_item() {
    let mut cfg = config::Config::NewConfig();
    cfg.TikvImporter.Backend = config::BackendLocal.into();
    let pd_addrs: Arc<dyn Fn(context::Context) -> Vec<String> + Send + Sync> =
        Arc::new(|_ctx| vec!["http://controlled-pd:2379".into()]);
    let observed_addrs = Arc::new(Mutex::new(Vec::new()));
    let captured_addrs = observed_addrs.clone();
    let status_getter: CDCPITRStatusGetter = Arc::new(move |_ctx, _cfg, addrs, keyspace_name| {
        assert!(keyspace_name.is_empty());
        *captured_addrs.lock().unwrap() = addrs.to_vec();
        Ok(false)
    });
    let mut ci = NewCDCPITRCheckItemWithStatusGetter(&cfg, pd_addrs.clone(), status_getter.clone());
    assert_eq!(ci.GetCheckItemID(), precheck::CheckTargetUsingCDCPITR);
    // The unit test owns the PD/etcd boundary and cannot silently use a local service.
    let res = ci.Check(precheck::context::Background()).unwrap();
    let r = assert_result(
        res,
        precheck::CheckTargetUsingCDCPITR,
        precheck::Critical,
        true,
    );
    assert!(r.Message.contains("no active CDC/PiTR"));
    assert_eq!(
        *observed_addrs.lock().unwrap(),
        ["http://controlled-pd:2379"]
    );

    cfg.TikvImporter.Backend = config::BackendTiDB.into();
    let mut ci = NewCDCPITRCheckItemWithStatusGetter(&cfg, pd_addrs, status_getter);
    let res = ci.Check(precheck::context::Background()).unwrap();
    assert!(res.is_none(), "TiDB backend skips CDC/PiTR check");
}

#[test]
// 语义说明：`test_pd_tidb_from_same_cluster` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
// 语义说明：当实现继续演进时，应优先保持这一层公开语义与 Go 对照一致。
// 回归说明：`test_pd_tidb_from_same_cluster` 这一层额外强调测试场景与 Go 对照的对应关系，便于后续回归定位。
// 场景说明：`test_pd_tidb_from_same_cluster` 这一层补充记录测试意图、夹具作用或断言边界，防止后续误改契约。
fn test_pd_tidb_from_same_cluster() {
    let db = sql::DB::new_memory();
    let pd_ok: Arc<dyn Fn(context::Context) -> Vec<String> + Send + Sync> = Arc::new(|_ctx| {
        vec![
            "https://1.2.3.4:2379".into(),
            "http://127.0.0.1:2379".into(),
        ]
    });
    let pd_empty: Arc<dyn Fn(context::Context) -> Vec<String> + Send + Sync> =
        Arc::new(|_ctx| vec![]);

    let mut ci = NewPDTiDBFromSameClusterCheckItem(Some(db.clone()), pd_ok.clone());
    assert_eq!(ci.GetCheckItemID(), precheck::CheckPDTiDBFromSameCluster);
    let res = ci.Check(precheck::context::Background()).unwrap();
    assert_result(
        res,
        precheck::CheckPDTiDBFromSameCluster,
        precheck::Critical,
        true,
    );

    let mut ci = NewPDTiDBFromSameClusterCheckItem(Some(db), pd_empty);
    let res = ci.Check(precheck::context::Background()).unwrap();
    assert_result(
        res,
        precheck::CheckPDTiDBFromSameCluster,
        precheck::Critical,
        false,
    );

    let mut ci = NewPDTiDBFromSameClusterCheckItem(None, pd_ok);
    let res = ci.Check(precheck::context::Background()).unwrap();
    assert_result(
        res,
        precheck::CheckPDTiDBFromSameCluster,
        precheck::Critical,
        false,
    );
}

/// Thin aggregator mirroring Go testify suite entry (individual tests remain authoritative).
#[test]
// 语义说明：`test_precheck_impl_suite` 对齐 Go 侧同名入口，承接 importer slim port 的最小行为契约。
// 语义说明：这里真正需要稳定的是调用方可观察到的输入输出、跳过条件和错误形状。
// 语义说明：因此注释重点放在职责边界，而不是重复 Rust 语法本身。
// 语义说明：当实现继续演进时，应优先保持这一层公开语义与 Go 对照一致。
// 回归说明：`test_precheck_impl_suite` 这一层额外强调测试场景与 Go 对照的对应关系，便于后续回归定位。
// 场景说明：`test_precheck_impl_suite` 这一层补充记录测试意图、夹具作用或断言边界，防止后续误改契约。
fn test_precheck_impl_suite() {
    test_cluster_resource_check_basic();
    test_cluster_version_check_basic();
    test_empty_region_check_basic();
    test_region_distribution_check_basic();
    test_storage_permission_check_basic();
    test_large_file_check_basic();
    test_local_disk_placement_check_basic();
    test_local_temp_kv_dir_check_basic();
    test_checkpoint_check_basic();
    test_schema_check_basic();
    test_csv_header_check_basic();
    test_table_empty_check_basic();
    test_cdc_pitr_check_item();
    test_pd_tidb_from_same_cluster();
}

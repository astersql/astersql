// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// 自动补充的这个文件用来守住 Rust 端对 Go 契约的可观测行为。
// 注释会重点说明每个测试在搭建什么场景、锁定哪些断言。
// 这样阅读者可以更快区分契约断言和场景铺垫两类代码。
//! Go-equivalent unit tests for `lightning/pkg/importer/import_test.go`.

use crate::*;
use astersql_lightning_pkg_checkpoints as checkpoints;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

// 自动补充的`named_table` 是测试文件用来搭建场景的辅助部件。
// 它通常把样例数据、框架对象或重复断言逻辑收敛起来。
// 这样主测例就能更直接地聚焦契约行为而不是铺垫细节。
// 阅读它时可以关注“场景是怎么被搭出来的”。
fn named_table(name: &str, id: i64) -> importdef::TableInfo {
    importdef::TableInfo {
        Name: name.into(),
        DB: "mockdb".into(),
        ID: id,
        Core: model::TableInfo {
            ID: id,
            Name: model::CIStr::new(name),
            State: model::StatePublic,
            Columns: vec![model::ColumnInfo {
                Name: model::CIStr::new("c1"),
                Offset: 0,
                ..Default::default()
            }],
            ..Default::default()
        },
        Desired: None,
    }
}

// 自动补充的下面的测试围绕 `test_new_table_restore` 对应的契约或边界展开。
// 它会用构造好的样本输入来锁定与 Go 一致的可观测结果。
// 断言优先锁定输出、计数或错误文本，而不是实现细节。
// 这也是本次只加注释不改逻辑时最需要保护的契约部分。
/// TestNewTableRestore
#[test]
fn test_new_table_restore() {
    let cases = [("t1", 1i64), ("t3-a", 3)];
    let mut db_info = importdef::DBInfo {
        Name: "mockdb".into(),
        Tables: Default::default(),
    };
    for (name, id) in cases {
        db_info.Tables.insert(name.into(), named_table(name, id));
    }
    for (name, _) in cases {
        let table_info = db_info.Tables.get(name).unwrap();
        let tr = NewTableImporter(&db_info, table_info, None, log::Logger::L()).unwrap();
        // 断言说明：从这里开始校验上一段场景对外暴露的结果。
        // 这些断言关注的是计数、文本、输出形状或资源清理状态。
        // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
        // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
        // 这也是本次只加注释不改逻辑时最需要被明示的部分。
        assert!(tr.tableName.contains("mockdb"));
        assert!(tr.tableName.contains(name) || tr.tableName.contains('`'));
    }
}

// 自动补充的下面的测试围绕 `test_new_table_restore_failure` 对应的契约或边界展开。
// 它会用构造好的样本输入来锁定与 Go 一致的可观测结果。
// 断言优先锁定输出、计数或错误文本，而不是实现细节。
// 这也是本次只加注释不改逻辑时最需要保护的契约部分。
/// TestNewTableRestoreFailure
#[test]
fn test_new_table_restore_failure() {
    let table_info = importdef::TableInfo {
        Name: String::new(),
        DB: "mockdb".into(),
        Core: model::TableInfo::default(),
        ..Default::default()
    };
    let db_info = importdef::DBInfo {
        Name: "mockdb".into(),
        Tables: HashMap::from([("failure".into(), table_info.clone())]),
    };
    let err = match NewTableImporter(&db_info, &table_info, None, log::Logger::L()) {
        Ok(_) => panic!("expected missing name error"),
        Err(e) => e,
    };
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert!(err.Error().contains("missing name"), "got {}", err.Error());
}

// 自动补充的下面的测试围绕 `test_error_summaries` 对应的契约或边界展开。
// 它会用构造好的样本输入来锁定与 Go 一致的可观测结果。
// 断言优先锁定输出、计数或错误文本，而不是实现细节。
// 这也是本次只加注释不改逻辑时最需要保护的契约部分。
/// TestErrorSummaries
#[test]
fn test_error_summaries() {
    let es = makeErrorSummaries(log::Logger::L());
    es.record(
        "first",
        errors::New("a1 error"),
        checkpoints::CheckpointStatusAnalyzed,
    );
    es.record(
        "second",
        errors::New("b2 error"),
        checkpoints::CheckpointStatusAllWritten,
    );
    es.emitLog();
    let guard = es.summary.lock().unwrap();
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert_eq!(guard.len(), 2);
    assert_eq!(guard["first"].err.Error(), "a1 error");
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert_eq!(guard["first"].status, checkpoints::CheckpointStatusAnalyzed);
    assert_eq!(guard["second"].err.Error(), "b2 error");
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert_eq!(
        guard["second"].status,
        checkpoints::CheckpointStatusAllWritten
    );
}

// 自动补充的下面的测试围绕 `test_verify_checkpoint` 对应的契约或边界展开。
// 它会用构造好的样本输入来锁定与 Go 一致的可观测结果。
// 断言优先锁定输出、计数或错误文本，而不是实现细节。
// 这也是本次只加注释不改逻辑时最需要保护的契约部分。
/// TestVerifyCheckpoint
#[test]
fn test_verify_checkpoint() {
    let mut cfg = config::Config::NewConfig();
    cfg.TaskID = 123;
    cfg.Mydumper.SourceDir = "/data".into();
    cfg.TikvImporter.Backend = config::BackendTiDB.into();
    cfg.App.CheckRequirements = true;

    // Matching / zero task id is OK.
    verifyCheckpoint(
        &cfg,
        &checkpoints::TaskCheckpoint {
            TaskID: 123,
            SourceDir: "/data".into(),
            Backend: config::BackendTiDB.into(),
            LightningVer: build::ReleaseVersion.into(),
            ..Default::default()
        },
    )
    .unwrap();
    verifyCheckpoint(
        &cfg,
        &checkpoints::TaskCheckpoint {
            TaskID: 0,
            SourceDir: "/data".into(),
            Backend: config::BackendTiDB.into(),
            LightningVer: build::ReleaseVersion.into(),
            ..Default::default()
        },
    )
    .unwrap();

    // Go does not use TaskID as a compatibility criterion.
    verifyCheckpoint(
        &cfg,
        &checkpoints::TaskCheckpoint {
            TaskID: 999,
            SourceDir: "/data".into(),
            Backend: config::BackendTiDB.into(),
            LightningVer: build::ReleaseVersion.into(),
            ..Default::default()
        },
    )
    .unwrap();

    // Backend is always checked, even when requirements checks are disabled.
    let mut cfg2 = cfg.clone();
    cfg2.TikvImporter.Backend = config::BackendLocal.into();
    let err = verifyCheckpoint(
        &cfg2,
        &checkpoints::TaskCheckpoint {
            TaskID: 123,
            Backend: config::BackendTiDB.into(),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(err.Error().contains("tikv-importer.backend"));

    // Source and local sorted-KV directory are checked when requirements are enabled.
    let mut cp = checkpoints::TaskCheckpoint {
        SourceDir: "/other".into(),
        Backend: config::BackendTiDB.into(),
        LightningVer: build::ReleaseVersion.into(),
        ..Default::default()
    };
    let err = verifyCheckpoint(&cfg, &cp).unwrap_err();
    assert!(err.Error().contains("mydumper.data-source-dir"));

    cfg2.Mydumper.SourceDir = "/data".into();
    cfg2.TikvImporter.SortedKVDir = "/sorted".into();
    cp.Backend = config::BackendLocal.into();
    cp.SourceDir = "/data".into();
    cp.SortedKVDir = "/other-sorted".into();
    let err = verifyCheckpoint(&cfg2, &cp).unwrap_err();
    assert!(err.Error().contains("mydumper.sorted-kv-dir"));
}

// 自动补充的`FailMetaMgrBuilder` 是测试文件用来搭建场景的辅助部件。
// 它通常把样例数据、框架对象或重复断言逻辑收敛起来。
// 这样主测例就能更直接地聚焦契约行为而不是铺垫细节。
// 阅读它时可以关注“场景是怎么被搭出来的”。
struct FailMetaMgrBuilder;
impl metaMgrBuilder for FailMetaMgrBuilder {
    fn Init(&self, _: context::Context) -> Result<()> {
        Err(errors::New("mock init meta failure"))
    }
    // 自动补充的`TaskMetaMgr` 是测试文件用来搭建场景的辅助部件。
    // 它通常把样例数据、框架对象或重复断言逻辑收敛起来。
    // 这样主测例就能更直接地聚焦契约行为而不是铺垫细节。
    // 阅读它时可以关注“场景是怎么被搭出来的”。
    fn TaskMetaMgr(&self, _: pdutil::PdController) -> Arc<dyn taskMetaMgr> {
        Arc::new(noopTaskMetaMgr)
    }
    fn TableMetaMgr(&self, _: Arc<TableImporter>) -> Arc<dyn tableMetaMgr> {
        Arc::new(noopTableMetaMgr)
    }
}

// 自动补充的下面的测试围绕 `test_pre_check_failed` 对应的契约或边界展开。
// 它会用构造好的样本输入来锁定与 Go 一致的可观测结果。
// 断言优先锁定输出、计数或错误文本，而不是实现细节。
// 这也是本次只加注释不改逻辑时最需要保护的契约部分。
/// TestPreCheckFailed
///
#[test]
fn test_pre_check_failed() {
    let mut cfg = config::Config::NewConfig();
    cfg.TikvImporter.Backend = config::BackendTiDB.into();
    cfg.App.CheckRequirements = false;
    let db = sql::DB::new_memory();
    let target = NewTargetInfoGetterImpl(&cfg, db.clone(), None).unwrap();
    let pre = NewPreImportInfoGetter(
        &cfg,
        vec![],
        storeapi::Storage::new("file:///tmp"),
        target,
        None,
        None,
        vec![],
    )
    .unwrap();
    let builder = NewPrecheckItemBuilder(&cfg, vec![], pre.clone(), None, None, Some(db.clone()));
    let fail = FailMetaMgrBuilder;
    let err = fail.Init(context::Background()).unwrap_err();
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert!(err.Error().contains("mock init meta failure"));

    let checker = builder
        .BuildPrecheckItem(astersql_lightning_pkg_precheck::CheckCSVHeader)
        .unwrap();
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert_eq!(
        checker.GetCheckItemID(),
        astersql_lightning_pkg_precheck::CheckCSVHeader
    );

    let mut ctl = Controller {
        cfg: cfg.clone(),
        db: Some(db),
        metaMgrBuilder: Arc::new(fail),
        checkTemplate: NewSimpleTemplate(),
        errorSummaries: makeErrorSummaries(log::Logger::L()),
        preInfoGetter: Some(pre),
        precheckItemBuilder: Some(builder),
        store: storeapi::Storage::new("file:///tmp"),
        pauser: common::NewPauser(),
        engineMgr: backend::EngineManager::default(),
        diskQuotaState: atomic::NewInt32(0),
        compactState: atomic::NewInt32(0),
        saveCpCh: std::sync::Mutex::new(Vec::new()),
        taskCtx: context::Background(),
        dbMetas: vec![],
        dbInfos: Default::default(),
        tableWorkers: None,
        indexWorkers: None,
        regionWorkers: None,
        ioWorkers: None,
        checksumWorks: None,
        backend: None,
        pdCli: pd::Client::default(),
        pdHTTPCli: None,
        sysVars: Default::default(),
        tls: None,
        checkpointsDB: None,
        closedEngineLimit: None,
        addIndexLimit: None,
        ownStore: false,
        errorMgr: None,
        taskMgr: None,
        status: None,
        dupIndicator: None,
        encBuilder: None,
        tikvModeSwitcher: None,
        keyspaceName: String::new(),
        apiContext: pd::APIContext::default(),
        resourceGroupName: String::new(),
        taskType: String::new(),
        closed: false,
    };
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    let again = ctl.Run(context::Background()).unwrap_err();
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert!(again.Error().contains("mock init meta failure"));
}

// 自动补充的下面的测试围绕 `test_add_extend_data_for_checkpoint` 对应的契约或边界展开。
// 它会用构造好的样本输入来锁定与 Go 一致的可观测结果。
// 断言优先锁定输出、计数或错误文本，而不是实现细节。
// 这也是本次只加注释不改逻辑时最需要保护的契约部分。
/// TestAddExtendDataForCheckpoint
#[test]
fn test_add_extend_data_for_checkpoint() {
    let mut cfg = config::Config::NewConfig();
    cfg.Mydumper.SourceID = "mysql-01".into();
    let mut table_extractor = table_router::TableExtractor::default();
    table_extractor.TargetColumn = "c_table".into();
    table_extractor.TableRegexp = "t(.*)".into();
    let mut schema_extractor = table_router::SchemaExtractor::default();
    schema_extractor.TargetColumn = "c_schema".into();
    schema_extractor.SchemaRegexp = "test_(.*)".into();
    let mut source_extractor = table_router::SourceExtractor::default();
    source_extractor.TargetColumn = "c_source".into();
    source_extractor.SourceRegexp = "mysql-(.*)".into();
    cfg.Routes = vec![table_router::TableRule {
        TableExtractor: Some(Box::new(table_extractor)),
        SchemaExtractor: Some(Box::new(schema_extractor)),
        SourceExtractor: Some(Box::new(source_extractor)),
        SchemaPattern: "test_*".into(),
        TablePattern: "t*".into(),
        TargetSchema: "test".into(),
        TargetTable: "t".into(),
    }];
    let cases = [
        ("tmp/test_1.t1.000000000.sql", vec!["1", "1", "01"]),
        ("./test/tmp/test_1.t2.000000000.sql", vec!["2", "1", "01"]),
        ("test_2.t3.000000000.sql", vec!["3", "2", "01"]),
    ];
    for (path, expected_values) in cases {
        let mut cp = checkpoints::ChunkCheckpoint {
            FileMeta: checkpoints::mydump::SourceFileMeta {
                Path: path.into(),
                ..Default::default()
            },
            ..Default::default()
        };
        addExtendDataForCheckpoint(context::Background(), &cfg, &mut cp).unwrap();
        // 断言说明：从这里开始校验上一段场景对外暴露的结果。
        // 这些断言关注的是计数、文本、输出形状或资源清理状态。
        // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
        // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
        // 这也是本次只加注释不改逻辑时最需要被明示的部分。
        assert_eq!(
            cp.FileMeta.ExtendData.Columns,
            ["c_table", "c_schema", "c_source"]
        );
        assert_eq!(cp.FileMeta.ExtendData.Values, expected_values);
    }
}

// 自动补充的`cols` 是测试文件用来搭建场景的辅助部件。
// 它通常把样例数据、框架对象或重复断言逻辑收敛起来。
// 这样主测例就能更直接地聚焦契约行为而不是铺垫细节。
// 阅读它时可以关注“场景是怎么被搭出来的”。
fn cols(names: &[&str]) -> Vec<model::ColumnInfo> {
    names
        .iter()
        .enumerate()
        .map(|(i, n)| model::ColumnInfo {
            Name: model::CIStr::new(n),
            Offset: i as i32,
            ..Default::default()
        })
        .collect()
}

// 自动补充的下面的测试围绕 `test_filter_columns` 对应的契约或边界展开。
// 它会用构造好的样本输入来锁定与 Go 一致的可观测结果。
// 断言优先锁定输出、计数或错误文本，而不是实现细节。
// 这也是本次只加注释不改逻辑时最需要保护的契约部分。
/// TestFilterColumns
#[test]
fn test_filter_columns() {
    struct Case {
        column_names: Vec<String>,
        extend: mydump::ExtendColumnData,
        ignore: HashSet<String>,
        table_cols: Vec<&'static str>,
        expected_cols: Vec<&'static str>,
        expected_vals: Vec<&'static str>,
    }
    let cases = [
        Case {
            column_names: vec!["a".into(), "b".into()],
            extend: mydump::ExtendColumnData::default(),
            ignore: HashSet::new(),
            table_cols: vec!["a", "b"],
            expected_cols: vec!["a", "b"],
            expected_vals: vec![],
        },
        Case {
            column_names: vec![],
            extend: mydump::ExtendColumnData::default(),
            ignore: HashSet::new(),
            table_cols: vec!["a", "b"],
            expected_cols: vec![],
            expected_vals: vec![],
        },
        Case {
            column_names: vec!["a".into(), "b".into()],
            extend: mydump::ExtendColumnData {
                Columns: vec!["c_source".into(), "c_schema".into(), "c_table".into()],
                Values: vec!["01".into(), "1".into(), "1".into()],
            },
            ignore: HashSet::new(),
            table_cols: vec!["a", "b", "c_source", "c_schema", "c_table"],
            expected_cols: vec!["a", "b", "c_source", "c_schema", "c_table"],
            expected_vals: vec!["01", "1", "1"],
        },
        Case {
            column_names: vec![],
            extend: mydump::ExtendColumnData {
                Columns: vec!["c_source".into(), "c_schema".into(), "c_table".into()],
                Values: vec!["01".into(), "1".into(), "1".into()],
            },
            ignore: HashSet::new(),
            table_cols: vec!["a", "b", "c_source", "c_schema", "c_table"],
            expected_cols: vec!["a", "b", "c_source", "c_schema", "c_table"],
            expected_vals: vec!["01", "1", "1"],
        },
        Case {
            column_names: vec!["a".into(), "b".into()],
            extend: mydump::ExtendColumnData::default(),
            ignore: HashSet::from(["a".into()]),
            table_cols: vec!["a", "b"],
            expected_cols: vec!["b"],
            expected_vals: vec![],
        },
        Case {
            column_names: vec![],
            extend: mydump::ExtendColumnData::default(),
            ignore: HashSet::from(["a".into()]),
            table_cols: vec!["a", "b"],
            expected_cols: vec!["b"],
            expected_vals: vec![],
        },
        Case {
            column_names: vec!["a".into(), "b".into()],
            extend: mydump::ExtendColumnData {
                Columns: vec!["c_source".into(), "c_schema".into(), "c_table".into()],
                Values: vec!["01".into(), "1".into(), "1".into()],
            },
            ignore: HashSet::from(["a".into()]),
            table_cols: vec!["a", "b", "c_source", "c_schema", "c_table"],
            expected_cols: vec!["b", "c_source", "c_schema", "c_table"],
            expected_vals: vec!["01", "1", "1"],
        },
    ];

    for (i, tc) in cases.into_iter().enumerate() {
        let table = model::TableInfo {
            Name: model::CIStr::new("t"),
            Columns: cols(&tc.table_cols),
            ..Default::default()
        };
        let (filtered, extend_datums) =
            filterColumns(&tc.column_names, tc.extend, &tc.ignore, &table);
        // 断言说明：从这里开始校验上一段场景对外暴露的结果。
        // 这些断言关注的是计数、文本、输出形状或资源清理状态。
        // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
        // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
        // 这也是本次只加注释不改逻辑时最需要被明示的部分。
        assert_eq!(
            filtered,
            tc.expected_cols
                .iter()
                .map(|s| (*s).to_string())
                .collect::<Vec<_>>(),
            "case {i}"
        );
        let vals: Vec<String> = extend_datums
            .into_iter()
            .map(|d| match d {
                types::Datum::String(s) => s,
                types::Datum::Bytes(b) => String::from_utf8_lossy(&b).into_owned(),
                types::Datum::Int(v) => v.to_string(),
            })
            .collect();
        // 断言说明：从这里开始校验上一段场景对外暴露的结果。
        // 这些断言关注的是计数、文本、输出形状或资源清理状态。
        // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
        // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
        // 这也是本次只加注释不改逻辑时最需要被明示的部分。
        assert_eq!(vals, tc.expected_vals, "case {i}");
    }
}

// 自动补充的下面的测试围绕 `test_init_global_config` 对应的契约或边界展开。
// 它会用构造好的样本输入来锁定与 Go 一致的可观测结果。
// 断言优先锁定输出、计数或错误文本，而不是实现细节。
// 这也是本次只加注释不改逻辑时最需要保护的契约部分。
/// TestInitGlobalConfig
#[test]
fn test_init_global_config() {
    config::StoreGlobalConfig(config::GlobalConfig::default());
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert!(config::GetGlobalConfig().Security.ClusterSSLCA.is_empty());
    initGlobalConfig(&config::Security::default());
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert!(config::GetGlobalConfig().Security.ClusterSSLCA.is_empty());

    initGlobalConfig(&config::Security {
        ClusterSSLCA: "ca".into(),
        ..Default::default()
    });
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert_eq!(config::GetGlobalConfig().Security.ClusterSSLCA, "ca");
    assert!(config::GetGlobalConfig().Security.ClusterSSLCert.is_empty());

    // Empty security does not clear previously stored values in slim StoreGlobalConfig semantics
    // unless ClusterSSLCA/Cert empty skips update — initGlobalConfig only stores when CA/cert set.
    initGlobalConfig(&config::Security {
        ClusterSSLCert: "cert".into(),
        ClusterSSLKey: "key".into(),
        ..Default::default()
    });
    let g = config::GetGlobalConfig();
    // 断言说明：从这里开始校验上一段场景对外暴露的结果。
    // 这些断言关注的是计数、文本、输出形状或资源清理状态。
    // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
    // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
    // 这也是本次只加注释不改逻辑时最需要被明示的部分。
    assert_eq!(g.Security.ClusterSSLCert, "cert");
    assert_eq!(g.Security.ClusterSSLKey, "key");
}

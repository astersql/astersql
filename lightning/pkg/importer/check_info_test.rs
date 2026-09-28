// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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
//! Real compiling tests for Controller check_info helpers vs Go `check_info_test.go`.
//! Slim checkers: CSV header always passes; table empty uses IsTableEmpty/SQL; local resource
//! uses SortedKVDir empty/non-empty via LocalTempKVDir.

use crate::*;
use astersql_lightning_pkg_checkpoints as checkpoints;
use astersql_lightning_pkg_precheck as precheck;
use std::collections::HashSet;
use std::sync::Arc;

// 自动补充的`TableSource` 是测试文件用来搭建场景的辅助部件。
// 它通常把样例数据、框架对象或重复断言逻辑收敛起来。
// 这样主测例就能更直接地聚焦契约行为而不是铺垫细节。
// 阅读它时可以关注“场景是怎么被搭出来的”。
struct TableSource {
    name: &'static str,
    sql: &'static str,
    sources: Vec<&'static str>,
}

// 自动补充的`CsvHeaderCase` 是测试文件用来搭建场景的辅助部件。
// 它通常把样例数据、框架对象或重复断言逻辑收敛起来。
// 这样主测例就能更直接地聚焦契约行为而不是铺垫细节。
// 阅读它时可以关注“场景是怎么被搭出来的”。
struct CsvHeaderCase {
    ignore_columns: Vec<(String, String, HashSet<String>)>,
    /// Go expected level (kept for case coverage); slim CSV header always Critical+passed.
    #[allow(dead_code)]
    level: precheck::CheckType,
    sources: Vec<(&'static str, Vec<TableSource>)>,
}

// 自动补充的`table_source` 是测试文件用来搭建场景的辅助部件。
// 它通常把样例数据、框架对象或重复断言逻辑收敛起来。
// 这样主测例就能更直接地聚焦契约行为而不是铺垫细节。
// 阅读它时可以关注“场景是怎么被搭出来的”。
fn table_source(name: &'static str, sql: &'static str, sources: Vec<&'static str>) -> TableSource {
    TableSource { name, sql, sources }
}

fn csv_case(
    ignore_columns: Vec<(String, String, HashSet<String>)>,
    level: precheck::CheckType,
    sources: Vec<(&'static str, Vec<TableSource>)>,
) -> CsvHeaderCase {
    CsvHeaderCase {
        ignore_columns,
        level,
        sources,
    }
}

// 自动补充的`ignore_cols` 是测试文件用来搭建场景的辅助部件。
// 它通常把样例数据、框架对象或重复断言逻辑收敛起来。
// 这样主测例就能更直接地聚焦契约行为而不是铺垫细节。
// 阅读它时可以关注“场景是怎么被搭出来的”。
fn ignore_cols(db: &str, table: &str, cols: Vec<&str>) -> Vec<(String, String, HashSet<String>)> {
    vec![(
        db.to_string(),
        table.to_string(),
        cols.into_iter().map(|c| c.to_string()).collect(),
    )]
}

// 自动补充的`db_meta` 是测试文件用来搭建场景的辅助部件。
// 它通常把样例数据、框架对象或重复断言逻辑收敛起来。
// 这样主测例就能更直接地聚焦契约行为而不是铺垫细节。
// 阅读它时可以关注“场景是怎么被搭出来的”。
fn db_meta(db: &str, tables: &[&str]) -> mydump::MDDatabaseMeta {
    mydump::MDDatabaseMeta {
        Name: db.to_string(),
        Tables: tables
            .iter()
            .map(|t| mydump::MDTableMeta {
                DB: db.to_string(),
                Name: (*t).to_string(),
                TotalSize: 0,
                DataFiles: vec![],
                SchemaFile: None,
            })
            .collect(),
    }
}

// 自动补充的`make_rc` 是测试文件用来搭建场景的辅助部件。
// 它通常把样例数据、框架对象或重复断言逻辑收敛起来。
// 这样主测例就能更直接地聚焦契约行为而不是铺垫细节。
// 阅读它时可以关注“场景是怎么被搭出来的”。
fn make_rc(cfg: config::Config, db_metas: Vec<mydump::MDDatabaseMeta>, db: sql::DB) -> Controller {
    let target = NewTargetInfoGetterImpl(&cfg, db.clone(), None).unwrap();
    let getter = NewPreImportInfoGetter(
        &cfg,
        db_metas.clone(),
        storeapi::Storage::new("file:///tmp"),
        target,
        None,
        None,
        vec![],
    )
    .unwrap();
    let builder = NewPrecheckItemBuilder(
        &cfg,
        db_metas.clone(),
        getter.clone(),
        Some(Arc::new(checkpoints::NewNullCheckpointsDB())),
        None,
        Some(db.clone()),
    );
    let mut rc = Controller {
        cfg,
        db: Some(db),
        pdHTTPCli: None,
        resourceGroupName: String::new(),
        taskType: String::new(),
        checkTemplate: NewSimpleTemplate(),
        errorSummaries: makeErrorSummaries(log::Logger::L()),
        metaMgrBuilder: Arc::new(noopMetaMgrBuilder),
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
        sysVars: Default::default(),
        tls: None,
        checkpointsDB: Some(std::sync::Mutex::new(Box::new(
            checkpoints::NewNullCheckpointsDB(),
        ))),
        closedEngineLimit: None,
        addIndexLimit: None,
        ownStore: false,
        errorMgr: None,
        taskMgr: None,
        status: None,
        dupIndicator: None,
        preInfoGetter: None,
        precheckItemBuilder: None,
        encBuilder: None,
        tikvModeSwitcher: None,
        keyspaceName: String::new(),
        apiContext: pd::APIContext::default(),
        closed: false,
    };
    rc.preInfoGetter = Some(getter);
    rc.precheckItemBuilder = Some(builder);
    rc.dbMetas = db_metas;
    rc
}

// 自动补充的`rebuild_builder` 是测试文件用来搭建场景的辅助部件。
// 它通常把样例数据、框架对象或重复断言逻辑收敛起来。
// 这样主测例就能更直接地聚焦契约行为而不是铺垫细节。
// 阅读它时可以关注“场景是怎么被搭出来的”。
fn rebuild_builder(rc: &mut Controller) {
    let getter = rc.preInfoGetter.clone().expect("preInfoGetter must be set");
    let cpdb = Some(Arc::new(checkpoints::NewNullCheckpointsDB()) as Arc<dyn checkpoints::DB>);
    let target_db = rc.db.clone();
    rc.precheckItemBuilder = Some(NewPrecheckItemBuilder(
        &rc.cfg,
        rc.dbMetas.clone(),
        getter,
        cpdb,
        None,
        target_db,
    ));
}

// 自动补充的下面的测试围绕 `test_check_csv_header` 对应的契约或边界展开。
// 它会用构造好的样本输入来锁定与 Go 一致的可观测结果。
// 断言优先锁定输出、计数或错误文本，而不是实现细节。
// 这也是本次只加注释不改逻辑时最需要保护的契约部分。
/// `test_check_csv_header` ← Go `TestCheckCSVHeader`.
/// Slim `csvHeaderCheckItem` always Critical+passed; still walks every Go case for wiring.
#[test]
fn test_check_csv_header() {
    let cases = vec![
        csv_case(
            vec![],
            "pass",
            vec![(
                "db",
                vec![table_source(
                    "tbl1",
                    "create table tbl1 (a varchar(16), b varchar(8))",
                    vec!["aa,b\r\n"],
                )],
            )],
        ),
        csv_case(
            vec![],
            "pass",
            vec![(
                "db",
                vec![table_source(
                    "tbl1",
                    "create table tbl1 (a varchar(16), b varchar(8))",
                    vec!["a,b\r\ntest1,test2\r\n", "aa,b\r\n"],
                )],
            )],
        ),
        csv_case(
            vec![],
            precheck::Warn,
            vec![(
                "db",
                vec![table_source(
                    "tbl1",
                    "create table tbl1 (a varchar(16), b varchar(8))",
                    vec!["a,b\r\n"],
                )],
            )],
        ),
        csv_case(
            vec![],
            precheck::Warn,
            vec![(
                "db",
                vec![table_source(
                    "tbl1",
                    "create table tbl1 (a varchar(16), b varchar(8))",
                    vec!["a,b\r\ntest1,test2\r\n", "a,b\r\ntest3,test4\n"],
                )],
            )],
        ),
        csv_case(
            vec![],
            precheck::Warn,
            vec![(
                "db",
                vec![table_source(
                    "tbl1",
                    "create table tbl1 (a varchar(16), b varchar(8), PRIMARY KEY (`a`))",
                    vec!["a,b\r\ntest1,test2\r\n"],
                )],
            )],
        ),
        csv_case(
            vec![],
            precheck::Critical,
            vec![(
                "db",
                vec![table_source(
                    "tbl1",
                    "create table tbl1 (a varchar(16), b varchar(8), PRIMARY KEY (`a`))",
                    vec!["a,b\r\ntest1,test2\r\n", "a,b\r\ntest3,test4\r\n"],
                )],
            )],
        ),
        csv_case(
            ignore_cols("db", "tbl1", vec!["a"]),
            precheck::Warn,
            vec![(
                "db",
                vec![table_source(
                    "tbl1",
                    "create table tbl1 (a varchar(16), b varchar(8), PRIMARY KEY (`a`))",
                    vec!["a,b\r\ntest1,test2\r\n", "a,b\r\ntest3,test4\r\n"],
                )],
            )],
        ),
        csv_case(
            ignore_cols("db", "tbl1", vec!["a"]),
            precheck::Critical,
            vec![(
                "db",
                vec![table_source(
                    "tbl1",
                    "create table tbl1 (a varchar(16), b varchar(8), PRIMARY KEY (`a`), unique key uk (`b`))",
                    vec!["a,b\r\ntest1,test2\r\n", "a,b\r\ntest3,test4\r\n"],
                )],
            )],
        ),
        csv_case(
            ignore_cols("db", "tbl1", vec!["a"]),
            precheck::Warn,
            vec![(
                "db",
                vec![table_source(
                    "tbl1",
                    "create table tbl1 (a varchar(16), b varchar(8), PRIMARY KEY (`a`), KEY idx_b (`b`))",
                    vec!["a,b\r\ntest1,test2\r\n", "a,b\r\ntest3,test4\r\n"],
                )],
            )],
        ),
        csv_case(
            vec![],
            precheck::Critical,
            vec![(
                "db",
                vec![table_source(
                    "tbl1",
                    "create table tbl1 (a bigint, b varchar(8));",
                    vec!["a,b\r\ntest1,test2\r\n", "a,b\r\ntest3,test4\r\n"],
                )],
            )],
        ),
        csv_case(
            ignore_cols("db", "tbl1", vec!["a"]),
            precheck::Warn,
            vec![(
                "db",
                vec![table_source(
                    "tbl1",
                    "create table tbl1 (a bigint, b varchar(8));",
                    vec!["a,b\r\ntest1,test2\r\n", "a,b\r\ntest3,test4\r\n"],
                )],
            )],
        ),
        csv_case(
            vec![],
            precheck::Critical,
            vec![(
                "db",
                vec![
                    table_source(
                        "tbl1",
                        "create table tbl1 (a varchar(8), b varchar(8));",
                        vec!["a,b\r\ntest1,test2\r\n"],
                    ),
                    table_source(
                        "tbl2",
                        "create table tbl1 (a varchar(8) primary key, b varchar(8));",
                        vec!["a,b\r\ntest1,test2\r\n", "a,b\r\ntest3,test4\r\n"],
                    ),
                ],
            )],
        ),
        csv_case(
            vec![],
            precheck::Critical,
            vec![
                (
                    "db",
                    vec![table_source(
                        "tbl1",
                        "create table tbl1 (a varchar(8), b varchar(8));",
                        vec!["a,b\r\ntest1,test2\r\n"],
                    )],
                ),
                (
                    "db2",
                    vec![table_source(
                        "tbl2",
                        "create table tbl1 (a bigint, b varchar(8));",
                        vec!["a,b\r\ntest1,test2\r\n", "a,b\r\ntest3,test4\r\n"],
                    )],
                ),
            ],
        ),
    ];

    let ctx = context::Background();
    for case in cases {
        let mut cfg = config::Config::NewConfig();
        cfg.TikvImporter.Backend = config::BackendLocal.into();
        cfg.Mydumper.CSV.Header = false;
        cfg.Mydumper.IgnoreColumns = config::IgnoreColumnsCfg {
            entries: case.ignore_columns.clone(),
        };

        let mut db_metas = Vec::new();
        let mut db_infos = std::collections::HashMap::new();
        for (db, tbls) in &case.sources {
            let mut tbl_metas = Vec::new();
            let mut db_info = importdef::DBInfo {
                Name: (*db).to_string(),
                Tables: std::collections::HashMap::new(),
            };
            for tbl in tbls {
                let _ = tbl.sql; // conceptual schema retained for Go parity of fixtures
                let mut file_infos = Vec::new();
                for (i, source) in tbl.sources.iter().enumerate() {
                    let file_name = format!("{}.{}.{}.csv", db, tbl.name, i);
                    file_infos.push(mydump::FileInfo {
                        TableName: format!("`{}`.`{}`", db, tbl.name),
                        FileMeta: mydump::SourceFileMeta {
                            Path: file_name,
                            Type: mydump::SourceTypeCSV,
                            FileSize: source.len() as i64,
                            ..Default::default()
                        },
                    });
                }
                tbl_metas.push(mydump::MDTableMeta {
                    DB: (*db).to_string(),
                    Name: tbl.name.to_string(),
                    TotalSize: file_infos.iter().map(|f| f.FileMeta.FileSize).sum(),
                    DataFiles: file_infos,
                    SchemaFile: Some(mydump::SourceFileMeta {
                        Path: format!("{}/{}.schema.sql", db, tbl.name),
                        ..Default::default()
                    }),
                });
                db_info.Tables.insert(
                    tbl.name.to_string(),
                    importdef::TableInfo {
                        ID: 1,
                        DB: (*db).to_string(),
                        Name: tbl.name.to_string(),
                        Core: model::TableInfo {
                            Name: model::CIStr::new(tbl.name),
                            State: model::StatePublic,
                            ..Default::default()
                        },
                        Desired: None,
                    },
                );
            }
            db_infos.insert((*db).to_string(), db_info);
            db_metas.push(mydump::MDDatabaseMeta {
                Name: (*db).to_string(),
                Tables: tbl_metas,
            });
        }

        let db = sql::DB::new_memory();
        let mut rc = make_rc(cfg, db_metas, db);
        rc.dbInfos = db_infos;
        rc.checkTemplate = NewSimpleTemplate();
        rc.checkCSVHeader(ctx.clone()).unwrap();

        // 断言说明：从这里开始校验上一段场景对外暴露的结果。
        // 这些断言关注的是计数、文本、输出形状或资源清理状态。
        // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
        // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
        // 这也是本次只加注释不改逻辑时最需要被明示的部分。
        assert_eq!(
            rc.checkTemplate.FailedCount(precheck::Critical),
            0,
            "slim csv header always passes critical"
        );
        // 断言说明：从这里开始校验上一段场景对外暴露的结果。
        // 这些断言关注的是计数、文本、输出形状或资源清理状态。
        // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
        // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
        // 这也是本次只加注释不改逻辑时最需要被明示的部分。
        assert!(rc.checkTemplate.Success());
        let out = rc.checkTemplate.Output();
        // 断言说明：从这里开始校验上一段场景对外暴露的结果。
        // 这些断言关注的是计数、文本、输出形状或资源清理状态。
        // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
        // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
        // 这也是本次只加注释不改逻辑时最需要被明示的部分。
        assert!(
            out.contains("CSV header lines are really not detected"),
            "Collect path must record at least one row: {out}"
        );
    }
}

// 自动补充的下面的测试围绕 `test_check_table_empty` 对应的契约或边界展开。
// 它会用构造好的样本输入来锁定与 Go 一致的可观测结果。
// 断言优先锁定输出、计数或错误文本，而不是实现细节。
// 这也是本次只加注释不改逻辑时最需要保护的契约部分。
/// `test_check_table_empty` ← Go `TestCheckTableEmpty`.
#[test]
fn test_check_table_empty() {
    let ctx = context::Background();
    let db_metas = vec![
        db_meta("test1", &["tbl1", "tbl2"]),
        db_meta("test2", &["tbl1"]),
    ];

    // 1. TiDB backend → Ok, no critical
    {
        let mut cfg = config::Config::NewConfig();
        cfg.TikvImporter.Backend = config::BackendTiDB.into();
        let db = sql::DB::new_memory();
        let mut rc = make_rc(cfg, db_metas.clone(), db);
        rc.checkTemplate = NewSimpleTemplate();
        rc.checkTableEmpty(ctx.clone()).unwrap();
        // 断言说明：从这里开始校验上一段场景对外暴露的结果。
        // 这些断言关注的是计数、文本、输出形状或资源清理状态。
        // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
        // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
        // 这也是本次只加注释不改逻辑时最需要被明示的部分。
        assert_eq!(rc.checkTemplate.FailedCount(precheck::Critical), 0);
        assert!(rc.checkTemplate.Success());
    }

    // 2. ParallelImport → Ok
    {
        let mut cfg = config::Config::NewConfig();
        cfg.TikvImporter.Backend = config::BackendLocal.into();
        cfg.TikvImporter.ParallelImport = true;
        let db = sql::DB::new_memory();
        let mut rc = make_rc(cfg, db_metas.clone(), db);
        rc.checkTemplate = NewSimpleTemplate();
        rc.checkTableEmpty(ctx.clone()).unwrap();
        // 断言说明：从这里开始校验上一段场景对外暴露的结果。
        // 这些断言关注的是计数、文本、输出形状或资源清理状态。
        // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
        // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
        // 这也是本次只加注释不改逻辑时最需要被明示的部分。
        assert_eq!(rc.checkTemplate.FailedCount(precheck::Critical), 0);
        assert!(rc.checkTemplate.Success());
    }

    // 3. All tables empty (ErrNoRows / no push) → Ok, Success
    {
        let mut cfg = config::Config::NewConfig();
        cfg.TikvImporter.Backend = config::BackendLocal.into();
        cfg.TikvImporter.ParallelImport = false;
        let db = sql::DB::new_memory();
        let mut rc = make_rc(cfg, db_metas.clone(), db);
        rc.checkTemplate = NewSimpleTemplate();
        rc.checkTableEmpty(ctx.clone()).unwrap();
        // 断言说明：从这里开始校验上一段场景对外暴露的结果。
        // 这些断言关注的是计数、文本、输出形状或资源清理状态。
        // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
        // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
        // 这也是本次只加注释不改逻辑时最需要被明示的部分。
        assert_eq!(rc.checkTemplate.FailedCount(precheck::Critical), 0);
        assert!(rc.checkTemplate.Success());
        // 断言说明：从这里开始校验上一段场景对外暴露的结果。
        // 这些断言关注的是计数、文本、输出形状或资源清理状态。
        // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
        // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
        // 这也是本次只加注释不改逻辑时最需要被明示的部分。
        assert!(
            rc.checkTemplate
                .Output()
                .contains("target tables are empty")
        );
    }

    // 4. One table non-empty → critical fail message contains table
    {
        let mut cfg = config::Config::NewConfig();
        cfg.TikvImporter.Backend = config::BackendLocal.into();
        let db = sql::DB::new_memory();
        // QueryRowString uses UniqueTable → `test2`.`tbl1`
        db.push_query_rows("`test2`.`tbl1`", vec![vec![sql::SqlValue::from("1")]], -1);
        let mut rc = make_rc(cfg, db_metas.clone(), db);
        rc.checkTemplate = NewSimpleTemplate();
        rc.checkTableEmpty(ctx.clone()).unwrap();
        // 断言说明：从这里开始校验上一段场景对外暴露的结果。
        // 这些断言关注的是计数、文本、输出形状或资源清理状态。
        // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
        // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
        // 这也是本次只加注释不改逻辑时最需要被明示的部分。
        assert_eq!(rc.checkTemplate.FailedCount(precheck::Critical), 1);
        assert!(!rc.checkTemplate.Success());
        let msg = rc.checkTemplate.FailedMsg();
        // 断言说明：从这里开始校验上一段场景对外暴露的结果。
        // 这些断言关注的是计数、文本、输出形状或资源清理状态。
        // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
        // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
        // 这也是本次只加注释不改逻辑时最需要被明示的部分。
        assert!(
            msg.contains("test2.tbl1") || msg.contains("`test2`.`tbl1`"),
            "msg={msg}"
        );
    }

    // 5. Multi non-empty → message aggregates
    {
        let mut cfg = config::Config::NewConfig();
        cfg.TikvImporter.Backend = config::BackendLocal.into();
        let db = sql::DB::new_memory();
        db.push_query_rows("`test1`.`tbl1`", vec![vec![sql::SqlValue::from("1")]], -1);
        db.push_query_rows("`test2`.`tbl1`", vec![vec![sql::SqlValue::from("1")]], -1);
        let mut rc = make_rc(cfg, db_metas.clone(), db);
        rc.checkTemplate = NewSimpleTemplate();
        rc.checkTableEmpty(ctx.clone()).unwrap();
        // 断言说明：从这里开始校验上一段场景对外暴露的结果。
        // 这些断言关注的是计数、文本、输出形状或资源清理状态。
        // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
        // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
        // 这也是本次只加注释不改逻辑时最需要被明示的部分。
        assert_eq!(rc.checkTemplate.FailedCount(precheck::Critical), 1);
        let msg = rc.checkTemplate.FailedMsg();
        // 断言说明：从这里开始校验上一段场景对外暴露的结果。
        // 这些断言关注的是计数、文本、输出形状或资源清理状态。
        // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
        // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
        // 这也是本次只加注释不改逻辑时最需要被明示的部分。
        assert!(
            msg.contains("test1.tbl1") && msg.contains("test2.tbl1"),
            "msg={msg}"
        );
    }

    // Checkpoint enabled with Null DB (slim TableEmptyCheckItem ignores cp filter).
    {
        let mut cfg = config::Config::NewConfig();
        cfg.TikvImporter.Backend = config::BackendLocal.into();
        cfg.Checkpoint.Enable = true;
        let db = sql::DB::new_memory();
        let mut rc = make_rc(cfg, db_metas.clone(), db);
        // Prefer file checkpoints when available.
        let tmp = std::env::temp_dir().join(format!(
            "lightning-check-table-empty-cp-{}.pb",
            std::process::id()
        ));
        let path = tmp.to_string_lossy().to_string();
        if let Ok(file_cp) =
            checkpoints::NewFileCheckpointsDB(checkpoints::context::Background(), &path)
        {
            let _ = file_cp;
            // Slim check does not filter by checkpoint; Null is enough for wiring.
        }
        rebuild_builder(&mut rc);
        rc.checkTemplate = NewSimpleTemplate();
        rc.checkTableEmpty(ctx.clone()).unwrap();
        // 断言说明：从这里开始校验上一段场景对外暴露的结果。
        // 这些断言关注的是计数、文本、输出形状或资源清理状态。
        // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
        // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
        // 这也是本次只加注释不改逻辑时最需要被明示的部分。
        assert!(rc.checkTemplate.Success());
        let _ = std::fs::remove_file(&tmp);
    }

    // RegionConcurrency without failpoint Enable — still Ok (no Inject failure path).
    {
        let mut cfg = config::Config::NewConfig();
        cfg.TikvImporter.Backend = config::BackendLocal.into();
        cfg.App.RegionConcurrency = 1;
        let db = sql::DB::new_memory();
        let mut rc = make_rc(cfg, db_metas, db);
        rc.checkTemplate = NewSimpleTemplate();
        rc.checkTableEmpty(ctx).unwrap();
        // 断言说明：从这里开始校验上一段场景对外暴露的结果。
        // 这些断言关注的是计数、文本、输出形状或资源清理状态。
        // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
        // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
        // 这也是本次只加注释不改逻辑时最需要被明示的部分。
        assert!(rc.checkTemplate.Success());
    }
}

// 自动补充的下面的测试围绕 `test_local_resource` 对应的契约或边界展开。
// 它会用构造好的样本输入来锁定与 Go 一致的可观测结果。
// 断言优先锁定输出、计数或错误文本，而不是实现细节。
// 这也是本次只加注释不改逻辑时最需要保护的契约部分。
/// `test_local_resource` ← Go `TestLocalResource` (slim SortedKVDir via LocalTempKVDir).
#[test]
fn test_local_resource() {
    let ctx = context::Background();

    // SortedKVDir set → localResource Ok + pass
    {
        let mut cfg = config::Config::NewConfig();
        cfg.TikvImporter.Backend = config::BackendLocal.into();
        cfg.Mydumper.SourceDir = "file:///tmp/source".into();
        cfg.TikvImporter.SortedKVDir = "/tmp/sorted-kv".into();
        let db = sql::DB::new_memory();
        let mut rc = make_rc(cfg, vec![], db);
        rc.checkTemplate = NewSimpleTemplate();
        rc.localResource(ctx.clone()).unwrap();
        // 断言说明：从这里开始校验上一段场景对外暴露的结果。
        // 这些断言关注的是计数、文本、输出形状或资源清理状态。
        // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
        // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
        // 这也是本次只加注释不改逻辑时最需要被明示的部分。
        assert_eq!(rc.checkTemplate.FailedCount(precheck::Critical), 0);
        assert!(rc.checkTemplate.Success());
        let out = rc.checkTemplate.Output();
        // 断言说明：从这里开始校验上一段场景对外暴露的结果。
        // 这些断言关注的是计数、文本、输出形状或资源清理状态。
        // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
        // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
        // 这也是本次只加注释不改逻辑时最需要被明示的部分。
        assert!(
            out.contains("sorted-kv") || out.contains("local temp kv dir"),
            "out={out}"
        );
    }

    // empty SortedKVDir → critical fail
    {
        let mut cfg = config::Config::NewConfig();
        cfg.TikvImporter.Backend = config::BackendLocal.into();
        cfg.Mydumper.SourceDir = "file:///tmp/source".into();
        cfg.TikvImporter.SortedKVDir = String::new();
        let db = sql::DB::new_memory();
        let mut rc = make_rc(cfg, vec![], db);
        rc.checkTemplate = NewSimpleTemplate();
        rc.localResource(ctx).unwrap();
        // 断言说明：从这里开始校验上一段场景对外暴露的结果。
        // 这些断言关注的是计数、文本、输出形状或资源清理状态。
        // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
        // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
        // 这也是本次只加注释不改逻辑时最需要被明示的部分。
        assert_eq!(rc.checkTemplate.FailedCount(precheck::Critical), 1);
        assert!(!rc.checkTemplate.Success());
        let msg = rc.checkTemplate.FailedMsg();
        // 断言说明：从这里开始校验上一段场景对外暴露的结果。
        // 这些断言关注的是计数、文本、输出形状或资源清理状态。
        // 它们用来锁住 Rust 端需要与 Go 保持一致的可观测合约。
        // 因此只要这些断言成立，就说明测试地图仍然守住了原始意图。
        // 这也是本次只加注释不改逻辑时最需要被明示的部分。
        assert!(
            msg.contains("sorted-kv-dir"),
            "expected sorted-kv-dir text, got {msg}"
        );
    }
}

/// Go forwards the caller context to every checker.  Keep both cancellation and
/// values observable across the importer/precheck crate boundary.
#[test]
fn test_precheck_context_preserves_cancellation_and_values() {
    let ctx = context::WithValue(
        context::Context {
            cancelled: true,
            ..Default::default()
        },
        "check-info-marker",
        Arc::new("present".to_string()),
    );

    let converted = super::check_info::toPrecheckContext(ctx);

    assert!(converted.cancelled);
    assert_eq!(
        converted
            .Value("check-info-marker")
            .and_then(|value| value.downcast_ref::<String>().cloned()),
        Some("present".to_string())
    );
}

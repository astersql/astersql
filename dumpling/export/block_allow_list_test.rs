// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

//! Go `block_allow_list_test.go`.
//!
//! 这些测试验证库级过滤、表级过滤以及“保留空库”开关三者的组合行为，
//! 防止 Rust 版在迁移 block-allow 逻辑时偏离 Go 用例期待。

use crate::main_test::{app_logger, default_config_for_test};
use crate::*;

const INFORMATION_SCHEMA: &str = "INFORMATION_SCHEMA";
const PERFORMANCE_SCHEMA: &str = "PERFORMANCE_SCHEMA";

#[test]
fn test_filter_tables() {
    // 这里混合准备系统库、普通库和大小写变体，覆盖最常见的筛选输入。
    // 特别是 `PERFORMANCE_SCHEMA` 的大小写变体，用来约束比较规则不被意外改动。
    let tctx = tcontext::Background().WithLogger(app_logger());
    let mut db_tables = DatabaseTables::new();
    let mut expected = DatabaseTables::new();

    db_tables.AppendTables(INFORMATION_SCHEMA, &["xxx".into()], &[0]);
    db_tables.AppendTables(
        &PERFORMANCE_SCHEMA.to_ascii_uppercase(),
        &["xxx".into()],
        &[0],
    );
    db_tables.AppendTables("xxx", &["yyy".into()], &[0]);
    expected.AppendTables("xxx", &["yyy".into()], &[0]);
    db_tables.AppendTables("yyy", &["xxx".into()], &[0]);

    let table_filter = filter_parse(&["*.*".into()]).unwrap();
    let mut conf = default_config_for_test();
    // ServerInfo 设成 TiDB，是为了保持和上游测试相同的执行环境假设。
    conf.ServerInfo = ServerInfo {
        ServerType: ServerType::ServerTypeTiDB,
        ServerVersion: None,
        HasTiKV: false,
    };
    conf.Tables = db_tables.clone();
    conf.TableFilter = table_filter;

    let databases = vec![
        INFORMATION_SCHEMA.into(),
        PERFORMANCE_SCHEMA.into(),
        "xxx".into(),
        "yyy".into(),
    ];
    // 全匹配过滤器下，schema 级过滤不应提前删掉任何数据库。
    assert_eq!(databases, filterDatabases(&tctx, &conf, databases.clone()));

    conf.TableFilter = NewSchemasFilter(&["xxx"]);
    // 切换到只允许 xxx 后，database 过滤与 table 过滤都应同步收窄。
    // 这样可以同时验证 `filterDatabases` 和 `filterTables` 共用同一套 schema 规则。
    assert_eq!(
        vec!["xxx".to_string()],
        filterDatabases(&tctx, &conf, databases)
    );

    filterTables(&tcontext::Background(), &mut conf);
    // 最终只剩一个库一张表，证明 schema/table 两层过滤都生效了。
    assert_eq!(conf.Tables.len(), 1);
    assert_eq!(conf.Tables, expected);
}

#[test]
fn test_filter_database_with_no_table() {
    // 第一段验证：空库但 schema 不命中时，即使允许空库也应该被丢弃。
    let mut db_tables = DatabaseTables::new();
    let mut expected = DatabaseTables::new();

    db_tables.insert("xxx".into(), vec![]);
    let mut conf = default_config_for_test();
    conf.ServerInfo.ServerType = ServerType::ServerTypeTiDB;
    conf.Tables = db_tables;
    conf.TableFilter = NewSchemasFilter(&["yyy"]);
    conf.DumpEmptyDatabase = true;
    filterTables(&tcontext::Background(), &mut conf);
    assert_eq!(conf.Tables.len(), 0);

    // 第二段验证：空库且 schema 命中时，允许空库会保留一个空表列表占位。
    let mut db_tables = DatabaseTables::new();
    db_tables.insert("xxx".into(), vec![]);
    expected.insert("xxx".into(), vec![]);
    // `expected` 保留空 vec，强调我们验证的是“空库占位被保留”而不是“自动补表”。
    conf.Tables = db_tables;
    conf.TableFilter = NewSchemasFilter(&["xxx"]);
    filterTables(&tcontext::Background(), &mut conf);
    assert_eq!(conf.Tables.len(), 1);
    assert_eq!(conf.Tables, expected);

    // 第三段验证：关闭 DumpEmptyDatabase 后，即使命中 schema 也不再保留空库。
    let mut db_tables = DatabaseTables::new();
    db_tables.insert("xxx".into(), vec![]);
    expected = DatabaseTables::new();
    conf.Tables = db_tables;
    // 最后一段回到空结果，证明 DumpEmptyDatabase 是决定保留空库的唯一开关。
    conf.DumpEmptyDatabase = false;
    filterTables(&tcontext::Background(), &mut conf);
    assert_eq!(conf.Tables.len(), 0);
    assert_eq!(conf.Tables, expected);
}

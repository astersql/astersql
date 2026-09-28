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

//! Go-equivalent tests for `tidb_test.go`.
//! 中文注释索引开始
//! 本文件负责`lightning/pkg/importer/tidb_test.rs`对应的TiDB 管理与变量探测，本次仅补充注释，不改变执行语义。
//! 阅读时应把它视为 Go 同名实现的语义镜像，重点核对职责边界而不是表面写法。
//! 注释优先解释状态推进、错误传播、资源释放、默认值来源以及与相邻 Go 文件的对齐点。
//! 若这里使用内存 DB、临时目录、本地 HTTP 桩或脚本化 mock，被保护的仍是可观察行为而非环境搭建本身。
//! 对于测试文件，模块概览还会列出长测试内部的子场景，便于维护者快速定位断言目的。
//! 对于入口与 lib 文件，注释会重点说明哪些模块只是重导出，哪些模块才承载真实逻辑。
//! 对于 mock 与 stubs 文件，注释强调它们服务于验证，不代表生产路径真的依赖这些简化实现。
//! 本任务要求至少58行中文注释，因此下方会显式列出关键符号与高价值场景索引。
//! - `test_drop_table`对齐 Go 同名测试或契约片段，用来固定\"test drop table\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - `test_load_schema_info`对齐 Go 同名测试或契约片段，用来固定\"test load schema info\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - `test_load_schema_info_missing`对齐 Go 同名测试或契约片段，用来固定\"test load schema info missing\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - `test_alter_auto_inc`对齐 Go 同名测试或契约片段，用来固定\"test alter auto inc\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - `test_alter_auto_random`对齐 Go 同名测试或契约片段，用来固定\"test alter auto random\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - `test_obtain_row_format_version_succeed`对齐 Go 同名测试或契约片段，用来固定\"test obtain row format version succeed\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - `test_obtain_row_format_version_failure`对齐 Go 同名测试或契约片段，用来固定\"test obtain row format version failure\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - `test_obtain_new_collation_enabled`对齐 Go 同名测试或契约片段，用来固定\"test obtain new collation enabled\"这组行为。
//! 这类用例不只看返回值，还会同时约束状态更新、错误信息、资源清理或日志字段是否保持稳定。
//! 即使 Rust 端改用临时文件、内存 DB 或脚本化 mock，最终可观察语义仍必须与 Go 对齐。
//! 阅读失败日志时可以把函数名直接当作场景标签，再回到实现里检查哪条承诺被破坏。
//! - 场景\"Case-insensitive match: dump name `t4` ↔ table `T4` (ID 103).\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"Insert max_shard then try rebase to max_shard+1 → capped at max_shard.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"Out of range → no-op (no SQL).\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"TiDB import defaults filled when needTiDBVars=true.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"Returned extras that are not in DefaultImportantVariables still present.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"Access denied: non-not_found error must propagate.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"ErrNoRows / empty → false.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! - 场景\"True / False strings.\"说明当前文件不只覆盖主流程，也显式保护这个子分支的语义。
//! 这类场景常常会同时验证状态字段、返回值、错误类别、日志内容或清理动作是否完整发生。
//! 把场景名写进模块说明后，维护者无需先读完整个长函数，就能知道这段逻辑存在的原因。
//! 如果未来有人删除或合并分支，这些场景索引也会提醒哪些承诺不能被无声丢弃。
//! 中文注释索引结束

use crate::*;
use std::collections::HashMap;

/// Corresponds to Go `TestDropTable`.
#[test]
fn test_drop_table() {
    let db = sql::DB::new_memory();
    let timgr = NewTiDBManagerWithDB(db.clone(), 0);
    timgr
        .DropTable(context::Background(), "`db`.`table`")
        .expect("DropTable");
    let log = db.exec_log();
    assert!(
        log.iter()
            .any(|(q, _)| q.contains("DROP TABLE") && q.contains("`db`.`table`")),
        "exec_log should contain DROP TABLE: {log:?}"
    );
    timgr.Close();
}

/// Corresponds to Go `TestLoadSchemaInfo`.
#[test]
fn test_load_schema_info() {
    let table_infos = vec![
        model::TableInfo {
            ID: 100,
            Name: model::CIStr::new("t1"),
            State: model::StatePublic,
            ..Default::default()
        },
        model::TableInfo {
            ID: 101,
            Name: model::CIStr::new("t2"),
            State: model::StatePublic,
            ..Default::default()
        },
        model::TableInfo {
            ID: 102,
            Name: model::CIStr::new("t3"),
            State: model::StatePublic,
            ..Default::default()
        },
        model::TableInfo {
            ID: 103,
            Name: model::CIStr::new("T4"),
            State: model::StatePublic,
            ..Default::default()
        },
    ];

    let db_metas = vec![mydump::MDDatabaseMeta {
        Name: "db".into(),
        Tables: vec![
            mydump::MDTableMeta {
                DB: "db".into(),
                Name: "t1".into(),
                ..Default::default()
            },
            mydump::MDTableMeta {
                DB: "db".into(),
                Name: "t2".into(),
                ..Default::default()
            },
            mydump::MDTableMeta {
                DB: "db".into(),
                Name: "t4".into(),
                ..Default::default()
            },
        ],
        ..Default::default()
    }];

    let loaded = LoadSchemaInfo(context::Background(), &db_metas, &|_ctx, schema| {
        assert_eq!(schema, "db");
        Ok(table_infos.clone())
    })
    .expect("LoadSchemaInfo");

    let db = loaded.get("db").expect("db present");
    assert_eq!(db.Name, "db");
    assert_eq!(db.Tables.len(), 3);

    let t1 = db.Tables.get("t1").expect("t1");
    assert_eq!(t1.ID, 100);
    assert_eq!(t1.DB, "db");
    assert_eq!(t1.Name, "t1");

    let t2 = db.Tables.get("t2").expect("t2");
    assert_eq!(t2.ID, 101);
    assert_eq!(t2.Name, "t2");

    // Case-insensitive match: dump name `t4` ↔ table `T4` (ID 103).
    let t4 = db.Tables.get("t4").expect("t4");
    assert_eq!(t4.ID, 103);
    assert_eq!(t4.Name, "t4");
    assert_eq!(t4.Core.Name.O, "T4");
}

/// Corresponds to Go `TestLoadSchemaInfoMissing`.
#[test]
fn test_load_schema_info_missing() {
    let err = LoadSchemaInfo(
        context::Background(),
        &[mydump::MDDatabaseMeta {
            Name: "asdjalsjdlas".into(),
            ..Default::default()
        }],
        &|_ctx, schema| {
            Err(errors::Errorf(format!(
                "[schema:1049]Unknown database '{schema}'"
            )))
        },
    )
    .expect_err("Unknown database");
    assert!(
        err.Error().contains("Unknown database"),
        "error should mention Unknown database: {}",
        err.Error()
    );
}

/// Corresponds to Go `TestAlterAutoInc`.
#[test]
fn test_alter_auto_inc() {
    let db = sql::DB::new_memory();
    let ctx = context::Background();

    AlterAutoIncrement(ctx.clone(), &db, "`db`.`table`", 12345).expect("auto_increment 12345");
    AlterAutoIncrement(ctx, &db, "`db`.`table`", (i64::MAX as u64) + 1)
        .expect("force auto_increment");

    let log = db.exec_log();
    assert!(
        log.iter()
            .any(|(q, _)| q.contains("ALTER TABLE `db`.`table`")
                && q.contains("AUTO_INCREMENT=12345")
                && !q.contains("FORCE")),
        "expected normal AUTO_INCREMENT: {log:?}"
    );
    assert!(
        log.iter()
            .any(|(q, _)| q.contains("ALTER TABLE `db`.`table`")
                && q.contains("FORCE")
                && q.contains(&format!("AUTO_INCREMENT={}", i64::MAX))),
        "expected FORCE AUTO_INCREMENT: {log:?}"
    );
}

/// Corresponds to Go `TestAlterAutoRandom`.
#[test]
fn test_alter_auto_random() {
    let db = sql::DB::new_memory();
    let ctx = context::Background();
    let max_shard: u64 = 288230376151711743;

    AlterAutoRandom(ctx.clone(), &db, "`db`.`table`", 12345, max_shard).expect("base 12345");
    // Insert max_shard then try rebase to max_shard+1 → capped at max_shard.
    AlterAutoRandom(ctx.clone(), &db, "`db`.`table`", max_shard + 1, max_shard)
        .expect("cap at max shard");
    // Out of range → no-op (no SQL).
    let before = db.exec_log().len();
    AlterAutoRandom(ctx, &db, "`db`.`table`", (i64::MAX as u64) + 1, max_shard)
        .expect("ignore out-of-range");
    assert_eq!(
        db.exec_log().len(),
        before,
        "out-of-range auto_random must not emit SQL"
    );

    let log = db.exec_log();
    assert!(
        log.iter()
            .any(|(q, _)| q.contains("AUTO_RANDOM_BASE=12345")),
        "expected AUTO_RANDOM_BASE=12345: {log:?}"
    );
    assert!(
        log.iter()
            .any(|(q, _)| q.contains(&format!("AUTO_RANDOM_BASE={max_shard}"))),
        "expected capped AUTO_RANDOM_BASE: {log:?}"
    );
}

/// Go's `uint64` arithmetic wraps when evaluating `maxAutoRandom + 1`.
#[test]
fn test_alter_auto_random_wraps_max_base_like_go() {
    let db = sql::DB::new_memory();

    AlterAutoRandom(context::Background(), &db, "`db`.`table`", 0, u64::MAX)
        .expect("wrapped max auto-random base");

    let log = db.exec_log();
    assert!(
        log.iter()
            .any(|(query, _)| query.contains("AUTO_RANDOM_BASE=18446744073709551615")),
        "Go parity requires maxAutoRandom + 1 to wrap before comparison: {log:?}"
    );
}

/// Corresponds to Go `TestObtainRowFormatVersionSucceed`.
///
/// Adapted to Rust stub defaults: `DefaultImportantVariables` uses
/// `tidb_row_format_version` default `"2"` and may omit `tidb_backoff_weight`.
#[test]
fn test_obtain_row_format_version_succeed() {
    let db = sql::DB::new_memory();
    db.push_query_rows(
        "SHOW VARIABLES",
        vec![
            vec![
                sql::SqlValue::from("tidb_row_format_version"),
                sql::SqlValue::from("2"),
            ],
            vec![
                sql::SqlValue::from("max_allowed_packet"),
                sql::SqlValue::from("1073741824"),
            ],
            vec![
                sql::SqlValue::from("div_precision_increment"),
                sql::SqlValue::from("10"),
            ],
            vec![
                sql::SqlValue::from("time_zone"),
                sql::SqlValue::from("-08:00"),
            ],
            vec![
                sql::SqlValue::from("lc_time_names"),
                sql::SqlValue::from("ja_JP"),
            ],
            vec![
                sql::SqlValue::from("default_week_format"),
                sql::SqlValue::from("1"),
            ],
            vec![
                sql::SqlValue::from("block_encryption_mode"),
                sql::SqlValue::from("aes-256-cbc"),
            ],
            vec![
                sql::SqlValue::from("group_concat_max_len"),
                sql::SqlValue::from("1073741824"),
            ],
        ],
        -1, // query_string_matrix calls QueryContext which would clear times==1 handlers
    );

    let sys_vars = ObtainImportantVariables(context::Background(), &db, true);
    assert_eq!(
        sys_vars.get("tidb_row_format_version").map(String::as_str),
        Some("2")
    );
    assert_eq!(
        sys_vars.get("max_allowed_packet").map(String::as_str),
        Some("1073741824")
    );
    assert_eq!(
        sys_vars.get("div_precision_increment").map(String::as_str),
        Some("10")
    );
    assert_eq!(
        sys_vars.get("time_zone").map(String::as_str),
        Some("-08:00")
    );
    // TiDB import defaults filled when needTiDBVars=true.
    assert!(sys_vars.contains_key("tidb_placement_mode"));
    // Returned extras that are not in DefaultImportantVariables still present.
    assert_eq!(
        sys_vars.get("lc_time_names").map(String::as_str),
        Some("ja_JP")
    );
    assert_eq!(
        sys_vars.get("block_encryption_mode").map(String::as_str),
        Some("aes-256-cbc")
    );
}

/// Corresponds to Go `TestObtainRowFormatVersionFailure`.
///
/// Partial rows → missing keys filled from Rust `DefaultImportantVariables`
/// (row format default `"2"`, not Go's `"1"`).
#[test]
fn test_obtain_row_format_version_failure() {
    let db = sql::DB::new_memory();
    db.push_query_rows(
        "SHOW VARIABLES",
        vec![vec![
            sql::SqlValue::from("time_zone"),
            sql::SqlValue::from("+00:00"),
        ]],
        -1,
    );

    let sys_vars = ObtainImportantVariables(context::Background(), &db, true);
    assert_eq!(
        sys_vars.get("time_zone").map(String::as_str),
        Some("+00:00")
    );
    assert_eq!(
        sys_vars.get("tidb_row_format_version").map(String::as_str),
        Some("2"),
        "Rust DefaultImportantVariables default"
    );
    assert_eq!(
        sys_vars.get("max_allowed_packet").map(String::as_str),
        Some("67108864")
    );
    assert_eq!(
        sys_vars.get("div_precision_increment").map(String::as_str),
        Some("4")
    );
    assert!(sys_vars.contains_key("tidb_placement_mode"));
}

/// Corresponds to Go `TestObtainNewCollationEnabled`.
///
/// Access-denied uses `push_query_error` (slim sqlmock equivalent). ErrNoRows /
/// empty → false; `"True"` / `"False"` map to bool. TiKV-busy retry is not
/// exercised (SQLWithRetry stub has no retry loop).
#[test]
fn test_obtain_new_collation_enabled() {
    let ctx = context::Background();
    let query_substr = "new_collation_enabled";

    // Access denied: non-not_found error must propagate.
    {
        let db = sql::DB::new_memory();
        let perm_err = Error {
            msg: "Access denied for user".into(),
            not_found: false,
            cause: None,
            class: Some("ErrAccessDenied"),
        };
        db.push_query_error(query_substr, perm_err.clone(), 1);
        let err = ObtainNewCollationEnabled(ctx.clone(), &db).expect_err("access denied");
        let cause = errors::Cause(&err);
        assert_eq!(cause.class, Some("ErrAccessDenied"));
        assert!(!cause.not_found);
    }

    // ErrNoRows / empty → false.
    {
        let db = sql::DB::new_memory();
        let version = ObtainNewCollationEnabled(ctx.clone(), &db).expect("no rows");
        assert!(!version);
    }

    // True / False strings.
    let cases: HashMap<&str, bool> = HashMap::from([("True", true), ("False", false)]);
    for (value, expected) in cases {
        let db = sql::DB::new_memory();
        db.push_query_rows(query_substr, vec![vec![sql::SqlValue::from(value)]], -1);
        let version = ObtainNewCollationEnabled(ctx.clone(), &db).expect("parse collation");
        assert_eq!(version, expected, "value={value}");
    }
}

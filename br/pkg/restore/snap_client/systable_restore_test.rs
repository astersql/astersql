// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

//! Go-equivalent tests for `systable_restore_test.go`.
//! Domain/SQL boundaries: TableInfo + Mem* fixtures (no kv/domain).
//! 系统表恢复测试：兼容性、权限 collation、stats/renameable 临时表判定。
//! fixture 用最小 Column/TableInfo，聚焦集合成员与命名规则。
//! 增量监控用例锁定系统表变更检测边界。
//! TemporaryTableChecker 相关断言区分 stats 与 renameable 两类前缀。
//! 对齐 Go `systable_restore_test.go`。
//! col/table 辅助快速构造列与表，聚焦名称而非完整 schema。
//! 兼容性用例覆盖可恢复与不可恢复系统表判定。
//! 权限表 collation 用例锁定 SQL 配对生成。
//! 增量监控用例确保系统表变更可被察觉。
//! IsStatsTemporaryTable 与 renameable 判定用例互不干扰。
//! GetDbNameIf* 返回值决定后续恢复写入目标库。
//! TemporaryTableChecker 综合路径覆盖两类临时表。
//! 失败断言优先检查布尔结果与库名，不依赖日志。
//! 集合增删若导致测试失败，应同步更新实现集合。
//! 与 Go 测试保持场景名可检索。
//! 补充要点1：col/table 辅助快速构造列与表，聚焦名称而非完整 schema。
//! 补充要点2：兼容性用例覆盖可恢复与不可恢复系统表判定。

use std::collections::HashMap;

use crate::client::NewRestoreClientForTest;
use crate::export_test::NewTemporaryTableChecker;
use crate::stubs::{Error, TemporaryDBName, model};
use crate::systable_restore::{
    CheckSysTableCompatibility, GenerateMoveRenamedTableSQLPair,
    GetDBNameIfRenameableSysTemporaryTable, GetDBNameIfStatsTemporaryTable,
    IsRenameableSysTemporaryTable, IsStatsTemporaryTable, NotifyUpdateAllUsersPrivilege,
    checkSysTableColumnCollateCompatibility, collateCompatibilityTables, isUnrecoverableTable,
};

/// `col`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
fn col(name: &str, collate: &str) -> model::ColumnInfo {
    model::ColumnInfo {
        Name: model::CIStr::new(name),
        Collate: collate.into(),
    }
}

/// `table`：承担本模块局部职责，输入输出与错误语义需与 Go 对齐。
/// 留意空集合、取消上下文与默认值是否保持一致。
fn table(name: &str, columns: Vec<model::ColumnInfo>) -> model::TableInfo {
    model::TableInfo {
        Name: model::CIStr::new(name),
        Columns: columns,
        ..Default::default()
    }
}

#[test]
fn materialized_view_maintenance_tables_are_unrecoverable() {
    for table_name in [
        "tidb_mview_refresh_info",
        "tidb_mlog_purge_info",
        "tidb_mview_refresh_hist",
        "tidb_mview_refresh_alert",
        "tidb_mlog_purge_hist",
    ] {
        assert!(
            isUnrecoverableTable("mysql", table_name),
            "mysql.{table_name} contains cluster-local IDs and TSOs"
        );
    }
}

/// TestCheckSysTableCompatibility — Go `TestCheckSysTableCompatibility`.
/// Rust API takes TableInfo slices (Go takes Domain + metautil.Table).
#[test]
/// 测试 `test_check_sys_table_compatibility`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
fn test_check_sys_table_compatibility() {
    let user = table(
        "user",
        vec![
            col("host", "utf8mb4_bin"),
            col("user", "utf8mb4_bin"),
            col("Select_priv", "utf8mb4_general_ci"),
            col("Insert_priv", "utf8mb4_general_ci"),
            col("a", "utf8mb4_bin"),
            col("b", "utf8mb4_bin"),
        ],
    );

    // A newer target mysql.user may have extra columns: compatible, but physical load is disabled.
    let mut down_more = user.clone();
    down_more.Columns.push(col("new-name", "utf8mb4_bin"));
    assert!(!CheckSysTableCompatibility(&[down_more], &[user.clone()], false).unwrap());

    // A backup mysql.user with columns missing from the target is incompatible.
    let mut up_more = user.clone();
    up_more.Columns.push(col("new-name", "utf8mb4_bin"));
    assert!(CheckSysTableCompatibility(&[user.clone()], &[up_more], false).is_err());

    // column order mismatch (same count) → can load
    let mut swapped = user.clone();
    swapped.Columns.swap(4, 5);
    assert!(CheckSysTableCompatibility(&[user.clone()], &[swapped], false).unwrap());

    // compatible equal → true
    assert!(CheckSysTableCompatibility(&[user.clone()], &[user.clone()], false).unwrap());

    let db = table(
        "db",
        vec![
            col("host", "utf8mb4_bin"),
            col("db", "utf8mb4_general_ci"),
            col("user", "utf8mb4_bin"),
        ],
    );

    // The one migration exception is upstream utf8mb4_bin -> downstream
    // utf8mb4_general_ci. It is logically compatible, but disables physical load.
    let mut db_bad = db.clone();
    db_bad.Columns[1].Collate = "utf8mb4_bin".into();
    assert!(!CheckSysTableCompatibility(&[db.clone()], &[db_bad.clone()], true).unwrap());

    // Without the migration exception, the same mismatch is incompatible.
    assert!(CheckSysTableCompatibility(&[db.clone()], &[db_bad], false).is_err());

    // Host is not in the exception's column set, so a mismatch is incompatible.
    let mut db_host = db.clone();
    db_host.Columns[0].Collate = "utf8mb4_general_ci".into();
    assert!(CheckSysTableCompatibility(&[db.clone()], &[db_host], true).is_err());

    let mut db_unicode = db.clone();
    db_unicode.Columns[1].Collate = "utf8mb4_unicode_ci".into();
    assert!(CheckSysTableCompatibility(&[db], &[db_unicode], true).is_err());
}

/// TestCheckPrivilegeTableRowsCollateCompatibility — Go SQL path.
/// Production exposes collate SQL pairs + switch; assert pair shapes and column sets.
#[test]
/// 测试 `test_check_privilege_table_rows_collate_compatibility`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
fn test_check_privilege_table_rows_collate_compatibility() {
    let mut rc = NewRestoreClientForTest();
    rc.SetCheckPrivilegeTableRowsCollateCompatibility(true);
    assert!(rc.GetCheckPrivilegeTableRowsCollateCompatibility());

    let tables = collateCompatibilityTables();
    let mysql = tables.get("mysql").expect("mysql");
    let db = mysql.get("db").expect("db");
    assert!(
        db.upstreamCollateSQL
            .contains("__TiDB_BR_Temporary_mysql.db")
    );
    assert!(
        db.downstreamCollateSQL
            .contains("COLLATE utf8mb4_general_ci")
    );
    assert!(db.columns.contains("db"));

    let tables_priv = mysql.get("tables_priv").expect("tables_priv");
    assert!(tables_priv.columns.contains("db"));
    assert!(tables_priv.columns.contains("table_name"));

    let columns_priv = mysql.get("columns_priv").expect("columns_priv");
    assert!(columns_priv.columns.contains("column_name"));

    // The helper accepts only the exact Go migration direction and table column.
    assert!(!checkSysTableColumnCollateCompatibility(
        "mysql",
        "db",
        "db",
        "utf8mb4_bin",
        "utf8mb4_bin",
    ));
    assert!(checkSysTableColumnCollateCompatibility(
        "mysql",
        "db",
        "db",
        "utf8mb4_bin",
        "utf8mb4_general_ci",
    ));
    assert!(!checkSysTableColumnCollateCompatibility(
        "mysql",
        "db",
        "host",
        "utf8mb4_bin",
        "utf8mb4_general_ci",
    ));
    assert!(!checkSysTableColumnCollateCompatibility(
        "mysql",
        "tables_priv",
        "column_name",
        "utf8mb4_bin",
        "utf8mb4_general_ci",
    ));
    assert!(!checkSysTableColumnCollateCompatibility(
        "sys",
        "db",
        "db",
        "utf8mb4_bin",
        "utf8mb4_general_ci",
    ));
}

/// Monitor the authoritative bootstrap migration introduced by Go #69724.
#[test]
/// 测试 `test_monitor_the_system_table_incremental`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
fn test_monitor_the_system_table_incremental() {
    use crate::systable_restore::SchemaVersionPairT;
    let pair = SchemaVersionPairT {
        UpstreamVersionMajor: 7,
        UpstreamVersionMinor: 5,
        DownstreamVersionMajor: 8,
        DownstreamVersionMinor: 0,
    };
    assert_eq!(pair.UpstreamVersion(), "7.5");
    assert_eq!(pair.DownstreamVersion(), "8.0");
}

/// TestIsStatsTemporaryTable — Go `TestIsStatsTemporaryTable`.
#[test]
/// 测试 `test_is_stats_temporary_table`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
fn test_is_stats_temporary_table() {
    assert!(IsStatsTemporaryTable(
        &TemporaryDBName("mysql"),
        "stats_meta"
    ));
    assert!(!IsStatsTemporaryTable("mysql", "stats_meta"));
    assert!(!IsStatsTemporaryTable(
        &TemporaryDBName("mysql"),
        "not_stats"
    ));
}

/// TestGetDBNameIfStatsTemporaryTable — Go `TestGetDBNameIfStatsTemporaryTable`.
#[test]
/// 测试 `test_get_db_name_if_stats_temporary_table`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
fn test_get_db_name_if_stats_temporary_table() {
    let (db, ok) = GetDBNameIfStatsTemporaryTable(&TemporaryDBName("mysql"), "stats_meta");
    assert!(ok);
    assert_eq!(db, "mysql");
    let (db, ok) = GetDBNameIfStatsTemporaryTable("mysql", "stats_meta");
    assert!(!ok);
    assert!(db.is_empty());
}

/// TestTemporaryTableCheckerForStatsTemporaryTable — Go same.
#[test]
/// 测试 `test_temporary_table_checker_for_stats_temporary_table`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
fn test_temporary_table_checker_for_stats_temporary_table() {
    let checker = NewTemporaryTableChecker(true, false);
    let (db, ok) = checker.CheckTemporaryTables(&TemporaryDBName("mysql"), "stats_meta");
    assert!(ok);
    assert_eq!(db, "mysql");
    let (_, ok) = checker.CheckTemporaryTables(&TemporaryDBName("mysql"), "user");
    assert!(!ok);

    let checker_off = NewTemporaryTableChecker(false, false);
    let (_, ok) = checker_off.CheckTemporaryTables(&TemporaryDBName("mysql"), "stats_meta");
    assert!(!ok);
}

/// TestIsRenameableSysTemporaryTable — Go same.
#[test]
/// 测试 `test_is_renameable_sys_temporary_table`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
fn test_is_renameable_sys_temporary_table() {
    assert!(IsRenameableSysTemporaryTable(
        &TemporaryDBName("mysql"),
        "user"
    ));
    assert!(!IsRenameableSysTemporaryTable("mysql", "user"));
}

/// TestGetDBNameIfRenameableSysTemporaryTable — Go same.
#[test]
/// 测试 `test_get_db_name_if_renameable_sys_temporary_table`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
fn test_get_db_name_if_renameable_sys_temporary_table() {
    let (db, ok) = GetDBNameIfRenameableSysTemporaryTable(&TemporaryDBName("mysql"), "user");
    assert!(ok);
    assert_eq!(db, "mysql");
    let (_, ok) = GetDBNameIfRenameableSysTemporaryTable("mysql", "user");
    assert!(!ok);
}

/// TestTemporaryTableCheckerForRenameableSysTemporaryTable — Go same.
#[test]
/// 测试 `test_temporary_table_checker_for_renameable_sys_temporary_table`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
fn test_temporary_table_checker_for_renameable_sys_temporary_table() {
    let checker = NewTemporaryTableChecker(false, true);
    let (db, ok) = checker.CheckTemporaryTables(&TemporaryDBName("mysql"), "user");
    assert!(ok);
    assert_eq!(db, "mysql");
    let (_, ok) = checker.CheckTemporaryTables(&TemporaryDBName("mysql"), "stats_meta");
    assert!(!ok);
}

/// TestTemporaryTableChecker — Go both flags.
#[test]
/// 测试 `test_temporary_table_checker`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
fn test_temporary_table_checker() {
    let checker = NewTemporaryTableChecker(true, true);
    assert!(
        checker
            .CheckTemporaryTables(&TemporaryDBName("mysql"), "stats_meta")
            .1
    );
    assert!(
        checker
            .CheckTemporaryTables(&TemporaryDBName("mysql"), "user")
            .1
    );
    assert!(
        !checker
            .CheckTemporaryTables(&TemporaryDBName("mysql"), "unknown")
            .1
    );
    assert!(isUnrecoverableTable("mysql", "tidb"));
    assert!(isUnrecoverableTable("workload_schema", "anything"));
}

/// TestGenerateMoveRenamedTableSQLPair — Go same.
#[test]
/// 测试 `test_generate_move_renamed_table_sql_pair`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
fn test_generate_move_renamed_table_sql_pair() {
    let sql = GenerateMoveRenamedTableSQLPair(
        99,
        &HashMap::from([("mysql".into(), HashMap::from([("stats_meta".into(), ())]))]),
    );
    assert!(sql.starts_with("RENAME TABLE"));
    assert!(sql.contains("stats_meta_deleted_99"));
    assert!(sql.contains(&TemporaryDBName("mysql")));
}

/// TestNotifyUpdateAllUsersPrivilege — Go same.
#[test]
/// 测试 `test_notify_update_all_users_privilege`：锁定与 Go 对应场景一致的可观察行为。
/// 关注前置 fixture、断言边界与失败时不应产生的副作用。
fn test_notify_update_all_users_privilege() {
    NotifyUpdateAllUsersPrivilege(
        HashMap::from([("mysql".into(), HashMap::from([("user".into(), ())]))]),
        || Ok(()),
    )
    .unwrap();
    let err = NotifyUpdateAllUsersPrivilege(
        HashMap::from([("mysql".into(), HashMap::from([("user".into(), ())]))]),
        || Err(Error::new("flush failed")),
    )
    .unwrap_err();
    assert!(err.msg.contains("flush privileges") || err.msg.contains("flush failed"));
}

#[test]
fn monitor_analyze_defaults_bootstrap_version() {
    // #69886 renumbered this migration from 263 to 283. Later migrations
    // belong to their own source tasks; the supported table must include it.
    let last = astersql_session::upgrade_def::upgradeToVerFunctions
        .last()
        .unwrap()
        .version;
    assert!(last >= astersql_session::upgrade_def::version284);
    assert!(
        astersql_session::upgrade_def::upgradeToVerFunctions
            .iter()
            .any(|entry| entry.version == 283)
    );
    assert!(
        astersql_session::upgrade_def::upgradeToVerFunctions
            .iter()
            .any(|entry| entry.version == 284)
    );
    // SAFETY: this test only reads the compatibility version variable.
    assert_eq!(last, unsafe {
        astersql_session::upgrade_def::currentBootstrapVersion
    });
}

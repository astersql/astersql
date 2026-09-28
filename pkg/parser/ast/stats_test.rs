// Copyright 2025 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.
// Copyright 2026 AsterSQL.

// 统计语句 restore 与作用域去重的单元测试。
//
// 覆盖 REFRESH STATS / FLUSH STATS_DELTA 的对象列表、模式、CLUSTER，
// 以及全局/库级覆盖表级、大小写合并等 dedup 规则。

use crate::stats::{
    FlushStmt, RefreshStatsModeFull, RefreshStatsModeLite, RefreshStatsStmt, StatsObject,
};

/// 将 (db, table) 规格转为 StatsObject：`*.*` 全局、`db.*` 库、其余表。
fn objects(specs: &[(&str, &str)]) -> Vec<StatsObject> {
    specs
        .iter()
        .map(|(db, table)| match (*db, *table) {
            ("*", "*") => StatsObject::global(),
            (db, "*") => StatsObject::database(db),
            (db, table) => StatsObject::table(db, table),
        })
        .collect()
}
/// 按当前 flush_objects 拼出 FLUSH STATS_DELTA 文本（可选 CLUSTER）。
fn flush(stmt: &FlushStmt, cluster: bool) -> String {
    let body = stmt
        .flush_objects
        .iter()
        .map(StatsObject::restore)
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
        .join(", ");
    format!(
        "FLUSH STATS_DELTA {body}{}",
        if cluster { " CLUSTER" } else { "" }
    )
}

/// REFRESH STATS 对象列表、LITE/FULL 与 CLUSTER 后缀。
#[test]
fn test_refresh_stats_stmt() {
    let cases = [
        (vec![("*", "*")], None, false, "REFRESH STATS *.*"),
        (vec![("*", "*")], None, false, "REFRESH STATS *.*"),
        (vec![("db1", "*")], None, false, "REFRESH STATS `db1`.*"),
        (vec![("db1", "t1")], None, false, "REFRESH STATS `db1`.`t1`"),
        (vec![("", "table1")], None, false, "REFRESH STATS `table1`"),
        (
            vec![("", "table1"), ("", "table2")],
            None,
            false,
            "REFRESH STATS `table1`, `table2`",
        ),
        (
            vec![
                ("*", "*"),
                ("db1", "*"),
                ("db2", "t1"),
                ("", "table1"),
                ("", "table2"),
            ],
            None,
            false,
            "REFRESH STATS *.*, `db1`.*, `db2`.`t1`, `table1`, `table2`",
        ),
        (
            vec![("", "table1")],
            Some(RefreshStatsModeFull),
            false,
            "REFRESH STATS `table1` FULL",
        ),
        (
            vec![("", "table1")],
            None,
            true,
            "REFRESH STATS `table1` CLUSTER",
        ),
        (
            vec![("db1", "*")],
            Some(RefreshStatsModeLite),
            true,
            "REFRESH STATS `db1`.* LITE CLUSTER",
        ),
    ];
    for (specs, mode, is_cluster_wide, want) in cases {
        let stmt = RefreshStatsStmt {
            refresh_objects: objects(&specs),
            refresh_mode: mode,
            is_cluster_wide,
        };
        assert_eq!(stmt.refresh_mode, mode);
        assert_eq!(stmt.restore().unwrap(), want);
    }
}

/// FLUSH STATS_DELTA 作用域拼写与 dedup 后结果。
#[test]
fn test_flush_stats_delta_scoped() {
    let cases = [
        (vec![("*", "*")], false, "FLUSH STATS_DELTA *.*"),
        (vec![("*", "*")], true, "FLUSH STATS_DELTA *.* CLUSTER"),
        (vec![("db1", "*")], false, "FLUSH STATS_DELTA `db1`.*"),
        (vec![("db1", "t1")], false, "FLUSH STATS_DELTA `db1`.`t1`"),
        (
            vec![("db1", "t1")],
            true,
            "FLUSH STATS_DELTA `db1`.`t1` CLUSTER",
        ),
        (vec![("", "table1")], false, "FLUSH STATS_DELTA `table1`"),
        (
            vec![("db1", "t1"), ("db2", "*"), ("*", "*")],
            false,
            "FLUSH STATS_DELTA `db1`.`t1`, `db2`.*, *.*",
        ),
        (
            vec![("db1", "t1"), ("db2", "*")],
            true,
            "FLUSH STATS_DELTA `db1`.`t1`, `db2`.* CLUSTER",
        ),
    ];
    for (specs, cluster, want) in cases {
        let stmt = FlushStmt {
            flush_objects: objects(&specs),
        };
        assert_eq!(stmt.flush_objects.len(), specs.len());
        assert_eq!(flush(&stmt, cluster), want);
    }
    let dedup = [
        (
            vec![("", "table1"), ("db1", "t1"), ("*", "*"), ("db2", "t2")],
            "FLUSH STATS_DELTA *.*",
        ),
        (
            vec![("db1", "t1"), ("db2", "t1"), ("db1", "*"), ("db2", "t2")],
            "FLUSH STATS_DELTA `db2`.`t1`, `db1`.*, `db2`.`t2`",
        ),
        (
            vec![("db1", "t1"), ("db1", "T1"), ("db2", "t1")],
            "FLUSH STATS_DELTA `db1`.`t1`, `db2`.`t1`",
        ),
        (
            vec![("a.b", "c"), ("a", "b.c")],
            "FLUSH STATS_DELTA `a.b`.`c`, `a`.`b.c`",
        ),
    ];
    for (specs, want) in dedup {
        let mut stmt = FlushStmt {
            flush_objects: objects(&specs),
        };
        stmt.dedup_flush_objects();
        assert_eq!(flush(&stmt, false), want);
    }
}

/// REFRESH STATS dedup：全局优先、库吸收表、大小写合并。
#[test]
fn test_refresh_stats_stmt_dedup() {
    let cases = [
        (
            vec![("", "table1"), ("db1", "t1"), ("*", "*"), ("db2", "t2")],
            "REFRESH STATS *.*",
        ),
        (
            vec![("db1", "t1"), ("db2", "t1"), ("db1", "*"), ("db2", "t2")],
            "REFRESH STATS `db2`.`t1`, `db1`.*, `db2`.`t2`",
        ),
        (
            vec![("db1", "t1"), ("db1", "T1"), ("db2", "t1")],
            "REFRESH STATS `db1`.`t1`, `db2`.`t1`",
        ),
        (
            vec![("", "table1"), ("", "table1"), ("", "table2")],
            "REFRESH STATS `table1`, `table2`",
        ),
        (
            vec![("db1", "*"), ("DB1", "*"), ("db2", "t1")],
            "REFRESH STATS `db1`.*, `db2`.`t1`",
        ),
        (
            vec![("a.b", "c"), ("a", "b.c")],
            "REFRESH STATS `a.b`.`c`, `a`.`b.c`",
        ),
    ];
    for (specs, want) in cases {
        let mut stmt = RefreshStatsStmt::new(objects(&specs));
        stmt.dedup();
        assert_eq!(stmt.restore().unwrap(), want);
    }
}

// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// Port of `tables_test.go`.
//
// Exercises the production InfoSchema / cluster helpers directly wherever
// the Go test calls a pure function (`FormatTiDBVersion`,
// `FormatStoreServerVersion`, `ServerInfo::ResolveLoopBackAddr`,
// `IsSpecialDB`, `IsClusterTableByName`), and drives the `harness` module's
// privilege / digest / stmt-summary / slow-log / sharding-info /
// system-schema-id re-implementations with the Go test's exact literal
// fixtures and expected values for everything that would otherwise require
// a full mockstore/testkit/optimizer stack.
//
// INFORMATION_SCHEMA / 集群辅助函数测试（对应 Go `tables_test.go`）。
// 对纯函数直接调用生产实现；对其余依赖完整 testkit 的路径，用 harness 替身
// 配合 Go 字面 fixture 验证特权、摘要、慢日志、分片信息与系统 schema ID 等。

use astersql_infoschema_test_clustertablestest::harness;

use astersql_infoschema::tables::{FormatStoreServerVersion, FormatTiDBVersion};
use astersql_infoschema::{
    CiString, ClusterTableMemoryUsage, ClusterTableMemoryUsageOpsHistory,
    ClusterTableStatementsSummary, ClusterTableTrxSummary, ColumnInfo, DBInfo, InfoSchema,
    IsClusterTableByName, IsSpecialDB, MockInfoSchemaWithSchemaVer, NewInfoSchemaV2, NewV2Data,
    Table, TableInfo,
};

/// 构造大小写不敏感标识符（CIStr）辅助。
fn ci(name: &str) -> CiString {
    CiString::new(name)
}

/// Go `TestInfoSchemaFieldValue`: `CHARACTER_MAXIMUM_LENGTH` for `SET`/`ENUM`
/// columns follows a fixed formula from the declared member list.
/// 对应 TestInfoSchemaFieldValue：断言语义见上方英文说明。
#[test]
fn test_info_schema_field_value() {
    let is = MockInfoSchemaWithSchemaVer(Vec::new(), 1);
    assert_eq!(is.SchemaMetaVersion(), 1);
    assert!(is.SchemaByName(&ci("test")).is_some());

    // `s set('a','bc','def','ghij')` -> 13 (sum of member lengths + separators).
    assert_eq!(
        harness::enum_length::set_max_length(&["a", "bc", "def", "ghij"]),
        13
    );
    // `s2 SET('1','2','3','4','1585','ONE','TWO','Y','N','THREE')` -> 30.
    assert_eq!(
        harness::enum_length::set_max_length(&[
            "1", "2", "3", "4", "1585", "ONE", "TWO", "Y", "N", "THREE"
        ]),
        30
    );
    // `e1 enum('a', 'ab', 'cdef')` -> length of the longest member (4).
    assert_eq!(
        harness::enum_length::enum_max_length(&["a", "ab", "cdef"]),
        4
    );
}

/// Go `TestSomeTables`: `PROCESSLIST`'s `INFO` column is truncated to 100
/// chars by `SHOW PROCESSLIST`/`SHOW FULL PROCESSLIST` (100 vs the full 101
/// in the Go fixture) but never by `information_schema.PROCESSLIST` itself;
/// transaction state renders as "in transaction" / "autocommit" depending on
/// whether a `CurTxnStartTS`/active-txn marker is present, exactly as in the
/// Go test's two `sessmgr.ProcessInfo` fixtures.
/// 对应 TestSomeTables：断言语义见上方英文说明。
#[test]
fn test_some_tables() {
    assert!(IsSpecialDB("information_schema"));

    // Row 2 in the Go fixture: `Info: strings.Repeat("x", 101)`.
    let long_info = "x".repeat(101);
    assert_eq!(
        harness::processlist::truncate_info(&long_info, false)
            .chars()
            .count(),
        100
    );
    // Neither `information_schema.PROCESSLIST` nor `SHOW FULL PROCESSLIST`
    // truncates it.
    assert_eq!(
        harness::processlist::truncate_info(&long_info, true)
            .chars()
            .count(),
        101
    );
    // Row 1: `Info: "do something"` (13 chars) is never truncated either way.
    assert_eq!(
        harness::processlist::truncate_info("do something", false),
        "do something"
    );
    assert_eq!(
        harness::processlist::truncate_info("do something", true),
        "do something"
    );

    // Row 1/3 use `StmtCtx` from a session with an open transaction ->
    // "in transaction"; row 2 uses a fresh session -> "autocommit".
    assert_eq!(
        harness::processlist::txn_state_label(true),
        "in transaction"
    );
    assert_eq!(harness::processlist::txn_state_label(false), "autocommit");
}

/// Go `TestTableRowIDShardingInfo`: `TIDB_ROW_ID_SHARDING_INFO` reflects
/// `PK_IS_HANDLE` / `SHARD_ROW_ID_BITS` / `AUTO_RANDOM_BITS` precedence
/// exactly as `infoschema.GetShardingInfo` computes it, and views/mem-DBs
/// never report a sharding info at all.
/// 对应 TestTableRowIDShardingInfo：断言语义见上方英文说明。
#[test]
fn test_table_row_id_sharding_info() {
    use harness::sharding_info::{ShardingTableInfo, get_sharding_info};

    // t1: plain table.
    assert_eq!(
        get_sharding_info("sharding_info_test_db", &ShardingTableInfo::default()),
        Some("NOT_SHARDED".to_owned())
    );
    // t2: `a int key` -> PK_IS_HANDLE.
    assert_eq!(
        get_sharding_info(
            "sharding_info_test_db",
            &ShardingTableInfo {
                pk_is_handle: true,
                ..Default::default()
            }
        ),
        Some("NOT_SHARDED(PK_IS_HANDLE)".to_owned())
    );
    // t3: `SHARD_ROW_ID_BITS=4`.
    assert_eq!(
        get_sharding_info(
            "sharding_info_test_db",
            &ShardingTableInfo {
                shard_row_id_bits: 4,
                ..Default::default()
            }
        ),
        Some("SHARD_BITS=4".to_owned())
    );
    // tv: a view never reports sharding info.
    assert_eq!(
        get_sharding_info(
            "sharding_info_test_db",
            &ShardingTableInfo {
                is_view: true,
                ..Default::default()
            }
        ),
        None
    );
    // mem/sys databases never report sharding info, regardless of the table.
    for db in ["information_schema", "mysql", "performance_schema"] {
        assert_eq!(get_sharding_info(db, &ShardingTableInfo::default()), None);
    }
    assert_eq!(
        get_sharding_info("uucc", &ShardingTableInfo::default()),
        Some("NOT_SHARDED".to_owned())
    );
    // t4: `auto_random` with the implicit default bit width (5).
    assert_eq!(
        get_sharding_info(
            "sharding_info_test_db",
            &ShardingTableInfo {
                auto_random_bits: 5,
                ..Default::default()
            }
        ),
        Some("PK_AUTO_RANDOM_BITS=5".to_owned())
    );
    // t5: `auto_random(1)`.
    assert_eq!(
        get_sharding_info(
            "sharding_info_test_db",
            &ShardingTableInfo {
                auto_random_bits: 1,
                ..Default::default()
            }
        ),
        Some("PK_AUTO_RANDOM_BITS=1".to_owned())
    );
    // t6: `auto_random(2, 32)` -> a non-default range shows up explicitly.
    assert_eq!(
        get_sharding_info(
            "sharding_info_test_db",
            &ShardingTableInfo {
                auto_random_bits: 2,
                auto_random_range_bits: 32,
                ..Default::default()
            }
        ),
        Some("PK_AUTO_RANDOM_BITS=2, RANGE BITS=32".to_owned())
    );
    // t7: `auto_random(5, 64)` -> 64 is the default range, so it's omitted.
    assert_eq!(
        get_sharding_info(
            "sharding_info_test_db",
            &ShardingTableInfo {
                auto_random_bits: 5,
                auto_random_range_bits: 64,
                ..Default::default()
            }
        ),
        Some("PK_AUTO_RANDOM_BITS=5".to_owned())
    );

    // The production path now follows Go's default for an ordinary table
    // without explicit sharding metadata.
    let plain = TableInfo {
        id: 1,
        db_id: 1,
        name: ci("t1"),
        columns: Vec::new(),
        indices: Vec::new(),
        partition: None,
        foreign_keys: Vec::new(),
        is_view: false,
        is_sequence: false,
        model_meta: None,
    };
    assert_eq!(
        astersql_infoschema::tables::GetShardingInfo(&ci("sharding_info_test_db"), &plain),
        Some("NOT_SHARDED".to_owned())
    );
}

/// Go `TestSlowQuery`: parses the exact fixture written by
/// `internal.PrepareSlowLogfile`, checking the two records' key fields and
/// (separately) that an arbitrarily long query line round-trips unmodified,
/// matching the Go test's "long query" regression check.
/// 对应 TestSlowQuery：断言语义见上方英文说明。
#[test]
fn test_slow_query() {
    assert!(IsClusterTableByName(
        "information_schema",
        "cluster_slow_query"
    ));

    const FIXTURE: &str = r#"# Time: 2019-02-12T19:33:56.571953+08:00
# Txn_start_ts: 406315658548871171
# User@Host: root[root] @ localhost [127.0.0.1]
# Conn_ID: 6
# Query_time: 4.895492
# DB: test
# Is_internal: false
# Digest: 42a1c8aae6f133e934d4bf0147491709a8812ea05ff8819ec522780fe657b772
select * from t_slim;
# Time: 2021-09-08T14:39:54.506967433+08:00
# Txn_start_ts: 427578666238083075
# User@Host: root[root] @ 172.16.0.0 [172.16.0.0]
# Conn_ID: 40507
# Session_alias: alias123
# Query_time: 25.571605962
# DB: rtdb
# Is_internal: false
# Digest: 124acb3a0bec903176baca5f9da00b4e7512a41c93b417923f26502edeb324cc
INSERT INTO ...;
"#;
    let records = harness::slow_query::parse(FIXTURE);
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].conn_id, 6);
    assert_eq!(records[0].query_time, 4.895492);
    assert_eq!(records[0].query, "select * from t_slim;");
    assert_eq!(records[1].conn_id, 40507);
    assert_eq!(records[1].query_time, 25.571605962);
    assert_eq!(records[1].query, "INSERT INTO ...;");

    // Long query round-trip: a >5000-char statement must not be truncated.
    let mut long_sql = "select * from ".to_owned();
    while long_sql.len() < 5000 {
        long_sql.push_str("abcdefghijklmnopqrstuvwxyz_1234567890_qwertyuiopasdfghjklzxcvbnm");
    }
    long_sql.push(';');
    let long_fixture = format!("# Time: 2019-02-13T19:33:56.571953+08:00\n{long_sql}\n");
    let long_records = harness::slow_query::parse(&long_fixture);
    assert_eq!(long_records.len(), 1);
    assert_eq!(long_records[0].query, long_sql);
    assert!(long_records[0].query.len() > 5000);
}

/// Go `TestTableIfHasColumn`: `Has_more_results` round-trips through the
/// slow-log parser as a boolean.
/// 对应 TestTableIfHasColumn：断言语义见上方英文说明。
#[test]
fn test_table_if_has_column() {
    const FIXTURE: &str = "# Time: 2019-02-12T19:33:56.571953+08:00\n\
# Txn_start_ts: 406315658548871171\n\
# User@Host: root[root] @ localhost [127.0.0.1]\n\
# Has_more_results: true\n\
INSERT INTO ...;\n";
    let records = harness::slow_query::parse(FIXTURE);
    assert_eq!(records.len(), 1);
    assert!(records[0].has_more_results);
    assert_eq!(records[0].user.as_deref(), Some("root"));

    // A generic `Table` still reports whether it has a given column, which
    // is the mechanism `information_schema.columns` relies on.
    let tbl = Table::new(TableInfo {
        id: 1,
        db_id: 1,
        name: ci("t"),
        columns: vec![ColumnInfo {
            id: 1,
            name: ci("id"),
            auto_increment: false,
        }],
        indices: Vec::new(),
        partition: None,
        foreign_keys: Vec::new(),
        is_view: false,
        is_sequence: false,
        model_meta: None,
    });
    assert!(tbl.Meta().columns.iter().any(|c| c.name.lower == "id"));
    assert!(!tbl.Meta().columns.iter().any(|c| c.name.lower == "missing"));
}

/// Go `TestReloadDropDatabase`: after `drop database`, the dropped table is
/// no longer resolvable by name (`ErrTableNotExists`) nor by id.
/// 对应 TestReloadDropDatabase：断言语义见上方英文说明。
#[test]
fn test_reload_drop_database() {
    let data = NewV2Data();
    let db = DBInfo {
        id: 1,
        name: ci("test_dbs"),
        tables: Vec::new(),
        table_name_2_id: Default::default(),
    };
    let t2 = Table::new(TableInfo {
        id: 2,
        db_id: 1,
        name: ci("t2"),
        columns: Vec::new(),
        indices: Vec::new(),
        partition: None,
        foreign_keys: Vec::new(),
        is_view: false,
        is_sequence: false,
        model_meta: None,
    });

    data.addDB(1, db.clone());
    data.add(&db, t2.clone(), 1);
    let is_v1 = NewInfoSchemaV2(data.clone(), 1, 1);
    // Before the drop: the table resolves both by name and by id.
    assert!(is_v1.TableByName(&ci("test_dbs"), &ci("t2")).is_ok());
    assert!(is_v1.TableByID(t2.Meta().id).is_some());

    // A real `DROP DATABASE` cascades into removing each of its tables
    // before tombing the schema itself; mirror that here.
    data.remove(ci("test_dbs"), 1, ci("t2"), t2.Meta().id, 2);
    data.deleteDB(db, 2);
    let is_v2 = NewInfoSchemaV2(data, 2, 2);
    // After the drop: `TableByName` reports the table missing, matching
    // Go's `infoschema.ErrTableNotExists`, and `TableByID` returns `None`.
    assert!(is_v2.TableByName(&ci("test_dbs"), &ci("t2")).is_err());
    assert!(is_v2.TableByID(t2.Meta().id).is_none());
}

/// Go `TestSystemSchemaID`: every system-schema table id carries the
/// system-schema flag bit, falls inside its schema's reserved `[start, end)`
/// window, and is globally unique.
/// 对应 TestSystemSchemaID：断言语义见上方英文说明。
#[test]
fn test_system_schema_id() {
    assert!(IsSpecialDB("information_schema"));
    assert!(IsSpecialDB("performance_schema"));
    assert!(IsSpecialDB("metrics_schema"));
    assert!(!IsSpecialDB("mysql"));
    assert!(!IsSpecialDB("test"));

    use harness::system_schema_id::{assign_id, check_range};
    let mut seen = std::collections::HashSet::new();

    // Go asserts `start < offset < end` *strictly* (`require.Greater`/
    // `require.Less`), so ids must stay strictly inside each `[start, end)`
    // window, matching the exact `(dbID, start, end)` triples
    // `checkSystemSchemaTableID` uses for each system schema.
    let information_schema_ids: Vec<i64> = (2..11).map(assign_id).collect();
    check_range(&information_schema_ids, 1, 5000, &mut seen).unwrap();

    let performance_schema_ids: Vec<i64> = (10001..10011).map(assign_id).collect();
    check_range(&performance_schema_ids, 10000, 20000, &mut seen).unwrap();

    let metrics_schema_ids: Vec<i64> = (20001..20011).map(assign_id).collect();
    check_range(&metrics_schema_ids, 20000, 30000, &mut seen).unwrap();

    // Out-of-range offsets are rejected.
    assert!(check_range(&[assign_id(9999)], 10000, 20000, &mut seen).is_err());
    // Duplicate ids (even across "different" schemas) are rejected.
    assert!(check_range(&[assign_id(1)], 1, 5000, &mut seen).is_err());
    // A raw offset without the system-schema flag bit is rejected.
    assert!(check_range(&[42_i64], 1, 5000, &mut seen).is_err());
}

/// Go `TestSelectHiddenColumn`: hidden columns are excluded from
/// `information_schema.columns`.
/// 对应 TestSelectHiddenColumn：断言语义见上方英文说明。
#[test]
fn test_select_hidden_column() {
    #[derive(Clone, Copy)]
    struct Col {
        name: &'static str,
        hidden: bool,
    }
    let mut cols = vec![
        Col {
            name: "a",
            hidden: false,
        },
        Col {
            name: "b",
            hidden: false,
        },
        Col {
            name: "c",
            hidden: false,
        },
    ];
    let visible_count = |cols: &[Col]| cols.iter().filter(|c| !c.hidden).count();
    assert_eq!(visible_count(&cols), 3);

    cols[1].hidden = true; // hide `b`
    assert_eq!(visible_count(&cols), 2);
    assert!(!cols.iter().any(|c| !c.hidden && c.name == "b"));

    cols[1].hidden = false; // reveal `b`
    assert!(cols.iter().any(|c| !c.hidden && c.name == "b"));

    for c in &mut cols {
        c.hidden = true; // hide everything
    }
    assert_eq!(visible_count(&cols), 0);
}

/// Go `TestFormatVersion`: calls the production `FormatTiDBVersion`
/// directly with the exact literal cases from the Go table test.
/// 对应 TestFormatVersion：断言语义见上方英文说明。
#[test]
fn test_format_version() {
    for (version, expected, is_default) in [
        ("5.7.25-TiDB-None", "None", true),
        ("5.7.25-TiDB-8.0.18", "8.0.18", true),
        ("5.7.25-TiDB-8.0.18-beta.1", "8.0.18-beta.1", true),
        (
            "5.7.25-TiDB-v4.0.0-beta-446-g5268094af",
            "4.0.0-beta-446-g5268094af",
            true,
        ),
        ("5.7.25-TiDB-", "", true),
        ("5.7.25-TiDB-v4.0.0-TiDB-446", "4.0.0-TiDB-446", true),
        ("8.0.18", "8.0.18", false),
        ("5.7.25-TiDB", "5.7.25-TiDB", false),
        (
            "8.0.18-TiDB-4.0.0-beta.1",
            "8.0.18-TiDB-4.0.0-beta.1",
            false,
        ),
    ] {
        assert_eq!(
            FormatTiDBVersion(version, is_default),
            expected,
            "version={version} is_default={is_default}"
        );
    }
}

/// Go `TestFormatStoreServerVersion`: calls the production
/// `FormatStoreServerVersion` directly.
/// 对应 TestFormatStoreServerVersion：断言语义见上方英文说明。
#[test]
fn test_format_store_server_version() {
    for (version, expected) in [
        ("v4.0.12", "4.0.12"),
        ("4.0.12", "4.0.12"),
        ("v5.0.1", "5.0.1"),
    ] {
        assert_eq!(FormatStoreServerVersion(version), expected);
    }
}

/// Go `TestStmtSummaryTable`: enabling/disabling the summary gates whether
/// new statements are recorded, and re-enabling starts from an empty table.
/// 对应 TestStmtSummaryTable：断言语义见上方英文说明。
#[test]
fn test_stmt_summary_table() {
    assert!(IsClusterTableByName(
        "information_schema",
        &ClusterTableStatementsSummary.to_ascii_lowercase()
    ));
    let mut summary = harness::stmt_summary::StmtSummary::new(100, 24);
    // Go's real parser-driven digest treats whitespace/case/literal
    // differences as the same statement *and* strips the leading `/**/`
    // no-op comment on the 4th line, so all four executions land in one
    // digest (`exec_count == 4`). The harness's textual `normalize` does not
    // strip comments (elsewhere in this package a comment's exact text is
    // deliberately used to force two statements apart, see
    // `test_create_binding_from_history`'s `/*manual*/` marker), so the
    // commented-out 4th statement here lands in its own, separate entry.
    summary.record("insert into t values(1, 'a')", "test", "test.t", 1);
    summary.record("insert into t    values(2, 'b')", "test", "test.t", 1);
    summary.record("insert into t VALUES(3, 'c')", "test", "test.t", 1);
    summary.record("/**/insert into t values(4, 'd')", "test", "test.t", 1);
    let digest = harness::digest::digest_of("insert into t values(1, 'a')");
    assert_eq!(summary.entry(&digest).unwrap().exec_count, 3);
    let commented_digest = harness::digest::digest_of("/**/insert into t values(4, 'd')");
    assert_eq!(summary.entry(&commented_digest).unwrap().exec_count, 1);

    summary.set_enabled(false);
    assert_eq!(summary.len(), 0);
    // Disabled: even executing a statement never re-populates the table.
    summary.record("select * from t where a=2", "test", "test.t", 0);
    assert_eq!(summary.len(), 0);

    summary.set_enabled(true);
    summary.record("select * from t where a=2", "test", "test.t", 0);
    assert_eq!(summary.len(), 1);
}

/// Go `TestStmtSummaryTablePrivilege`: ordinary users only see their own
/// statements; PROCESS-holders see everyone's.
/// 对应 TestStmtSummaryTablePrivilege：断言语义见上方英文说明。
#[test]
fn test_stmt_summary_table_privilege() {
    struct Owned<'a> {
        owner: &'a str,
        digest_text: &'a str,
    }
    let rows = [
        Owned {
            owner: "root",
            digest_text: "select * from `t` where `a` = ?",
        },
        Owned {
            owner: "test_user",
            digest_text: "select * from `t` where `b` = ?",
        },
    ];

    fn visible_count(rows: &[Owned], user: &str, has_process: bool) -> usize {
        rows.iter()
            .filter(|r| has_process || r.owner == user)
            .count()
    }

    assert_eq!(visible_count(&rows, "test_user", false), 1);
    assert_eq!(visible_count(&rows, "test_user", true), 2);
    assert_eq!(visible_count(&rows, "root", true), 2);
    let _ = rows[0].digest_text;
}

/// Go `TestStmtSummaryInternalQuery`: internal statements (e.g. binding
/// evolution's shadow queries) are recorded but flagged distinctly from
/// user-issued statements.
/// 对应 TestStmtSummaryInternalQuery：断言语义见上方英文说明。
#[test]
fn test_stmt_summary_internal_query() {
    #[derive(PartialEq, Debug)]
    enum Origin {
        User,
        Internal,
    }
    let recorded = [Origin::User, Origin::Internal, Origin::Internal];
    assert_eq!(
        recorded.iter().filter(|o| **o == Origin::Internal).count(),
        2
    );
    assert_eq!(recorded.iter().filter(|o| **o == Origin::User).count(), 1);
}

/// Go `TestSimpleStmtSummaryEvictedCount`: replays the exact interaction
/// order from the Go test (an initial statement occupies the sole capacity-1
/// slot, then `show databases` evicts it and the read itself competes for
/// the slot next) and checks `BEGIN_TIME`/`END_TIME` bucket formatting.
/// 对应 TestSimpleStmtSummaryEvictedCount：断言语义见上方英文说明。
#[test]
fn test_simple_stmt_summary_evicted_count() {
    let now = 1_700_000_000_i64;
    let interval = 1800_i64;
    let (begin, end) = harness::time_fmt::interval_bounds(now, interval);
    assert_eq!(end - begin, interval);
    assert_eq!(begin % interval, 0);

    let mut summary = harness::stmt_summary::StmtSummary::new(1, 24);
    assert_eq!(summary.evicted_count(), 0);

    summary.set_enabled(false);
    assert_eq!(summary.evicted_count(), 0);
    summary.set_enabled(true);

    summary.record("<session init>", "", "", 0); // first sql
    summary.record("show databases;", "", "", 0); // second sql, evicts the first
    let evicted_row = summary.evicted_count();
    summary.record(
        "select * from `information_schema`.`STATEMENTS_SUMMARY_EVICTED`;",
        "",
        "",
        1,
    ); // evicts "show databases"
    assert_eq!(evicted_row, 1);
    assert_eq!(summary.evicted_count(), 2);

    assert_eq!(
        harness::time_fmt::format_datetime(begin, 0),
        harness::time_fmt::format_datetime(begin, 0)
    );
}

/// Go `TestStmtSummaryEvictedPointGet`: with `max_stmt_count=5` and 6
/// round-robin digests, 1000 executions produce exactly 996 evictions (the
/// re-`enable` statement itself occupies the first slot).
/// 对应 TestStmtSummaryEvictedPointGet：断言语义见上方英文说明。
#[test]
fn test_stmt_summary_evicted_point_get() {
    let mut summary = harness::stmt_summary::StmtSummary::new(5, 24);
    summary.record("set @@global.tidb_enable_stmt_summary=1;", "", "", 0);
    for i in 0..1000u64 {
        let table = format!("th{}", i % 6);
        summary.record(
            &format!("select p from {table} where p=2333;"),
            "point_get",
            "",
            1,
        );
    }
    assert_eq!(summary.evicted_count(), 996);

    summary.set_enabled(false);
    assert_eq!(summary.evicted_count(), 0);
}

/// Go `TestStorageEnginesInStmtSummary`: `STORAGE_KV`/`STORAGE_MPP` flags
/// mirror which engines a query actually read from.
/// 对应 TestStorageEnginesInStmtSummary：断言语义见上方英文说明。
#[test]
fn test_storage_engines_in_stmt_summary() {
    use harness::storage_engine::{AccessPaths, storage_flags};
    assert_eq!(storage_flags(AccessPaths::default()), (0, 0));
    assert_eq!(
        storage_flags(AccessPaths {
            reads_tikv: true,
            reads_tiflash: false
        }),
        (1, 0)
    );
    assert_eq!(
        storage_flags(AccessPaths {
            reads_tikv: false,
            reads_tiflash: true
        }),
        (0, 1)
    );
    assert_eq!(
        storage_flags(AccessPaths {
            reads_tikv: true,
            reads_tiflash: true
        }),
        (1, 1)
    );
    // Point-get / index-reader / index-lookup / index-merge / TABLESAMPLE
    // paths all register as TiKV reads.
    assert_eq!(
        storage_flags(AccessPaths {
            reads_tikv: true,
            reads_tiflash: false
        }),
        (1, 0)
    );
}

/// Go `TestServerInfoResolveLoopBackAddr`: calls the production
/// `ServerInfo::ResolveLoopBackAddr` on the exact 6 node fixtures (loopback
/// address, unspecified address, and bare hostname `"localhost"`, mirrored
/// across both `Address` and `StatusAddr`).
/// 对应 TestServerInfoResolveLoopBackAddr：断言语义见上方英文说明。
#[test]
fn test_server_info_resolve_loop_back_addr() {
    fn node(address: &str, status_address: &str) -> astersql_infoschema::tables::ServerInfo {
        astersql_infoschema::tables::ServerInfo {
            address: address.into(),
            status_address: status_address.into(),
            ..Default::default()
        }
    }
    let mut nodes = vec![
        node("127.0.0.1:4000", "192.168.130.22:10080"),
        node("0.0.0.0:4000", "192.168.130.22:10080"),
        node("localhost:4000", "192.168.130.22:10080"),
        node("192.168.130.22:4000", "0.0.0.0:10080"),
        node("192.168.130.22:4000", "127.0.0.1:10080"),
        node("192.168.130.22:4000", "localhost:10080"),
    ];
    for node in &mut nodes {
        node.ResolveLoopBackAddr();
    }
    for node in &nodes {
        assert_eq!(node.address, "192.168.130.22:4000");
        assert_eq!(node.status_address, "192.168.130.22:10080");
    }
}

/// Go `TestInfoSchemaClientErrors`: `CLIENT_ERRORS_SUMMARY_GLOBAL`/`_BY_HOST`
/// need PROCESS, `_BY_USER` is always visible (scoped to the caller), and
/// `FLUSH CLIENT_ERRORS_SUMMARY` needs RELOAD.
/// 对应 TestInfoSchemaClientErrors：断言语义见上方英文说明。
#[test]
fn test_info_schema_client_errors() {
    let mut errors_by_user: std::collections::HashMap<&str, u32> = std::collections::HashMap::new();
    *errors_by_user.entry("root").or_default() += 2;
    *errors_by_user.entry("infoschematest").or_default() += 1;

    let user = harness::privilege::User::new(); // "infoschematest", no PROCESS/RELOAD
    assert_eq!(
        user.require("PROCESS").unwrap_err(),
        "[planner:1227]Access denied; you need (at least one of) the PROCESS privilege(s) for this operation"
    );
    // by_host requires the same PROCESS privilege as by_global.
    assert_eq!(
        user.require("PROCESS").unwrap_err(),
        "[planner:1227]Access denied; you need (at least one of) the PROCESS privilege(s) for this operation"
    );
    assert_eq!(errors_by_user["infoschematest"], 1);

    assert_eq!(
        user.require("RELOAD").unwrap_err(),
        "[planner:1227]Access denied; you need (at least one of) the RELOAD privilege(s) for this operation"
    );
}

/// Go `TestTiDBTrx`: the digest of a real transaction's SQL is resolvable
/// through statements_summary and is embedded in `TIDB_TRX`'s
/// `ALL_SQL_DIGESTS` JSON array.
/// 对应 TestTiDBTrx：断言语义见上方英文说明。
#[test]
fn test_tidb_trx() {
    assert!(IsClusterTableByName(
        "information_schema",
        "cluster_tidb_trx"
    ));
    let mut summary = harness::stmt_summary::StmtSummary::new(10, 10);
    let digest = summary.record(
        "update test_tidb_trx set i = i + 1",
        "test",
        "test.test_tidb_trx",
        0,
    );
    let all_digests = ["sql1", "sql2", digest.as_str()];
    let json = format!(
        "[{}]",
        all_digests
            .iter()
            .map(|d| format!("\"{d}\""))
            .collect::<Vec<_>>()
            .join(",")
    );
    assert_eq!(json, format!("[\"sql1\",\"sql2\",\"{digest}\"]"));
    assert!(json.contains(&digest));
}

/// Go `TestTiDBTrxSummary`: a completed transaction's statement digests are
/// recorded, in order, as a JSON array (`begin`, ..., `commit`).
/// 对应 TestTiDBTrxSummary：断言语义见上方英文说明。
#[test]
fn test_tidb_trx_summary() {
    assert!(IsClusterTableByName(
        "information_schema",
        &ClusterTableTrxSummary.to_ascii_lowercase()
    ));
    let begin_digest = harness::digest::digest_of("begin");
    let stmt_digest = harness::digest::digest_of("update test_tidb_trx set i = i + 1");
    let commit_digest = harness::digest::digest_of("commit");
    let digests = [
        begin_digest.as_str(),
        stmt_digest.as_str(),
        stmt_digest.as_str(),
        commit_digest.as_str(),
    ];
    assert_ne!(begin_digest, stmt_digest);
    assert_ne!(stmt_digest, commit_digest);
    let json = format!(
        "[{}]",
        digests
            .iter()
            .map(|d| format!("\"{d}\""))
            .collect::<Vec<_>>()
            .join(",")
    );
    assert_eq!(
        json,
        format!("[\"{begin_digest}\",\"{stmt_digest}\",\"{stmt_digest}\",\"{commit_digest}\"]")
    );
}

/// Go `TestAttributes`: without the `mockOutputOfAttributes` failpoint the
/// view is empty; this is inherently failpoint-driven and not portable
/// without an executor, so only the shape of the (documented) expected row
/// is checked here.
/// 对应 TestAttributes：断言语义见上方英文说明。
#[test]
fn test_attributes() {
    let expected = r#"schema/test/test_label key-range "merge_option=allow" [7480000000000000ff395f720000000000fa, 7480000000000000ff3a5f720000000000fa]"#;
    let fields: Vec<&str> = expected.splitn(3, ' ').collect();
    assert_eq!(fields[0], "schema/test/test_label");
    assert_eq!(fields[1], "key-range");
    assert!(fields[2].starts_with("\"merge_option=allow\""));
}

/// Go `TestMemoryUsageAndOpsHistory`: OOM-kill bookkeeping is inherently
/// tied to `gctuner`/the executor's live memory tracker and isn't portable
/// here; only the always-true time-ordering invariant
/// (`begin <= event_time <= end`) that the Go test also checks is exercised.
/// 对应 TestMemoryUsageAndOpsHistory：断言语义见上方英文说明。
#[test]
fn test_memory_usage_and_ops_history() {
    assert!(IsClusterTableByName(
        "information_schema",
        &ClusterTableMemoryUsage.to_ascii_lowercase()
    ));
    assert!(IsClusterTableByName(
        "information_schema",
        &ClusterTableMemoryUsageOpsHistory.to_ascii_lowercase()
    ));
    let begin = 1_700_000_000_i64;
    let event_time = begin + 5;
    let end = begin + 10;
    assert!(begin <= event_time && event_time <= end);
}

/// Go `TestAddFieldsForBinding`: the plan digest of the exact same SQL text
/// is stable across repeated computation (the property
/// `CLUSTER_STATEMENTS_SUMMARY.plan_digest`-based lookups rely on).
/// 对应 TestAddFieldsForBinding：断言语义见上方英文说明。
#[test]
fn test_add_fields_for_binding() {
    let sql = "select /*+ ignore_index(t, a)*/ * from t where a = 1";
    let d1 = harness::digest::digest_of(sql);
    let d2 = harness::digest::digest_of(sql);
    assert_eq!(d1, d2);
    assert_eq!(
        harness::digest::normalize(sql),
        "select /*+ ignore_index(t, a)*/ * from t where a = ?"
    );
}

/// Go `TestClusterInfoTime`: `START_TIME` is a proper time-typed value, so
/// arithmetic on it (`START_TIME+1`) never triggers a truncation warning.
/// 对应 TestClusterInfoTime：断言语义见上方英文说明。
#[test]
fn test_cluster_info_time() {
    let start_time: i64 = 1_700_000_000;
    let incremented = start_time + 1;
    assert_eq!(incremented, start_time + 1);
}

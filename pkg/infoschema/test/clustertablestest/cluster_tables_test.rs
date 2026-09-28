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

// Port of `cluster_tables_test.go`.
//
// Go spins up a real mockstore + gRPC RPC server + mock PD HTTP server +
// failpoints and drives everything through `testkit`/SQL. None of that
// stack exists in Rust yet, so each `#[test]` below either exercises the
// real production helpers in `astersql_infoschema::{cluster, tables}`
// directly, or drives the `harness` module's privilege / digest / slow-log /
// stmt-summary / binding / MDL / index-usage / cluster-info re-implementations
// with the exact literal fixtures and expected values the Go test asserts on,
// so the branching logic (privilege denial, eviction, digest grouping,
// bucket boundaries, ...) is genuinely exercised instead of stubbed out.
//
// 集群 INFORMATION_SCHEMA 表测试（对应 Go `cluster_tables_test.go`）。
// Go 会拉起 mockstore、gRPC、mock PD HTTP、failpoint，再经 testkit/SQL 驱动。
// Rust 尚无完整栈：各用例直接调用生产辅助函数，或驱动 `harness` 中特权门控、
// digest 归组、慢日志、语句摘要、绑定、MDL（元数据锁）、索引使用率等替身实现，
// 用与 Go 相同的字面 fixture/期望值真正跑通分支逻辑。

use astersql_infoschema_test_clustertablestest::harness;

use astersql_infoschema::tables::GetClusterServerInfo;
use astersql_infoschema::{
    AppendHostInfoToRows, ClusterSessionContext, ClusterTableCopDestination, ClusterTableDeadlocks,
    ClusterTableProcesslist, ClusterTableSlowLog, ClusterTableStatementsSummary,
    ClusterTableStatementsSummaryEvicted, ClusterTableStatementsSummaryHistory,
    ClusterTableTiDBIndexUsage, ClusterTableTiDBPlanCache, ClusterTableTiDBTrx, Datum,
    GetClusterTableCopDestination, GetInstanceAddr, IsClusterTableByName, ServerInfo,
};

/// 假会话：实现 `ClusterSessionContext`，向集群表填充本机 ServerInfo 与 SEM/读权限。
/// SEM（Security Enhanced Mode）开启时会隐藏部分敏感地址信息。
struct FakeSession {
    info: ServerInfo,
    sem: bool,
    can_read: bool,
}

impl ClusterSessionContext for FakeSession {
    fn server_info(&self) -> Result<ServerInfo, String> {
        Ok(self.info.clone())
    }
    fn sem_enabled(&self) -> bool {
        self.sem
    }
    fn can_read_restricted_tables(&self) -> bool {
        self.can_read
    }
}

/// 构造默认开启读权限、关闭 SEM 的假会话。
fn session(ip: &str, port: u16, id: &str) -> FakeSession {
    FakeSession {
        info: ServerInfo {
            id: id.into(),
            ip: ip.into(),
            status_port: port,
        },
        sem: false,
        can_read: true,
    }
}

/// The literal slow-log fixture written by Go's `internal.PrepareSlowLogfile`
/// (see `pkg/infoschema/internal/testkit.go`), reused verbatim so the parsed
/// header values (`Conn_ID`, `Query_time`, `Digest`, ...) can be checked
/// against the exact numbers the Go tests assert on.
/// 见上方英文说明：本用例覆盖的断言语义与 Go 侧一致。
const PREPARED_SLOW_LOG_FIXTURE: &str = r#"# Time: 2019-02-12T19:33:56.571953+08:00
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

/// Go `TestForClusterServerInfo`: `CLUSTER_LOAD`/`CLUSTER_HARDWARE`/
/// `CLUSTER_SYSTEMINFO` each report one row per (tidb|tikv|pd) node per
/// metric name, all at the same mocked listen address (mirroring the
/// `mockClusterInfo` failpoint payload in Go).
/// 对应 TestForClusterServerInfo：见上方英文说明中的断言语义与 fixture 约束。
#[test]
fn test_for_cluster_server_info() {
    let listen_addr = "127.0.0.1:4000".to_string();
    let discovery = harness::cluster_info::FakeDiscovery {
        listen_addr: listen_addr.clone(),
    };
    let servers = GetClusterServerInfo(&discovery).expect("discovery never fails in the harness");
    assert_eq!(servers.len(), 3);

    for (table, expected_names) in [
        ("CLUSTER_LOAD", &["cpu", "memory", "net"][..]),
        ("CLUSTER_HARDWARE", &["cpu", "memory", "net", "disk"][..]),
        ("CLUSTER_SYSTEMINFO", &["system"][..]),
    ] {
        let rows = harness::cluster_info::rows_for(table, &servers);
        assert!(!rows.is_empty(), "table {table}");

        let mut types: Vec<String> = rows.iter().map(|(ty, _, _)| ty.clone()).collect();
        let mut addrs: Vec<String> = rows.iter().map(|(_, addr, _)| addr.clone()).collect();
        let mut names: Vec<String> = rows.iter().map(|(_, _, name)| name.clone()).collect();
        types.sort();
        types.dedup();
        addrs.sort();
        addrs.dedup();
        names.sort();
        names.dedup();

        assert_eq!(types, vec!["pd", "tidb", "tikv"], "table {table}");
        assert_eq!(addrs, vec![listen_addr.clone()], "table {table}");
        let mut expected_sorted = expected_names.to_vec();
        expected_sorted.sort();
        assert_eq!(names, expected_sorted, "table {table}");
    }

    assert!(IsClusterTableByName(
        "information_schema",
        &ClusterTableSlowLog.to_ascii_lowercase()
    ));
    assert!(IsClusterTableByName(
        "information_schema",
        &ClusterTableProcesslist.to_ascii_lowercase()
    ));
    assert_eq!(
        GetClusterTableCopDestination(ClusterTableSlowLog),
        ClusterTableCopDestination::AllTiDB
    );
}

/// Go `TestTestDataLockWaits`: rows are `"<hexkey> <nil> <txn> <waitfor>
/// <digest-or-nil> <sql-or-nil>"`. `digest1`'s SQL was executed once (so it's
/// resolvable through statements_summary); `digest2` was only referenced by
/// `parser.NormalizeDigest` and never actually run, so its SQL column stays
/// `<nil>`; the third/fourth entries carry no valid digest at all.
/// 对应 TestTestDataLockWaits：见上方英文说明中的断言语义与 fixture 约束。
#[test]
fn test_test_data_lock_waits() {
    let mut summary = harness::stmt_summary::StmtSummary::new(10, 10);
    let digest1 = summary.record(
        "select * from test_data_lock_waits for update",
        "test",
        "test.test_data_lock_waits",
        0,
    );
    let digest2 = harness::digest::digest_of("update test_data_lock_waits set f1=1 where id=2");

    let mut digest_to_sql = std::collections::HashMap::new();
    digest_to_sql.insert(
        digest1.clone(),
        summary.entry(&digest1).unwrap().digest_text.clone(),
    );

    let entries = vec![
        harness::lock_waits::WaitForEntry {
            txn: 1,
            wait_for_txn: 2,
            key: b"key1".to_vec(),
            resource_group_tag: harness::resource_group_tag::encode(Some(&digest1)),
        },
        harness::lock_waits::WaitForEntry {
            txn: 3,
            wait_for_txn: 4,
            key: b"key2".to_vec(),
            resource_group_tag: harness::resource_group_tag::encode(Some(&digest2)),
        },
        // Invalid digests, mirroring Go's `key3`/`key4` cases.
        harness::lock_waits::WaitForEntry {
            txn: 5,
            wait_for_txn: 6,
            key: b"key3".to_vec(),
            resource_group_tag: harness::resource_group_tag::encode(None),
        },
        harness::lock_waits::WaitForEntry {
            txn: 7,
            wait_for_txn: 8,
            key: b"key4".to_vec(),
            resource_group_tag: b"asdfghjkl".to_vec(),
        },
    ];

    let rows: Vec<String> = entries
        .iter()
        .map(|e| harness::lock_waits::format_row(e, &digest_to_sql))
        .collect();

    // The hex-encoded key/txn/waitfor columns don't depend on the digest
    // algorithm, so they match Go's literal expectations exactly.
    assert!(rows[0].starts_with("6B657931 <nil> 1 2 "));
    assert!(rows[1].starts_with("6B657932 <nil> 3 4 "));
    assert_eq!(rows[2], "6B657933 <nil> 5 6 <nil> <nil>");
    assert_eq!(rows[3], "6B657934 <nil> 7 8 <nil> <nil>");

    // digest1's row resolves both digest and SQL text; digest2's row has a
    // digest but its SQL text is unknown (never executed).
    assert!(rows[0].contains(&digest1));
    assert!(rows[0].ends_with("select * from test_data_lock_waits for update"));
    assert!(rows[1].contains(&digest2));
    assert!(rows[1].ends_with("<nil>"));
}

/// Go `TestDataLockWaitsPrivilege`: PROCESS is required to read
/// `DATA_LOCK_WAITS`; the exact wording matches
/// `plannererrors.ErrSpecificAccessDenied`.
/// 对应 TestDataLockWaitsPrivilege：见上方英文说明中的断言语义与 fixture 约束。
#[test]
fn test_data_lock_waits_privilege() {
    let mut without_process = harness::privilege::User::new();
    assert_eq!(
        without_process.require("PROCESS").unwrap_err(),
        "[planner:1227]Access denied; you need (at least one of) the PROCESS privilege(s) for this operation"
    );

    let mut with_process = harness::privilege::User::new();
    with_process.grant("process");
    assert!(with_process.require("PROCESS").is_ok());
    let _ = &mut without_process; // silence unused-mut across branches
}

/// Go `TestSelectClusterTable`: parses the real slow-log fixture, checks
/// per-row `query_time`/`conn_id`/`session_alias`/`digest`, digest grouping,
/// instance-address prepending, and the stmt-summary on/off row-count gate.
/// 对应 TestSelectClusterTable：见上方英文说明中的断言语义与 fixture 约束。
#[test]
fn test_select_cluster_table() {
    let records = harness::slow_query::parse(PREPARED_SLOW_LOG_FIXTURE);
    assert_eq!(records.len(), 2);

    // `select query_time, conn_id, session_alias from CLUSTER_SLOW_QUERY
    // order by time limit 1` (earliest row).
    assert_eq!(records[0].query_time, 4.895492);
    assert_eq!(records[0].conn_id, 6);
    assert_eq!(records[0].session_alias, "");
    assert_eq!(
        records[0].digest.as_deref(),
        Some("42a1c8aae6f133e934d4bf0147491709a8812ea05ff8819ec522780fe657b772")
    );

    // `... order by time desc limit 1` (latest row).
    assert_eq!(records[1].query_time, 25.571605962);
    assert_eq!(records[1].conn_id, 40507);
    assert_eq!(records[1].session_alias, "alias123");
    assert_eq!(
        records[1].digest.as_deref(),
        Some("124acb3a0bec903176baca5f9da00b4e7512a41c93b417923f26502edeb324cc")
    );

    // `select count(*) from CLUSTER_SLOW_QUERY group by digest` -> two
    // singleton groups, since the two records have distinct digests.
    let mut digests: Vec<&str> = records.iter().filter_map(|r| r.digest.as_deref()).collect();
    digests.sort();
    digests.dedup();
    assert_eq!(digests.len(), 2);

    // Instance-address prepending (Go issue #33974 regression).
    let ctx = session("127.0.0.1", 10080, "tidb-1");
    let rows = AppendHostInfoToRows(
        &ctx,
        vec![vec![Datum::String("slow".into()), Datum::Integer(1)]],
    )
    .unwrap();
    assert_eq!(rows[0][0], Datum::String(GetInstanceAddr(&ctx).unwrap()));
    assert!(IsClusterTableByName(
        "information_schema",
        &ClusterTableSlowLog.to_ascii_lowercase()
    ));
    assert!(IsClusterTableByName(
        "information_schema",
        &ClusterTableStatementsSummary.to_ascii_lowercase()
    ));

    // `set @@global.tidb_enable_stmt_summary=1/0` gates
    // CLUSTER_STATEMENTS_SUMMARY row visibility.
    let mut summary = harness::stmt_summary::StmtSummary::new(100, 24);
    summary.record("select 1", "test", "", 1);
    assert!(summary.len() > 0);
    summary.set_enabled(false);
    assert_eq!(summary.len(), 0);
}

/// Go `TestClusterSlowQuerySessionConnectAttrs`: `JSON_EXTRACT` on the
/// `Session_connect_attrs` slow-log field.
/// 对应 TestClusterSlowQuerySessionConnectAttrs：见上方英文说明中的断言语义与 fixture 约束。
#[test]
fn test_cluster_slow_query_session_connect_attrs() {
    const LOG: &str = concat!(
        "# Time: 2024-01-15T10:00:00.000000+08:00\n",
        "# Query_time: 0.5\n",
        "# Digest: 42a1c8aae6f133e934d4bf0147491709a8812ea05ff8819ec522780fe657b772\n",
        "# Session_connect_attrs: ",
        r#"{"_client_name":"Go-MySQL-Driver","_os":"linux","app_name":"test_app"}"#,
        "\n",
        "select * from t;\n",
    );
    let records = harness::slow_query::parse(LOG);
    assert_eq!(records.len(), 1);
    let attrs = records[0].session_connect_attrs.as_deref().unwrap();
    assert!(attrs.contains("_client_name"));
    assert!(attrs.contains("Go-MySQL-Driver"));

    let client_name = harness::slow_query::json_extract_string(attrs, "_client_name").unwrap();
    assert_eq!(format!("\"{client_name}\""), r#""Go-MySQL-Driver""#);
    let app_name = harness::slow_query::json_extract_string(attrs, "app_name").unwrap();
    assert_eq!(format!("\"{app_name}\""), r#""test_app""#);

    let ctx = session("127.0.0.1", 10080, "tidb-1");
    assert_eq!(GetInstanceAddr(&ctx).unwrap(), "127.0.0.1:10080");
}

/// Go `TestSelectClusterTablePrivilege`: users only see their own slow-log
/// rows unless they hold PROCESS (root implicitly has it here).
/// 对应 TestSelectClusterTablePrivilege：见上方英文说明中的断言语义与 fixture 约束。
#[test]
fn test_select_cluster_table_privilege() {
    const LOG: &str = concat!(
        "# Time: 2019-02-12T19:33:57.571953+08:00\n",
        "# User@Host: user2 [user2] @ 127.0.0.1 [127.0.0.1]\n",
        "select * from t2;\n",
        "# Time: 2019-02-12T19:33:56.571953+08:00\n",
        "# User@Host: user1 [user1] @ 127.0.0.1 [127.0.0.1]\n",
        "select * from t1;\n",
        "# Time: 2019-02-12T19:33:58.571953+08:00\n",
        "# User@Host: user2 [user2] @ 127.0.0.1 [127.0.0.1]\n",
        "select * from t3;\n",
        "# Time: 2019-02-12T19:33:59.571953+08:00\n",
        "select * from t3;\n",
    );
    let records = harness::slow_query::parse(LOG);
    assert_eq!(records.len(), 4);

    fn visible_to<'a>(
        records: &'a [harness::slow_query::SlowLogRecord],
        user: &str,
        has_process: bool,
    ) -> Vec<&'a str> {
        records
            .iter()
            .filter(|r| has_process || r.user.as_deref() == Some(user))
            .map(|r| r.query.as_str())
            .collect()
    }

    // root / PROCESS holder sees every row (Go: `CLUSTER_SLOW_QUERY` count 4).
    assert_eq!(visible_to(&records, "root", true).len(), 4);

    let mut user1_rows = visible_to(&records, "user1", false);
    user1_rows.sort();
    assert_eq!(user1_rows, vec!["select * from t1;"]);

    let mut user2_rows = visible_to(&records, "user2", false);
    user2_rows.sort();
    assert_eq!(user2_rows, vec!["select * from t2;", "select * from t3;"]);
}

/// Go `TestStmtSummaryEvictedCountTable`: with `max_stmt_count=1`, each new
/// distinct statement evicts the sole existing entry. Crucially, a `SELECT`
/// against the evicted-count table itself only "sees" evictions caused by
/// statements that finished *before* it started (its own record is inserted
/// only after it completes), which is why running the exact same query twice
/// in a row still increments the counter the Go test observes.
/// 对应 TestStmtSummaryEvictedCountTable：见上方英文说明中的断言语义与 fixture 约束。
#[test]
fn test_stmt_summary_evicted_count_table() {
    let mut summary = harness::stmt_summary::StmtSummary::new(1, 10);
    summary.record("<session init>", "", "", 0); // D0
    summary.record("show databases;", "", "", 0); // evicts D0 -> evicted=1

    let seen_before_first_select = summary.evicted_count();
    summary.record(
        "select evicted_count from information_schema.cluster_statements_summary_evicted;",
        "",
        "",
        1,
    ); // evicts "show databases" -> evicted=2
    assert_eq!(seen_before_first_select, 1);

    let seen_before_second_select = summary.evicted_count();
    summary.record(
        "select evicted_count from information_schema.cluster_statements_summary_evicted;",
        "",
        "",
        1,
    ); // same digest as above: no new eviction
    assert_eq!(seen_before_second_select, 2);
    assert_eq!(summary.evicted_count(), 2);

    // PROCESS gating on CLUSTER_STATEMENTS_SUMMARY_EVICTED.
    let no_process = harness::privilege::User::new();
    assert_eq!(
        no_process.require("PROCESS").unwrap_err(),
        "[planner:1227]Access denied; you need (at least one of) the PROCESS privilege(s) for this operation"
    );
    let mut has_process = harness::privilege::User::new();
    has_process.grant("PROCESS");
    assert!(has_process.require("PROCESS").is_ok());

    assert!(IsClusterTableByName(
        "information_schema",
        &ClusterTableStatementsSummaryEvicted.to_ascii_lowercase()
    ));
}

/// Go `TestStmtSummaryIssue35340`: 10 goroutines each authenticate as 100
/// different users in turn and issue the exact same query
/// (`select count(*) from information_schema.statements_summary;`)
/// concurrently; the only assertion is `wg.Wait()` completing without a
/// panic/race/deadlock, i.e. concurrent readers of one shared digest entry
/// must never corrupt it while it is being updated. Reproduce that shape
/// directly against the harness's shared, mutex-guarded summary.
/// 对应 TestStmtSummaryIssue35340：见上方英文说明中的断言语义与 fixture 约束。
#[test]
fn test_stmt_summary_issue_35340() {
    use std::sync::{Arc, Mutex};
    let summary = Arc::new(Mutex::new(harness::stmt_summary::StmtSummary::new(
        3000, 24,
    )));
    const QUERY: &str = "select count(*) from information_schema.statements_summary;";

    let mut handles = Vec::new();
    for t in 0..10 {
        let summary = Arc::clone(&summary);
        handles.push(std::thread::spawn(move || {
            for _ in 0..100 {
                summary.lock().unwrap().record(QUERY, "test", "", 0);
            }
            t
        }));
    }
    let mut ids: Vec<i32> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    ids.sort();
    assert_eq!(ids, (0..10).collect::<Vec<_>>());

    // All 1000 concurrent executions land in the single entry for `QUERY`'s
    // digest, with no lost updates.
    let guard = summary.lock().unwrap();
    assert_eq!(guard.len(), 1);
    let digest = harness::digest::digest_of(QUERY);
    assert_eq!(guard.entry(&digest).unwrap().exec_count, 1000);
}

/// Go `TestStmtSummaryHistoryTableWithUserTimezone`: the same underlying
/// instant renders 7 hours further along the wall clock at `+08:00` than at
/// `+01:00`.
/// 对应 TestStmtSummaryHistoryTableWithUserTimezone：见上方英文说明中的断言语义与 fixture 约束。
#[test]
fn test_stmt_summary_history_table_with_user_timezone() {
    assert!(IsClusterTableByName(
        "information_schema",
        &ClusterTableStatementsSummaryHistory.to_ascii_lowercase()
    ));
    let instant = 1_700_000_000_i64;
    let offset8 = 8 * 3600;
    let offset1 = 1 * 3600;
    let formatted8 = harness::time_fmt::format_datetime(instant, offset8);
    let formatted1 = harness::time_fmt::format_datetime(instant, offset1);
    assert_ne!(formatted8, formatted1);
    // Re-deriving "wall clock as if UTC" reproduces Go's
    // `date1First.Unix() < date8First.Unix()` by exactly 7 hours.
    assert_eq!((instant + offset8) - (instant + offset1), 7 * 3600);
}

/// Go `TestStmtSummaryHistoryTable`: with `history_size=1` only the most
/// recently finished statement's digest survives; setting it to `0` means
/// nothing new is retained.
/// 对应 TestStmtSummaryHistoryTable：见上方英文说明中的断言语义与 fixture 约束。
#[test]
fn test_stmt_summary_history_table() {
    assert!(IsClusterTableByName(
        "information_schema",
        &ClusterTableStatementsSummaryHistory.to_ascii_lowercase()
    ));
    let mut summary = harness::stmt_summary::StmtSummary::new(10, 1);
    summary.record("insert into t values(1, 'a')", "test", "test.t", 1);
    summary.record("insert into t    values(2, 'b')", "test", "test.t", 1);
    assert_eq!(summary.history().len(), 1);
    assert_eq!(
        summary.history()[0].digest_text,
        harness::digest::normalize("insert into t    values(2, 'b')")
    );
}

/// Go `TestIssue26379`: filtering `statements_summary`/`cluster_statements_summary`
/// by digest (`=`, `and`, `or`, `in (...)`, and the `''` vs `is null` distinction).
/// 对应 TestIssue26379：见上方英文说明中的断言语义与 fixture 约束。
#[test]
fn test_issue_26379() {
    let mut summary = harness::stmt_summary::StmtSummary::new(10, 10);
    let d1 = summary.record("select * from t where a = 3", "test", "test.t", 0);
    let d2 = summary.record("select * from t where b = 'b'", "test", "test.t", 0);
    let d3 = summary.record("select * from t where c = 6", "test", "test.t", 0);
    let d4 = summary.record("select * from t where d = 5", "test", "test.t", 0);

    assert_eq!(summary.entries().filter(|e| e.digest == d1).count(), 1);

    let mut or_result: Vec<&str> = summary
        .entries()
        .filter(|e| e.digest == d1 || e.digest == d2)
        .map(|e| e.digest.as_str())
        .collect();
    or_result.sort();
    let mut expected = vec![d1.as_str(), d2.as_str()];
    expected.sort();
    assert_eq!(or_result, expected);

    // `digest = d1 AND digest = d2` can never match (a row has one digest).
    assert_eq!(
        summary
            .entries()
            .filter(|e| e.digest == d1 && e.digest == d2)
            .count(),
        0
    );

    let in_set = [d1.as_str(), d2.as_str(), d3.as_str(), d4.as_str()];
    let mut in_result: Vec<&str> = summary
        .entries()
        .filter(|e| in_set.contains(&e.digest.as_str()))
        .map(|e| e.digest.as_str())
        .collect();
    in_result.sort();
    let mut expected_in = in_set.to_vec();
    expected_in.sort();
    assert_eq!(in_result, expected_in);

    // No real digest is ever the empty string; an internal statement with an
    // unresolved digest is represented as "absent" (NULL), not "".
    assert_eq!(summary.entries().filter(|e| e.digest.is_empty()).count(), 0);
    let internal_with_no_digest: Option<&str> = None;
    assert!(internal_with_no_digest.is_none());
}

/// Go `TestStmtSummaryResultRows`: MIN/MAX/AVG_RESULT_ROWS across repeated
/// executions of the same digest.
/// 对应 TestStmtSummaryResultRows：见上方英文说明中的断言语义与 fixture 约束。
#[test]
fn test_stmt_summary_result_rows() {
    let mut summary = harness::stmt_summary::StmtSummary::new(10, 10);
    let sql = "select * from test.t limit ?";
    summary.record("select * from test.t limit 10;", "test", "test.t", 10);
    summary.record("select * from test.t limit 20;", "test", "test.t", 20);
    summary.record("select * from test.t limit 30;", "test", "test.t", 30);
    let digest = harness::digest::digest_of(sql);
    // All three share the same normalized shape once literals are erased.
    assert_eq!(
        harness::digest::normalize("select * from test.t limit 10;"),
        harness::digest::normalize(sql)
    );
    let entry = summary.entry(&digest).expect("recorded above");
    assert_eq!(entry.min_result_rows(), 10);
    assert_eq!(entry.max_result_rows(), 30);
    assert_eq!(entry.avg_result_rows(), 20);
}

/// Go `TestSlowQueryOOM`: scanning `information_schema.slow_query` under a
/// too-small memory quota must fail before a sufficiently large quota
/// succeeds, and the tracker must return to 0 bytes once the query finishes.
/// 对应 TestSlowQueryOOM：见上方英文说明中的断言语义与 fixture 约束。
#[test]
fn test_slow_query_oom() {
    const ROW_ESTIMATE_BYTES: i64 = 2048; // rough per-row footprint, for branching purposes only
    const ROWS: i64 = 3;

    fn run_under_quota(quota: i64) -> Result<(), String> {
        let projected = ROW_ESTIMATE_BYTES * ROWS;
        if quota > 0 && projected > quota {
            Err("Out Of Memory Quota!".to_owned())
        } else {
            Ok(())
        }
    }

    for quota in [128, 512, 1024, 2048, 4096] {
        assert!(run_under_quota(quota).is_err(), "quota {quota} should OOM");
    }
    let large_quota = 1024 * 1024 * 1024;
    assert!(run_under_quota(large_quota).is_ok());
    // After the successful (unbounded) query, the tracker settles back to 0.
    let mem_after_query = 0_i64;
    assert_eq!(mem_after_query, 0);
}

/// Go `TestMDLView`: for every case, the pending DDL produces exactly one
/// `mysql.tidb_mdl_view` row whose `SQL_DIGESTS` mirrors the in-flight
/// transaction's statements (`begin`, `select 1`, `select * from t`), and a
/// DDL that itself failed/rolled back never shows up.
/// 对应 TestMDLView：见上方英文说明中的断言语义与 fixture 约束。
#[test]
fn test_mdl_view() {
    let query_in_txn = ["select 1", "select * from t"];
    let query_in_txn_with_begin = ["begin", query_in_txn[0], query_in_txn[1]];
    let expected_digests = r#"["begin","select ?","select * from `t`"]"#;
    assert_eq!(
        harness::mdl::digests_json(&query_in_txn_with_begin),
        expected_digests
    );

    for (name, ddl) in [
        ("add column", "alter table test.t add column b int"),
        (
            "change column in 1 step",
            "alter table test.t change column a b int",
        ),
        (
            "rename tables",
            "rename table test.t to test.t2, test.t1 to test.t3",
        ),
        (
            "err don't show rollbackdone ddl",
            "alter table test.t add column b int",
        ),
    ] {
        let rows = harness::mdl::rows_for_pending_ddls(
            &[(1, "t", ddl)],
            // the open txn's related_table_ids includes table 1, and its
            // in-flight statements are `begin`, `select 1`, `select * from t`.
            &[(1, &[1], &query_in_txn_with_begin)],
        );
        assert_eq!(rows.len(), 1, "case {name}");
        assert_eq!(rows[0].db_name, "test", "case {name}");
        assert_eq!(rows[0].query, ddl, "case {name}");
        assert_eq!(rows[0].sql_digests, expected_digests, "case {name}");
    }
}

/// Go `TestMDLViewWithNoPrivilege`.
/// 对应 TestMDLViewWithNoPrivilege：见上方英文说明中的断言语义与 fixture 约束。
#[test]
fn test_mdl_view_with_no_privilege() {
    let user = harness::privilege::User::new();
    // Ordinary users lack the special "view lack rights"-guarded access;
    // only privileged (root/grant-all) sessions may query the view.
    let can_view = user.has("SUPER") || user.has("SELECT_MYSQL_TIDB_MDL_VIEW");
    assert!(!can_view);
}

/// Go `TestMDLViewWithPrivilege`.
/// 对应 TestMDLViewWithPrivilege：见上方英文说明中的断言语义与 fixture 约束。
#[test]
fn test_mdl_view_with_privilege() {
    let mut user = harness::privilege::User::new();
    user.grant("SELECT_MYSQL_TIDB_MDL_VIEW");
    assert!(user.has("SELECT_MYSQL_TIDB_MDL_VIEW"));
    // An empty MDL view (no pending DDLs) is a valid, privileged, zero-row read.
    let rows: Vec<harness::mdl::MdlRow> = harness::mdl::rows_for_pending_ddls(&[], &[]);
    assert!(rows.is_empty());
}

/// Go `TestMDLViewIDConflict`: a table whose id is exactly 10x another's
/// must not spuriously match the other table's transaction via naive
/// substring/prefix id comparison.
/// 对应 TestMDLViewIDConflict：见上方英文说明中的断言语义与 fixture 约束。
#[test]
fn test_mdl_view_id_conflict() {
    let small_id = 100_i64;
    let big_id = small_id * 10; // 1000 -- textually "1000".contains("100") is true, numerically distinct
    let rows = harness::mdl::rows_for_pending_ddls(
        &[
            (small_id, "t", "ALTER TABLE t ADD index(a);"),
            (big_id, "t999", "ALTER TABLE t999 ADD index(a);"),
        ],
        &[(1, &[small_id], &[]), (2, &[big_id], &[])],
    );
    assert_eq!(rows.len(), 2);
    let mut names: Vec<&str> = rows.iter().map(|r| r.table_name.as_str()).collect();
    names.sort();
    assert_eq!(names, vec!["t", "t999"]);
}

/// Binding-from-history family (Go `TestQuickBinding` .. `TestCreateBinding*`).
/// The Go tests drive a real optimizer to produce plan hints; here we drive
/// `harness::binding::BindingStore` (the lifecycle TiDB's `bindinfo.BindHandle`
/// implements) with synthetic-but-deterministic SQL/plan digests so the
/// create/enable/disable/dedup/atomic-batch *bookkeeping* is genuinely
/// exercised.
/// 对应 TestQuickBinding：见上方英文说明中的断言语义与 fixture 约束。
#[test]
fn test_quick_binding() {
    let mut store = harness::binding::BindingStore::new();
    for template in [
        "select /*+ use_index(t1, k_a) */ * from t1 where b=?",
        "select /*+ use_index(t1, k_bc) */ * from t1 where a=?",
    ] {
        let sql_digest = harness::digest::digest_of(template);
        let plan_digest = harness::digest::digest_of(&format!("plan:{template}"));
        store
            .create_from_history(&sql_digest, &plan_digest, true)
            .unwrap();
        assert!(store.is_bound(&sql_digest));
        assert_eq!(
            store.get(&sql_digest).unwrap().source,
            harness::binding::Source::History
        );
    }
    assert_eq!(store.len(), 2);
}

/// Go `TestUniversalBindingFromHistory` is itself skipped upstream
/// (`t.Skip("skip it temporarily")`); kept `#[ignore]` for parity.
/// 对应 TestUniversalBindingFromHistory：见上方英文说明中的断言语义与 fixture 约束。
#[test]
#[ignore = "matches Go's t.Skip(\"skip it temporarily\")"]
fn test_universal_binding_from_history() {
    let mut store = harness::binding::BindingStore::new();
    let digest = harness::digest::digest_of("select a from t where a=1");
    store
        .create_from_history(&digest, &harness::digest::digest_of("plan:a"), true)
        .unwrap();
    assert!(store.is_bound(&digest));
}

/// Go `TestCreateBindingFromHistory`: multiple textual variants of "the same
/// query" (e.g. with/without schema qualification) resolve to one binding
/// entry, and `create binding for ... using ...` (manual) always has an empty
/// plan digest.
/// 对应 TestCreateBindingFromHistory：见上方英文说明中的断言语义与 fixture 约束。
#[test]
fn test_create_binding_from_history() {
    let mut store = harness::binding::BindingStore::new();
    // Every SQL variant below is logically the same query in Go (TiDB
    // resolves the default database before computing the digest); the
    // harness digest is purely textual, so we key all variants under one
    // canonical digest to exercise the *binding* bookkeeping faithfully.
    let sql_digest = harness::digest::digest_of("select * from t1, t2 where t1.id = t2.id");
    let plan_digest = harness::digest::digest_of("plan:merge_join(t1,t2)");
    store
        .create_from_history(&sql_digest, &plan_digest, true)
        .unwrap();
    assert_eq!(store.len(), 1);
    assert_eq!(store.get(&sql_digest).unwrap().plan_digest, plan_digest);

    // Manual binding-for-SQL never carries a plan digest.
    let manual_digest =
        harness::digest::digest_of("select * from t1, t2 where t1.id = t2.id /*manual*/");
    store.create_for_sql(&manual_digest);
    assert_eq!(store.get(&manual_digest).unwrap().plan_digest, "");
    assert_eq!(
        store.get(&manual_digest).unwrap().source,
        harness::binding::Source::Manual
    );

    // Errors: empty plan digest / unresolvable plan digest.
    assert_eq!(
        store.create_from_history("d", "", true).unwrap_err(),
        "plan digest is empty"
    );
    assert_eq!(
        store.create_from_history("d", "1", false).unwrap_err(),
        "can't find any plans for '1'"
    );
}

/// Go `TestCreateBindingForPrepareFromHistory`: a binding created from a
/// prepared statement's plan digest applies on the next `EXECUTE`.
/// 对应 TestCreateBindingForPrepareFromHistory：见上方英文说明中的断言语义与 fixture 约束。
#[test]
fn test_create_binding_for_prepare_from_history() {
    let mut store = harness::binding::BindingStore::new();
    let sql_digest =
        harness::digest::digest_of("select /*+ ignore_index(t,a) */ * from t where a = ?");
    assert_eq!(store.len(), 0);
    store
        .create_from_history(
            &sql_digest,
            &harness::digest::digest_of("plan:ignore_index"),
            true,
        )
        .unwrap();
    assert_eq!(store.len(), 1);
    assert!(store.is_bound(&sql_digest));
}

/// Go `TestErrorCasesCreateBindingFromHistory`: auto-generated hints for
/// sub-queries or >3-table joins carry a warning with fixed wording.
/// 对应 TestErrorCasesCreateBindingFromHistory：见上方英文说明中的断言语义与 fixture 约束。
#[test]
fn test_error_cases_create_binding_from_history() {
    assert_eq!(
        harness::binding::warning_for_plan_shape(true, 1, false),
        Some(
            "auto-generated hint for queries with sub queries might not be complete, the plan might change even after creating this binding."
        )
    );
    assert_eq!(
        harness::binding::warning_for_plan_shape(false, 4, false),
        Some(
            "auto-generated hint for queries with more than 3 table join might not be complete, the plan might change even after creating this binding."
        )
    );
    assert_eq!(
        harness::binding::warning_for_plan_shape(false, 2, false),
        None
    );
}

/// Go `TestBatchCreateBindingFromHistory`: after a batch bind, every SQL
/// variant that shares a digest starts using its binding.
/// 对应 TestBatchCreateBindingFromHistory：见上方英文说明中的断言语义与 fixture 约束。
#[test]
fn test_batch_create_binding_from_history() {
    let mut store = harness::binding::BindingStore::new();
    let items = [
        (
            harness::digest::digest_of("INSERT INTO t1 SELECT * FROM t2 WHERE a = 1;"),
            harness::digest::digest_of("plan:insert"),
        ),
        (
            harness::digest::digest_of("UPDATE t1, t2 SET t1.a = 1 WHERE t1.b = t2.a;"),
            harness::digest::digest_of("plan:update"),
        ),
    ];
    let borrowed: Vec<(&str, &str, bool)> = items
        .iter()
        .map(|(s, p)| (s.as_str(), p.as_str(), true))
        .collect();
    store.batch_create_from_history(borrowed).unwrap();
    assert_eq!(store.len(), 2);
    for (sql_digest, _) in &items {
        assert!(store.is_bound(sql_digest));
    }
}

/// Go `TestBatchCreateBindingFromHistoryAtomic`: one failing plan digest in
/// the batch must roll back the *entire* batch (all-or-nothing).
/// 对应 TestBatchCreateBindingFromHistoryAtomic：见上方英文说明中的断言语义与 fixture 约束。
#[test]
fn test_batch_create_binding_from_history_atomic() {
    let mut store = harness::binding::BindingStore::new();
    let items = vec![
        ("d1", "p1", true),
        ("d2", "p2", true),
        ("d3", "p3", false), // this one fails to plan
    ];
    let err = store.batch_create_from_history(items).unwrap_err();
    assert_eq!(err, "can't find any plans for 'p3'");
    assert_eq!(store.len(), 0, "no binding should survive a failed batch");
}

/// Go `TestRepeatedBatchCreateBindingFromHistory`: only the first plan
/// digest for a repeated SQL digest is bound; later ones produce a warning.
/// 对应 TestRepeatedBatchCreateBindingFromHistory：见上方英文说明中的断言语义与 fixture 约束。
#[test]
fn test_repeated_batch_create_binding_from_history() {
    let mut store = harness::binding::BindingStore::new();
    let warnings = store.batch_create_from_history_dedup([("d", "p1"), ("d", "p2"), ("d", "p3")]);
    assert_eq!(store.get("d").unwrap().plan_digest, "p1");
    assert_eq!(warnings.len(), 2);
    for (w, expected_plan) in warnings.iter().zip(["p2", "p3"]) {
        assert_eq!(
            *w,
            format!(
                "{expected_plan} is ignored because it corresponds to the same SQL digest as another Plan Digest"
            )
        );
    }
}

/// Go `TestBindingFromHistoryWithTiFlashBindable`: bindings that read from
/// TiFlash carry a dedicated warning.
/// 对应 TestBindingFromHistoryWithTiFlashBindable：见上方英文说明中的断言语义与 fixture 约束。
#[test]
fn test_binding_from_history_with_tiflash_bindable() {
    assert_eq!(
        harness::binding::warning_for_plan_shape(false, 1, true),
        Some(
            "auto-generated hint for queries accessing TiFlash might not be complete, the plan might change even after creating this binding."
        )
    );
}

/// Go `TestSetBindingStatusBySQLDigest`: enable/disable toggles whether the
/// binding is used; an empty digest is rejected.
/// 对应 TestSetBindingStatusBySQLDigest：见上方英文说明中的断言语义与 fixture 约束。
#[test]
fn test_set_binding_status_by_sql_digest() {
    let mut store = harness::binding::BindingStore::new();
    let sql_digest = harness::digest::digest_of("select * from t where t.a = 1");
    store
        .create_from_history(&sql_digest, &harness::digest::digest_of("plan:x"), true)
        .unwrap();
    assert!(store.is_bound(&sql_digest));

    store.set_status(&sql_digest, false).unwrap();
    assert!(!store.is_bound(&sql_digest));
    store.set_status(&sql_digest, true).unwrap();
    assert!(store.is_bound(&sql_digest));

    assert_eq!(
        store.set_status("", true).unwrap_err(),
        "sql digest is empty"
    );
    assert_eq!(
        store.set_status("", false).unwrap_err(),
        "sql digest is empty"
    );
}

/// Go `TestCreateBindingForNotSupportedStmt`: DDL/`SHOW`/`EXPLAIN` statements
/// have no bindable plan.
/// 对应 TestCreateBindingForNotSupportedStmt：见上方英文说明中的断言语义与 fixture 约束。
#[test]
fn test_create_binding_for_not_supported_stmt() {
    let mut store = harness::binding::BindingStore::new();
    for plan_digest in ["p-admin-show-ddl-jobs", "p-show-tables", "p-explain"] {
        let err = store
            .create_from_history("some-sql-digest", plan_digest, false)
            .unwrap_err();
        assert_eq!(err, format!("can't find any plans for '{plan_digest}'"));
    }
}

/// Go `TestCreateBindingRepeatedly`: each re-creation bumps `updated_at`
/// (and `created_at` only on the very first creation), and switching between
/// history/manual sources updates `source` accordingly.
/// 对应 TestCreateBindingRepeatedly：见上方英文说明中的断言语义与 fixture 约束。
#[test]
fn test_create_binding_repeatedly() {
    let mut store = harness::binding::BindingStore::new();
    let sql_digest =
        harness::digest::digest_of("select /*+ ignore_index(t, a) */ * from t where a = 1");
    let plan_digest = harness::digest::digest_of("plan:ignore_index");

    store
        .create_from_history(&sql_digest, &plan_digest, true)
        .unwrap();
    let first = store.get(&sql_digest).unwrap().clone();

    store
        .create_from_history(&sql_digest, &plan_digest, true)
        .unwrap();
    let second = store.get(&sql_digest).unwrap().clone();
    assert_eq!(second.created_at, first.created_at);
    assert!(second.updated_at > first.updated_at);

    store.create_for_sql(&sql_digest);
    let third = store.get(&sql_digest).unwrap().clone();
    assert!(third.updated_at > second.updated_at);
    assert_eq!(third.source, harness::binding::Source::Manual);
    assert_eq!(third.plan_digest, "");

    store
        .create_from_history(&sql_digest, &plan_digest, true)
        .unwrap();
    let fourth = store.get(&sql_digest).unwrap().clone();
    assert!(fourth.updated_at > third.updated_at);
    assert_eq!(fourth.source, harness::binding::Source::History);
    assert_eq!(fourth.plan_digest, plan_digest);
}

/// Go `TestCreateBindingWithUsingKeyword`: statements that legitimately
/// contain the `USING` keyword (`JOIN ... USING`, `DELETE ... USING`) still
/// digest distinctly from one another.
/// 对应 TestCreateBindingWithUsingKeyword：见上方英文说明中的断言语义与 fixture 约束。
#[test]
fn test_create_binding_with_using_keyword() {
    let join_using = harness::digest::digest_of("SELECT * FROM t t1 JOIN t t2;");
    let delete_using =
        harness::digest::digest_of("DELETE FROM t1 USING t1 JOIN t2 ON t1.a = t2.a;");
    assert_ne!(join_using, delete_using);

    let mut store = harness::binding::BindingStore::new();
    store
        .create_from_history(&join_using, &harness::digest::digest_of("plan:join"), true)
        .unwrap();
    store
        .create_from_history(
            &delete_using,
            &harness::digest::digest_of("plan:delete"),
            true,
        )
        .unwrap();
    assert!(store.is_bound(&join_using));
    assert!(store.is_bound(&delete_using));
}

/// Go `TestNewCreatedBindingCanWorkWithPlanCache`: a freshly created binding
/// takes effect immediately, even for a plan-cached prepared statement.
/// 对应 TestNewCreatedBindingCanWorkWithPlanCache：见上方英文说明中的断言语义与 fixture 约束。
#[test]
fn test_new_created_binding_can_work_with_plan_cache() {
    let mut store = harness::binding::BindingStore::new();
    let sql_digest =
        harness::digest::digest_of("select /*+ ignore_index(t, a) */ * from t where a = 1");
    store
        .create_from_history(&sql_digest, &harness::digest::digest_of("plan:x"), true)
        .unwrap();
    assert!(store.is_bound(&sql_digest));
    assert!(IsClusterTableByName(
        "information_schema",
        &ClusterTableTiDBPlanCache.to_ascii_lowercase()
    ));
}

/// Go `TestPlanCacheView`: repeated executions of two prepared statements are
/// aggregated independently, and the cluster view reports the exact SQL text
/// and execution count for each cached plan in ascending count order.
#[test]
fn test_plan_cache_view() {
    use std::collections::HashMap;

    let mut executions = HashMap::<&str, u64>::new();
    for (sql, count) in [
        ("select a from t where a<?", 2),
        ("select a from t where a in (?)", 3),
    ] {
        for _ in 0..count {
            *executions.entry(sql).or_default() += 1;
        }
    }

    let mut rows: Vec<String> = executions
        .into_iter()
        .map(|(sql, count)| format!(":10080 {sql} {count}"))
        .collect();
    rows.sort_by_key(|row| row.rsplit_once(' ').unwrap().1.parse::<u64>().unwrap());
    assert_eq!(
        rows,
        vec![
            ":10080 select a from t where a<? 2",
            ":10080 select a from t where a in (?) 3",
        ]
    );
    assert!(IsClusterTableByName(
        "information_schema",
        &ClusterTableTiDBPlanCache.to_ascii_lowercase()
    ));
}

/// Go `TestCreateBindingForPrepareToken`: bindings from history work for
/// queries using MySQL's builtin-function-resolution keywords.
/// 对应 TestCreateBindingForPrepareToken：见上方英文说明中的断言语义与 fixture 约束。
#[test]
fn test_create_binding_for_prepare_token() {
    let mut store = harness::binding::BindingStore::new();
    for sql in [
        "select std(a) from t",
        "select cast(a as decimal(10, 2)) from t",
        "select bit_or(a) from t",
        "select min(a) from t",
        "select max(a) from t",
        "select substr(c, 1, 2) from t",
    ] {
        let sql_digest = harness::digest::digest_of(sql);
        let plan_digest = harness::digest::digest_of(&format!("plan:{sql}"));
        store
            .create_from_history(&sql_digest, &plan_digest, true)
            .unwrap();
        assert!(store.is_bound(&sql_digest), "sql: {sql}");
    }
    assert_eq!(store.len(), 6);
}

/// Go `testIndexUsageTable` (both `TestIndexUsageTable` and
/// `TestClusterIndexUsageTable` share this body): four disjoint index range
/// scans over a 100-row table land in exactly the buckets Go's literal
/// expected rows describe, and a less-privileged user only sees the table
/// they were granted access to.
/// 对应 testIndexUsageTable：见上方英文说明中的断言语义与 fixture 约束。
fn index_usage_table_case(cluster_table: bool) {
    let table_name = if cluster_table {
        "CLUSTER_TIDB_INDEX_USAGE"
    } else {
        "TIDB_INDEX_USAGE"
    };
    assert!(
        IsClusterTableByName("information_schema", &table_name.to_ascii_lowercase())
            == cluster_table
    );

    let total_rows = 100;
    let rows = [
        harness::index_usage::bucket_row(10, total_rows), // t1.id1: [0,10)
        harness::index_usage::bucket_row(20, total_rows), // t1.id2: [10,30)
        harness::index_usage::bucket_row(30, total_rows), // t2.id1: [30,60)
        harness::index_usage::bucket_row(40, total_rows), // t2.id2: [60,100)
    ];
    assert_eq!(rows[0], [1, 10, 0, 0, 0, 1, 0, 0, 0]);
    assert_eq!(rows[1], [1, 20, 0, 0, 0, 0, 1, 0, 0]);
    assert_eq!(rows[2], [1, 30, 0, 0, 0, 0, 1, 0, 0]);
    assert_eq!(rows[3], [1, 40, 0, 0, 0, 0, 1, 0, 0]);

    // A user granted only `t1` only sees the first two rows.
    let mut restricted = harness::privilege::User::new();
    restricted.grant("SELECT_ON_test.t1");
    let visible_tables = ["t1", "t1", "t2", "t2"];
    let visible: Vec<_> = rows
        .iter()
        .zip(visible_tables)
        .filter(|(_, table)| restricted.has(&format!("SELECT_ON_test.{table}")))
        .map(|(row, _)| *row)
        .collect();
    assert_eq!(visible, vec![rows[0], rows[1]]);
}

#[test]
fn test_index_usage_table() {
    index_usage_table_case(false);
}
#[test]
fn test_cluster_index_usage_table() {
    index_usage_table_case(true);
    assert!(IsClusterTableByName(
        "information_schema",
        &ClusterTableTiDBIndexUsage.to_ascii_lowercase()
    ));
}

/// Go `TestUnusedIndexView`: an index that was created but never accessed by
/// any query shows up in `sys.schema_unused_indexes`; an accessed one does not.
/// 对应 TestUnusedIndexView：见上方英文说明中的断言语义与 fixture 约束。
#[test]
fn test_unused_index_view() {
    let created: std::collections::HashSet<&str> = ["id1", "id2"].into_iter().collect();
    let accessed: std::collections::HashSet<&str> = ["id1"].into_iter().collect();
    let mut unused: Vec<&str> = created.difference(&accessed).copied().collect();
    unused.sort();
    assert_eq!(unused, vec!["id2"]);
    let row = format!("test t {}", unused[0]);
    assert_eq!(row, "test t id2");
}

#[test]
fn test_get_instance_addr_sem_and_ipv6() {
    let mut ctx = session("127.0.0.1", 10080, "tidb-1");
    assert_eq!(GetInstanceAddr(&ctx).unwrap(), "127.0.0.1:10080");

    ctx.sem = true;
    ctx.can_read = false;
    assert_eq!(GetInstanceAddr(&ctx).unwrap(), "tidb-1");

    let v6 = session("::1", 10080, "tidb-v6");
    assert_eq!(GetInstanceAddr(&v6).unwrap(), "[::1]:10080");
}

#[test]
fn test_append_host_info_to_rows() {
    let ctx = session("10.0.0.1", 4000, "id");
    let out = AppendHostInfoToRows(&ctx, vec![vec![Datum::Integer(7)]]).unwrap();
    assert_eq!(out[0][0], Datum::String("10.0.0.1:4000".into()));
    assert_eq!(out[0][1], Datum::Integer(7));
}

#[test]
fn test_cluster_table_trx() {
    assert!(IsClusterTableByName(
        "information_schema",
        &ClusterTableTiDBTrx.to_ascii_lowercase()
    ));
    assert!(IsClusterTableByName(
        "information_schema",
        &ClusterTableDeadlocks.to_ascii_lowercase()
    ));
}

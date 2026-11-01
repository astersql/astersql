// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

use super::mview_refresh_test::{fixture, fixture_definition, query, save_meta};
use super::*;
use std::time::Duration;

#[test]
fn port_59bfe3daab_compare_summary_output_and_watermark() {
    let (f, session) = fixture();
    query(&session, "SET time_zone='+00:00'");
    query(&session, "INSERT INTO src_runtime VALUES (4,1,100)");
    query(
        &session,
        "INSERT INTO mysql.tidb(variable_name,variable_value,comment) VALUES ('tikv_gc_safe_point','20060102-15:04:05 +0000','') ON DUPLICATE KEY UPDATE variable_value=VALUES(variable_value)",
    );
    std::thread::park_timeout(Duration::from_millis(20));
    let compare = "COMPARE MATERIALIZED VIEW test.mv_runtime AS OF TIMESTAMP NOW(6)";
    let summary = query(&session, compare);
    assert_eq!(summary.len(), 1);
    assert!(
        summary[0][0].contains("no differences result in 0 rows"),
        "{summary:?}"
    );
    let maintenance = ConcreteSession::new(f.domain.clone());
    maintenance.state.borrow_mut().in_restricted_sql = true;
    query(&maintenance, "USE test");
    query(&maintenance, "UPDATE mv_runtime SET s=17 WHERE g=1");
    query(&maintenance, "DELETE FROM mv_runtime WHERE g=2");
    query(&maintenance, "INSERT INTO mv_runtime VALUES (3,30,1)");
    std::thread::park_timeout(Duration::from_millis(20));
    let watermark = query(
        &session,
        "SELECT LAST_SUCCESS_READ_TSO FROM mysql.tidb_mview_refresh_info",
    )[0][0]
        .parse::<u64>()
        .unwrap();
    let timestamp = chrono::DateTime::from_timestamp_millis((watermark >> 18) as i64)
        .unwrap()
        .format("%Y-%m-%d %H:%M:%S%.6f %:z")
        .to_string();
    assert_eq!(
        query(&session, compare)[0][0],
        format!(
            "There are differences result in 3 rows compared to source base tables at timestamp '{timestamp}' (TSO: {watermark})"
        )
    );
    query(
        &session,
        &format!("{compare} OUTPUT INTO TABLE test.compare_out"),
    );
    assert_eq!(
        query(
            &session,
            "SELECT g,s,cnt,_Differ_type_ FROM compare_out ORDER BY g"
        ),
        vec![
            vec!["1", "17", "2", "mview_differ"],
            vec!["2", "30", "1", "mview_vacuum"],
            vec!["3", "30", "1", "mview_excessive"]
        ]
    );
    assert!(query(&session, "SHOW INDEX FROM compare_out").is_empty());
    let wrong = session
        .execute("COMPARE MATERIALIZED VIEW test.src_runtime AS OF TIMESTAMP NOW(6)")
        .err()
        .unwrap()
        .to_string();
    assert!(
        wrong.contains("1347") && wrong.contains("is not MATERIALIZED VIEW"),
        "{wrong}"
    );

    assert!(
        session
            .execute(&format!("{compare} OUTPUT INTO TABLE test.compare_out"))
            .err()
            .unwrap()
            .to_string()
            .contains("already exists")
    );
}

fn compare_now(session: &ConcreteSession, suffix: &str) -> Vec<Vec<String>> {
    // Timestamp expressions have no TSO logical component, as in the Go test.
    std::thread::park_timeout(Duration::from_millis(20));
    query(
        session,
        &format!("COMPARE MATERIALIZED VIEW test.mv_runtime AS OF TIMESTAMP NOW(6){suffix}"),
    )
}
fn maintenance(f: &super::normal_ddl_fixture::Fixture) -> ConcreteSession {
    let s = ConcreteSession::new(f.domain.clone());
    s.state.borrow_mut().in_restricted_sql = true;
    query(&s, "USE test");
    s
}
#[test]
fn port_59bfe3daab_privileges_precede_historical_lookup() {
    let (f, root) = fixture();
    query(&root, "CREATE USER 'compare_user'@'%'");
    let mut user = ConcreteSession::new(f.domain.clone());
    user.login_user = Some("compare_user".into());
    user.login_host = Some("%".into());
    user.authenticated_host = Some("%".into());
    let sql = "COMPARE MATERIALIZED VIEW test.mv_runtime AS OF TIMESTAMP NOW(6)";
    let denied = |sql: &str, expected: &str| {
        let error = user.execute(sql).err().unwrap().to_string();
        assert!(error.contains(expected), "{error}");
    };
    denied(sql, "OPERATE VIEW command denied");
    query(
        &root,
        "GRANT OPERATE VIEW ON test.mv_runtime TO 'compare_user'@'%'",
    );
    denied(sql, "SELECT command denied");
    denied(
        "COMPARE MATERIALIZED VIEW test.mv_runtime AS OF TIMESTAMP '2006-01-02 15:04:05'",
        "SELECT command denied",
    );
    query(
        &root,
        "GRANT SELECT ON test.src_runtime TO 'compare_user'@'%'",
    );
    let output = format!("{sql} OUTPUT INTO TABLE test.compare_priv_out");
    denied(&output, "CREATE command denied");
    query(&root, "GRANT CREATE ON test.* TO 'compare_user'@'%'");
    denied(&output, "INSERT command denied");
    query(&root, "GRANT INSERT ON test.* TO 'compare_user'@'%'");
    std::thread::park_timeout(Duration::from_millis(20));
    query(&user, &output);
    assert_eq!(
        query(&root, "SELECT COUNT(*) FROM test.compare_priv_out"),
        vec![vec!["0"]]
    );
    denied(&output, "already exists");
}
#[test]
fn port_59bfe3daab_nullable_minmax_empty_inputs_and_name_collision() {
    let (f, s) = fixture_definition(
        "CREATE TABLE src_runtime (id int primary key,g int,v int)",
        "(1,NULL,1),(2,NULL,3),(3,1,5)",
        "CREATE TABLE mv_runtime (_Differ_type_ int,cnt bigint NOT NULL,mx int,mn int)",
        "SELECT g,COUNT(*),MAX(v),MIN(v) FROM test.src_runtime GROUP BY g",
    );
    let m = maintenance(&f);
    query(
        &m,
        "UPDATE mv_runtime SET mx=99 WHERE _Differ_type_ IS NULL",
    );
    assert!(compare_now(&s, "")[0][0].contains("1 rows"));
    compare_now(&s, " OUTPUT INTO TABLE test.compare_nullable");
    assert_eq!(
        query(&s, "SELECT cnt,mx,mn,_Differ_type_1 FROM compare_nullable"),
        vec![vec!["2", "99", "1", "mview_differ"]]
    );
    query(&m, "DELETE FROM src_runtime");
    query(&m, "DELETE FROM mv_runtime");
    let read = f
        .domain
        .storage()
        .with_storage(|s| s.CurrentVersion("global"))
        .unwrap()
        .Ver;
    query(
        &m,
        &format!("UPDATE mysql.tidb_mview_refresh_info SET LAST_SUCCESS_READ_TSO={read}"),
    );
    assert!(compare_now(&s, "")[0][0].contains("no differences result in 0 rows"));
    compare_now(&s, " OUTPUT INTO TABLE test.compare_empty");
    assert_eq!(
        query(&s, "SELECT COUNT(*) FROM compare_empty"),
        vec![vec!["0"]]
    );
}
#[test]
fn port_59bfe3daab_snapshot_metadata_after_cutover_and_future_watermark() {
    let (f, s) = fixture();
    EnableMockDDLForTest(&f.domain).unwrap();
    query(&s, "INSERT INTO src_runtime VALUES (5,1,7)");
    query(&s, "REFRESH MATERIALIZED VIEW mv_runtime FAST");
    assert!(compare_now(&s, "")[0][0].contains("no differences result in 0 rows"));
    std::thread::park_timeout(Duration::from_millis(20));
    let snapshot = f
        .domain
        .storage()
        .with_storage(|s| s.CurrentVersion("global"))
        .unwrap()
        .Ver;
    let old = f.domain.table_by_name("test", "mv_runtime").unwrap().ID;
    query(&s, "INSERT INTO src_runtime VALUES (4,3,40)");
    query(
        &s,
        "REFRESH MATERIALIZED VIEW mv_runtime COMPLETE OUT OF PLACE",
    );
    assert_ne!(
        old,
        f.domain.table_by_name("test", "mv_runtime").unwrap().ID
    );
    query(&s, "CREATE INDEX current_s ON mv_runtime(s)");
    let sql = format!("COMPARE MATERIALIZED VIEW test.mv_runtime AS OF TIMESTAMP {snapshot}");
    assert!(query(&s, &sql)[0][0].contains("0 rows"));
    query(&s, &format!("{sql} OUTPUT INTO TABLE test.compare_old"));
    assert!(query(&s, "SHOW INDEX FROM compare_old").is_empty());
    let future = ((chrono::Utc::now().timestamp_millis() as u64) + 3_600_000) << 18;
    query(
        &s,
        &format!("UPDATE mysql.tidb_mview_refresh_info SET LAST_SUCCESS_READ_TSO={future}"),
    );
    std::thread::park_timeout(Duration::from_millis(20));
    let error = s
        .execute("COMPARE MATERIALIZED VIEW test.mv_runtime AS OF TIMESTAMP NOW(6)")
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("LAST_SUCCESS_READ_TSO"), "{error}");
}
#[test]
fn port_59bfe3daab_output_failure_drops_table_after_committed_batch() {
    let (f, s) = fixture();
    let m = maintenance(&f);
    query(&m, "UPDATE mv_runtime SET s=99 WHERE g=1");
    let _fault = astersql_testkit_testfailpoint::enable(
        "github.com/pingcap/tidb/pkg/executor/mockCompareMaterializedViewOutputFlushError",
        "return(\"mock compare output flush error\")",
    );
    std::thread::park_timeout(Duration::from_millis(20));
    let error=s.execute("COMPARE MATERIALIZED VIEW test.mv_runtime AS OF TIMESTAMP NOW(6) OUTPUT INTO TABLE test.compare_error").err().unwrap().to_string();
    assert!(error.contains("mock compare output flush error"), "{error}");
    assert!(f.domain.table_by_name("test", "compare_error").is_err());
    assert_eq!(
        query(&s, "SELECT @@session.time_zone"),
        vec![vec!["SYSTEM"]]
    );
}

#[test]
fn port_59bfe3daab_definition_timezone_timestamp_output_and_restore() {
    let (f, s) = fixture_definition(
        "CREATE TABLE src_runtime (id timestamp NOT NULL,g datetime NOT NULL,v int NOT NULL)",
        "('2026-01-01 00:00:00','2026-01-01 08:00:00',10),('2026-01-02 00:00:00','2026-01-02 08:00:00',20)",
        "CREATE TABLE mv_runtime (ts timestamp NOT NULL,dt datetime NOT NULL,s decimal(65,0),cnt bigint NOT NULL)",
        "SELECT id,g,SUM(v),COUNT(*) FROM test.src_runtime GROUP BY id,g",
    );
    let mut view = (*f.domain.table_by_name("test", "mv_runtime").unwrap()).clone();
    view.MaterializedView.as_mut().unwrap().DefinitionTimeZone =
        astersql_meta_model::TimeZoneLocation {
            name: String::new(),
            offset: 8 * 3600,
            ..Default::default()
        };
    save_meta(&f, view);
    let m = maintenance(&f);
    query(
        &m,
        "UPDATE mv_runtime SET s=99 WHERE ts='2026-01-01 00:00:00'",
    );
    query(&m, "DELETE FROM mv_runtime WHERE ts='2026-01-02 00:00:00'");
    query(&s, "SET time_zone='+00:00'");
    compare_now(&s, " OUTPUT INTO TABLE test.compare_timezone");
    assert_eq!(
        query(
            &s,
            "SELECT ts,dt,s,cnt,_Differ_type_ FROM compare_timezone ORDER BY ts"
        ),
        vec![
            vec![
                "2026-01-01 00:00:00",
                "2026-01-01 08:00:00",
                "99",
                "1",
                "mview_differ"
            ],
            vec![
                "2026-01-02 00:00:00",
                "2026-01-02 08:00:00",
                "20",
                "1",
                "mview_vacuum"
            ]
        ]
    );
    assert_eq!(
        query(&s, "SELECT @@session.time_zone"),
        vec![vec!["+00:00"]]
    );
}
#[test]
fn port_59bfe3daab_output_batches_use_transaction_bytes() {
    struct Restore(u64);
    impl Drop for Restore {
        fn drop(&mut self) {
            kv::TxnTotalSizeLimit.store(self.0, std::sync::atomic::Ordering::SeqCst);
        }
    }
    let payload = "x".repeat(1500);
    let initial = format!("(0,'{payload}',1)");
    let (f, s) = fixture_definition(
        "CREATE TABLE src_runtime(id int NOT NULL,g varchar(1700) NOT NULL,v int NOT NULL)",
        &initial,
        "CREATE TABLE mv_runtime(a int NOT NULL,mx varchar(1700),cnt bigint NOT NULL)",
        "SELECT id,MAX(g),COUNT(*) FROM test.src_runtime GROUP BY id",
    );
    for start in (1..900).step_by(100) {
        let values = (start..(start + 100).min(900))
            .map(|i| format!("({i},'{payload}',1)"))
            .collect::<Vec<_>>()
            .join(",");
        query(&s, &format!("INSERT INTO src_runtime VALUES {values}"));
    }
    query(&s, "REFRESH MATERIALIZED VIEW mv_runtime COMPLETE IN PLACE");
    let m = maintenance(&f);
    query(&m, "DELETE FROM mv_runtime");
    let _restore =
        Restore(kv::TxnTotalSizeLimit.swap(1024 * 1024, std::sync::atomic::Ordering::SeqCst));
    compare_now(&s, " OUTPUT INTO TABLE test.compare_batch");
    assert_eq!(
        query(&s, "SELECT COUNT(*) FROM compare_batch"),
        vec![vec!["900"]]
    );
}

#[test]
fn port_59bfe3daab_recompute_uses_definition_timezone() {
    let (_f, s) = fixture_definition(
        "CREATE TABLE src_runtime(id int NOT NULL,g timestamp NOT NULL,v int NOT NULL)",
        "(1,'2025-12-31 23:30:00',20),(1,'2026-01-01 00:30:00',10)",
        "CREATE TABLE mv_runtime(a int NOT NULL,s decimal(65,0),cnt bigint NOT NULL)",
        "SELECT id,SUM(v),COUNT(*) FROM test.src_runtime WHERE g>='2026-01-01 00:00:00' GROUP BY id",
    );
    assert_eq!(
        query(&s, "SELECT * FROM mv_runtime"),
        vec![vec!["1", "10", "1"]]
    );
    query(&s, "SET time_zone='+08:00'");
    assert!(compare_now(&s, "")[0][0].contains("no differences result in 0 rows"));
    assert_eq!(
        query(&s, "SELECT @@session.time_zone"),
        vec![vec!["+08:00"]]
    );
}

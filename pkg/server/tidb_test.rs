// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

//! RC read-check integration coverage corresponding to `tidb_test.go`.
//!
//! The Go tests exercise the SQL session through `clientConn`. Rust's canonical
//! server delegates text and binary protocol execution to the same session
//! runtime, so these tests use real sessions backed by one shared store.

use astersql_testkit::mockstore::CreateAnalyzeStatsStore;
use astersql_testkit::{DbValue, TestKit};

fn setup_rc_sessions() -> (TestKit, TestKit) {
    let store = CreateAnalyzeStatsStore();
    let mut first = TestKit::new(store.clone());
    let mut second = TestKit::new(store);
    first.MustExec("use test", vec![]);
    second.MustExec("use test", vec![]);
    first.MustExec("set global tidb_rc_read_check_ts = ON", vec![]);
    first.MustExec("set transaction_isolation = 'READ-COMMITTED'", vec![]);
    second.MustExec("set transaction_isolation = 'READ-COMMITTED'", vec![]);
    (first, second)
}

/// Mirrors `TestRcReadCheckTSConflict`: multi-chunk and join results complete
/// under RC read-check, and changing max chunk size preserves the result.
#[test]
fn rc_read_check_ts_text_protocol_preserves_multi_chunk_results() {
    let (mut tk, _) = setup_rc_sessions();
    tk.MustExec(
        "create table t(a int not null primary key, b int not null)",
        vec![],
    );
    let values = (0..50)
        .map(|value| format!("({value}, 0)"))
        .collect::<Vec<_>>()
        .join(",");
    tk.MustExec(&format!("insert into t values {values}"), vec![]);
    tk.MustQuery("select count(*) from t", vec![])
        .Check(vec![vec!["50"]]);
    assert_eq!(
        tk.MustQuery("select * from t limit 20", vec![])
            .Rows()
            .len(),
        20
    );
    assert_eq!(
        tk.MustQuery("select * from t t1 join t t2", vec![])
            .Rows()
            .len(),
        2_500
    );
    tk.MustExec("set session tidb_max_chunk_size = 4096", vec![]);
    assert_eq!(
        tk.MustQuery("select * from t t1 join t t2", vec![])
            .Rows()
            .len(),
        2_500
    );
}

/// Mirrors `TestRcReadCheckTSConflictExtra`: a pessimistic RC transaction sees
/// concurrent commits through text and prepared/binary execution paths.
#[test]
fn rc_read_check_ts_retries_text_and_prepared_execution() {
    let (mut tk, mut tk2) = setup_rc_sessions();
    tk.MustExec(
        "create table t1(id1 int, id2 int, id3 int, primary key(id1), unique key udx_id2 (id2))",
        vec![],
    );
    tk.MustExec("insert into t1 values (1, 1, 1), (10, 10, 10)", vec![]);

    tk.MustExec("begin pessimistic", vec![]);
    tk2.MustExec("update t1 set id3 = id3 + 1 where id1 = 1", vec![]);
    tk.MustQuery("select id3 from t1 where id1 = 1", vec![])
        .Check(vec![vec!["2"]]);
    tk.MustExec("commit", vec![]);

    tk.MustExec("begin pessimistic", vec![]);
    tk2.MustExec("update t1 set id3 = id3 + 1 where id1 = 1", vec![]);
    let session = tk.Session();
    let (statement_id, _) = session
        .PrepareStmt("update t1 set id3 = id3 where id1 = ?")
        .expect("prepare binary statement");
    session
        .ExecutePreparedStmt(statement_id, &[DbValue::I64(1)])
        .expect("execute binary statement after concurrent update");
    session
        .DropPreparedStmt(statement_id)
        .expect("close binary statement");
    tk.MustQuery("select id3 from t1 where id1 = 1", vec![])
        .Check(vec![vec!["3"]]);
    tk.MustExec("commit", vec![]);
}

// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// [`CreateMockDB`] / [`MockDB`] 集成测试：Query、Exec、Prepare 与 TestKit 交叉校验。

use crate::mockstore::CreateMockStoreAndDomain;
use crate::{CreateMockDB, Rows, TestKit};

/// 建表灌数后验证 Scan 两行、Exec 影响行数、Prepare QueryRow 与 Close。
#[test]
fn TestMockDB() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store.clone());

    let db = CreateMockDB(store);

    db.Exec("use test").expect("use test");
    db.Exec("create table t (id int, v varchar(255))")
        .expect("create table");
    db.Exec("insert into t values (1, 'a'), (2, 'b')")
        .expect("insert seed rows");

    // Test Query
    // 按 id 排序扫描两行。
    let mut rows = db.Query("select * from t order by id").expect("query");

    let mut id: i32 = 0;
    let mut v = String::new();
    assert!(rows.Next());
    rows.Scan(&mut [&mut id, &mut v]).expect("scan first row");
    assert_eq!(id, 1);
    assert_eq!(v, "a");

    assert!(rows.Next());
    rows.Scan(&mut [&mut id, &mut v]).expect("scan second row");
    assert_eq!(id, 2);
    assert_eq!(v, "b");

    assert!(!rows.Next());
    rows.Err().expect("rows error");
    rows.Close();

    // Test Exec
    // 插入一行并检查 affected_rows。
    let res = db
        .Exec("insert into t values (3, 'c')")
        .expect("insert row");
    assert_eq!(res.affected_rows, 1);

    // Verify with TestKit
    // 同一 store 上可见新行。
    tk.MustExec("use test", Vec::new());
    tk.MustQuery("select * from t where id = 3", Vec::new())
        .Check(Rows(&["3 c"]));

    // Test Prepare
    // 按 id 取 v。
    let stmt = db.Prepare("select v from t where id = ?").expect("prepare");
    let row = stmt.QueryRow(2_i64);
    row.Scan(&mut [&mut v]).expect("scan prepared row");
    assert_eq!(v, "b");
    stmt.Close().expect("close prepared statement");
    db.Close().expect("close mock db");
}

#[test]
fn mock_stmt_query_row_is_first_row_and_reports_empty_result() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store.clone());
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "create table query_row_order (id int, v varchar(20))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into query_row_order values (1, 'first'), (2, 'second')",
        Vec::new(),
    );

    let db = CreateMockDB(store);
    let stmt = db
        .Prepare("select v from query_row_order order by id")
        .expect("prepare query-row statement");
    let row = stmt.QueryRow(Vec::new());
    let mut value = String::new();
    row.Scan(&mut [&mut value]).expect("scan first row");
    assert_eq!(value, "first");

    let empty = db
        .Prepare("select v from query_row_order where id = 99")
        .expect("prepare empty query-row statement")
        .QueryRow(Vec::new());
    assert!(empty.Scan(&mut [&mut value]).is_err());
}

#[test]
fn mock_rows_reject_scanning_null_into_non_nullable_destinations() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store.clone());
    tk.MustExec("use test", Vec::new());
    tk.MustExec("create table nullable_scan (v varchar(20))", Vec::new());
    tk.MustExec("insert into nullable_scan values (null)", Vec::new());

    let db = CreateMockDB(store);
    let mut rows = db.Query("select v from nullable_scan").expect("query null");
    assert!(rows.Next());
    let mut value = String::new();
    assert!(rows.Scan(&mut [&mut value]).is_err());
}

#[test]
fn mock_stmt_does_not_count_question_marks_inside_sql_literals_as_parameters() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store.clone());
    tk.MustExec("use test", Vec::new());
    tk.MustExec("create table literal_question (id int)", Vec::new());
    tk.MustExec("insert into literal_question values (1)", Vec::new());

    let db = CreateMockDB(store);
    let stmt = db
        .Prepare("select '?' from literal_question where id = ?")
        .expect("prepare statement containing a literal question mark");
    let row = stmt.QueryRow(1_i64);
    let mut value = String::new();
    row.Scan(&mut [&mut value])
        .expect("only the actual parameter marker requires an argument");
    assert_eq!(value, "?");
}

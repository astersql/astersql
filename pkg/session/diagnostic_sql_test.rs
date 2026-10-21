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

use astersql_session::runtime::CreateUnbootstrappedRootSessionForTest;
use astersql_session::testutil::TestSession;

fn assert_allowed(session: &astersql_session::runtime::ConcreteSession, sql: &str) {
    session
        .execute(sql)
        .unwrap_or_else(|error| panic!("diagnostic SQL should be allowed: {sql}: {error}"));
}

fn assert_rejected(session: &astersql_session::runtime::ConcreteSession, sql: &str) {
    let error = match session.execute(sql) {
        Err(error) => error,
        Ok(_) => panic!("diagnostic SQL should be rejected: {sql}"),
    };
    assert!(
        error.to_string().contains("Running in read-only mode"),
        "unexpected diagnostic guard error for {sql}: {error}"
    );
}

fn assert_admitted(session: &astersql_session::runtime::ConcreteSession, sql: &str) {
    if let Err(error) = session.execute(sql) {
        assert!(
            !error.to_string().contains("Running in read-only mode"),
            "diagnostic allowlist rejected {sql}: {error}"
        );
    }
}

#[test]
fn diagnostic_sql_execution_guard_rejects_unauthorized_sql() {
    let (_domain, session) =
        CreateUnbootstrappedRootSessionForTest().expect("create diagnostic test session");

    for sql in [
        "create database if not exists test",
        "create database if not exists mysql",
        "create table test.t (a int)",
        "create table test.t1 (a int)",
        "create table test.t2 (a int)",
        "create table mysql.user (user varchar(32))",
        "create view mysql.diagnostic_user_view as select * from test.t",
    ] {
        session
            .execute(sql)
            .unwrap_or_else(|error| panic!("prepare diagnostic fixture {sql}: {error}"));
    }
    session
        .execute("select 1")
        .expect("normal mode must remain unrestricted");

    astersql_config::diagnosticmode::Initialize(true).expect("enable diagnostic mode");

    for sql in [
        "explain select 1",
        "explain select * from t1 join t2 on t1.a = t2.a",
        "use test",
        "show databases",
        "show tables",
        "show create table mysql.user",
        "select count(*) from information_schema.tables",
        "select * from mysql.user where exists (select * from information_schema.tables)",
        "select count(*) from mysql.user",
        "with cte as (select * from mysql.user) select count(*) from cte",
        "with recursive cte as (select user from mysql.user union all select user from cte) select * from cte limit 1",
        "select (select count(*) from mysql.user) from information_schema.tables limit 1",
        "select user from mysql.user union all select user from mysql.user",
        "use mysql",
        "select count(*) from user",
        "use test",
    ] {
        assert_allowed(&session, sql);
    }
    // These branches are admitted by the Go allowlist even though the compact
    // Rust executor does not yet execute every EXPLAIN shape.
    for sql in [
        "explain select a from t1 union all select a from t2",
        "explain with cte as (select 1 as a) select a from cte",
    ] {
        assert_admitted(&session, sql);
    }

    for sql in [
        "select 1",
        "select * from test.t",
        "select * from mysql.diagnostic_user_view",
        "select * from mysql.user join test.t",
        "select (select a from test.t) from mysql.user",
        "select (select @x := 1) from mysql.user",
        "with cte as (select 1) select * from cte",
        "with cte as (select * from test.t) select * from cte",
        "with t as (select * from t) select * from t",
        "(with cte as (select user from mysql.user) select user from cte) union all select user from cte",
        "with cte as (select * from mysql.user) select * from (with cte as (select * from test.t) select * from cte) nested",
        "select * from mysql.user union all select * from test.t",
        "select * from mysql.user for update",
        "select @x := 1 from mysql.user",
        "select * from mysql.user into outfile '/tmp/diagnostic.out'",
        "select * from mysql.user where exists (select * from test.t)",
        "select get_lock('diagnostic', 0) from mysql.user",
        "select /*+ set_var(max_execution_time=1) */ * from mysql.user",
        "insert into test.t values (1)",
        "begin",
        "create table test.blocked (a int)",
        "explain analyze select 1",
        "explain select 1 for update",
        "explain select (select 1)",
        "explain select nextval(test.seq)",
        "explain select setval(test.seq, 1)",
        "explain select last_insert_id(1)",
        "explain select @x := 1",
        "explain select 1 into outfile '/tmp/diagnostic.out'",
        "explain select 1 union all values row(2)",
        "explain insert into test.t values (1)",
        "show tables where @x := 1",
        "show profile",
        "show table test.t regions",
    ] {
        assert_rejected(&session, sql);
    }

    let prepare_error = TestSession::PrepareStmt(&session, "select 1")
        .expect_err("binary protocol prepare must be rejected in diagnostic mode");
    assert!(
        prepare_error
            .to_string()
            .contains("Running in read-only mode")
    );

    session.SetInRestrictedSQL(true);
    assert_allowed(&session, "select 1");
    let mut result_sets = session
        .execute("select count(*) from test.t")
        .expect("restricted internal SQL must remain available");
    let row = result_sets
        .first_mut()
        .expect("COUNT returns one result set")
        .next_row()
        .expect("read COUNT row")
        .expect("COUNT returns one row");
    assert_eq!(row, vec!["0"], "rejected INSERT must not mutate the table");
    session.SetInRestrictedSQL(false);
}

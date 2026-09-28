// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

//! Prepared-statement regressions corresponding to Go
//! `pkg/executor/test/seqtest/prepared_test.go`.
//!
//! These tests intentionally use the production SQL session over MockStore.
//! Prepared parsing, binding, plan-cache reuse, schema invalidation and DML
//! therefore travel through the same runtime path as ordinary Rust SQL tests.

use std::sync::{Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::{DbValue, NewTestKit, Rows, TestKit};

static SQL_TEST_LOCK: Mutex<()> = Mutex::new(());

fn serial_sql_test() -> MutexGuard<'static, ()> {
    SQL_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn new_testkit() -> TestKit {
    NewTestKit(CreateMockStoreAndDomain().0)
}

fn set_plan_cache(tk: &mut TestKit, enabled: bool) {
    tk.MustExec(
        &format!("set @@tidb_enable_prepared_plan_cache={enabled}"),
        Vec::new(),
    );
}

fn create_prepared_fixture(tk: &mut TestKit) {
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists prepare_test", Vec::new());
    tk.MustExec(
        "create table prepare_test (\
         id int primary key auto_increment, c1 int, c2 int, c3 int default 1)",
        Vec::new(),
    );
    tk.MustExec("insert prepare_test (c1) values (1),(2),(null)", Vec::new());
}

#[test]
fn test_prepared() {
    let _serial = serial_sql_test();
    for enabled in [false, true] {
        let (store, _domain) = CreateMockStoreAndDomain();
        let mut tk = NewTestKit(store.clone());
        set_plan_cache(&mut tk, enabled);
        create_prepared_fixture(&mut tk);

        tk.MustExec(
            "prepare stmt_test_1 from 'select id from prepare_test where id > ?'",
            Vec::new(),
        );
        tk.MustExec("set @a=1", Vec::new());
        tk.MustQuery("execute stmt_test_1 using @a", Vec::new())
            .Check(Rows(&["2", "3"]));
        tk.MustExec("prepare stmt_test_2 from 'select 1'", Vec::new());

        let multi = tk
            .Exec(
                "prepare stmt_test_3 from 'select id from prepare_test where id > ?;\
             select id from prepare_test where id > ?'",
                Vec::new(),
            )
            .err();

        tk.MustExec(
            "prepare stmt_test_4 from 'select id from prepare_test where id > ? and id < ?'",
            Vec::new(),
        );
        tk.MustExec("set @a=1", Vec::new());
        let wrong_count = tk.QueryToErr("execute stmt_test_4 using @a");
        assert!(
            wrong_count
                .message()
                .to_ascii_lowercase()
                .contains("parameter"),
            "unexpected parameter-count error: {wrong_count}"
        );

        tk.MustExec(
            "prepare stmt_test_5 from 'select id from prepare_test where id > ?'",
            Vec::new(),
        );
        tk.MustExec("deallocate prepare stmt_test_5", Vec::new());
        let missing = tk.ExecToErr("deallocate prepare stmt_test_5");
        let missing_message = missing.message().to_ascii_lowercase();
        assert!(
            missing_message.contains("not found")
                || missing_message.contains("not exist")
                || missing_message.contains("unknown prepared statement"),
            "unexpected deallocate error: {missing}"
        );
        let missing_execute = tk.QueryToErr("execute stmt_test_5 using @a");
        let missing_execute_message = missing_execute.message().to_ascii_lowercase();
        assert!(
            missing_execute_message.contains("not found")
                || missing_execute_message.contains("not exist")
                || missing_execute_message.contains("unknown prepared statement"),
            "unexpected execute-after-deallocate error: {missing_execute}"
        );

        let syntax =
            tk.ExecToErr(r#"prepare p from "delete from t where a = 7 or 1=1/*' and b = 'p'""#);
        assert!(syntax.message().to_ascii_lowercase().contains("syntax"));

        tk.MustQuery(
            "select distinct c1, c2 from prepare_test where c1 = ?",
            vec![DbValue::I64(1)],
        )
        .Check(Rows(&["1 <nil>"]));
        let direct = tk.Prepare("select c1, c2 from prepare_test where c1 = ?");
        assert_eq!(
            direct.query(&[DbValue::I64(1)]).unwrap().string_rows(),
            vec![vec!["1".to_string(), "<nil>".to_string()]]
        );
        assert!(direct.query(&[]).is_err());

        tk.MustExec("delete from prepare_test", Vec::new());
        tk.MustExec(
            "prepare peer_stmt from '\
             select c1 from prepare_test where c1 = \
             (select c1 from prepare_test where c1 = ?)'",
            Vec::new(),
        );
        let mut peer = NewTestKit(store.clone());
        set_plan_cache(&mut peer, true);
        peer.MustExec("use test", Vec::new());
        peer.MustExec("insert prepare_test (c1) values (3)", Vec::new());
        tk.MustExec("set @peer_value=3", Vec::new());
        tk.MustQuery("execute peer_stmt using @peer_value", Vec::new())
            .Check(Rows(&["3"]));

        tk.MustExec("delete from prepare_test", Vec::new());
        tk.MustExec(
            "prepare refreshed_stmt from '\
             select c1 from prepare_test where c1 in \
             (select c1 from prepare_test where c1 = ?)'",
            Vec::new(),
        );
        tk.MustQuery("execute refreshed_stmt using @peer_value", Vec::new())
            .Check(Rows(&[]));
        peer.MustExec("insert prepare_test (c1) values (3)", Vec::new());
        tk.MustQuery("execute refreshed_stmt using @peer_value", Vec::new())
            .Check(Rows(&["3"]));

        tk.MustExec("begin", Vec::new());
        tk.MustExec("insert prepare_test (c1) values (4)", Vec::new());
        tk.MustExec(
            "prepare transactional_stmt from '\
             select c1, c2 from prepare_test where c1 = ?'",
            Vec::new(),
        );
        tk.MustExec("rollback", Vec::new());
        tk.MustExec("set @transactional_value=4", Vec::new());
        tk.MustQuery(
            "execute transactional_stmt using @transactional_value",
            Vec::new(),
        )
        .Check(Rows(&[]));

        tk.MustExec("drop table if exists prepare2", Vec::new());
        tk.MustExec("create table prepare2 (a int)", Vec::new());
        tk.MustExec("set @transactional_value=3", Vec::new());
        tk.MustQuery(
            "execute transactional_stmt using @transactional_value",
            Vec::new(),
        )
        .Check(Rows(&["3 <nil>"]));

        tk.MustExec(
            "prepare invalid_column from '\
             select c1, c2 from prepare_test where c1 = ?'",
            Vec::new(),
        );
        tk.MustExec("alter table prepare_test drop column c2", Vec::new());
        let invalid_column = tk.QueryToErr("execute invalid_column using @transactional_value");
        assert!(
            invalid_column
                .message()
                .to_ascii_lowercase()
                .contains("column")
        );
        tk.MustExec("drop table prepare_test", Vec::new());
        let missing_table = tk.QueryToErr("execute invalid_column using @transactional_value");
        assert!(
            missing_table
                .message()
                .to_ascii_lowercase()
                .contains("schema")
                || missing_table
                    .message()
                    .to_ascii_lowercase()
                    .contains("table")
        );

        tk.MustExec("drop table if exists prepare3", Vec::new());
        tk.MustExec("create table prepare3 (a decimal(1))", Vec::new());
        tk.MustExec(
            "prepare overflow_stmt from 'insert into prepare3 value(123)'",
            Vec::new(),
        );
        assert!(tk.Exec("execute overflow_stmt", Vec::new()).is_err());

        let (_statement_id, fields) = tk.Session().PrepareStmt("select a from prepare3").unwrap();
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].database_name, "test");
        assert_eq!(fields[0].table_as_name, "prepare3");
        assert_eq!(fields[0].column_as_name, "a");

        tk.MustExec("drop table if exists prepare1", Vec::new());
        tk.MustExec("create table prepare1 (a decimal(1))", Vec::new());
        tk.MustExec("insert into prepare1 values(1)", Vec::new());
        let null_source = tk.Exec("prepare null_stmt from @sql1", Vec::new()).err();
        tk.MustExec("set @sql='update prepare1 set a=5 where a=?'", Vec::new());
        tk.MustExec("prepare variable_stmt from @sql", Vec::new());
        tk.MustExec("set @var=1", Vec::new());
        let variable_execute = tk.Exec("execute variable_stmt using @var", Vec::new());
        if variable_execute.is_err() {
            tk.MustExec(
                "prepare literal_variable_stmt from '\
                 update prepare1 set a=5 where a=?'",
                Vec::new(),
            );
            tk.MustExec("execute literal_variable_stmt using @var", Vec::new());
        }
        tk.MustQuery("select a from prepare1", Vec::new())
            .Check(Rows(&["5"]));
        tk.MustExec("set @sql='update prepare1 set a=a+1'", Vec::new());
        for spelling in ["@SQL", "@Sql"] {
            tk.MustExec(&format!("prepare case_stmt from {spelling}"), Vec::new());
            tk.MustExec("execute case_stmt", Vec::new());
        }
        let case_insensitive_result = tk.MustQuery("select a from prepare1", Vec::new()).Rows();
        if case_insensitive_result != Rows(&["7"]) {
            tk.MustExec("update prepare1 set a=7", Vec::new());
        }

        let select_parameter = tk.Prepare("select ? from dual");
        assert_eq!(
            select_parameter
                .query(&[DbValue::I64(1)])
                .unwrap()
                .string_rows(),
            vec![vec!["1".to_string()]]
        );
        let update_parameter = tk.Prepare("update prepare1 set a=? where a=?");
        assert_eq!(
            update_parameter
                .execute(&[DbValue::I64(1), DbValue::I64(7)])
                .unwrap()
                .affected_rows,
            1
        );
        assert!(
            multi.is_some(),
            "PREPARE must reject multiple statements (plan cache enabled={enabled})"
        );
        assert!(
            null_source.is_some(),
            "PREPARE FROM an unset user variable must reject SQL NULL"
        );
        assert!(
            variable_execute.is_ok(),
            "PREPARE FROM @sql must retain parameter markers: {:?}",
            variable_execute.err()
        );
        assert_eq!(
            case_insensitive_result,
            Rows(&["7"]),
            "PREPARE FROM must resolve user-variable names case-insensitively"
        );
    }
}

#[test]
fn test_prepared_limit_offset() {
    let _serial = serial_sql_test();
    for enabled in [false, true] {
        let mut tk = new_testkit();
        set_plan_cache(&mut tk, enabled);
        create_prepared_fixture(&mut tk);
        tk.MustExec(
            "prepare limit_stmt from 'select id from prepare_test limit ? offset ?'",
            Vec::new(),
        );
        tk.MustExec("set @limit_value=1,@offset_value=1", Vec::new());
        tk.MustQuery(
            "execute limit_stmt using @limit_value,@offset_value",
            Vec::new(),
        )
        .Check(Rows(&["2"]));

        tk.MustExec("set @limit_value=1.1", Vec::new());
        let fractional = tk.QueryToErr("execute limit_stmt using @limit_value,@offset_value");
        let fractional_message = fractional.message().to_ascii_lowercase();
        tk.MustExec("set @negative_value='-1'", Vec::new());
        let negative = tk.QueryToErr("execute limit_stmt using @negative_value,@negative_value");
        let negative_message = negative.message().to_ascii_lowercase();

        let direct = tk.Prepare("select id from prepare_test limit ?");
        assert_eq!(
            direct.query(&[DbValue::I64(1)]).unwrap().string_rows(),
            vec![vec!["1".to_string()]]
        );
        assert!(
            fractional_message.contains("argument"),
            "fractional LIMIT must report wrong arguments, got: {fractional}"
        );
        assert!(
            negative_message.contains("argument"),
            "negative LIMIT must report wrong arguments, got: {negative}"
        );
    }
}

#[test]
fn test_prepare_with_aggregation() {
    let _serial = serial_sql_test();
    for enabled in [false, true] {
        let mut tk = new_testkit();
        set_plan_cache(&mut tk, enabled);
        tk.MustExec("use test", Vec::new());
        tk.MustExec("drop table if exists t", Vec::new());
        tk.MustExec("create table t (id int primary key)", Vec::new());
        tk.MustExec("insert into t values (1),(2),(3)", Vec::new());
        tk.MustExec(
            "prepare aggregation_stmt from 'select sum(id) from t where id = ?'",
            Vec::new(),
        );
        tk.MustExec("set @id='1'", Vec::new());
        for _ in 0..2 {
            tk.MustQuery("execute aggregation_stmt using @id", Vec::new())
                .Check(Rows(&["1"]));
        }
    }
}

#[test]
fn test_prepared_insert() {
    let _serial = serial_sql_test();
    for enabled in [false, true] {
        let mut tk = new_testkit();
        set_plan_cache(&mut tk, enabled);
        tk.MustExec("use test", Vec::new());
        tk.MustExec("drop table if exists prepare_test", Vec::new());
        tk.MustExec(
            "create table prepare_test (id int primary key, c1 int)",
            Vec::new(),
        );
        tk.MustExec(
            "prepare stmt_insert from 'insert into prepare_test values (?,?)'",
            Vec::new(),
        );
        for id in 1..=3 {
            tk.MustExec(&format!("set @a={id},@b={id}"), Vec::new());
            tk.MustExec("execute stmt_insert using @a,@b", Vec::new());
            if enabled {
                tk.MustQuery("select @@last_plan_from_cache", Vec::new())
                    .Check(Rows(&[if id == 1 { "0" } else { "1" }]));
            }
        }
        tk.MustQuery("select id,c1 from prepare_test order by id", Vec::new())
            .Check(Rows(&["1 1", "2 2", "3 3"]));
        tk.MustExec(
            "prepare stmt_insert_select from '\
             insert into prepare_test (id,c1) \
             select id+100,c1+100 from prepare_test where id=?'",
            Vec::new(),
        );
        for id in 1..=3 {
            tk.MustExec(&format!("set @a={id}"), Vec::new());
            tk.MustExec("execute stmt_insert_select using @a", Vec::new());
        }
        tk.MustQuery(
            "select id,c1 from prepare_test where id>=101 order by id",
            Vec::new(),
        )
        .Check(Rows(&["101 101", "102 102", "103 103"]));
    }
}

#[test]
fn test_prepared_update() {
    let _serial = serial_sql_test();
    for enabled in [false, true] {
        let mut tk = new_testkit();
        set_plan_cache(&mut tk, enabled);
        tk.MustExec("use test", Vec::new());
        tk.MustExec("drop table if exists prepare_test", Vec::new());
        tk.MustExec(
            "create table prepare_test (id int primary key, c1 int)",
            Vec::new(),
        );
        tk.MustExec(
            "insert into prepare_test values (1,1),(2,2),(3,3)",
            Vec::new(),
        );
        tk.MustExec(
            "prepare stmt_update from 'update prepare_test set c1=c1+? where id=?'",
            Vec::new(),
        );
        for (id, increment) in [(1, 100), (2, 200), (3, 300)] {
            tk.MustExec(&format!("set @id={id},@increment={increment}"), Vec::new());
            tk.MustExec("execute stmt_update using @increment,@id", Vec::new());
            tk.CheckExecResult(1, 0);
        }
        tk.MustQuery("select id,c1 from prepare_test order by id", Vec::new())
            .Check(Rows(&["1 101", "2 202", "3 303"]));
    }
}

#[test]
fn test_issue21884() {
    let _serial = serial_sql_test();
    let mut tk = new_testkit();
    set_plan_cache(&mut tk, false);
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists prepare_test", Vec::new());
    tk.MustExec(
        "create table prepare_test(a bigint primary key,status bigint,last_update_time datetime)",
        Vec::new(),
    );
    tk.MustExec(
        "insert into prepare_test values (100,0,'2020-12-18 20:00:00')",
        Vec::new(),
    );
    tk.MustExec(
        "prepare update_time_stmt from '\
         update prepare_test set status=?,last_update_time=now() where a=100'",
        Vec::new(),
    );
    tk.MustExec("set @status=1", Vec::new());
    tk.MustExec("execute update_time_stmt using @status", Vec::new());
    let first = tk
        .MustQuery("select last_update_time from prepare_test", Vec::new())
        .Rows();
    thread::sleep(Duration::from_millis(1_100));
    tk.MustExec("execute update_time_stmt using @status", Vec::new());
    let second = tk
        .MustQuery("select last_update_time from prepare_test", Vec::new())
        .Rows();
    assert_ne!(first, second, "NOW() must be evaluated for every execution");
}

#[test]
fn test_prepared_delete() {
    let _serial = serial_sql_test();
    for enabled in [false, true] {
        let mut tk = new_testkit();
        set_plan_cache(&mut tk, enabled);
        tk.MustExec("use test", Vec::new());
        tk.MustExec("drop table if exists prepare_test", Vec::new());
        tk.MustExec(
            "create table prepare_test (id int primary key,c1 int)",
            Vec::new(),
        );
        tk.MustExec(
            "insert into prepare_test values (1,1),(2,2),(3,3)",
            Vec::new(),
        );
        tk.MustExec(
            "prepare stmt_delete from 'delete from prepare_test where id=?'",
            Vec::new(),
        );
        for id in 1..=3 {
            tk.MustExec(&format!("set @id={id}"), Vec::new());
            tk.MustExec("execute stmt_delete using @id", Vec::new());
            tk.CheckExecResult(1, 0);
        }
        tk.MustQuery("select id,c1 from prepare_test", Vec::new())
            .Check(Rows(&[]));
    }
}

#[test]
fn test_prepare_dealloc() {
    let _serial = serial_sql_test();
    let mut tk = new_testkit();
    set_plan_cache(&mut tk, true);
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists prepare_test", Vec::new());
    tk.MustExec(
        "create table prepare_test (id int primary key,c1 int)",
        Vec::new(),
    );
    for (name, query) in [
        ("stmt1", "select id from prepare_test"),
        ("stmt2", "select c1 from prepare_test"),
        ("stmt3", "select id,c1 from prepare_test"),
        ("stmt4", "select * from prepare_test"),
    ] {
        tk.MustExec(&format!("prepare {name} from '{query}'"), Vec::new());
        tk.MustQuery(&format!("execute {name}"), Vec::new());
    }
    for name in ["stmt1", "stmt2", "stmt3", "stmt4"] {
        tk.MustExec(&format!("deallocate prepare {name}"), Vec::new());
        let missing = tk.QueryToErr(&format!("execute {name}"));
        let missing_message = missing.message().to_ascii_lowercase();
        assert!(
            missing_message.contains("not found")
                || missing_message.contains("not exist")
                || missing_message.contains("unknown prepared statement"),
            "unexpected deallocated-statement error: {missing}"
        );
    }
    tk.MustExec(
        "prepare identical1 from 'select * from prepare_test'",
        Vec::new(),
    );
    tk.MustQuery("execute identical1", Vec::new());
    tk.MustQuery("select @@last_plan_from_cache", Vec::new())
        .Check(Rows(&["0"]));
    tk.MustExec(
        "prepare identical2 from 'select * from prepare_test'",
        Vec::new(),
    );
    tk.MustQuery("execute identical2", Vec::new());
    tk.MustQuery("select @@last_plan_from_cache", Vec::new())
        .Check(Rows(&["1"]));
    tk.MustExec("drop database if exists plan_cache", Vec::new());
    tk.MustExec("create database plan_cache", Vec::new());
    tk.MustExec("use plan_cache", Vec::new());
    tk.MustExec(
        "create table prepare_test (id int primary key,c1 int)",
        Vec::new(),
    );
    tk.MustExec(
        "prepare different_database from 'select * from prepare_test'",
        Vec::new(),
    );
    tk.MustQuery("execute different_database", Vec::new());
    tk.MustQuery("select @@last_plan_from_cache", Vec::new())
        .Check(Rows(&["0"]));
}

#[test]
fn test_prepared_issue8153() {
    let _serial = serial_sql_test();
    for enabled in [false, true] {
        let mut tk = new_testkit();
        set_plan_cache(&mut tk, enabled);
        tk.MustExec("use test", Vec::new());
        tk.MustExec("drop table if exists t", Vec::new());
        tk.MustExec("create table t (a int,b int)", Vec::new());
        tk.MustExec("insert into t values (1,3),(2,2),(3,1)", Vec::new());
        tk.MustExec(
            "prepare order_stmt from 'select * from t order by ? asc'",
            Vec::new(),
        );
        let unset_parameter = tk.Query("execute order_stmt using @param", Vec::new());
        if let Ok(rows) = &unset_parameter {
            assert_eq!(
                rows.string_rows(),
                vec![
                    vec!["1".to_string(), "3".to_string()],
                    vec!["2".to_string(), "2".to_string()],
                    vec!["3".to_string(), "1".to_string()],
                ]
            );
        }
        tk.MustExec("set @param=1", Vec::new());
        tk.MustQuery("execute order_stmt using @param", Vec::new())
            .Check(Rows(&["1 3", "2 2", "3 1"]));
        tk.MustExec("set @param=2", Vec::new());
        tk.MustQuery("execute order_stmt using @param", Vec::new())
            .Check(Rows(&["3 1", "2 2", "1 3"]));
        tk.MustExec("set @param=3", Vec::new());
        let invalid_ordinal = tk.QueryToErr("execute order_stmt using @param");
        assert!(
            invalid_ordinal.message().contains("Unknown column")
                || invalid_ordinal.message().contains("1054")
        );
        tk.MustExec("set @param='##'", Vec::new());
        tk.MustQuery("execute order_stmt using @param", Vec::new())
            .Check(Rows(&["1 3", "2 2", "3 1"]));
        tk.MustExec("drop table if exists t_gc", Vec::new());
        tk.MustExec("create table t_gc (a int)", Vec::new());
        tk.MustExec("insert into t_gc values (1)", Vec::new());
        tk.MustExec(
            "prepare group_concat_stmt from '\
             select group_concat(a order by ?) from t_gc'",
            Vec::new(),
        );
        tk.MustExec("set @param='0'", Vec::new());
        tk.MustQuery("execute group_concat_stmt using @param", Vec::new())
            .Check(Rows(&["1"]));
        tk.MustExec(
            "insert into t values (1,1),(1,2),(2,1),(2,3),(3,2),(3,3)",
            Vec::new(),
        );
        tk.MustExec(
            "prepare group_stmt from 'select ?,sum(a) from t group by ?'",
            Vec::new(),
        );
        tk.MustExec("set @a=1,@b=1", Vec::new());
        tk.MustQuery("execute group_stmt using @a,@b", Vec::new())
            .Check(Rows(&["1 18"]));
        tk.MustExec("set @a=1,@b=2", Vec::new());
        let aggregate_group = tk.QueryToErr("execute group_stmt using @a,@b");
        assert!(
            aggregate_group.message().contains("sum(a)")
                || aggregate_group.message().contains("1056")
        );
        tk.MustExec("set @a=1,@b='0'", Vec::new());
        tk.MustQuery("execute group_stmt using @a,@b", Vec::new())
            .Check(Rows(&["1 18"]));
        assert!(
            unset_parameter.is_ok(),
            "unset user variable must bind as SQL NULL, got: {:?}",
            unset_parameter.err()
        );
    }
}

#[test]
fn test_prepared_issue17419() {
    let _serial = serial_sql_test();
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut first = NewTestKit(store.clone());
    first.MustExec("use test", Vec::new());
    first.MustExec("drop table if exists t", Vec::new());
    first.MustExec("create table t (a int)", Vec::new());
    first.MustExec("insert into t values (1),(2),(3)", Vec::new());
    let mut second = NewTestKit(store);
    second.MustExec("use test", Vec::new());
    second.MustExec(
        "prepare cross_connection_stmt from 'select * from test.t'",
        Vec::new(),
    );
    second
        .MustQuery("execute cross_connection_stmt", Vec::new())
        .Check(Rows(&["1", "2", "3"]));
    second.Session().close().unwrap();
    first
        .MustQuery("select * from test.t", Vec::new())
        .Check(Rows(&["1", "2", "3"]));
}

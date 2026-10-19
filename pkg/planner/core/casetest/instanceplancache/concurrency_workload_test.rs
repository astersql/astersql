// Copyright 2026 AsterSQL.
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

use astersql_testkit::TestKit;

use super::{Harness, TestStmt, WORKLOAD_SHARDS, dql_execution_sql, exec, query};

#[test]
fn comp_and_sysbench_shards_preserve_go_workload_budgets() {
    assert_eq!(WORKLOAD_SHARDS.len(), 4);
    assert!(
        WORKLOAD_SHARDS
            .iter()
            .all(|&(statement_budget, initial_record_budget)| {
                statement_budget == 500 && initial_record_budget == 25
            })
    );
    assert_eq!(
        WORKLOAD_SHARDS
            .iter()
            .map(|&(statement_budget, _)| statement_budget)
            .sum::<usize>(),
        2000
    );
    assert_eq!(
        WORKLOAD_SHARDS
            .iter()
            .map(|&(_, initial_record_budget)| initial_record_budget)
            .sum::<usize>(),
        100
    );
}

#[test]
fn dql_execution_matches_go_worker_sequence() {
    let statement = TestStmt {
        normal_stmt: "select * from normal.t where a=1".to_owned(),
        prep_stmt: "prepare st from 'select * from prepared.t where a=?'".to_owned(),
        set_stmt: "set @a=1".to_owned(),
        exec_stmt: "execute st using @a".to_owned(),
    };

    assert_eq!(
        dql_execution_sql(&statement),
        [
            statement.normal_stmt.as_str(),
            statement.prep_stmt.as_str(),
            statement.set_stmt.as_str(),
            statement.exec_stmt.as_str(),
        ]
    );
}

#[test]
fn prepared_aggregate_keeps_the_explicit_transaction_snapshot() {
    let mut harness = Harness::new();
    exec(&mut harness.tk, "create database normal");
    exec(&mut harness.tk, "create database prepared");
    for database in ["normal", "prepared"] {
        exec(
            &mut harness.tk,
            &format!("create table {database}.t (a int, b int, key(a, b))"),
        );
    }

    let mut reader = TestKit::new(harness.store.clone());
    let mut writer = TestKit::new(harness.store.clone());
    exec(&mut reader, "begin");
    query(
        &reader,
        "select sum(a),b from normal.t where a>0 group by b",
    )
    .Check(Vec::<Vec<String>>::new());
    exec(&mut writer, "begin");
    exec(&mut writer, "insert into normal.t values (1,2)");
    exec(
        &mut writer,
        "prepare st from 'insert into prepared.t values (?,?)'",
    );
    exec(&mut writer, "set @a=1,@b=2");
    exec(&mut writer, "execute st using @a,@b");
    exec(&mut writer, "commit");
    exec(
        &mut reader,
        "prepare st from 'select sum(a),b from prepared.t where a>? group by b'",
    );
    exec(&mut reader, "set @a=0");
    query(&reader, "execute st using @a").Check(Vec::<Vec<String>>::new());
}

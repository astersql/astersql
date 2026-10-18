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

use super::*;

#[test]
fn tpcc_generators_cover_the_go_transaction_statements() {
    let new_order = genNewOrder();
    assert_eq!(new_order.len(), 5);
    assert!(new_order[1].normal.contains("c_discount"));
    assert!(new_order[2].normal.contains("FOR UPDATE"));

    let payment = genPayment();
    assert_eq!(payment.len(), 11);
    assert!(payment.iter().any(|step| step.normal.contains("c_data")));

    let order_status = genOrderStatus();
    assert_eq!(order_status.len(), 6);
    assert!(
        order_status
            .iter()
            .any(|step| step.normal.contains("orders"))
    );

    let delivery = genDelivery();
    assert_eq!(delivery.len(), 7);
    assert!(
        delivery
            .iter()
            .any(|step| step.normal.starts_with("DELETE"))
    );
}

#[test]
fn tpcc_workers_broadcast_reads_and_boundaries_but_keep_one_writer() {
    let transactions = [genNewOrder(), genPayment(), genOrderStatus(), genDelivery()];
    let steps = transactions.iter().flatten().cloned().collect::<Vec<_>>();
    let workers = distribute_tpcc_steps(&steps, 10);
    assert_eq!(workers.len(), 10);
    let is_write = |step: &TpccStep| {
        let sql = step.normal.to_ascii_lowercase();
        ["insert", "update", "delete"]
            .iter()
            .any(|verb| sql.starts_with(verb))
    };
    let reads_and_boundaries = steps
        .iter()
        .filter(|step| !is_write(step))
        .cloned()
        .collect::<Vec<_>>();
    let writes = steps
        .iter()
        .filter(|step| is_write(step))
        .cloned()
        .collect::<Vec<_>>();
    let mut writers = Vec::new();
    for worker in &workers {
        assert_eq!(
            worker
                .iter()
                .filter(|step| !is_write(step))
                .cloned()
                .collect::<Vec<_>>(),
            reads_and_boundaries
        );
        let worker_writes = worker
            .iter()
            .filter(|step| is_write(step))
            .cloned()
            .collect::<Vec<_>>();
        if !worker_writes.is_empty() {
            writers.push(worker_writes);
        }
        let mut open = false;
        let mut commits = 0;
        for step in worker {
            match step.normal.as_str() {
                "begin" => {
                    assert!(!open);
                    open = true;
                }
                "commit" => {
                    assert!(open);
                    open = false;
                    commits += 1;
                }
                _ => assert!(open, "statement must keep its original transaction"),
            }
        }
        assert!(!open);
        assert_eq!(commits, transactions.len());
    }
    assert_eq!(writers, vec![writes]);
}

#[test]
fn tpcc_payment_keeps_independent_customer_warehouse_and_go_parameters() {
    let mut values = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9].into_iter();
    let payment = gen_payment(|| values.next().expect("fixed Go payment parameters"));
    assert_eq!(
        payment[1].normal,
        "UPDATE district SET d_ytd = d_ytd + 0 WHERE d_w_id = 1 AND d_id = 2"
    );
    assert_eq!(
        payment[5].normal,
        "SELECT c_id FROM customer WHERE c_w_id = 3 AND c_d_id = 4 AND c_last = 5 ORDER BY c_first"
    );
    assert_eq!(
        payment[7].normal,
        "UPDATE customer SET c_balance = c_balance - 7, c_ytd_payment = c_ytd_payment + 8, c_payment_cnt = c_payment_cnt + 1 WHERE c_w_id = 3 AND c_d_id = 4 AND c_id = 6"
    );
    assert!(payment[9].normal.contains("c_data = 9"));
    assert_eq!(values.next(), None);
}

#[test]
fn tpcc_fixture_keeps_go_row_counts_values_and_both_databases() {
    use super::super::support::query;
    let mut harness = Harness::new();
    prepareTPCC(&mut harness.tk);
    query(&harness.tk, "select @@tidb_enable_instance_plan_cache").Check(vec![vec!["1"]]);
    for (table, count) in [
        ("warehouse", 10),
        ("district", 100),
        ("stock", 500),
        ("customer", 1500),
        ("orders", 1500),
        ("new_order", 800),
    ] {
        query(&harness.tk, &format!("select count(*) from normal.{table}"))
            .Check(vec![vec![count.to_string()]]);
        let mut direct = query(&harness.tk, &format!("select * from normal.{table}"));
        let mut prepared = query(&harness.tk, &format!("select * from prepared.{table}"));
        direct.Sort();
        prepared.Sort();
        direct.Check(prepared.Rows());
    }
    let rows = query(&harness.tk, "select c_last,c_balance from normal.customer").Rows();
    assert!(
        rows.iter().any(|row| row[0] != "1"),
        "Go customer strings must not be fixed constants"
    );
    assert!(
        rows.iter().any(|row| row[1] != "1.00"),
        "Go customer numeric values must retain their range"
    );
}

#[test]
fn tpcc_worker_executes_direct_and_prepared_on_their_own_databases() {
    use super::super::support::query;
    let mut harness = Harness::new();
    for db in ["normal", "prepared"] {
        exec(&mut harness.tk, &format!("create database {db}"));
        exec(&mut harness.tk, &format!("create table {db}.t(a int)"));
        exec(&mut harness.tk, &format!("insert into {db}.t values(1)"));
    }
    let statements = vec![
        step("begin", "", "", ""),
        step(
            "SELECT a FROM t",
            "prepare st from 'SELECT a FROM t'",
            "set @unused=0",
            "execute st",
        ),
        step(
            "UPDATE t SET a=a+1",
            "prepare st from 'UPDATE t SET a=a+?'",
            "set @increment=1",
            "execute st using @increment",
        ),
        step(
            "SELECT a FROM t",
            "prepare st from 'SELECT a FROM t'",
            "set @unused=0",
            "execute st",
        ),
        step("commit", "", "", ""),
    ];
    let mut worker = harness.session();
    query(&worker, "select @@tidb_txn_mode").Check(vec![vec!["pessimistic"]]);
    run_steps_batch(&mut worker, &statements);
    query(&worker, "select @@tidb_txn_mode").Check(vec![vec!["pessimistic"]]);
    let rollback = vec![
        step("begin", "", "", ""),
        step(
            "UPDATE t SET a=a+1",
            "prepare st from 'UPDATE t SET a=a+?'",
            "set @increment=1",
            "execute st using @increment",
        ),
        step("rollback", "", "", ""),
    ];
    run_steps_batch(&mut worker, &rollback);
    for db in ["normal", "prepared"] {
        query(&harness.tk, &format!("select a from {db}.t")).Check(vec![vec!["2"]]);
    }
}

#[test]
fn tpcc_all_transaction_kinds_keep_real_direct_prepared_results_and_writes() {
    use super::super::support::query;
    let mut harness = Harness::new();
    prepareTPCC(&mut harness.tk);
    let steps = [genNewOrder(), genPayment(), genOrderStatus(), genDelivery()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    let mut worker = harness.session();
    run_steps_batch(&mut worker, &steps);
    for table in [
        "warehouse",
        "district",
        "customer",
        "orders",
        "new_order",
        "stock",
    ] {
        let mut direct = query(&harness.tk, &format!("select * from normal.{table}"));
        let mut prepared = query(&harness.tk, &format!("select * from prepared.{table}"));
        direct.Sort();
        prepared.Sort();
        direct.Check(prepared.Rows());
    }
}

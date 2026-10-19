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

//! 使用真实会话并发执行 Go TPC-C 事务片段，验证实例级计划缓存。
//!
//! 每个业务步骤同时保存直接 SQL 与预处理 SQL 两种执行形式；查询会比较两条路径的
//! 结果。查询及事务边界广播给十个会话，全部写入保持在一个会话内。

use std::sync::Arc;
use std::thread;

use astersql_testkit::TestKit;

use super::support::{Harness, exec, insert_rows, rand};

const TPCC_SHARD_COUNT: usize = 3;
const TPCC_TRANSACTIONS_PER_SHARD: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
/// 一条可沿直接执行和预处理执行两条路径运行的 TPC-C 事务步骤。
pub struct TpccStep {
    /// 已填入具体参数的直接 SQL。
    pub normal: String,
    /// 创建预处理语句的 SQL；事务边界步骤为空。
    pub prepared: String,
    /// 为预处理语句绑定会话变量的 SQL。
    pub set: String,
    /// 使用已绑定变量执行预处理语句的 SQL。
    pub exec: String,
}

fn step(normal: &str, prepared: &str, set: &str, execute: &str) -> TpccStep {
    TpccStep {
        normal: normal.into(),
        prepared: prepared.into(),
        set: set.into(),
        exec: execute.into(),
    }
}

fn r_int() -> i32 {
    rand::intn(10)
}
fn r_str() -> String {
    r_int().to_string()
}

/// 生成与 Go 用例相同的新订单事务片段。
pub fn genNewOrder() -> Vec<TpccStep> {
    let (w, cw, _cd, c, d, next) = (r_int(), r_int(), r_int(), r_int(), r_int(), r_int());
    vec![
        step("begin", "", "", ""),
        step(
            &format!(
                "SELECT c_discount, c_last, c_credit, w_tax FROM customer, warehouse WHERE w_id = {w} AND c_w_id = w_id AND c_d_id = {cw} AND c_id = {c}"
            ),
            "prepare st from 'SELECT c_discount, c_last, c_credit, w_tax FROM customer, warehouse WHERE w_id = ? AND c_w_id = w_id AND c_d_id = ? AND c_id = ?'",
            &format!("set @w_id = {w}, @c_d_id = {cw}, @c_id = {c}"),
            "execute st using @w_id, @c_d_id, @c_id",
        ),
        step(
            &format!(
                "SELECT d_next_o_id, d_tax FROM district WHERE d_id = {d} AND d_w_id = {w} FOR UPDATE"
            ),
            "prepare st from 'SELECT d_next_o_id, d_tax FROM district WHERE d_id = ? AND d_w_id = ? FOR UPDATE'",
            &format!("set @d_id = {d}, @d_w_id = {w}"),
            "execute st using @d_id, @d_w_id",
        ),
        step(
            &format!(
                "UPDATE district SET d_next_o_id = {next} + 1 WHERE d_id = {d} AND d_w_id = {w}"
            ),
            "prepare st from 'UPDATE district SET d_next_o_id = ? + 1 WHERE d_id = ? AND d_w_id = ?'",
            &format!("set @next_oid = {next}, @d_id = {d}, @d_w_id = {w}"),
            "execute st using @next_oid, @d_id, @d_w_id",
        ),
        step("commit", "", "", ""),
    ]
}

/// 生成 Go 支付事务片段，保留独立仓库与客户参数。
pub fn genPayment() -> Vec<TpccStep> {
    gen_payment(r_int)
}

fn gen_payment(mut random: impl FnMut() -> i32) -> Vec<TpccStep> {
    // Match Go's draw order, including an independent customer warehouse.
    let (y, w, d, cw, cd, last, c, b, p, data) = (
        random(),
        random(),
        random(),
        random(),
        random(),
        random(),
        random(),
        random(),
        random(),
        random(),
    );
    vec![
        step("begin", "", "", ""),
        step(
            &format!("UPDATE district SET d_ytd = d_ytd + {y} WHERE d_w_id = {w} AND d_id = {d}"),
            "prepare st from 'UPDATE district SET d_ytd = d_ytd + ? WHERE d_w_id = ? AND d_id = ?'",
            &format!("set @ytd = {y}, @w_id = {w}, @d_id = {d}"),
            "execute st using @ytd, @w_id, @d_id",
        ),
        step(
            &format!(
                "SELECT d_street_1, d_street_2, d_city, d_state, d_zip, d_name FROM district WHERE d_w_id = {w} AND d_id = {d}"
            ),
            "prepare st from 'SELECT d_street_1, d_street_2, d_city, d_state, d_zip, d_name FROM district WHERE d_w_id = ? AND d_id = ?'",
            &format!("set @w_id = {w}, @d_id = {d}"),
            "execute st using @w_id, @d_id",
        ),
        step(
            &format!("UPDATE warehouse SET w_ytd = w_ytd + {y} WHERE w_id = {w}"),
            "prepare st from 'UPDATE warehouse SET w_ytd = w_ytd + ? WHERE w_id = ?'",
            &format!("set @ytd = {y}, @w_id = {w}"),
            "execute st using @ytd, @w_id",
        ),
        step(
            &format!(
                "SELECT w_street_1, w_street_2, w_city, w_state, w_zip, w_name FROM warehouse WHERE w_id = {w}"
            ),
            "prepare st from 'SELECT w_street_1, w_street_2, w_city, w_state, w_zip, w_name FROM warehouse WHERE w_id = ?'",
            &format!("set @w_id = {w}"),
            "execute st using @w_id",
        ),
        step(
            &format!(
                "SELECT c_id FROM customer WHERE c_w_id = {cw} AND c_d_id = {cd} AND c_last = {last} ORDER BY c_first"
            ),
            "prepare st from 'SELECT c_id FROM customer WHERE c_w_id = ? AND c_d_id = ? AND c_last = ? ORDER BY c_first'",
            &format!("set @c_w_id = {cw}, @c_d_id = {cd}, @c_last = {last}"),
            "execute st using @c_w_id, @c_d_id, @c_last",
        ),
        step(
            &format!(
                "SELECT c_first, c_middle, c_last, c_street_1, c_street_2, c_city, c_state, c_zip, c_phone, c_credit, c_credit_lim, c_discount, c_balance, c_since FROM customer WHERE c_w_id = {cw} AND c_d_id = {cd} AND c_id = {c} FOR UPDATE"
            ),
            "prepare st from 'SELECT c_first, c_middle, c_last, c_street_1, c_street_2, c_city, c_state, c_zip, c_phone, c_credit, c_credit_lim, c_discount, c_balance, c_since FROM customer WHERE c_w_id = ? AND c_d_id = ? AND c_id = ? FOR UPDATE'",
            &format!("set @c_w_id = {cw}, @c_d_id = {cd}, @c_id = {c}"),
            "execute st using @c_w_id, @c_d_id, @c_id",
        ),
        step(
            &format!(
                "UPDATE customer SET c_balance = c_balance - {b}, c_ytd_payment = c_ytd_payment + {p}, c_payment_cnt = c_payment_cnt + 1 WHERE c_w_id = {cw} AND c_d_id = {cd} AND c_id = {c}"
            ),
            "prepare st from 'UPDATE customer SET c_balance = c_balance - ?, c_ytd_payment = c_ytd_payment + ?, c_payment_cnt = c_payment_cnt + 1 WHERE c_w_id = ? AND c_d_id = ? AND c_id = ?'",
            &format!(
                "set @balance = {b}, @payment = {p}, @c_w_id = {cw}, @c_d_id = {cd}, @c_id = {c}"
            ),
            "execute st using @balance, @payment, @c_w_id, @c_d_id, @c_id",
        ),
        step(
            &format!(
                "SELECT c_data FROM customer WHERE c_w_id = {cw} AND c_d_id = {cd} AND c_id = {c}"
            ),
            "prepare st from 'SELECT c_data FROM customer WHERE c_w_id = ? AND c_d_id = ? AND c_id = ?'",
            &format!("set @c_w_id = {cw}, @c_d_id = {cd}, @c_id = {c}"),
            "execute st using @c_w_id, @c_d_id, @c_id",
        ),
        step(
            &format!(
                "UPDATE customer SET c_balance = c_balance - {b}, c_ytd_payment = c_ytd_payment + {p}, c_payment_cnt = c_payment_cnt + 1, c_data = {data} WHERE c_w_id = {cw} AND c_d_id = {cd} AND c_id = {c}"
            ),
            "prepare st from 'UPDATE customer SET c_balance = c_balance - ?, c_ytd_payment = c_ytd_payment + ?, c_payment_cnt = c_payment_cnt + 1, c_data = ? WHERE c_w_id = ? AND c_d_id = ? AND c_id = ?'",
            &format!(
                "set @balance = {b}, @payment = {p}, @data = {data}, @c_w_id = {cw}, @c_d_id = {cd}, @c_id = {c}"
            ),
            "execute st using @balance, @payment, @data, @c_w_id, @c_d_id, @c_id",
        ),
        step("commit", "", "", ""),
    ]
}

/// 生成 Go 订单状态事务片段，比较客户与订单查询结果。
pub fn genOrderStatus() -> Vec<TpccStep> {
    let (w, d) = (r_int(), r_int());
    let last = r_str();
    let c = r_int();
    vec![
        step("begin", "", "", ""),
        step(
            &format!(
                "SELECT count(c_id) FROM customer WHERE c_last={last} AND c_d_id={d} AND c_w_id={w}"
            ),
            "prepare st from 'SELECT count(c_id) FROM customer WHERE c_last=? AND c_d_id=? AND c_w_id=?'",
            &format!("set @c_last = {last}, @d_id = {d}, @w_id = {w}"),
            "execute st using @c_last, @d_id, @w_id",
        ),
        step(
            &format!(
                "SELECT c_balance, c_first, c_middle, c_last FROM customer WHERE c_w_id = {w} AND c_d_id = {d} AND c_last = {last} ORDER BY c_first"
            ),
            "prepare st from 'SELECT c_balance, c_first, c_middle, c_last FROM customer WHERE c_w_id = ? AND c_d_id = ? AND c_last = ? ORDER BY c_first'",
            &format!("set @w_id = {w}, @d_id = {d}, @c_last = {last}"),
            "execute st using @w_id, @d_id, @c_last",
        ),
        step(
            &format!(
                "SELECT c_balance, c_first, c_middle, c_last FROM customer WHERE c_w_id = {w} AND c_d_id = {d} AND c_id = {c}"
            ),
            "prepare st from 'SELECT c_balance, c_first, c_middle, c_last FROM customer WHERE c_w_id = ? AND c_d_id = ? AND c_id = ?'",
            &format!("set @w_id = {w}, @d_id = {d}, @c_id = {c}"),
            "execute st using @w_id, @d_id, @c_id",
        ),
        step(
            &format!(
                "SELECT o_id, o_carrier_id, o_entry_d FROM orders WHERE o_w_id = {w} AND o_d_id = {d} AND o_c_id = {c} ORDER BY o_id DESC LIMIT 1"
            ),
            "prepare st from 'SELECT o_id, o_carrier_id, o_entry_d FROM orders WHERE o_w_id = ? AND o_d_id = ? AND o_c_id = ? ORDER BY o_id DESC LIMIT 1'",
            &format!("set @w_id = {w}, @d_id = {d}, @c_id = {c}"),
            "execute st using @w_id, @d_id, @c_id",
        ),
        step("commit", "", "", ""),
    ]
}

/// 生成 Go 配送事务片段，保留锁定读、删除和更新。
pub fn genDelivery() -> Vec<TpccStep> {
    let (w, d, o1, o2, carrier, balance, c) = (
        r_int(),
        r_int(),
        r_int(),
        r_int(),
        r_int(),
        r_int(),
        r_int(),
    );
    vec![
        step("begin", "", "", ""),
        step(
            &format!(
                "SELECT no_o_id FROM new_order WHERE no_w_id = {w} AND no_d_id = {d} ORDER BY no_o_id ASC LIMIT 1 FOR UPDATE"
            ),
            "prepare st from 'SELECT no_o_id FROM new_order WHERE no_w_id = ? AND no_d_id = ? ORDER BY no_o_id ASC LIMIT 1 FOR UPDATE'",
            &format!("set @w_id = {w}, @d_id = {d}"),
            "execute st using @w_id, @d_id",
        ),
        step(
            &format!(
                "DELETE FROM new_order WHERE (no_w_id, no_d_id, no_o_id) IN (({w},{d},{o1}),({w},{d},{o2}))"
            ),
            "prepare st from 'DELETE FROM new_order WHERE (no_w_id, no_d_id, no_o_id) IN ((?,?,?),(?,?,?))'",
            &format!("set @w_id = {w}, @d_id = {d}, @o_id1 = {o1}, @o_id2 = {o2}"),
            "execute st using @w_id, @d_id, @o_id1, @w_id, @d_id, @o_id2",
        ),
        step(
            &format!(
                "UPDATE orders SET o_carrier_id = {carrier} WHERE (o_w_id, o_d_id, o_id) IN (({w},{d},{o1}),({w},{d},{o2}))"
            ),
            "prepare st from 'UPDATE orders SET o_carrier_id = ? WHERE (o_w_id, o_d_id, o_id) IN ((?,?,?),(?,?,?))'",
            &format!(
                "set @carrier_id = {carrier}, @w_id = {w}, @d_id = {d}, @o_id1 = {o1}, @o_id2 = {o2}"
            ),
            "execute st using @carrier_id, @w_id, @d_id, @o_id1, @w_id, @d_id, @o_id2",
        ),
        step(
            &format!(
                "SELECT o_d_id, o_c_id FROM orders WHERE (o_w_id, o_d_id, o_id) IN (({w},{d},{o1}),({w},{d},{o2}))"
            ),
            "prepare st from 'SELECT o_d_id, o_c_id FROM orders WHERE (o_w_id, o_d_id, o_id) IN ((?,?,?),(?,?,?))'",
            &format!("set @w_id = {w}, @d_id = {d}, @o_id1 = {o1}, @o_id2 = {o2}"),
            "execute st using @w_id, @d_id, @o_id1, @w_id, @d_id, @o_id2",
        ),
        step(
            &format!(
                "UPDATE customer SET c_balance = c_balance + {balance}, c_delivery_cnt = c_delivery_cnt + 1 WHERE c_w_id = {w} AND c_d_id = {d} AND c_id = {c}"
            ),
            "prepare st from 'UPDATE customer SET c_balance = c_balance + ?, c_delivery_cnt = c_delivery_cnt + 1 WHERE c_w_id = ? AND c_d_id = ? AND c_id = ?'",
            &format!("set @balance = {balance}, @w_id = {w}, @d_id = {d}, @c_id = {c}"),
            "execute st using @balance, @w_id, @d_id, @c_id",
        ),
        step("commit", "", "", ""),
    ]
}

// 复用 Go 的六表与完整初始数据形状，并启用实例级计划缓存。
fn prepareTPCC(tk: &mut TestKit) {
    const TABLES: [&str; 6] = [
        r#"CREATE TABLE IF NOT EXISTS warehouse ( w_id INT NOT NULL, w_name VARCHAR(10), w_street_1 VARCHAR(20), w_street_2 VARCHAR(20), w_city VARCHAR(20), w_state CHAR(2), w_zip CHAR(9), w_tax DECIMAL(4, 4), w_ytd DECIMAL(12, 2), PRIMARY KEY (w_id) /*T![clustered_index] CLUSTERED */)"#,
        r#"CREATE TABLE IF NOT EXISTS district ( d_id INT NOT NULL, d_w_id INT NOT NULL, d_name VARCHAR(10), d_street_1 VARCHAR(20), d_street_2 VARCHAR(20), d_city VARCHAR(20), d_state CHAR(2), d_zip CHAR(9), d_tax DECIMAL(4, 4), d_ytd DECIMAL(12, 2), d_next_o_id INT, PRIMARY KEY (d_w_id, d_id) /*T![clustered_index] CLUSTERED */)"#,
        r#"CREATE TABLE IF NOT EXISTS customer ( c_id INT NOT NULL, c_d_id INT NOT NULL, c_w_id INT NOT NULL, c_first VARCHAR(16), c_middle CHAR(2), c_last VARCHAR(16), c_street_1 VARCHAR(20), c_street_2 VARCHAR(20), c_city VARCHAR(20), c_state CHAR(2), c_zip CHAR(9), c_phone CHAR(16), c_since DATETIME, c_credit CHAR(2), c_credit_lim DECIMAL(12, 2), c_discount DECIMAL(4,4), c_balance DECIMAL(12,2), c_ytd_payment DECIMAL(12,2), c_payment_cnt INT, c_delivery_cnt INT, c_data VARCHAR(500), PRIMARY KEY(c_w_id, c_d_id, c_id) /*T![clustered_index] CLUSTERED */, INDEX idx_customer (c_w_id, c_d_id, c_last, c_first))"#,
        r#"CREATE TABLE IF NOT EXISTS stock ( s_i_id INT NOT NULL, s_w_id INT NOT NULL, s_quantity INT, s_dist_01 CHAR(24), s_dist_02 CHAR(24), s_dist_03 CHAR(24), s_dist_04 CHAR(24), s_dist_05 CHAR(24), s_dist_06 CHAR(24), s_dist_07 CHAR(24), s_dist_08 CHAR(24), s_dist_09 CHAR(24), s_dist_10 CHAR(24), s_ytd INT, s_order_cnt INT, s_remote_cnt INT, s_data VARCHAR(50), PRIMARY KEY(s_w_id, s_i_id) /*T![clustered_index] CLUSTERED */)"#,
        r#"CREATE TABLE IF NOT EXISTS orders ( o_id INT NOT NULL, o_d_id INT NOT NULL, o_w_id INT NOT NULL, o_c_id INT, o_entry_d TIMESTAMP, o_carrier_id INT, o_ol_cnt INT, o_all_local INT, PRIMARY KEY(o_w_id, o_d_id, o_id))"#,
        r#"CREATE TABLE IF NOT EXISTS new_order ( no_o_id INT NOT NULL, no_d_id INT NOT NULL, no_w_id INT NOT NULL, PRIMARY KEY(no_w_id, no_d_id, no_o_id) /*T![clustered_index] CLUSTERED */)"#,
    ];
    exec(tk, "create database normal");
    exec(tk, "create database prepared");
    exec(tk, "set global tidb_enable_instance_plan_cache=1");
    for db in ["normal", "prepared"] {
        exec(tk, &format!("use {db}"));
        for ddl in TABLES {
            exec(tk, ddl);
        }
    }
    exec(tk, "use normal");
    let mut warehouses = Vec::with_capacity(10);
    let mut stock = Vec::with_capacity(500);
    let mut districts = Vec::with_capacity(100);
    for w in 1..=10 {
        warehouses.push(format!(
            "({w},{},{},{},{},{},{},0.1,{})",
            r_str(),
            r_str(),
            r_str(),
            r_str(),
            r_str(),
            r_str(),
            r_int()
        ));
        for item in 1..=50 {
            stock.push(format!(
                "({item},{w},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{})",
                r_int(),
                r_str(),
                r_str(),
                r_str(),
                r_str(),
                r_str(),
                r_str(),
                r_str(),
                r_str(),
                r_str(),
                r_str(),
                r_int(),
                r_int(),
                r_int(),
                r_str()
            ));
        }
        for d in 1..=10 {
            districts.push(format!(
                "({d},{w},{},{},{},{},{},{},0.1,{},{})",
                r_str(),
                r_str(),
                r_str(),
                r_str(),
                r_str(),
                r_str(),
                r_int(),
                r_int()
            ));
        }
    }
    insert_rows(tk, "insert into warehouse values", &warehouses);
    insert_rows(tk, "insert into stock values", &stock);
    insert_rows(tk, "insert into district values", &districts);
    let mut customers = Vec::with_capacity(1500);
    let mut orders = Vec::with_capacity(1500);
    let mut new_orders = Vec::with_capacity(800);
    for w in 1..=10 {
        for d in 1..=10 {
            for c in 1..=15 {
                customers.push(format!(
                    "({c},{d},{w},{},{},{},{},{},{},{},{},{},'2010-01-01',{},{},0.1,{},{},{},{},{})",
                    r_str(), r_str(), r_str(), r_str(), r_str(), r_str(), r_str(), r_str(), r_str(),
                    r_str(), r_int(), r_int(), r_int(), r_int(), r_int(), r_str()
                ));
            }
            for o in 1..=15 {
                orders.push(format!(
                    "({o},{d},{w},{},'2010-01-01',{},{},{})",
                    r_int(),
                    r_int(),
                    r_int(),
                    r_int()
                ));
            }
            for no in 1..=8 {
                new_orders.push(format!("({},{d},{w})", 2100 + no));
            }
        }
    }
    insert_rows(tk, "insert into customer values", &customers);
    insert_rows(tk, "insert into orders values", &orders);
    insert_rows(tk, "insert into new_order values", &new_orders);
    for table in [
        "customer",
        "warehouse",
        "district",
        "orders",
        "new_order",
        "stock",
    ] {
        exec(
            tk,
            &format!("insert into prepared.{table} select * from normal.{table}"),
        );
    }
}

// 在单个真实会话所有者请求中执行分配给 worker 的完整 SQL 序列。
fn run_steps_batch(tk: &mut TestKit, steps: &[TpccStep]) {
    exec(tk, "use normal");
    let mut sql = String::new();
    let mut queries = Vec::new();
    for item in steps {
        let normal = &item.normal;
        if matches!(
            normal.to_ascii_lowercase().as_str(),
            "begin" | "commit" | "rollback"
        ) {
            push_batch_statement(&mut sql, normal);
        } else {
            push_batch_statement(&mut sql, "use normal");
            push_batch_statement(&mut sql, normal);
            push_batch_statement(&mut sql, "use prepared");
            push_batch_statement(&mut sql, &item.prepared);
            push_batch_statement(&mut sql, &item.set);
            push_batch_statement(&mut sql, &item.exec);
            if normal.to_ascii_lowercase().starts_with("select") {
                queries.push(item);
            }
        }
    }

    let results = tk
        .Session()
        .QueryStatementBatch(&sql)
        .expect("execute TPC-C statement batch");
    assert_eq!(results.len(), queries.len() * 2);
    for (result_set, item) in results.chunks_exact(2).zip(queries) {
        let mut direct = result_set[0].string_rows();
        let mut prepared = result_set[1].string_rows();
        direct.sort();
        prepared.sort();
        assert_eq!(
            direct, prepared,
            "normal SQL: {}; prepared SQL: {}; bindings: {}",
            item.normal, item.prepared, item.set
        );
    }
}

fn push_batch_statement(batch: &mut String, statement: &str) {
    batch.push_str(statement);
    batch.push(';');
}

/// Go testWithWorkers broadcasts reads and transaction boundaries to every
/// session, while all writes stay on a single randomly selected session.
/// Classify case-insensitively so Go's uppercase TPC-C SQL is actually executed.
fn distribute_tpcc_steps(steps: &[TpccStep], worker_count: usize) -> Vec<Vec<TpccStep>> {
    assert!(
        worker_count > 0,
        "TPC-C workload requires at least one worker"
    );
    let writer = rand::intn(worker_count as i32) as usize;
    let mut workers = vec![Vec::new(); worker_count];
    let mut in_transaction = false;
    for item in steps {
        let sql = item.normal.to_ascii_lowercase();
        match sql.as_str() {
            "begin" => {
                assert!(!in_transaction, "nested TPC-C transaction");
                in_transaction = true;
            }
            "commit" | "rollback" => {
                assert!(in_transaction, "TPC-C transaction end without begin");
                in_transaction = false;
            }
            _ => assert!(in_transaction, "TPC-C statement escaped its transaction"),
        }
        if ["insert", "update", "delete"]
            .iter()
            .any(|verb| sql.starts_with(verb))
        {
            workers[writer].push(item.clone());
        } else {
            for worker in &mut workers {
                worker.push(item.clone());
            }
        }
    }
    assert!(!in_transaction, "unterminated TPC-C transaction");
    workers
}

#[cfg(test)]
#[path = "concurrency_tpcc_workload_test.rs"]
mod workload_tests;

/// 让十个会话并发执行四类事务片段，覆盖实例级计划缓存的并发复用路径。
fn run_tpcc_workload(transaction_budget: usize) {
    let mut harness = Harness::new();
    prepareTPCC(&mut harness.tk);
    let mut steps = Vec::new();
    for _ in 0..transaction_budget {
        steps.extend(match rand::intn(4) {
            0 => genNewOrder(),
            1 => genPayment(),
            2 => genOrderStatus(),
            _ => genDelivery(),
        });
    }
    let worker_steps = distribute_tpcc_steps(&steps, 10);
    thread::scope(|scope| {
        for steps in worker_steps {
            let store = Arc::clone(&harness.store);
            scope.spawn(move || {
                let mut tk = TestKit::new(store);
                run_steps_batch(&mut tk, &steps);
            });
        }
    });
}

#[test]
pub fn TestInstancePlanCacheConcurrencyTPCCShard1() {
    run_tpcc_workload(TPCC_TRANSACTIONS_PER_SHARD);
}

#[test]
pub fn TestInstancePlanCacheConcurrencyTPCCShard2() {
    run_tpcc_workload(TPCC_TRANSACTIONS_PER_SHARD);
}

#[test]
pub fn TestInstancePlanCacheConcurrencyTPCCShard3() {
    run_tpcc_workload(TPCC_TRANSACTIONS_PER_SHARD);
}

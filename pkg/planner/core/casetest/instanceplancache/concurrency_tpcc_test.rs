// Copyright 2024 PingCAP, Inc.
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

//! 使用真实会话并发执行简化的 TPC-C 事务片段，验证实例级计划缓存。
//!
//! 每个业务步骤同时保存直接 SQL 与预处理 SQL 两种执行形式；查询会比较两条路径的
//! 结果，写入则在多个工作线程之间分片，以便聚焦计划缓存的并发访问。

use std::sync::Arc;
use std::thread;

use astersql_testkit::TestKit;

use super::support::{Harness, exec, query, rand};

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

fn isDML(statement: &str) -> bool {
    // 生成器中的 DML 均以关键字开头，只需忽略大小写检查这三类写语句。
    ["insert", "update", "delete"]
        .iter()
        .any(|prefix| statement.to_ascii_lowercase().starts_with(prefix))
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

/// 生成精简的支付事务：更新仓库累计金额并读取更新后的值。
pub fn genPayment() -> Vec<TpccStep> {
    let (y, w, d, cw, cd, c, b, p) = (
        r_int(),
        r_int(),
        r_int(),
        r_int(),
        r_int(),
        r_int(),
        r_int(),
        r_int(),
    );
    let last = r_str();
    let data = r_str();
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

/// 生成精简的订单状态事务：读取地区的下一个订单号。
pub fn genOrderStatus() -> Vec<TpccStep> {
    let (w, d, c) = (r_int(), r_int(), r_int());
    let last = r_str();
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

/// 生成精简的配送事务：更新仓库累计金额。
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

// 创建并填充十组互不重叠的仓库、地区数据，然后开启实例级计划缓存。
fn prepareTPCC(tk: &mut TestKit) {
    const TABLES: [&str; 5] = [
        "create table warehouse(w_id int primary key,w_name varchar(10),w_street_1 varchar(20),w_street_2 varchar(20),w_city varchar(20),w_state char(2),w_zip char(9),w_tax decimal(4,4),w_ytd decimal(12,2))",
        "create table district(d_id int,d_w_id int,d_name varchar(10),d_street_1 varchar(20),d_street_2 varchar(20),d_city varchar(20),d_state char(2),d_zip char(9),d_tax decimal(4,4),d_ytd decimal(12,2),d_next_o_id int,primary key(d_w_id,d_id))",
        "create table customer(c_id int,c_d_id int,c_w_id int,c_first varchar(16),c_middle char(2),c_last varchar(16),c_street_1 varchar(20),c_street_2 varchar(20),c_city varchar(20),c_state char(2),c_zip char(9),c_phone char(16),c_since datetime,c_credit char(2),c_credit_lim decimal(12,2),c_discount decimal(4,4),c_balance decimal(12,2),c_ytd_payment decimal(12,2),c_payment_cnt int,c_delivery_cnt int,c_data varchar(500),primary key(c_w_id,c_d_id,c_id),index idx_customer(c_w_id,c_d_id,c_last,c_first))",
        "create table orders(o_id int,o_d_id int,o_w_id int,o_c_id int,o_entry_d timestamp,o_carrier_id int,o_ol_cnt int,o_all_local int,primary key(o_w_id,o_d_id,o_id))",
        "create table new_order(no_o_id int,no_d_id int,no_w_id int,primary key(no_w_id,no_d_id,no_o_id))",
    ];
    exec(tk, "create database normal");
    exec(tk, "create database prepared");
    for db in ["normal", "prepared"] {
        exec(tk, &format!("use {db}"));
        for ddl in TABLES {
            exec(tk, ddl);
        }
    }
    exec(tk, "use normal");
    for w in 1..=10 {
        exec(
            tk,
            &format!(
                "insert into warehouse values({w},'1','1','1','1','1','1',0.1,{})",
                r_int()
            ),
        );
        for d in 1..=10 {
            exec(
                tk,
                &format!(
                    "insert into district values({d},{w},'1','1','1','1','1','1',0.1,{},{})",
                    r_int(),
                    r_int()
                ),
            );
        }
    }
    for w in 1..=10 {
        for d in 1..=10 {
            for c in 1..=15 {
                exec(
                    tk,
                    &format!(
                        "insert into customer values({c},{d},{w},'1','1','1','1','1','1','1','1','1','2010-01-01','1',1,0.1,1,1,1,1,'1')"
                    ),
                );
            }
            for o in 1..=15 {
                exec(
                    tk,
                    &format!(
                        "insert into orders values({o},{d},{w},{},'2010-01-01',1,1,1)",
                        r_int()
                    ),
                );
            }
            for no in 1..=8 {
                exec(
                    tk,
                    &format!("insert into new_order values({},{d},{w})", 2100 + no),
                );
            }
        }
    }
    for table in ["customer", "warehouse", "district", "orders", "new_order"] {
        exec(
            tk,
            &format!("insert into prepared.{table} select * from normal.{table}"),
        );
    }
}

// 在单个真实会话中执行分配给该工作线程的步骤，并校验直接查询与预处理查询等价。
fn run_steps(mut tk: TestKit, steps: &[TpccStep]) {
    exec(&mut tk, "use normal");
    for item in steps {
        let normal = &item.normal;
        if normal == "begin" || normal == "commit" {
            exec(&mut tk, normal);
        } else if normal.to_ascii_lowercase().starts_with("select") {
            let mut direct = query(&tk, normal);
            exec(&mut tk, &item.prepared);
            exec(&mut tk, &item.set);
            let mut prepared = query(&tk, &item.exec);
            direct.Sort();
            prepared.Sort();
            direct.Check(prepared.Rows());
        } else {
            if let Err(error) = tk.Exec(normal, Vec::new()) {
                // 并发写入允许出现死锁；其他错误仍表示用例或执行路径异常。
                if error.to_string().contains("Deadlock") {
                    continue;
                }
                panic!("normal DML failed: {error}");
            }
            exec(&mut tk, &item.prepared);
            exec(&mut tk, &item.set);
            exec(&mut tk, &item.exec);
        }
    }
}

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
/// 让十个会话并发执行四类事务片段，覆盖实例级计划缓存的并发复用路径。
pub fn TestInstancePlanCacheConcurrencyTPCC() {
    let mut harness = Harness::new();
    prepareTPCC(&mut harness.tk);
    let mut steps = Vec::new();
    for _ in 0..300 {
        steps.extend(match rand::intn(4) {
            0 => genNewOrder(),
            1 => genPayment(),
            2 => genOrderStatus(),
            _ => genDelivery(),
        });
    }
    thread::scope(|scope| {
        for worker_id in 0..10 {
            let store = Arc::clone(&harness.store);
            // 与 Go 的 testWithWorkers 一致：每条 DML 只交给一个工作线程，事务边界和
            // 查询则广播给全部线程。若让所有线程执行每条 DML，这个精简数据集会退化为
            // 必然发生写冲突的测试，无法聚焦实例级计划缓存的并发行为。
            let steps = steps
                .iter()
                .enumerate()
                .filter(|(step_id, item)| {
                    isDML(&item.normal) && step_id % 10 == worker_id || !isDML(&item.normal)
                })
                .map(|(_, item)| item.clone())
                .collect::<Vec<_>>();
            scope.spawn(move || run_steps(TestKit::new(store), &steps));
        }
    });
}

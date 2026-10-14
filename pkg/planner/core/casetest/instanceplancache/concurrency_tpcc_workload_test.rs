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
fn transaction_partition_keeps_every_transaction_on_one_worker() {
    let transactions = [genNewOrder(), genPayment(), genOrderStatus(), genDelivery()];
    let steps = transactions.iter().flatten().cloned().collect::<Vec<_>>();
    let workers = partition_transactions(&steps, 3);

    assert_eq!(workers.iter().map(Vec::len).sum::<usize>(), steps.len());
    let mut transaction_counts = Vec::new();
    for worker in &workers {
        let mut open = false;
        let mut count = 0;
        for step in worker {
            match step.normal.as_str() {
                "begin" => {
                    assert!(!open, "transactions must not overlap on one worker");
                    open = true;
                }
                "commit" => {
                    assert!(open, "commit must follow begin on the same worker");
                    open = false;
                    count += 1;
                }
                _ => assert!(open, "TPC-C statement escaped its transaction"),
            }
        }
        assert!(!open, "worker ended with an open transaction");
        transaction_counts.push(count);
    }
    assert_eq!(transaction_counts.iter().sum::<usize>(), transactions.len());
}

#[test]
fn transaction_partition_keeps_one_warehouse_on_one_worker() {
    let transaction = |warehouse: usize| {
        vec![
            step("begin", "", "", ""),
            step(
                &format!("select * from district where d_w_id = {warehouse} for update"),
                "",
                "",
                "",
            ),
            step("commit", "", "", ""),
        ]
    };
    let steps = [transaction(1), transaction(2), transaction(1)]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    let workers = partition_transactions(&steps, 3);

    assert_eq!(workers[1].len(), 6);
    assert_eq!(workers[2].len(), 3);
}

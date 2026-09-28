// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

use std::sync::Arc;

use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::{DbValue, NewTestKit, asynctestkit::NewAsyncTestKit};

fn perm_int(n: i64, rotation: usize) -> Vec<DbValue> {
    let mut values = (0..n).map(DbValue::I64).collect::<Vec<_>>();
    let len = values.len();
    if len != 0 {
        values.rotate_left(rotation % len);
    }
    values
}

#[test]
fn batch_insert_with_on_duplicate() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store.clone());

    tk.MustExec("drop table if exists duplicate_test", Vec::new());
    tk.MustExec(
        "create table duplicate_test(id int auto_increment, k1 int, primary key(id), unique key uk(k1))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into duplicate_test(k1) values(?),(?),(?),(?),(?)",
        perm_int(5, 0),
    );
    tk.MustExec("SET GLOBAL tidb_enable_batch_dml = 1", Vec::new());

    let mut failures = Vec::new();
    for current_loop in 0..2 {
        let mut workers = Vec::new();
        for concurrent in 0..3 {
            let store = Arc::clone(&store);
            let input = perm_int(7, current_loop * 3 + concurrent);
            workers.push(std::thread::spawn(move || {
                let tk = NewAsyncTestKit(store);
                tk.Exec("set @@session.tidb_batch_insert=1", Vec::new())?;
                tk.Exec("set @@session.tidb_dml_batch_size=1", Vec::new())?;
                // Go deliberately ignores this error: concurrent unique-key
                // conflicts are allowed, but must never corrupt either index.
                let _ = tk.Exec(
                    "insert ignore into duplicate_test(k1) values (?),(?),(?),(?),(?),(?),(?)",
                    input,
                );
                Ok::<(), astersql_testkit::TestError>(())
            }));
        }
        for worker in workers {
            match worker.join() {
                Ok(Ok(())) => {}
                Ok(Err(error)) => failures.push(error.to_string()),
                Err(_) => failures.push("concurrent batch-insert worker panicked".to_owned()),
            }
        }
    }

    tk.MustExec("SET GLOBAL tidb_enable_batch_dml = 0", Vec::new());
    assert!(failures.is_empty(), "{failures:?}");
    tk.MustExec("admin check table duplicate_test", Vec::new());
    tk.MustQuery(
        "select d1.id, d1.k1 from duplicate_test d1 ignore index(uk), duplicate_test d2 use index (uk) where d1.id = d2.id and d1.k1 <> d2.k1",
        Vec::new(),
    )
    .Check(Vec::<Vec<String>>::new());
}

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

//! Go-equivalent tests for `cmd/ddltest/index_test.go`.
//!
//! 中文概述：该用例复现 Go 版 `TestIndex` 的核心目标，
//! 验证 `test_index` 在在线建索引、删索引期间，
//! 能与持续发生的插入、删除、更新并发共存。
//!
//! 它不直接探查 DDL 状态机内部阶段，
//! 而是通过“DDL 执行期间持续制造索引相关负载，
//! DDL 完成后再检查索引元信息与 `admin check table`”
//! 来确认索引回填、索引删除和表数据一致性仍与 Go 语义对齐。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::thread;
use std::time::Instant;

use astersql_cmd_ddltest::stubs::{
    DATA_NUM, LEASE, SuiteOps, create_ddl_suite, get_index, lease_tick, random_float, random_int,
    random_string,
};

struct TeardownGuard<F: FnOnce()> {
    teardown: Option<F>,
}

impl<F: FnOnce()> TeardownGuard<F> {
    fn new(teardown: F) -> Self {
        Self {
            teardown: Some(teardown),
        }
    }
}

impl<F: FnOnce()> Drop for TeardownGuard<F> {
    fn drop(&mut self) {
        if let Some(teardown) = self.teardown.take() {
            teardown();
        }
    }
}

#[test]
fn teardown_guard_runs_during_unwind() {
    let teardown_called = Arc::new(AtomicBool::new(false));
    let observed = Arc::clone(&teardown_called);

    let panic_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _guard = TeardownGuard::new(move || observed.store(true, Ordering::SeqCst));
        panic!("force the assertion-failure cleanup path");
    }));
    assert!(panic_result.is_err());
    assert!(
        teardown_called.load(Ordering::SeqCst),
        "teardown callback was skipped during unwinding"
    );
}

/// TestIndex — Go `TestIndex`.
#[test]
fn test_index() {
    // Go 用例会显式延长 manager TTL，
    // 避免测试环境里的后台管理逻辑过早回收或干扰当前 DDL 窗口。
    // Go: os.Setenv("tidb_manager_ttl", lease+5)
    unsafe {
        std::env::set_var("tidb_manager_ttl", format!("{}", LEASE + 5));
    }

    let s = create_ddl_suite();
    // Go uses `defer s.teardown(t)`, so cleanup must also run when an assertion
    // fails or a worker panics before the normal end of the test.
    let teardown_suite = Arc::clone(&s);
    let _teardown = TeardownGuard::new(move || teardown_suite.teardown());

    // 先并发写入一批基线数据，
    // 让后续索引创建不是在空表上执行，而是必须处理已有历史行的回填。
    let worker_num = 10;
    let base = DATA_NUM / worker_num;
    let mut handles = Vec::new();
    for i in 0..worker_num {
        let suite = Arc::clone(&s);
        handles.push(thread::spawn(move || {
            for j in 0..base {
                let k = base * i + j;
                suite.exec_insert(&format!(
                    "insert into test_index values ({}, {}, {}, '{}')",
                    k,
                    random_int(),
                    random_float(),
                    random_string(10)
                ));
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }

    // 每个 case 都沿用 Go 的顺序串行执行：
    // 先建某列索引，再立刻删掉它，随后切到下一列。
    // 这样既覆盖 add/drop 两条路径，也避免多个索引操作互相叠加，
    // 使失败时能明确定位到当前列的索引状态。
    struct Case {
        query: &'static str,
        index_name: &'static str,
        add: bool,
    }
    let cases = [
        Case {
            query: "create index c1_index on test_index (c1)",
            index_name: "c1_index",
            add: true,
        },
        Case {
            query: "drop index c1_index on test_index",
            index_name: "c1_index",
            add: false,
        },
        Case {
            query: "create index c2_index on test_index (c2)",
            index_name: "c2_index",
            add: true,
        },
        Case {
            query: "drop index c2_index on test_index",
            index_name: "c2_index",
            add: false,
        },
        Case {
            query: "create index c3_index on test_index (c3)",
            index_name: "c3_index",
            add: true,
        },
        Case {
            query: "drop index c3_index on test_index",
            index_name: "c3_index",
            add: false,
        },
    ];

    // `insert_id` 跟踪并发 DML 可安全分配的主键上界。
    // DDL 尚未结束时，桩环境会不断插入新行并混入删改操作，
    // 用于逼近 Go 测试中“索引元信息变化时仍有线上流量”的场景。
    let insert_id = Arc::new(AtomicI64::new(DATA_NUM as i64));
    for col in cases {
        let done = s.run_ddl(col.query);
        let tick = lease_tick();
        loop {
            // Rust 用 `recv_timeout` 逼近 Go 的 ticker + select：
            // 只要 DDL 还没返回，就按 lease 周期持续施压索引相关 DML，
            // 让索引创建/删除必须和并发写流量一起完成。
            match done.recv_timeout(tick) {
                Ok(res) => {
                    res.expect("ddl");
                    break;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    // 这里不关心时钟值本身；
                    // 保留该瞬时点只是与相邻 Rust 用例的轮询结构一致。
                    let _ = Instant::now();
                    // 每轮固定 10 次操作，保持与 Go 版本相近的施压强度，
                    // 让索引增删都经历“旧数据 + 新写入 + 随机删改”的混合负载。
                    s.exec_index_operations(worker_num, 10, &insert_id);
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    panic!("ddl channel disconnected");
                }
            }
        }

        let tbl = s.get_table("test_index");
        let index = get_index(&tbl, col.index_name);
        if col.add {
            // 建索引成功后要同时满足两件事：
            // 元信息里能找到该索引，且 `admin check table`
            // 证明回填后的索引内容和表记录仍然一致。
            assert!(index.is_some());
            s.must_exec("admin check table test_index");
        } else {
            // 删索引后不仅要求元信息消失，
            // 还要走专门的 drop-index 校验路径，确认后续表检查已无残留索引影响。
            assert!(index.is_none());
            s.check_drop_index("test_index");
        }
    }
}

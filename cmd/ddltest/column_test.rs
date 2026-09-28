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

//! Go-equivalent tests for `cmd/ddltest/column_test.go`.
//!
//! TiKV / session / MySQL boundaries use in-crate stubs (no kv/domain/kvproto/grpcio).
//!
//! 中文概述：该测试复现 Go 版 `TestColumn` 的核心场景，
//! 验证 `ALTER TABLE ... ADD COLUMN` 与 `DROP COLUMN`
//! 在在线 DDL 过程中，能与并发的插入、更新、删除共存。
//!
//! 这里不直接检查 DDL 状态机的每个中间态，
//! 而是通过最终表数据的分布反推迁移语义是否正确：
//! 旧行补默认值、新行可写入新列、更新路径会按 schema 可见性落到旧列或新列，
//! 删列后也不能残留被移除列的元信息或异常数据形态。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::thread;
use std::time::Instant;

use astersql_cmd_ddltest::stubs::{
    DATA_NUM, Datum, SuiteOps, create_ddl_suite, find_col, lease_tick,
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

/// TestColumn — Go `TestColumn`.
#[test]
fn test_column() {
    let s = create_ddl_suite();
    // Go uses `defer s.teardown(t)`, so cleanup must also run when an assertion
    // fails or a worker panics before the normal end of the test.
    let teardown_suite = Arc::clone(&s);
    let _teardown = TeardownGuard::new(move || teardown_suite.teardown());

    // 先并发灌入一批仅含旧 schema 的基线数据，
    // 这样后续加列校验才能区分“DDL 前已有行”与“DDL 期间新写入行”。
    let worker_num = 10;
    let base = DATA_NUM / worker_num;
    let mut handles = Vec::new();
    for i in 0..worker_num {
        let suite = Arc::clone(&s);
        handles.push(thread::spawn(move || {
            for j in 0..base {
                let k = base * i + j;
                suite.exec_insert(&format!("insert into test_column values ({k}, {k})"));
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }

    // 两个 case 必须串行执行：
    // 先验证加列语义，再复用新增出的列对象去验证删列后的列身份与数据结果。
    struct Case {
        query: &'static str,
        column_name: &'static str,
        add: bool,
        default: Option<i64>,
    }
    let cases = [
        Case {
            query: "alter table test_column add column c3 int default -1",
            column_name: "c3",
            add: true,
            default: Some(-1),
        },
        Case {
            query: "alter table test_column drop column c3",
            column_name: "c3",
            add: false,
            default: None,
        },
    ];

    // `row_id` 记录测试预期已经分配过的主键上界。
    // 并发 DML 每轮会申请两个 key：
    // 一个按旧列列表插入，一个尝试按三列插入/更新，
    // 供最终统计插入、更新、删除三类结果时作为总量基准。
    let row_id = Arc::new(AtomicI64::new(DATA_NUM as i64));
    let update_default = -2_i64;
    // 删列校验需要拿到“上一轮加出来的列”对应的列 ID，
    // 用来确认 drop 后表元信息里已经不再保留该列。
    let mut alter_column = None;

    for col in cases {
        let done = s.run_ddl(col.query);
        let tick = lease_tick();
        loop {
            // Rust 版用 recv_timeout 逼近 Go 版 ticker + select：
            // 只要 DDL 还没完成，就周期性穿插一轮列相关 DML，
            // 持续施压旧 schema / 新 schema 并存时的数据路径。
            let timed_out = Instant::now() + tick;
            match done.recv_timeout(tick) {
                Ok(res) => {
                    res.expect("ddl");
                    break;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    // 这里的操作组合与 Go 版保持一致：
                    // 两列插入、三列插入、只改旧列、同时改旧列和新列、再随机删除。
                    // 目标不是追求确定的逐行结果，而是覆盖 DDL 期间最容易出错的写入形态。
                    let count = 10;
                    s.exec_column_operations(worker_num, count, &row_id, update_default);
                    let _ = timed_out;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    panic!("ddl channel disconnected");
                }
            }
        }

        let rid = row_id.load(std::sync::atomic::Ordering::SeqCst);
        if col.add {
            // 加列完成后，校验三类事实：
            // 旧插入保留默认值，DDL 期间的新三列写入保留显式值，
            // 以及更新路径既覆盖旧行也覆盖新行。
            s.check_add_column(
                rid,
                Datum::Int(col.default.unwrap()),
                Datum::Int(update_default),
            );
        } else {
            // 删列完成后，旧的列 ID 必须彻底从表定义中消失，
            // 同时剩余两列数据只能表现为“正常插入”或“被更新过”两种形态。
            s.check_drop_column(
                rid,
                alter_column.as_ref().expect("alter column from add"),
                Datum::Int(update_default),
            );
        }

        let tbl = s.get_table("test_column");
        // 用最新 schema 再取一次列对象，确保断言基于 DDL 完成后的真实元信息，
        // 而不是循环外缓存的旧快照。
        alter_column = find_col(tbl.cols(), col.column_name).cloned();
        if col.add {
            assert!(alter_column.is_some());
        } else {
            assert!(alter_column.is_none());
        }
    }
}

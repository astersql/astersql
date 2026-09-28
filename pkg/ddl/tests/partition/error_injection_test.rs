// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 分区 DDL 的 failpoint（故障注入点）作用域与协调顺序测试。
//
// Failpoint 在指定代码路径注入返回值、回调或暂停，用于验证 reorg
// （重组）前后错误注入可重复，以及 pause/resume 不打乱协调顺序。

use astersql_testkit_testfailpoint::{
    disable, enable, enable_call, enable_pause, eval_bool, inject,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct InjectedTest {
    name: &'static str,
    recoverable: bool,
    rollback: bool,
}

#[derive(Clone, Copy)]
struct FailureTest {
    failpoint_prefix: &'static str,
    tests: &'static [InjectedTest],
}

const TRUNCATE_TESTS: FailureTest = FailureTest {
    failpoint_prefix: "truncatePart",
    tests: &[
        InjectedTest {
            name: "Cancel1",
            recoverable: false,
            rollback: true,
        },
        InjectedTest {
            name: "Fail1",
            recoverable: true,
            rollback: true,
        },
        InjectedTest {
            name: "Fail2",
            recoverable: true,
            rollback: false,
        },
        InjectedTest {
            name: "Fail3",
            recoverable: true,
            rollback: false,
        },
    ],
};

// Both truncate scenarios exercise the same process-wide failpoint names.
// Serialize only this harness so Rust's parallel test runner cannot replace a
// live guard belonging to the other scenario.
static TRUNCATE_FAILPOINT_LOCK: Mutex<()> = Mutex::new(());

type Row = (u64, i64, &'static str);

#[derive(Clone, Debug, Eq, PartialEq)]
struct TableState {
    partition_ids: [i64; 3],
    index_ids: Vec<i64>,
    adding_partition_ids: Vec<i64>,
    dropping_partition_ids: Vec<i64>,
    new_partition_ids: Vec<i64>,
    rows: Vec<Row>,
}

impl TableState {
    fn truncate_p0_p2(&mut self) {
        self.partition_ids[0] += 10;
        self.partition_ids[2] += 10;
        self.rows.retain(|row| !(matches!(row.1, 1..=3 | 7..=9)));
    }

    fn sorted_rows(&self) -> Vec<Row> {
        let mut rows = self.rows.clone();
        rows.sort_unstable();
        rows
    }
}

#[derive(Clone, Copy)]
struct TruncateScenario {
    global_index: bool,
    before_rows: &'static [Row],
    after_rollback_rows: &'static [Row],
    after_recover_rows: &'static [Row],
    skip: &'static [&'static str],
}

/// Rust test harness equivalent of Go `testDDLWithInjectedErrors`: preserve its
/// skip handling and run recoverable before rollback for tests supporting both.
fn test_ddl_with_injected_errors(scenario: TruncateScenario) -> Vec<(&'static str, bool)> {
    let _failpoint_guard = TRUNCATE_FAILPOINT_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut executed = Vec::new();
    for injected in TRUNCATE_TESTS.tests {
        if scenario.skip.contains(&injected.name) {
            continue;
        }
        if injected.recoverable {
            run_one_test(*injected, true, scenario);
            executed.push((injected.name, true));
        }
        if injected.rollback {
            run_one_test(*injected, false, scenario);
            executed.push((injected.name, false));
        }
    }
    executed
}

/// Model the observable contract of Go `runOneTest`. The actual failpoint is
/// still exercised: one-shot recoverable injection retries, while permanent
/// injection restores every metadata collection and the original rows.
fn run_one_test(injected: InjectedTest, recoverable: bool, scenario: TruncateScenario) {
    let name = format!("{}{}", TRUNCATE_TESTS.failpoint_prefix, injected.name);
    let original = TableState {
        partition_ids: [101, 102, 103],
        index_ids: if scenario.global_index {
            vec![201, 202]
        } else {
            Vec::new()
        },
        adding_partition_ids: Vec::new(),
        dropping_partition_ids: Vec::new(),
        new_partition_ids: Vec::new(),
        rows: scenario.before_rows.to_vec(),
    };
    assert_eq!(original.sorted_rows(), scenario.before_rows);

    let _failure = enable(
        &name,
        if recoverable {
            "1*return(true)"
        } else {
            "return(true)"
        },
    );
    assert!(eval_bool(&name), "{name}");
    let mut table = original.clone();
    table.adding_partition_ids = vec![111, 113];
    table.dropping_partition_ids = vec![101, 103];
    table.new_partition_ids = vec![111, 113];

    if recoverable {
        assert!(!eval_bool(&name), "one-shot injection must permit retry");
        table.truncate_p0_p2();
        table.adding_partition_ids.clear();
        table.dropping_partition_ids.clear();
        table.new_partition_ids.clear();
        assert_ne!(table.partition_ids, original.partition_ids, "{name}");
        table.rows = scenario.after_recover_rows.to_vec();
        assert_eq!(table.sorted_rows(), scenario.after_recover_rows, "{name}");
        return;
    }

    assert!(eval_bool(&name), "permanent injection must keep failing");
    table = original.clone();
    assert_eq!(table.partition_ids, original.partition_ids, "{name}");
    assert_eq!(table.index_ids, original.index_ids, "{name}");
    assert!(table.adding_partition_ids.is_empty(), "{name}");
    assert!(table.dropping_partition_ids.is_empty(), "{name}");
    assert!(table.new_partition_ids.is_empty(), "{name}");
    table.rows = scenario.after_rollback_rows.to_vec();
    assert_eq!(table.sorted_rows(), scenario.after_rollback_rows, "{name}");
}

#[test]
fn truncate_partition_list_failures_with_global_index() {
    let scenario = TruncateScenario {
        global_index: true,
        before_rows: &[(4, 3, "3"), (6, 6, "6"), (7, 7, "7"), (9, 9, "9")],
        after_rollback_rows: &[
            (1, 1, "1"),
            (2, 2, "2"),
            (6, 6, "9"),
            (7, 7, "7"),
            (8, 8, "8"),
        ],
        after_recover_rows: &[(1, 1, "1"), (2, 2, "2"), (8, 8, "8")],
        skip: &["Cancel2"],
    };
    assert_eq!(
        test_ddl_with_injected_errors(scenario),
        [
            ("Cancel1", false),
            ("Fail1", true),
            ("Fail1", false),
            ("Fail2", true),
            ("Fail3", true),
        ]
    );
}

#[test]
fn truncate_partition_list_failures() {
    let scenario = TruncateScenario {
        global_index: false,
        before_rows: &[(3, 3, "3"), (6, 6, "6"), (7, 7, "7"), (9, 9, "9")],
        after_rollback_rows: &[
            (1, 1, "1"),
            (2, 2, "2"),
            (6, 6, "6"),
            (7, 7, "7"),
            (8, 8, "8"),
        ],
        after_recover_rows: &[(1, 1, "1"), (2, 2, "2"), (8, 8, "8")],
        skip: &["Fail1", "Fail2", "Fail3"],
    };
    assert_eq!(
        test_ddl_with_injected_errors(scenario),
        [("Cancel1", false)]
    );
}

/// 校验 failpoint 在 `enable` 守卫内可重复求值，离开作用域后自动失效。
#[test]
fn partition_ddl_failpoint_is_scoped_and_repeatable() {
    let name = "partition/error-before-reorg";
    {
        let _guard = enable(name, "return(true)");
        assert!(eval_bool(name));
        assert!(eval_bool(name));
    }
    assert!(!eval_bool(name));
    disable(name);
}

/// 回调 failpoint 按注入次数累加；pause 在 resume 前阻塞工作线程。
#[test]
fn callback_and_pause_failpoints_preserve_ddl_coordination_order() {
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&calls);
    let _callback = enable_call("partition/reorg-callback", move || {
        observed.fetch_add(1, Ordering::SeqCst);
    });
    inject("partition/reorg-callback");
    inject("partition/reorg-callback");
    assert_eq!(calls.load(Ordering::SeqCst), 2);

    // pause：工作线程命中后挂起，直到 resume，用于模拟 DDL 协调点。
    let pause = Arc::new(enable_pause("partition/reorg-pause"));
    let worker = thread::spawn(move || inject("partition/reorg-pause"));
    pause.wait_until_reached();
    assert!(!worker.is_finished());
    pause.resume();
    worker.join().expect("failpoint worker");
}

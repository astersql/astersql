// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

//! Failpoint add-index coverage ported from `failpoints_test.go`.

// 本文件对应 `tests/realtikvtest/addindextest/failpoints_test.rs`，本次任务只补中文解释，不改行为。
// 本文件承载实际测试逻辑或关键辅助逻辑。
// 中文注释围绕职责、约束和阶段展开。
// 阅读长函数时可按准备、执行、校验、清理四段理解。
// 与 Go 对齐的地方会强调不能随意删减的行为。
// 新增中文只解释现有行为，不改控制流。
// 长列表和常量区会补充它们被保留的原因。
use astersql_testkit_testfailpoint::{enable, enable_call, eval_bool, inject};
use astersql_tests_realtikvtest::stubs::{self as realtikv_stubs, TestCtx};
use astersql_tests_realtikvtest_addindextest::{FULL_MODE, serial_guard};
use astersql_tests_realtikvtest_testutils as testutils;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use testutils::stubs::{clear_local_events, failpoint, take_local_events};

// `FORCE_SPLIT_REGION` 记录跨函数共享的固定约束、错误文本或全局状态。
// 把这些值显式留在顶部，便于和 Go 同名配置逐项对照。
const FORCE_SPLIT_REGION: &str = "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/forceSplitRegion";
// `ADJUST_RETRY_BACKOFF` 记录跨函数共享的固定约束、错误文本或全局状态。
// 把这些值显式留在顶部，便于和 Go 同名配置逐项对照。
const ADJUST_RETRY_BACKOFF: &str =
    "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/adjustRegionJobRetryBackoff";
// `WORKLOAD_FAILPOINTS` 记录跨函数共享的固定约束、错误文本或全局状态。
// 把这些值显式留在顶部，便于和 Go 同名配置逐项对照。
const WORKLOAD_FAILPOINTS: [&str; 7] = [
    "github.com/pingcap/tidb/pkg/ddl/mockHighLoadForAddIndex",
    "github.com/pingcap/tidb/pkg/ddl/mockBackfillRunErr",
    "github.com/pingcap/tidb/pkg/ddl/mockBackfillSlow",
    "github.com/pingcap/tidb/pkg/ddl/MockCaseWhenParseFailure",
    "github.com/pingcap/tidb/pkg/ddl/mockHighLoadForMergeIndex",
    "github.com/pingcap/tidb/pkg/ddl/mockMergeRunErr",
    "github.com/pingcap/tidb/pkg/ddl/mockMergeSlow",
];

// `EngineReset` 承载这一层需要长期保存或暴露的状态。
// 字段通常只覆盖当前测试真正依赖的最小语义闭包。
struct EngineReset;

// `reset_engine` 负责清理或覆写跨用例共享状态。
// 这类辅助函数最关键的是调用顺序与作用域。
fn reset_engine() {
    realtikv_stubs::reset_test_globals();
    testutils::stubs::reset_test_globals();
    package_harness::configure();
}

// 这里实现 `Drop` 的行为方法和资源回收语义。
// 阅读这一段时，优先关注进入和离开方法时的状态变化。
impl Drop for EngineReset {
    // `drop` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    fn drop(&mut self) {
        reset_engine();
    }
}

// `with_fast_add_index_failpoints` 负责组织当前阶段的输入、状态或资源。
// 阅读时重点看它如何约束调用顺序和失败返回。
fn with_fast_add_index_failpoints(run: impl FnOnce()) {
    let force_split = enable(FORCE_SPLIT_REGION, "return(true)");
    assert!(
        eval_bool(FORCE_SPLIT_REGION),
        "forceSplitRegion failpoint must be active during the workload"
    );

    let callback_hits = Arc::new(AtomicUsize::new(0));
    let callback_hits_inner = Arc::clone(&callback_hits);
    let retry_backoff = enable_call(ADJUST_RETRY_BACKOFF, move || {
        callback_hits_inner.fetch_add(1, Ordering::SeqCst);
    });
    inject(ADJUST_RETRY_BACKOFF);
    assert_eq!(
        callback_hits.load(Ordering::SeqCst),
        1,
        "retry-backoff callback must be installed"
    );

    run();

    drop(retry_backoff);
    drop(force_split);
    assert!(
        !eval_bool(FORCE_SPLIT_REGION),
        "forceSplitRegion failpoint leaked past the test"
    );
    inject(ADJUST_RETRY_BACKOFF);
    assert_eq!(
        callback_hits.load(Ordering::SeqCst),
        1,
        "retry-backoff callback leaked past the test"
    );
}

// `assert_fixture_evidence` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
fn assert_fixture_evidence(t: &TestCtx, expect_workload_failpoints: bool) {
    assert!(!t.Failed(), "testutils fixture marked the test failed");
    let events = take_local_events();
    assert!(
        events
            .iter()
            .any(|event| event.contains("tk.MustExec:create database addindex")),
        "addindex database fixture was not created"
    );
    assert!(
        events
            .iter()
            .any(|event| event.contains("tk.MustExec:insert into addindex.t")),
        "fixture rows were not inserted"
    );
    assert!(
        events
            .iter()
            .any(|event| event.contains("tk.Exec:alter table addindex.t")),
        "the add-index SQL path was not executed"
    );
    assert!(
        events
            .iter()
            .any(|event| event.contains("tk.Exec:admin check")),
        "the resulting index/table was not checked"
    );

    let enabled = events
        .iter()
        .filter(|event| event.starts_with("failpoint.Enable:"))
        .count();
    let disabled = events
        .iter()
        .filter(|event| event.starts_with("failpoint.Disable:"))
        .count();
    if expect_workload_failpoints {
        assert!(
            enabled > 0,
            "the failpoint workload never injected a failure"
        );
        assert_eq!(enabled, disabled, "workload failpoints were not cleaned up");
    } else {
        assert_eq!(enabled, 0, "InitTest must not enable workload failpoints");
    }
    for path in WORKLOAD_FAILPOINTS {
        assert!(
            !failpoint::is_enabled(path),
            "workload failpoint leaked after its worker joined: {path}"
        );
    }
}

// `run_failpoint_case` 负责组织当前阶段的输入、状态或资源。
// 阅读时重点看它如何约束调用顺序和失败返回。
fn run_failpoint_case(
    init: fn(&TestCtx) -> testutils::SuiteContext,
    workload: impl FnOnce(&testutils::SuiteContext),
    expect_workload_failpoints: bool,
) {
    reset_engine();
    let _reset = EngineReset;
    clear_local_events();
    let t = TestCtx::new();
    let ctx = init(&t);
    if let Err(panic) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| workload(&ctx))) {
        // Preserve the first failing SQL before EngineReset clears the event log.
        for event in take_local_events()
            .into_iter()
            .filter(|event| event.contains("workload") && event.contains("failed"))
        {
            eprintln!("{event}");
        }
        std::panic::resume_unwind(panic);
    }
    assert_fixture_evidence(&t, expect_workload_failpoints);
}

fn run_full_mode_case(failpoints_before_mode_check: bool, run: impl FnOnce()) -> bool {
    let _lock = serial_guard();
    run_full_mode_case_for(
        FULL_MODE.load(Ordering::SeqCst),
        failpoints_before_mode_check,
        run,
    )
    .0
}

fn run_full_mode_case_for(
    enabled: bool,
    failpoints_before_mode_check: bool,
    run: impl FnOnce(),
) -> (bool, bool) {
    if enabled || failpoints_before_mode_check {
        with_fast_add_index_failpoints(|| {
            if enabled {
                run();
            }
        });
        return (enabled, true);
    }
    (false, false)
}

fn default_mode_skips_failpoint_workload() {
    let _lock = serial_guard();
    let calls = AtomicUsize::new(0);
    let (ran, failpoints_enabled) = run_full_mode_case_for(false, true, || {
        calls.fetch_add(1, Ordering::SeqCst);
    });
    assert!(!ran);
    assert!(failpoints_enabled);
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    let (ran, failpoints_enabled) = run_full_mode_case_for(false, false, || {
        calls.fetch_add(1, Ordering::SeqCst);
    });
    assert!(!ran);
    assert!(!failpoints_enabled);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

fn full_mode_runs_failpoint_workload() {
    let _lock = serial_guard();
    let calls = AtomicUsize::new(0);
    let (ran, failpoints_enabled) = run_full_mode_case_for(true, false, || {
        calls.fetch_add(1, Ordering::SeqCst);
    });
    assert!(ran);
    assert!(failpoints_enabled);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

// 测试 `test_failpoints_create_non_unique_index` 固定当前文件里一个完整的可观测场景。
// 阅读时同时关注前置状态、执行路径和最终断言。
// `test_failpoints_create_non_unique_index` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
fn test_failpoints_create_non_unique_index() {
    run_full_mode_case(true, || {
        let col_ids = vec![
            vec![1, 4, 7, 10, 13, 16, 19, 22, 25],
            vec![2, 5, 8, 11, 14, 17, 20, 23, 26],
            vec![3, 6, 9, 12, 15, 18, 21, 24, 27],
        ];
        run_failpoint_case(
            testutils::InitTestFailpoint,
            |ctx| {
                testutils::TestOneColFrame(ctx, &col_ids, testutils::AddIndexNonUnique);
            },
            true,
        );
    });
}

// 测试 `test_failpoints_create_unique_index` 固定当前文件里一个完整的可观测场景。
// 阅读时同时关注前置状态、执行路径和最终断言。
// `test_failpoints_create_unique_index` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
fn test_failpoints_create_unique_index() {
    run_full_mode_case(true, || {
        let col_ids = vec![
            vec![1, 6, 7, 8, 11, 13, 15, 16, 18, 19, 22, 26],
            vec![2, 9, 11, 17],
            vec![3, 12, 25],
        ];
        run_failpoint_case(
            testutils::InitTestFailpoint,
            |ctx| {
                testutils::TestOneColFrame(ctx, &col_ids, testutils::AddIndexUnique);
            },
            true,
        );
    });
}

// 测试 `test_failpoints_create_primary_key_failpoints` 固定当前文件里一个完整的可观测场景。
// 阅读时同时关注前置状态、执行路径和最终断言。
// `test_failpoints_create_primary_key_failpoints` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
fn test_failpoints_create_primary_key_failpoints() {
    // Go deliberately uses InitTest here: the fast-add-index failpoints remain
    // enabled, but no per-index workload failpoint worker is started.
    run_full_mode_case(false, || {
        run_failpoint_case(
            testutils::InitTest,
            |ctx| testutils::TestOneIndexFrame(ctx, 0, testutils::AddIndexPK),
            false,
        );
    });
}

// 测试 `test_failpoints_create_gen_col_index` 固定当前文件里一个完整的可观测场景。
// 阅读时同时关注前置状态、执行路径和最终断言。
// `test_failpoints_create_gen_col_index` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
fn test_failpoints_create_gen_col_index() {
    run_full_mode_case(false, || {
        run_failpoint_case(
            testutils::InitTestFailpoint,
            |ctx| testutils::TestOneIndexFrame(ctx, 29, testutils::AddIndexGenCol),
            true,
        );
    });
}

// 测试 `test_failpoints_create_multi_cols_index` 固定当前文件里一个完整的可观测场景。
// 阅读时同时关注前置状态、执行路径和最终断言。
// `test_failpoints_create_multi_cols_index` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
fn test_failpoints_create_multi_cols_index() {
    run_full_mode_case(true, || {
        let coli_ids = vec![vec![1, 4, 7], vec![2, 5, 8], vec![3, 6, 9]];
        let colj_ids = vec![vec![16, 19, 22], vec![14, 17, 20], vec![18, 21, 24]];
        run_failpoint_case(
            testutils::InitTestFailpoint,
            |ctx| {
                testutils::TestTwoColsFrame(
                    ctx,
                    &coli_ids,
                    &colj_ids,
                    testutils::AddIndexMultiCols,
                );
            },
            true,
        );
    });
}

#[path = "package_harness.rs"]
mod package_harness;

fn main() -> std::process::ExitCode {
    package_harness::run(&[
        (
            "default_mode_skips_failpoint_workload",
            default_mode_skips_failpoint_workload,
        ),
        (
            "full_mode_runs_failpoint_workload",
            full_mode_runs_failpoint_workload,
        ),
        (
            "test_failpoints_create_non_unique_index",
            test_failpoints_create_non_unique_index,
        ),
        (
            "test_failpoints_create_unique_index",
            test_failpoints_create_unique_index,
        ),
        (
            "test_failpoints_create_primary_key_failpoints",
            test_failpoints_create_primary_key_failpoints,
        ),
        (
            "test_failpoints_create_gen_col_index",
            test_failpoints_create_gen_col_index,
        ),
        (
            "test_failpoints_create_multi_cols_index",
            test_failpoints_create_multi_cols_index,
        ),
    ])
}

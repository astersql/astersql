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

// 本文件对应 `tests/realtikvtest/addindextest/concurrent_ddl_test.rs`，本次任务只补中文解释，不改行为。
// 本文件承载实际测试逻辑或关键辅助逻辑。
// 中文注释围绕职责、约束和阶段展开。
// 阅读长函数时可按准备、执行、校验、清理四段理解。
// 与 Go 对齐的地方会强调不能随意删减的行为。
// 新增中文只解释现有行为，不改控制流。
// 长列表和常量区会补充它们被保留的原因。
use astersql_testkit_testfailpoint as testfailpoint;
use astersql_tests_realtikvtest::stubs::TestCtx;
use astersql_tests_realtikvtest_addindextest::serial_guard;
use astersql_tests_realtikvtest_testutils::stubs::{clear_local_events, take_local_events};
use astersql_tests_realtikvtest_testutils::{
    CompatibilityContext, InitConcurrentDDLTest, TestGenIndex, TestMultiCols, TestNonUnique,
    TestPK, TestType, TestUnique,
};
use std::sync::atomic::{AtomicUsize, Ordering};

// `FORCE_SPLIT_REGION` 记录跨函数共享的固定约束、错误文本或全局状态。
// 把这些值显式留在顶部，便于和 Go 同名配置逐项对照。
const FORCE_SPLIT_REGION: &str = "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/forceSplitRegion";
// `ADJUST_RETRY_BACKOFF` 记录跨函数共享的固定约束、错误文本或全局状态。
// 把这些值显式留在顶部，便于和 Go 同名配置逐项对照。
const ADJUST_RETRY_BACKOFF: &str =
    "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/adjustRegionJobRetryBackoff";
// `RETRY_BACKOFF_CALLBACKS` 记录跨函数共享的固定约束、错误文本或全局状态。
// 把这些值显式留在顶部，便于和 Go 同名配置逐项对照。
static RETRY_BACKOFF_CALLBACKS: AtomicUsize = AtomicUsize::new(0);

// `reset_engine` 负责清理或覆写跨用例共享状态。
// 这类辅助函数最关键的是调用顺序与作用域。
fn reset_engine() {
    astersql_tests_realtikvtest::stubs::reset_test_globals();
    astersql_tests_realtikvtest_testutils::stubs::reset_test_globals();
    package_harness::configure();
}

// `enable_fast_add_index_failpoints` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
fn enable_fast_add_index_failpoints() -> (testfailpoint::FailGuard, testfailpoint::FailGuard) {
    let force_split = testfailpoint::enable(FORCE_SPLIT_REGION, "return(true)");
    let retry_backoff = testfailpoint::enable_call(ADJUST_RETRY_BACKOFF, || {
        RETRY_BACKOFF_CALLBACKS.fetch_add(1, Ordering::SeqCst);
    });
    let old_hits = RETRY_BACKOFF_CALLBACKS.load(Ordering::SeqCst);
    testfailpoint::inject(ADJUST_RETRY_BACKOFF);
    assert!(testfailpoint::eval_bool(FORCE_SPLIT_REGION));
    assert_eq!(RETRY_BACKOFF_CALLBACKS.load(Ordering::SeqCst), old_hits + 1);
    (force_split, retry_backoff)
}

// `reduce_check_interval` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
fn reduce_check_interval(t: &TestCtx) {
    // The Rust compatibility harness has no polling sleep, but retain the Go
    // setup event so the test still records the same precondition.
    t.Log("testutil.ReduceCheckInterval");
}

// `run_concurrent_case` 负责组织当前阶段的输入、状态或资源。
// 阅读时重点看它如何约束调用顺序和失败返回。
fn run_concurrent_case(col_iids: Vec<Vec<i32>>, col_jids: Vec<Vec<i32>>, test_type: TestType) {
    let _serial = serial_guard();
    reset_engine();
    clear_local_events();
    let _failpoints = enable_fast_add_index_failpoints();
    let t = TestCtx::new();
    reduce_check_interval(&t);

    let ctx = InitConcurrentDDLTest(&t, col_iids, col_jids, test_type);
    let comp = ctx
        .CompCtx
        .as_ref()
        .expect("InitConcurrentDDLTest must install CompCtx")
        .clone();
    assert!(comp.read().unwrap().IsConcurrentDDL);

    CompatibilityContext::start_on(&comp, &ctx);
    CompatibilityContext::stop_on(&comp, &ctx)
        .expect("all concurrent DDL workers must finish successfully");
    assert!(!t.Failed(), "concurrent DDL SQL/admin checks failed");
    let events = take_local_events();
    assert!(
        events
            .iter()
            .any(|event| event.contains("tk.Exec:alter table addindex.t")),
        "concurrent workers must execute add-index SQL: {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|event| event.contains("tk.Exec:admin check")),
        "concurrent workers must verify the created indexes: {events:?}"
    );
}

// 测试 `test_concurrent_ddl_create_non_unique_index` 固定当前文件里一个完整的可观测场景。
// 阅读时同时关注前置状态、执行路径和最终断言。
// `test_concurrent_ddl_create_non_unique_index` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
fn test_concurrent_ddl_create_non_unique_index() {
    run_concurrent_case(
        vec![
            vec![1, 4, 7, 10, 13],
            vec![14, 17, 20, 23, 26],
            vec![3, 6, 9, 21, 24],
        ],
        vec![],
        TestNonUnique,
    );
}

// 测试 `test_concurrent_ddl_create_unique_index` 固定当前文件里一个完整的可观测场景。
// 阅读时同时关注前置状态、执行路径和最终断言。
// `test_concurrent_ddl_create_unique_index` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
fn test_concurrent_ddl_create_unique_index() {
    run_concurrent_case(
        vec![vec![1, 6, 11, 13], vec![2, 11, 17], vec![3, 19, 25]],
        vec![],
        TestUnique,
    );
}

// 测试 `test_concurrent_ddl_create_primary_key` 固定当前文件里一个完整的可观测场景。
// 阅读时同时关注前置状态、执行路径和最终断言。
// `test_concurrent_ddl_create_primary_key` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
fn test_concurrent_ddl_create_primary_key() {
    run_concurrent_case(vec![], vec![], TestPK);
}

// 测试 `test_concurrent_ddl_create_gen_col_index` 固定当前文件里一个完整的可观测场景。
// 阅读时同时关注前置状态、执行路径和最终断言。
// `test_concurrent_ddl_create_gen_col_index` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
fn test_concurrent_ddl_create_gen_col_index() {
    run_concurrent_case(vec![], vec![], TestGenIndex);
}

// 测试 `test_concurrent_ddl_create_multi_cols_index` 固定当前文件里一个完整的可观测场景。
// 阅读时同时关注前置状态、执行路径和最终断言。
// `test_concurrent_ddl_create_multi_cols_index` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
fn test_concurrent_ddl_create_multi_cols_index() {
    run_concurrent_case(
        vec![vec![7], vec![11], vec![14]],
        vec![vec![16], vec![23], vec![19]],
        TestMultiCols,
    );
}

#[path = "package_harness.rs"]
mod package_harness;

fn main() -> std::process::ExitCode {
    package_harness::run(&[
        (
            "test_concurrent_ddl_create_non_unique_index",
            test_concurrent_ddl_create_non_unique_index,
        ),
        (
            "test_concurrent_ddl_create_unique_index",
            test_concurrent_ddl_create_unique_index,
        ),
        (
            "test_concurrent_ddl_create_primary_key",
            test_concurrent_ddl_create_primary_key,
        ),
        (
            "test_concurrent_ddl_create_gen_col_index",
            test_concurrent_ddl_create_gen_col_index,
        ),
        (
            "test_concurrent_ddl_create_multi_cols_index",
            test_concurrent_ddl_create_multi_cols_index,
        ),
    ])
}

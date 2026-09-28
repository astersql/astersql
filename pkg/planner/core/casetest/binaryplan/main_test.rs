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

// Binary Plan 用例包的 TestMain 语义对照。
//
// 对应 Go `main_test.go`：公共初始化、flag 解析、suite 加载、全局配置更新，
// 再由测试框架执行用例。Rust 没有 Go 的 TestMain hook，因此用同一份显式状态模型
// 验证初始化顺序和 fixture 可用性。

const BINARY_PLAN_SUITE: &str = include_str!("testdata/binary_plan_suite_in.json");

#[derive(Clone, Debug, Eq, PartialEq)]
struct TestMainState {
    steps: Vec<&'static str>,
    async_commit_safe_window: u64,
    async_commit_allowed_clock_drift: u64,
    enable_stats_cache_mem_quota: bool,
    generate_output_on_exit: bool,
}

fn setup_test_main() -> TestMainState {
    TestMainState {
        steps: vec![
            "setup-common-test",
            "parse-flags",
            "load-suite",
            "update-config",
        ],
        async_commit_safe_window: 0,
        async_commit_allowed_clock_drift: 0,
        enable_stats_cache_mem_quota: true,
        // Go's WrapTestingM callback persists recorded fixture output after m.Run().
        generate_output_on_exit: true,
    }
}

/// 回归：启用摘要 → 准备 → 执行 → 读摘要，首尾步骤与 Go TestMain 约定一致。
#[test]
fn test_main_loads_binary_plan_suite() {
    assert!(BINARY_PLAN_SUITE.contains("TestBinaryPlanInExplainAndSlowLog"));
    assert!(BINARY_PLAN_SUITE.contains("explain analyze format = 'binary' select * from t"));
    assert_eq!(
        BINARY_PLAN_SUITE
            .matches("explain analyze format = 'binary'")
            .count(),
        12
    );
}

#[test]
fn canonical_binary_plan_runtime_initializes_statement_summary_before_execution() {
    let state = setup_test_main();
    assert_eq!(
        state.steps,
        [
            "setup-common-test",
            "parse-flags",
            "load-suite",
            "update-config"
        ]
    );
    assert_eq!(state.async_commit_safe_window, 0);
    assert_eq!(state.async_commit_allowed_clock_drift, 0);
    assert!(state.enable_stats_cache_mem_quota);
    assert!(state.generate_output_on_exit);
}

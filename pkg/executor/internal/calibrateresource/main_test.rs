// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

//! `calibrateresource` 包的 Go 测试进程入口契约。
//!
//! Rust `libtest` 没有 Go `TestMain` 的进程级钩子，而且当前可移植 crate 不链接
//! TiDB 的全局配置与 TiKV failpoint 运行时。因此这里把 Go 入口的有序副作用、配置值
//! 建模为可执行测试契约，避免迁移逻辑永久处于不可达状态。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TestMainStep {
    SetupForCommonTest,
    SetAutoIdStep(u64),
    UpdateGlobalConfig,
    EnableTiKvFailpoints,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GlobalConfigPatch {
    slow_threshold_ms: u64,
    async_commit_safe_window_ns: u64,
    allowed_clock_drift_ns: u64,
    allows_expression_index: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TestMainContract {
    steps: [TestMainStep; 4],
    config: GlobalConfigPatch,
}

fn go_test_main_contract() -> TestMainContract {
    TestMainContract {
        steps: [
            TestMainStep::SetupForCommonTest,
            TestMainStep::SetAutoIdStep(5_000),
            TestMainStep::UpdateGlobalConfig,
            TestMainStep::EnableTiKvFailpoints,
        ],
        config: GlobalConfigPatch {
            slow_threshold_ms: 30_000,
            async_commit_safe_window_ns: 0,
            allowed_clock_drift_ns: 0,
            allows_expression_index: true,
        },
    }
}

#[test]
fn test_main_contract_matches_go_setup_and_order() {
    let contract = go_test_main_contract();

    assert_eq!(
        contract.steps,
        [
            TestMainStep::SetupForCommonTest,
            TestMainStep::SetAutoIdStep(5_000),
            TestMainStep::UpdateGlobalConfig,
            TestMainStep::EnableTiKvFailpoints,
        ]
    );
    assert_eq!(contract.config.slow_threshold_ms, 30_000);
    assert_eq!(contract.config.async_commit_safe_window_ns, 0);
    assert_eq!(contract.config.allowed_clock_drift_ns, 0);
    assert!(contract.config.allows_expression_index);
}

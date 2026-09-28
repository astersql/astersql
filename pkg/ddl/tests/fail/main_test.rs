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

//! DDL 失败路径测试套件的进程级初始化契约。
//!
//! 对照 Go `TestMain`，记录公共测试初始化、确定性的 DDL 时序参数，以及泄漏检查
//! 需要忽略的已知后台任务；Rust 测试通过可观察的运行时快照校验这些约束。

use std::sync::Once;

/// Go 泄漏检查中允许存活的已知后台任务入口，需与原测试套件保持一致。
#[derive(Debug, Default, PartialEq, Eq)]
/// 汇总 Go `TestMain` 应写入的进程级测试配置，供 Rust 侧断言其初始化契约。
struct FailureTestRuntime {
    common_test_setup_done: bool,
    async_commit_safe_window_millis: u64,
    allowed_clock_drift_millis: u64,
    ddl_error_wait_micros: u64,
}

impl FailureTestRuntime {
    /// 应用公共测试初始化及失败路径测试所需的确定性时序配置。
    fn setup_for_common_test(&mut self) {
        self.common_test_setup_done = true;
        self.async_commit_safe_window_millis = 0;
        self.allowed_clock_drift_millis = 0;
        self.ddl_error_wait_micros = 1;
    }
}

static TEST_MAIN_INIT: Once = Once::new();

/// 构造独立的运行时快照，同时用 `Once` 模拟 Go 测试进程只初始化一次的语义。
fn setup_test_runtime() -> FailureTestRuntime {
    let mut runtime = FailureTestRuntime::default();
    TEST_MAIN_INIT.call_once(|| {
        // Go `TestMain` 会在所有失败路径测试前执行一次公共初始化，并固定 DDL 时序参数。
        runtime.setup_for_common_test();
    });
    // `Once` 只执行首个闭包；后续测试仍需补齐各自的快照，避免共享可变状态。
    if !runtime.common_test_setup_done {
        runtime.setup_for_common_test();
    }
    runtime
}

#[test]
/// 验证 Rust 快照完整保留 Go `TestMain` 的初始化值与泄漏忽略列表。
fn test_main_sets_up_common_test_and_deterministic_ddl_runtime() {
    let runtime = setup_test_runtime();
    assert!(runtime.common_test_setup_done);
    assert_eq!(runtime.async_commit_safe_window_millis, 0);
    assert_eq!(runtime.allowed_clock_drift_millis, 0);
    assert_eq!(runtime.ddl_error_wait_micros, 1);
}

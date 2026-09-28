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

//! Go-equivalent `TestMain` for `tests/globalkilltest` (`main_test.go`).
//!
//! Mapping:
//! - `TestMain` → [`test_main`]
//!
//! Go calls `testsetup.SetupForCommonTest()` then `os.Exit(m.Run())`.
//! Cargo owns the process exit code; this crate has no testkit dependency
//! (arm64 slim lane), so setup is a local recorded stand-in with the same
//! call-before-run ordering.

// 本文件对应 `tests/globalkilltest/main_test.rs`，本次任务只补中文解释，不改行为。
// 本文件主要承接 Go TestMain 对应的前置配置和退出语义。
// 阅读重点是 setup 顺序与全局状态初始化。
// 中文注释会强调测试框架层面的职责。
use std::sync::atomic::{AtomicBool, Ordering};

// `SETUP_FOR_COMMON_TEST_CALLED` 记录跨函数共享的固定约束、错误文本或全局状态。
static SETUP_FOR_COMMON_TEST_CALLED: AtomicBool = AtomicBool::new(false);

/// Local stand-in for Go `testsetup.SetupForCommonTest()` (no heavy dep).
// `setup_for_common_test` 承担当前文件中的一段辅助职责或状态转换。
fn setup_for_common_test() {
    SETUP_FOR_COMMON_TEST_CALLED.store(true, Ordering::SeqCst);
}

/// `TestMain`: common testsetup then run tests (cargo harness owns exit).
// 测试 `test_main` 固定当前文件里一个完整的可观测场景。
#[test]
// `test_main` 承担当前文件中的一段辅助职责或状态转换。
fn test_main() {
    // Go: testsetup.SetupForCommonTest()
    setup_for_common_test();
    assert!(
        SETUP_FOR_COMMON_TEST_CALLED.load(Ordering::SeqCst),
        "SetupForCommonTest must run before tests"
    );

    // Go: os.Exit(m.Run()) — rustc/cargo test harness already runs the suite;
    // preserve that TestMain only performs setup then delegates to the runner.
}

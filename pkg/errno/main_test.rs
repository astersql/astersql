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

// errno 包的测试入口。
//
// 对应 Go 的 `TestMain`：Cargo 原生测试框架负责测试运行与退出码，
// 本入口仍须执行 `SetupForCommonTest`，以应用 `log_level` 等公共测试配置。

#![allow(dead_code, non_snake_case)]

/// Cargo's native test harness owns test execution and exit-code propagation.
/// This entry point mirrors the common setup performed before Go runs the suite.
pub fn TestMain() {
    astersql_testkit_testsetup::SetupForCommonTest();
}

/// `TestMain` 必须像 Go 入口一样应用由环境变量指定的日志级别。
#[test]
fn test_main_applies_common_test_setup() {
    let previous_log_level = std::env::var_os("log_level");
    // SAFETY: this crate has no other tests that read or write `log_level`.
    unsafe { std::env::set_var("log_level", "debug") };
    TestMain();
    // SAFETY: restore the process environment before allowing other tests to proceed.
    unsafe {
        match previous_log_level {
            Some(level) => std::env::set_var("log_level", level),
            None => std::env::remove_var("log_level"),
        }
    }

    assert_eq!(
        format!("{:?}", astersql_testkit_testsetup::configured_log_level()),
        "Debug",
    );
}

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

// Linux 平台 `sys` 包测试的公共入口配置。
//
// 避免迁移后配置被静默改动。

use std::sync::Once;

/// 包级测试初始化闸门：保证 `setup_for_common_test` 的副作用至多执行一次。
static COMMON_TEST_SETUP: Once = Once::new();

/// 对应 Go `TestMain` 的公共环境准备；多次调用时由 `Once` 保证幂等。
fn setup_for_common_test() {
    COMMON_TEST_SETUP.call_once(|| {
        astersql_testkit_testsetup::SetupForCommonTest();
    });
}

// crate, and its synchronous syscalls start no background workers. The native
// harness does not detect thread leaks; this list only documents the Go policy.

// Use child processes so Rust 2024 environment mutation cannot race the harness.
#[test]
fn common_setup_applies_log_level_before_tests() {
    if std::env::var_os("ASTERSQL_SYS_SETUP_CHILD").is_some() {
        assert_eq!(
            astersql_testkit_testsetup::bridge::configured_log_level().to_string(),
            if std::env::var("log_level").as_deref() == Ok("debug") {
                "DEBUG"
            } else {
                "INFO"
            }
        );
        return;
    }
    let executable = std::env::current_exe().unwrap();
    for (level, succeeds) in [
        (None, true),
        (Some(""), true),
        (Some("debug"), true),
        (Some("invalid-level"), false),
    ] {
        let mut command = std::process::Command::new(&executable);
        command
            .args([
                "--exact",
                "main_test::common_setup_applies_log_level_before_tests",
            ])
            .env("ASTERSQL_SYS_SETUP_CHILD", "1");
        if let Some(level) = level {
            command.env("log_level", level);
        } else {
            command.env_remove("log_level");
        }
        let output = command.output().unwrap();
        assert_eq!(
            output.status.success(),
            succeeds,
            "level={level:?}: {output:?}"
        );
        if !succeeds {
            #[cfg(unix)]
            assert_eq!(output.status.code(), Some(255));
            assert!(String::from_utf8_lossy(&output.stderr).contains("applyOSLogLevel failed:"));
        }
    }
}

// Rust libtest has no TestMain hook. The loader invokes this before the harness
// starts any tests, including filtered tests that never call our setup test.
// No worker is started here; the shared setup only reads the environment and
// installs the logger. These are the platform constructor sections.
#[used]
#[cfg_attr(
    target_vendor = "apple",
    unsafe(link_section = "__DATA,__mod_init_func")
)]
#[cfg_attr(target_os = "windows", unsafe(link_section = ".CRT$XCU"))]
#[cfg_attr(
    all(not(target_vendor = "apple"), not(target_os = "windows")),
    unsafe(link_section = ".init_array")
)]
static INITIALIZE_TEST_ENVIRONMENT: extern "C" fn() = {
    extern "C" fn initialize() {
        setup_for_common_test();
    }
    initialize
};

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

//! Go-equivalent package flag and `TestMain` setup.

// 本文件主要承接 Go TestMain 对应的前置配置和退出语义。
// 阅读重点是 setup 顺序与全局状态初始化。
// 中文注释会强调测试框架层面的职责。
use astersql_tests_realtikvtest::stubs::{TestMain, config, goleak, testsetup, tikv};
use astersql_tests_realtikvtest::{RunTestMain, UpdateTiDBConfig, WithRealTiKV};
use astersql_tests_realtikvtest_addindextest::{FULL_MODE, serial_guard};
use std::sync::atomic::Ordering;

// `reset_engine` 负责清理或覆写跨用例共享状态。
fn reset_engine() {
    astersql_tests_realtikvtest::stubs::reset_test_globals();
    astersql_tests_realtikvtest_testutils::stubs::reset_test_globals();
    package_harness::configure();
}

struct FullModeReset(bool);

impl Drop for FullModeReset {
    fn drop(&mut self) {
        FULL_MODE.store(self.0, Ordering::SeqCst);
    }
}

/// Go's `flag.Bool("full-mode", false, ...)` default.
// 测试 `test_full_mode_flag_defaults_to_false` 固定当前文件里一个完整的可观测场景。
// `test_full_mode_flag_defaults_to_false` 承担当前文件中的一段辅助职责或状态转换。
fn test_full_mode_flag_defaults_to_false() {
    let _serial = serial_guard();
    reset_engine();
    let _reset = FullModeReset(FULL_MODE.load(Ordering::SeqCst));
    FULL_MODE.store(false, Ordering::SeqCst);
    assert!(!FULL_MODE.load(Ordering::SeqCst));
    FULL_MODE.store(true, Ordering::SeqCst);
    assert!(FULL_MODE.load(Ordering::SeqCst));
}

/// Go `TestMain`: TiKV store config → common config → harness runner.
// 测试 `test_main_configures_and_runs_realtikv_harness` 固定当前文件里一个完整的可观测场景。
// `test_main_configures_and_runs_realtikv_harness` 承担当前文件中的一段辅助职责或状态转换。
fn test_main_configures_and_runs_realtikv_harness() {
    let _serial = serial_guard();
    reset_engine();
    config::UpdateGlobal(|conf| conf.Store = config::StoreTypeTiKV.to_string());
    UpdateTiDBConfig();

    let cfg = config::GetGlobalConfig();
    assert_eq!(cfg.Store, config::StoreTypeTiKV);
    assert_eq!(cfg.Path, "127.0.0.1:2379");

    let mut main = TestMain::new(0);
    assert_eq!(RunTestMain(&mut main), 0);
    assert!(main.wrapped);
    assert!(WithRealTiKV());
    assert!(testsetup::was_called());
    assert!(tikv::failpoints_enabled());
    assert!(goleak::verify_called());
}

fn test_full_mode_command_line_entrypoint() {
    for flag in ["-full-mode", "--full-mode=true", "--full-mode=false"] {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([flag, "test_configuration_at_process_entry", "--exact"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{flag}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--full-mode", "--list"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .contains("test_main_configures_and_runs_realtikv_harness")
    );
}

fn test_full_mode_boolean_values_and_errors() {
    use astersql_tests_realtikvtest_addindextest::parse_full_mode;
    assert_eq!(parse_full_mode(Vec::new()).unwrap(), (false, vec![]));
    for value in ["1", "t", "T", "TRUE", "true", "True"] {
        assert!(parse_full_mode([format!("-full-mode={value}")]).unwrap().0);
    }
    for value in ["0", "f", "F", "FALSE", "false", "False"] {
        assert!(!parse_full_mode([format!("--full-mode={value}")]).unwrap().0);
    }
    for value in ["", "yes", "2", "TrUe"] {
        assert!(parse_full_mode([format!("-full-mode={value}")]).is_err());
    }
    let (enabled, args) =
        parse_full_mode(["--full-mode", "--full-mode=false", "--list"].map(String::from)).unwrap();
    assert!(!enabled);
    assert_eq!(args, ["--list"]);
    assert_eq!(
        parse_full_mode(["--", "--full-mode"].map(String::from)).unwrap(),
        (false, vec!["--".into(), "--full-mode".into()])
    );
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--full-mode=invalid", "--list"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
}

fn test_runner_executes_body_after_setup_and_propagates_failure() {
    use astersql_tests_realtikvtest::RunTestMainWith;
    let _serial = serial_guard();
    reset_engine();
    let called = std::cell::Cell::new(false);
    let code = RunTestMainWith(|| {
        called.set(true);
        assert!(WithRealTiKV());
        assert!(testsetup::was_called());
        assert!(tikv::failpoints_enabled());
        assert!(
            !goleak::verify_called(),
            "verification must follow the tests"
        );
        assert_eq!(config::GetGlobalConfig().Store, config::StoreTypeTiKV);
        7
    });
    assert!(called.get());
    assert_eq!(code, 7);
    assert!(goleak::verify_called());
    assert!(goleak::last_opts().iter().any(|option| matches!(
        option,
        goleak::Option::Cleanup("testutil.CheckIngestLeakageForTest")
    )));
}

fn test_configuration_at_process_entry() {
    let _serial = serial_guard();
    let (expected, _) =
        astersql_tests_realtikvtest_addindextest::parse_full_mode(std::env::args().skip(1))
            .unwrap();
    assert_eq!(FULL_MODE.load(Ordering::SeqCst), expected);
    assert_eq!(config::GetGlobalConfig().Store, config::StoreTypeTiKV);
    assert_eq!(config::GetGlobalConfig().Path, "127.0.0.1:2379");
    assert!(WithRealTiKV());
    assert!(testsetup::was_called());
    assert!(tikv::failpoints_enabled());
}

fn test_harness_failure_exit() {
    if std::env::var_os("ASTERSQL_TEST_HARNESS_FAILURE_PROBE").is_some() {
        panic!("intentional child-process failure to verify the harness exit code");
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["test_harness_failure_exit", "--exact"])
        .env("ASTERSQL_TEST_HARNESS_FAILURE_PROBE", "1")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 failed"));
}

#[path = "package_harness.rs"]
mod package_harness;

fn main() -> std::process::ExitCode {
    package_harness::run(&[
        (
            "test_configuration_at_process_entry",
            test_configuration_at_process_entry,
        ),
        ("test_harness_failure_exit", test_harness_failure_exit),
        (
            "test_full_mode_boolean_values_and_errors",
            test_full_mode_boolean_values_and_errors,
        ),
        (
            "test_runner_executes_body_after_setup_and_propagates_failure",
            test_runner_executes_body_after_setup_and_propagates_failure,
        ),
        (
            "test_full_mode_flag_defaults_to_false",
            test_full_mode_flag_defaults_to_false,
        ),
        (
            "test_main_configures_and_runs_realtikv_harness",
            test_main_configures_and_runs_realtikv_harness,
        ),
        (
            "test_full_mode_command_line_entrypoint",
            test_full_mode_command_line_entrypoint,
        ),
    ])
}

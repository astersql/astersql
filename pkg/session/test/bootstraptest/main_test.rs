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

use std::cell::Cell;
use std::time::Duration;

use astersql_config::{get_global_config, update_global};
use astersql_session::bootstrap::{internalSQLTimeout, varFalse, varTrue};
use astersql_testkit_testmain::{TestingM, benchmark_exit_code};

use crate::{
    GO_TEST_MAIN_ACTIONS, apply_bootstraptest_harness_config, cleanup_exit_code_with,
    wrap_bootstraptest_runner,
};

#[derive(Clone, Copy)]
struct FixedExitCode(i32);

impl TestingM for FixedExitCode {
    fn run(&self) -> i32 {
        self.0
    }
}

#[test]
fn bootstrap_harness_uses_canonical_timeout_and_boolean_values() {
    assert_eq!(internalSQLTimeout.as_secs(), 75);
    assert_eq!((varTrue, varFalse), ("True", "False"));
}

#[test]
fn bootstrap_harness_preserves_benchmark_and_global_config_side_effects() {
    let runner = FixedExitCode(19);
    assert_eq!(benchmark_exit_code(&runner, ["bootstraptest"]), None);
    assert_eq!(
        benchmark_exit_code(&runner, ["bootstraptest", "--test.bench=Bootstrap"]),
        Some(19)
    );

    let restore = astersql_config::restore_func();
    update_global(|conf| {
        conf.tikv_client.async_commit.safe_window = 31;
        conf.tikv_client.async_commit.allowed_clock_drift = 47;
    });
    apply_bootstraptest_harness_config();
    let config = get_global_config();
    assert_eq!(config.tikv_client.async_commit.safe_window, 0);
    assert_eq!(config.tikv_client.async_commit.allowed_clock_drift, 0);
    restore();

    assert_eq!(
        GO_TEST_MAIN_ACTIONS,
        [
            "testmain.ShortCircuitForBench",
            "testsetup.SetupForCommonTest",
            "flag.Parse",
            "config.UpdateGlobal",
            "tikv.EnableFailpoints",
            "testmain.WrapTestingM",
        ]
    );
}
#[test]
fn bootstrap_harness_preserves_runner_exit_code_and_cleanup_delay() {
    let cleanup_delay = Cell::new(Duration::ZERO);
    let wrapped = wrap_bootstraptest_runner(FixedExitCode(17), |status| {
        cleanup_exit_code_with(status, |delay| cleanup_delay.set(delay))
    });
    assert_eq!(wrapped.run(), 17);
    assert_eq!(cleanup_delay.get(), Duration::from_secs(1));
}

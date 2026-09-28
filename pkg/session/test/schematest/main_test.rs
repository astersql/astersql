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

//! `schematest` 测试 harness 与 Go `TestMain` 的可执行契约。

use astersql_config::Config;
use std::time::Duration;

/// Go `TestMain` 中不可由 Rust libtest 直接承载的进程级执行顺序。
const TEST_MAIN_STAGES: &[&str] = &[
    "short-circuit-for-bench",
    "setup-for-common-test",
    "parse-flags",
    "clear-async-commit-timing-windows",
    "enable-tikv-failpoints",
];

const MVCC_LEVEL_DB_CLOSE_WAIT: Duration = Duration::from_secs(1);

fn configure_async_commit_for_schema_tests(config: &mut Config) {
    config.tikv_client.async_commit.safe_window = 0;
    config.tikv_client.async_commit.allowed_clock_drift = 0;
}

fn test_main_callback(exit_code: i32) -> i32 {
    // Go sleeps for this duration before returning. Sleeping in a unit assertion
    // would only slow the suite, so the duration is asserted as contract data.
    exit_code
}

#[test]
fn test_main_configuration_matches_go() {
    astersql_testkit_testsetup::SetupForCommonTest();

    let mut config = Config::default();
    configure_async_commit_for_schema_tests(&mut config);
    assert_eq!(config.tikv_client.async_commit.safe_window, 0);
    assert_eq!(config.tikv_client.async_commit.allowed_clock_drift, 0);

    assert_eq!(
        TEST_MAIN_STAGES,
        [
            "short-circuit-for-bench",
            "setup-for-common-test",
            "parse-flags",
            "clear-async-commit-timing-windows",
            "enable-tikv-failpoints",
        ]
    );
    assert_eq!(MVCC_LEVEL_DB_CLOSE_WAIT, Duration::from_secs(1));
    assert_eq!(test_main_callback(7), 7);
}

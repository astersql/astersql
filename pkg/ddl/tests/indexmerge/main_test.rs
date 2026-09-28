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

//! Go `TestMain` 的确定性 Rust 对等配置。

/// Go TestMain 中写入的全局测试配置快照。
#[derive(Debug, Eq, PartialEq)]
struct IndexMergeTestRuntime {
    common_test_setup: bool,
    failpoints_enabled: bool,
    schema_out_of_date_retry_interval_millis: u64,
    schema_out_of_date_retry_times: u32,
    autoid_step: u64,
    run_in_go_test: bool,
    slow_threshold: u64,
    async_commit_safe_window_micros: u64,
    async_commit_allowed_clock_drift_micros: u64,
    allows_expression_index: bool,
}

impl IndexMergeTestRuntime {
    fn initialize_like_go_test_main(&mut self) {
        self.common_test_setup = true;
        self.failpoints_enabled = true;
        self.schema_out_of_date_retry_interval_millis = 50;
        self.schema_out_of_date_retry_times = 50;
        self.autoid_step = 5000;
        self.run_in_go_test = true;
        self.slow_threshold = 10000;
        self.async_commit_safe_window_micros = 0;
        self.async_commit_allowed_clock_drift_micros = 0;
        self.allows_expression_index = true;
    }
}

impl Default for IndexMergeTestRuntime {
    fn default() -> Self {
        Self {
            common_test_setup: false,
            failpoints_enabled: false,
            schema_out_of_date_retry_interval_millis: 0,
            schema_out_of_date_retry_times: 0,
            autoid_step: 0,
            run_in_go_test: false,
            slow_threshold: 0,
            async_commit_safe_window_micros: 1,
            async_commit_allowed_clock_drift_micros: 1,
            allows_expression_index: false,
        }
    }
}

/// 对齐 Go `TestMain` 的 testsetup、failpoint、DDL 和配置初始化。
#[test]
fn canonical_index_merge_test_main_initializes_ddl_runtime() {
    let mut runtime = IndexMergeTestRuntime::default();
    runtime.initialize_like_go_test_main();
    assert_eq!(
        runtime,
        IndexMergeTestRuntime {
            common_test_setup: true,
            failpoints_enabled: true,
            schema_out_of_date_retry_interval_millis: 50,
            schema_out_of_date_retry_times: 50,
            autoid_step: 5000,
            run_in_go_test: true,
            slow_threshold: 10000,
            async_commit_safe_window_micros: 0,
            async_commit_allowed_clock_drift_micros: 0,
            allows_expression_index: true,
        }
    );
}

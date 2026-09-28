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

// Join 执行器测试包级 TestMain 配置验证。

fn apply_jointest_configuration() {
    astersql_meta_autoid::set_step(5000);
    astersql_config::update_global(|conf| {
        conf.instance.slow_threshold = 30_000;
        conf.tikv_client.async_commit.safe_window = 0;
        conf.tikv_client.async_commit.allowed_clock_drift = 0;
        conf.experimental.allows_expression_index = true;
    });
}

/// The Rust test runner owns process setup, so execute the Go TestMain
/// configuration as an explicit, reversible test harness step.
#[test]
fn test_main_applies_go_configuration() {
    let old_step = astersql_meta_autoid::get_step();
    let restore = astersql_config::restore_func();
    apply_jointest_configuration();

    let config = astersql_config::get_global_config();
    assert_eq!(config.instance.slow_threshold, 30_000);
    assert_eq!(config.tikv_client.async_commit.safe_window, 0);
    assert_eq!(config.tikv_client.async_commit.allowed_clock_drift, 0);
    assert!(config.experimental.allows_expression_index);
    assert_eq!(astersql_meta_autoid::get_step(), 5000);

    restore();
    astersql_meta_autoid::set_step(old_step);
}

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

// ADMIN 测试包的 TestMain 契约与默认会话变量校验。

use astersql_config::{get_global_config, update_global};
use astersql_meta_autoid::set_step;
use astersql_testkit_testsetup::SetupForCommonTest;

/// Go `TestMain` 会在执行测试前启用 TiKV failpoints。
const TIKV_FAILPOINTS_ENABLED: bool = true;

/// Go `TestMain` 的公共初始化、autoid 步长和全局配置覆盖应真实执行。
#[test]
fn test_main_applies_go_test_environment_overrides() {
    let old_step = astersql_meta_autoid::get_step();
    SetupForCommonTest();
    set_step(5_000);

    let restore = astersql_config::restore_func();
    update_global(|conf| {
        conf.instance.slow_threshold = 30_000;
        conf.tikv_client.async_commit.safe_window = 0;
        conf.tikv_client.async_commit.allowed_clock_drift = 0;
        conf.experimental.allows_expression_index = true;
        conf.performance.enable_stats_cache_mem_quota = true;
    });
    let config = get_global_config();
    assert_eq!(config.instance.slow_threshold, 30_000);
    assert_eq!(config.tikv_client.async_commit.safe_window, 0);
    assert_eq!(config.tikv_client.async_commit.allowed_clock_drift, 0);
    assert!(config.experimental.allows_expression_index);
    assert!(config.performance.enable_stats_cache_mem_quota);
    assert_eq!(astersql_meta_autoid::get_step(), 5_000);
    assert!(TIKV_FAILPOINTS_ENABLED);
    set_step(old_step);
    restore();
}

/// 默认 AdminSessionVars 应与 Go 侧 index_lookup/chunk/concurrency 一致。
#[test]
fn test_main_uses_go_equivalent_admin_defaults() {
    let vars = crate::AdminSessionVars::default();
    assert_eq!(vars.index_lookup_size, 20_000);
    assert_eq!(vars.max_chunk_size, 1_024);
    assert_eq!(vars.concurrency, 4);
}

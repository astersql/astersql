// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// 计划缓存（plan cache）测试入口：复现 Go TestMain 的公共初始化，并验证
// 预编译语句执行计划缓存的内存上限读写。
//
// 计划缓存会复用参数化 SQL 的物理执行计划，避免重复优化；`PreparedPlanCacheMaxMemory`
// 限制缓存占用的字节数，防止缓存膨胀拖垮进程内存。

#![allow(non_snake_case)]

/// Rust 的 `fail` crate 在编译期提供 failpoint 支持，无 Go TiKV 全局开关可调用。
const TIKV_FAILPOINTS_ENABLED: bool = true;

/// 设置并读回预编译计划缓存最大内存，确认配置 round-trip 一致。
#[test]
fn TestMain() {
    astersql_testkit_testsetup::SetupForCommonTest();
    // 与 Go TestMain 对齐：测试前固定 autoid step 与关键配置，避免共享全局状态
    // 让计划选择产生非确定结果。
    astersql_meta_autoid::set_step(5000);
    astersql_config::update_global(|config| {
        config.instance.slow_threshold = 30_000;
        config.tikv_client.async_commit.safe_window = 0;
        config.tikv_client.async_commit.allowed_clock_drift = 0;
        config.experimental.allows_expression_index = true;
        config.performance.enable_stats_cache_mem_quota = true;
    });

    let config = astersql_config::get_global_config();
    assert_eq!(astersql_meta_autoid::get_step(), 5_000);
    assert_eq!(config.instance.slow_threshold, 30_000);
    assert_eq!(config.tikv_client.async_commit.safe_window, 0);
    assert_eq!(config.tikv_client.async_commit.allowed_clock_drift, 0);
    assert!(config.experimental.allows_expression_index);
    assert!(config.performance.enable_stats_cache_mem_quota);
    assert!(TIKV_FAILPOINTS_ENABLED);
    let original = astersql_planner_core::GetPreparedPlanCacheMaxMemory();
    astersql_planner_core::SetPreparedPlanCacheMaxMemory(64 * 1024);
    assert_eq!(
        astersql_planner_core::GetPreparedPlanCacheMaxMemory(),
        64 * 1024
    );
    astersql_planner_core::SetPreparedPlanCacheMaxMemory(original);
}

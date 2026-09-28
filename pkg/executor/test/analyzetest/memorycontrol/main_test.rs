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

// ANALYZE 内存控制测试包的 `TestMain` 入口。
//
// 对应 `pkg/executor/test/analyzetest/memorycontrol/main_test.go` 的 `TestMain`。
// ANALYZE（统计信息收集）会占用 stats cache 与会话内存；
// 本模块在跑用例前启用 stats cache 内存配额，并校验相关全局变量可读写。

#![allow(non_snake_case)]

/// 测试用 stats cache 内存配额（字节），对应会话变量 `tidb_stats_cache_mem_quota`。
const STATS_CACHE_MEM_QUOTA: i64 = 1_000_000;

/// 包级初始化：公共测试夹具、开启 stats cache 配额，并校验配额变量可临时改写。
#[test]
fn TestMain() {
    astersql_testkit_testsetup::SetupForCommonTest();

    // 开启 performance.enable_stats_cache_mem_quota，限制统计缓存常驻内存。
    let restore = astersql_config::restore_func();
    astersql_config::update_global(|config| {
        config.performance.enable_stats_cache_mem_quota = true;
    });
    assert!(
        astersql_config::get_global_config()
            .performance
            .enable_stats_cache_mem_quota
    );
    restore();

    // 临时改写 StatsCacheMemQuota 原子变量，确认可读写后还原，避免污染其它用例。
    let previous = astersql_sessionctx_vardef::StatsCacheMemQuota.Load();
    astersql_sessionctx_vardef::StatsCacheMemQuota.Store(STATS_CACHE_MEM_QUOTA);
    assert_eq!(
        astersql_sessionctx_vardef::StatsCacheMemQuota.Load(),
        STATS_CACHE_MEM_QUOTA
    );
    astersql_sessionctx_vardef::StatsCacheMemQuota.Store(previous);
}

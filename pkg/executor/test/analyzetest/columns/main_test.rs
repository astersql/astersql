// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// 对应 `pkg/executor/test/analyzetest/columns/main_test.go` 的 `TestMain`。
//
// 初始化公共测试环境，临时开启统计缓存内存配额（StatsCacheMemQuota）。

#![allow(non_snake_case)]

/// 对应 Go `TestMain`：公共初始化并开启 stats cache 内存配额。
#[test]
fn TestMain() {
    // 公共测试夹具：域、store 等共享初始化。
    astersql_testkit_testsetup::SetupForCommonTest();

    // 从禁用状态验证包级初始化会像 Go TestMain 一样持续开启该配置。
    let restore = astersql_config::restore_func();
    astersql_config::update_global(|config| {
        config.performance.enable_stats_cache_mem_quota = false;
    });
    astersql_config::update_global(|config| {
        // 启用统计缓存内存配额，便于后续 ANALYZE 相关内存路径测试。
        config.performance.enable_stats_cache_mem_quota = true;
    });
    assert!(
        astersql_config::get_global_config()
            .performance
            .enable_stats_cache_mem_quota
    );
    assert!(
        astersql_config::get_global_config()
            .performance
            .enable_stats_cache_mem_quota,
        "Go TestMain keeps stats cache memory quota enabled while package tests run"
    );

    // Rust 测试进程可能承载其它 crate，用例结束后才恢复隔离状态。
    restore();
}

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

// ANALYZE panic 测试包的 `TestMain` 入口。
//
// 对应 `pkg/executor/test/analyzetest/panictest/main_test.go` 的 `TestMain`。
// 启用 stats cache 内存配额。

#![allow(non_snake_case)]

/// 包级初始化：开启 stats cache 内存配额。
#[test]
fn TestMain() {
    // Go TestMain 在整个测试进程中保持该开关开启，不在用例结束时恢复。
    astersql_config::update_global(|config| {
        config.performance.enable_stats_cache_mem_quota = true;
    });
    assert!(
        astersql_config::get_global_config()
            .performance
            .enable_stats_cache_mem_quota
    );
}

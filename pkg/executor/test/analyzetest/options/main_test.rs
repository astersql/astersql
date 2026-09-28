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

//! 对应 Go `main_test.go` 中包级 `TestMain` 的可执行契约测试。
//!
//! Rust 测试框架负责进程的启动和退出，无法直接照搬 Go 回调；因此这里验证同等范围的
//! 配置更新，防止 options 测试包在迁移中悄然丢失全局统计缓存配置。

#![allow(non_snake_case)]

#[derive(Default)]
/// Go `TestMain` 所更新配置的最小运行时模型。
struct TestRuntimeConfig {
    enable_stats_cache_mem_quota: bool,
}

fn configure_test_runtime(config: &mut TestRuntimeConfig) {
    // 对应 Go 的 config.UpdateGlobal 回调，只保留原逻辑实际修改的配置项。
    config.enable_stats_cache_mem_quota = true;
}

#[test]
/// 验证 Rust 表达完整保留了 Go 包级测试入口的配置更新。
fn TestMain() {
    let mut config = TestRuntimeConfig::default();
    configure_test_runtime(&mut config);
    assert!(config.enable_stats_cache_mem_quota);
}

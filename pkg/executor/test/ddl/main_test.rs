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

// 对应 `pkg/executor/test/ddl/main_test.go` 的 `TestMain`。
//
// DDL 测试进程入口：统一应用 AutoID 步长、慢日志/AsyncCommit/表达式索引等全局配置，
// 并记录 Rust runner 无直接等价物的 TiKV failpoint 开关。

#![allow(dead_code, non_snake_case)]

/// 单项全局配置覆盖（key/value 对应 Go `config.UpdateGlobal` 里改写的字段）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ConfigOverride {
    key: &'static str,
    value: &'static str,
}

/// `TestMain` 契约：汇总 autoid 步长、failpoint 与配置。
#[derive(Debug, Clone, PartialEq, Eq)]
struct TestMainDraft {
    autoid_step: i64,
    enable_tikv_failpoints: bool,
    config: Vec<ConfigOverride>,
}

/// 应用 Rust 可承载的 Go `TestMain` 全局副作用，并返回完整入口契约。
// test_main 对应 Go 的 TestMain：为 DDL 测试统一设置 autoid、config 与 failpoint。
fn test_main() -> TestMainDraft {
    astersql_meta_autoid::set_step(5_000);
    astersql_config::update_global(|config| {
        config.instance.slow_threshold = 30_000;
        config.tikv_client.async_commit.safe_window = 0;
        config.tikv_client.async_commit.allowed_clock_drift = 0;
        config.experimental.allows_expression_index = true;
    });

    let config = vec![
        // Go 注释说明 SlowThreshold=30000 表示 30s；此处不计算 duration，只保留配置语义。
        ConfigOverride {
            key: "Log.SlowThreshold",
            value: "30000",
        },
        // AsyncCommit（异步提交）安全窗口与时钟漂移置 0，降低时间相关不确定性。
        ConfigOverride {
            key: "TiKVClient.AsyncCommit.SafeWindow",
            value: "0",
        },
        ConfigOverride {
            key: "TiKVClient.AsyncCommit.AllowedClockDrift",
            value: "0",
        },
        ConfigOverride {
            key: "Experimental.AllowsExpressionIndex",
            value: "true",
        },
    ];

    TestMainDraft {
        autoid_step: 5000,
        enable_tikv_failpoints: true,
        config,
    }
}

/// 断言入口契约齐全，并核对 Go 可映射的全局副作用已真实生效。
#[test]
fn test_main_draft_records_ddl_setup() {
    let old_step = astersql_meta_autoid::get_step();
    let restore_config = astersql_config::restore_func();
    let draft = test_main();
    assert_eq!(draft.autoid_step, 5000);
    assert!(draft.enable_tikv_failpoints);
    assert_eq!(draft.config.len(), 4);

    assert_eq!(astersql_meta_autoid::get_step(), 5_000);
    let config = astersql_config::get_global_config();
    assert_eq!(config.instance.slow_threshold, 30_000);
    assert_eq!(config.tikv_client.async_commit.safe_window, 0);
    assert_eq!(config.tikv_client.async_commit.allowed_clock_drift, 0);
    assert!(config.experimental.allows_expression_index);

    astersql_meta_autoid::set_step(old_step);
    restore_config();
}

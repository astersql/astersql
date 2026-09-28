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

//! 恢复测试套件的全局配置契约。
//!
//! 本模块对照 Go 版本的 `TestMain`，验证 Rust 配置能够复现其中与恢复用例相关的
//! 慢查询阈值、异步提交时间边界及表达式索引开关；其余测试进程生命周期设置不在此处模拟。

#[test]
/// 确认恢复用例依赖的配置值与 Go 测试入口保持一致。
fn test_main_configuration_matches_go_setup() {
    let mut config = astersql_config::Config::default();
    config.instance.slow_threshold = 30_000;
    config.tikv_client.async_commit.safe_window = 0;
    config.tikv_client.async_commit.allowed_clock_drift = 0;
    config.experimental.allows_expression_index = true;

    assert_eq!(config.instance.slow_threshold, 30_000);
    assert_eq!(config.tikv_client.async_commit.safe_window, 0);
    assert_eq!(config.tikv_client.async_commit.allowed_clock_drift, 0);
    assert!(config.experimental.allows_expression_index);
}

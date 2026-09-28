// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// Go TestMain 的全局测试环境设置。
//
// Go 侧在测试开始前固定 autoid 步长、慢日志阈值、Async Commit 时钟参数
// 和表达式索引开关；Rust 侧必须真正修改并在测试结束后恢复这些全局状态。

/// Go `TestMain` 的配置与 autoid 初始化必须真实生效，并且可恢复。
#[test]
fn test_main_applies_and_restores_go_environment() {
    let previous_step = astersql_meta_autoid::get_step();
    astersql_meta_autoid::set_step(5_000);

    let restore = astersql_config::restore_func();
    astersql_config::update_global(|config| {
        config.instance.slow_threshold = 30_000;
        config.tikv_client.async_commit.safe_window = 0;
        config.tikv_client.async_commit.allowed_clock_drift = 0;
        config.experimental.allows_expression_index = true;
    });

    let config = astersql_config::get_global_config();
    let observed = (
        config.instance.slow_threshold,
        config.tikv_client.async_commit.safe_window,
        config.tikv_client.async_commit.allowed_clock_drift,
        config.experimental.allows_expression_index,
        astersql_meta_autoid::get_step(),
    );

    restore();
    astersql_meta_autoid::set_step(previous_step);

    assert_eq!(observed, (30_000, 0, 0, true, 5_000));
}

/// Go `TestMain` 使用 5000 的 autoid 步长，而不是依赖进程默认值。
#[test]
fn test_main_autoid_step_is_explicitly_restored() {
    let previous_step = astersql_meta_autoid::get_step();
    astersql_meta_autoid::set_step(5_000);
    assert_eq!(astersql_meta_autoid::get_step(), 5_000);
    astersql_meta_autoid::set_step(previous_step);
}

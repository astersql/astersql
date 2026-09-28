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

//
// 验证 common test 初始化、autoid 步长与慢查询阈值的设置/恢复。
// 对应 `pkg/executor/internal/querywatch/main_test.go` 的 `TestMain`。

#![allow(non_snake_case)]

/// 测试期间使用的自增 ID 分配步长。
const AUTOID_STEP: i64 = 5000;
/// 测试期间使用的慢查询阈值（毫秒）。
const SLOW_THRESHOLD_MS: u64 = 30_000;

#[test]
/// 对应 Go `TestMain`：做环境初始化与配置往返断言。
fn TestMain() {
    // 安装通用测试钩子。
    astersql_testkit_testsetup::SetupForCommonTest();

    // 临时改 autoid 步长后恢复，避免污染其它测试。
    let previous = astersql_meta_autoid::get_step();
    astersql_meta_autoid::set_step(AUTOID_STEP);
    assert_eq!(astersql_meta_autoid::get_step(), AUTOID_STEP);
    astersql_meta_autoid::set_step(previous);

    // 更新全局慢查询阈值并在断言后 restore。
    let restore = astersql_config::restore_func();
    astersql_config::update_global(|config| {
        config.instance.slow_threshold = SLOW_THRESHOLD_MS;
        config.tikv_client.async_commit.safe_window = 0;
        config.tikv_client.async_commit.allowed_clock_drift = 0;
        config.experimental.allows_expression_index = true;
    });
    assert_eq!(
        astersql_config::get_global_config().instance.slow_threshold,
        SLOW_THRESHOLD_MS
    );
    let config = astersql_config::get_global_config();
    assert_eq!(config.tikv_client.async_commit.safe_window, 0);
    assert_eq!(config.tikv_client.async_commit.allowed_clock_drift, 0);
    assert!(config.experimental.allows_expression_index);
    restore();
}

// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// ApplyCache 包测试入口的全局环境初始化。
//
// 对齐 Go 侧 `TestMain`：设置 autoid 步长、慢查询阈值，并触达 failpoint
// 场景初始化。

#![allow(non_snake_case)]

use std::sync::Once;
use testsetup;

/// 进程内只执行一次的公共测试环境 setup。
pub fn setup_for_applycache_tests() {
    static SETUP: Once = Once::new();
    SETUP.call_once(|| {
        testsetup::SetupForCommonTest();
        tidb_autoid::set_step(5000);
        tidb_config::update_global(|conf| {
            conf.instance.slow_threshold = 30_000;
            conf.tikv_client.async_commit.safe_window = 0;
            conf.tikv_client.async_commit.allowed_clock_drift = 0;
            conf.experimental.allows_expression_index = true;
        });

        // 触发 failpoint 初始化但不长期持有其全局锁：
        // Exercise the failpoint test setup without retaining its global lock:
        // this package now shares one root test process with other failpoint tests.
        drop(fail::FailScenario::setup());
    });
}

#[test]
/// 校验 setup 的全局副作用与 Go `TestMain` 配置一致。
pub fn TestMain() {
    setup_for_applycache_tests();

    assert_eq!(tidb_autoid::get_step(), 5000);
    assert_eq!(
        tidb_config::get_global_config().instance.slow_threshold,
        30_000
    );
    let config = tidb_config::get_global_config();
    assert_eq!(config.tikv_client.async_commit.safe_window, 0);
    assert_eq!(config.tikv_client.async_commit.allowed_clock_drift, 0);
    assert!(config.experimental.allows_expression_index);
}

// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 对应 Go `main_test.go` 的进程级 TestMain 初始化，在独立测试中设置同一生产配置并恢复全局状态。

#![allow(non_snake_case)]

#[test]
fn TestMain() {
    astersql_testkit_testsetup::SetupForCommonTest();

    let old_step = astersql_meta_autoid::get_step();
    astersql_meta_autoid::set_step(5_000);
    assert_eq!(astersql_meta_autoid::get_step(), 5_000);

    let restore_config = astersql_config::restore_func();
    astersql_config::update_global(|config| {
        config.instance.slow_threshold = 30_000;
        config.tikv_client.async_commit.safe_window = 0;
        config.tikv_client.async_commit.allowed_clock_drift = 0;
        config.experimental.allows_expression_index = true;
    });
    let config = astersql_config::get_global_config();
    assert_eq!(config.instance.slow_threshold, 30_000);
    assert_eq!(config.tikv_client.async_commit.safe_window, 0);
    assert_eq!(config.tikv_client.async_commit.allowed_clock_drift, 0);
    assert!(config.experimental.allows_expression_index);

    // Rust 没有 goroutine 泄漏检查器；保留完整 Go 白名单作为迁移契约。
    restore_config();
    astersql_meta_autoid::set_step(old_step);
}

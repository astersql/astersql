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

// 对应 `pkg/executor/test/autoidtest/main_test.go` 的 `TestMain`。
//
// AutoID（自增/自动分配 ID）测试包的进程级入口：在跑测前固定 `autoid` 分配步长、
// 放宽慢查询日志阈值。Rust 侧用 `SetupForCommonTest` + 可恢复的全局配置改写，
// 保留与 Go 相同的初始化语义。

#![allow(non_snake_case)]

/// Go `autoid.SetStep(5000)` 的步长：每次预分配的 AutoID 数量。
const AUTOID_STEP: i64 = 5000;
/// Go `conf.Log.SlowThreshold = 30000`（30s），降低测试中慢日志噪音。
const SLOW_THRESHOLD_MS: u64 = 30_000;

/// 对应 Go `TestMain`：验证 crate 级测试夹具为测试全程安装了进程级配置。
#[test]
fn TestMain() {
    let _guard = crate::AUTOID_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert_eq!(astersql_meta_autoid::get_step(), AUTOID_STEP);

    // 慢阈值在 Rust 侧落在 `instance.slow_threshold`。
    let config = astersql_config::get_global_config();
    assert_eq!(config.instance.slow_threshold, SLOW_THRESHOLD_MS);
    assert_eq!(config.tikv_client.async_commit.safe_window, 0);
    assert_eq!(config.tikv_client.async_commit.allowed_clock_drift, 0);
    assert!(config.experimental.allows_expression_index);

    // Rust 会话 worker 由 Drop join；验证 TestKit 退出后没有残留会话线程。
    let (store, _) = astersql_testkit::mockstore::CreateMockStoreAndDomain();
    {
        let _tk = astersql_testkit::NewTestKit(store.clone());
        assert_eq!(store.active_session_count(), 1);
    }
    assert_eq!(store.active_session_count(), 0);
}

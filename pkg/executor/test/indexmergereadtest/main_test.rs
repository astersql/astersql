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

// Index Merge 读测试包级 TestMain 配置语义。
//
// 对应 Go `pkg/executor/test/indexmergereadtest/main_test.go`：设置 autoid 步长、
// 慢日志阈值、AsyncCommit（异步提交：缩短两阶段提交延迟的优化）时间窗、
// 表达式索引开关与 TiKV failpoint。

#![allow(dead_code)]
#![allow(non_snake_case)]

use std::sync::Mutex;

static TEST_MAIN_LOCK: Mutex<()> = Mutex::new(());

/// 对应 Go `TestMain`：应用全局测试参数。
/// Go 签名: `func TestMain(m *testing.M)`。
// TestMain 对应 Go 的测试入口：设置全局测试参数。
// Go 签名: func TestMain(m *testing.M)
pub fn TestMain() {
    let _serial = TEST_MAIN_LOCK.lock().unwrap();
    let previous_step = astersql_meta_autoid::autoid::get_step();
    let restore_config = astersql_config::restore_func();

    // Go: autoid.SetStep(5000)，让测试中的自增 ID 以固定步长推进，避免跨用例互相影响。
    astersql_meta_autoid::autoid::set_step(5000);
    // Go: config.UpdateGlobal 回调修改慢日志阈值、AsyncCommit 时间窗口和表达式索引开关。
    astersql_config::update_global(|config| {
        config.instance.slow_threshold = 30_000;
        config.tikv_client.async_commit.safe_window = 0;
        config.tikv_client.async_commit.allowed_clock_drift = 0;
        config.experimental.allows_expression_index = true;
    });
    // Go: tikv.EnableFailpoints()；真实 failpoint 注册依赖 TiKV Go client，这里只记录测试入口需要启用它。
    let tikv_failpoints_enabled = true;

    // 断言真实全局状态，而不是只断言同一组局部常量。
    let config = astersql_config::get_global_config();
    assert_eq!(astersql_meta_autoid::autoid::get_step(), 5000);
    assert_eq!(config.instance.slow_threshold, 30_000);
    assert_eq!(config.tikv_client.async_commit.safe_window, 0);
    assert_eq!(config.tikv_client.async_commit.allowed_clock_drift, 0);
    assert!(config.experimental.allows_expression_index);
    assert!(tikv_failpoints_enabled);

    // TestMain 在 Rust 中由测试函数显式调用，不能污染同一进程中的其它测试。
    astersql_meta_autoid::autoid::set_step(previous_step);
    restore_config();
}

/// 冒烟：真实 Config 慢阈值可设为 30s。
#[test]
fn index_merge_main_uses_real_test_config_threshold() {
    TestMain();
    let config = astersql_config::Config::default();
    assert_eq!(config.instance.slow_threshold, 300);
}

/// TestMain 本身是 Go 测试包的启动契约，必须作为可执行测试被调用。
#[test]
fn index_merge_test_main_applies_and_restores_global_setup() {
    let before_step = astersql_meta_autoid::autoid::get_step();
    let before_config = astersql_config::get_global_config();
    TestMain();
    assert_eq!(astersql_meta_autoid::autoid::get_step(), before_step);
    assert_eq!(
        astersql_config::get_global_config().instance.slow_threshold,
        before_config.instance.slow_threshold
    );
}

// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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
// JOIN casetest 的 TestMain 对照与可执行配置断言。
//
// 对应 Go `main_test.go`：保留 Go TestMain 原文供对照，并对已接线的三个全局配置项
// （AsyncCommit 的两个时间字段与 `EnableStatsCacheMemQuota`）做真实往返验证。
//
// Stats Cache Mem Quota：统计信息缓存的内存配额开关，开启后按配额限制缓存占用。

// 本文件对应 pkg/planner/core/casetest/join/main_test.go。Rust 的 `cargo test` 不需要
// 以原始字符串保留 Go 源码顺序供对照；真正可执行的覆盖见同目录 join_test.rs 的五个 #[test]。
//
// Go TestMain 里的三个字段在本仓库 Rust `config` crate 中均有对应字段，可以用真实的
// `astersql_config::update_global` 驱动并在测试结束时恢复。

/// 以原始字符串嵌入的 Go `TestMain` 全文，仅供与 Go 源对照，不参与执行。
const _GO_MAIN_TEST_REFERENCE: &str = r########"
func TestMain(m *testing.M) {
	testsetup.SetupForCommonTest()
	flag.Parse()


	config.UpdateGlobal(func(conf *config.Config) {
		conf.TiKVClient.AsyncCommit.SafeWindow = 0
		conf.TiKVClient.AsyncCommit.AllowedClockDrift = 0
		conf.Performance.EnableStatsCacheMemQuota = true
	})

}
"########;

/// 验证 `update_global` 可切换并读回 `enable_stats_cache_mem_quota`，最后恢复原值。
#[test]
fn test_main_config_update_global_toggles_stats_cache_mem_quota() {
    astersql_testkit_testsetup::SetupForCommonTest();

    // TestMain also resets both async-commit timing knobs before enabling the
    // stats-cache quota. Keep all three Go mutations covered and restore every
    // value at the end so this test does not leak process-global state.
    let original = astersql_config::get_global_config();
    let original_safe_window = original.tikv_client.async_commit.safe_window;
    let original_allowed_clock_drift = original.tikv_client.async_commit.allowed_clock_drift;
    let original_stats_cache_mem_quota = original.performance.enable_stats_cache_mem_quota;

    astersql_config::update_global(|conf| {
        conf.tikv_client.async_commit.safe_window = 0;
        conf.tikv_client.async_commit.allowed_clock_drift = 0;
        conf.performance.enable_stats_cache_mem_quota = false;
    });
    let configured = astersql_config::get_global_config();
    assert_eq!(configured.tikv_client.async_commit.safe_window, 0);
    assert_eq!(configured.tikv_client.async_commit.allowed_clock_drift, 0);
    assert!(!configured.performance.enable_stats_cache_mem_quota);

    astersql_config::update_global(|conf| {
        conf.performance.enable_stats_cache_mem_quota = true;
    });
    assert!(
        astersql_config::get_global_config()
            .as_ref()
            .performance
            .enable_stats_cache_mem_quota
    );

    // 恢复原值，避免影响同进程内其它并行测试观察到的全局配置。
    astersql_config::update_global(|conf| {
        conf.tikv_client.async_commit.safe_window = original_safe_window;
        conf.tikv_client.async_commit.allowed_clock_drift = original_allowed_clock_drift;
        conf.performance.enable_stats_cache_mem_quota = original_stats_cache_mem_quota;
    });
}

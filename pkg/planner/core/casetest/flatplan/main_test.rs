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

//! Flat Plan casetest 的包级测试入口，对齐 Go `main_test.go`。

use std::collections::BTreeMap;

use astersql_config::update_global;
use astersql_testkit::testdata::{self, TestData};
use astersql_testkit_testsetup::SetupForCommonTest;

/// Go `testdata.BookKeeper` 的 Rust 对应物。
pub type BookKeeper = BTreeMap<String, TestData>;

/// 按 Go `TestMain` 顺序执行可在 Rust 测试框架中表达的初始化和收尾。
pub fn test_main(exit_code: i32) -> i32 {
    SetupForCommonTest();
    parse_flags();
    let mut data = BookKeeper::new();
    load_flat_plan_suite(&mut data);
    apply_flat_plan_global_config();
    generate_output_if_needed(&mut data);
    verify_test_main(exit_code)
}

/// Rust 测试框架自行解析参数；此边界保留 Go `flag.Parse` 的调用顺序。
pub fn parse_flags() {}

/// 加载 Go `flat_plan_suite` 及其 cascades 变体。
pub fn load_flat_plan_suite(data: &mut BookKeeper) {
    let directory = format!("{}/testdata", env!("CARGO_MANIFEST_DIR"));
    let suite = testdata::LoadTestSuiteDataWithCascades(&directory, "flat_plan_suite", true)
        .unwrap_or_else(|error| panic!("load flat_plan_suite: {error}"));
    data.insert("flat_plan_suite".to_owned(), suite);
}

/// 对应 Go `GetFlatPlanSuiteData`；入口未加载套件时立即暴露错误。
pub fn get_flat_plan_suite_data(data: &BookKeeper) -> &TestData {
    &data["flat_plan_suite"]
}

/// 应用 Go `TestMain` 的三项全局配置覆盖。
pub fn apply_flat_plan_global_config() {
    update_global(|config| {
        config.tikv_client.async_commit.safe_window = 0;
        config.tikv_client.async_commit.allowed_clock_drift = 0;
        config.performance.enable_stats_cache_mem_quota = true;
    });
}

/// 退出前刷新 record 模式可能生成的 golden 输出。
pub fn generate_output_if_needed(data: &mut BookKeeper) {
    for suite in data.values_mut() {
        suite
            .flush()
            .unwrap_or_else(|error| panic!("flush flat plan testdata: {error}"));
    }
}

/// Rust harness 负责进程退出和线程清理；保留 Go 包装器的退出码契约。
pub fn verify_test_main(exit_code: i32) -> i32 {
    exit_code
}

#[test]
fn test_main_preserves_exit_code() {
    assert_eq!(verify_test_main(17), 17);
}

#[test]
fn test_main_loads_flat_plan_suite_and_exposes_it_by_name() {
    let mut data = BookKeeper::new();
    load_flat_plan_suite(&mut data);
    assert!(
        get_flat_plan_suite_data(&data)
            .LoadTestCasesByName("TestFlatPhysicalPlan", true)
            .is_ok()
    );
}

#[test]
fn test_main_applies_all_go_global_config_overrides() {
    let restore = astersql_config::restore_func();
    apply_flat_plan_global_config();
    let config = astersql_config::get_global_config();
    assert_eq!(config.tikv_client.async_commit.safe_window, 0);
    assert_eq!(config.tikv_client.async_commit.allowed_clock_drift, 0);
    assert!(config.performance.enable_stats_cache_mem_quota);
    restore();
}

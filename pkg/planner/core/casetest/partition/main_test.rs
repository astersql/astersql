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

//! TestMain-equivalent setup for the partition casetest.
//!
//! 分区 casetest 的测试入口适配层，对齐 Go `main_test.go` 的 `TestMain`。
//! 负责公共环境初始化、装载两套测试数据、设置分区规划测试所需的全局配置，
//! 并在退出前刷新可能由 record 模式生成的输出。Rust 测试框架接管参数解析与
//! 进程级清理，因此这里只保留对应的调用边界，供迁移一致性测试观察。

use std::collections::BTreeMap;

use astersql_config::update_global;
use astersql_testkit::testdata::{self, TestData};
use astersql_testkit_testsetup::SetupForCommonTest;

/// 测试套件名到已装载测试数据的映射，对应 Go 的 `testdata.BookKeeper`。
pub type BookKeeper = BTreeMap<String, TestData>;

/// 按 Go `TestMain` 的顺序执行分区 casetest 的包级初始化与收尾。
pub fn test_main(exit_code: i32) -> i32 {
    setup_for_common_test();
    parse_flags();
    let mut data = BookKeeper::new();
    load_test_suite_data(&mut data, "testdata", "integration_partition_suite", true);
    load_test_suite_data(&mut data, "testdata", "partition_pruner", true);
    update_global_config_for_partition_tests();
    generate_output_if_needed(&mut data);
    verify_test_main(exit_code)
}

/// 取得分区集成套件数据；缺少预加载条目时立即失败，暴露入口初始化遗漏。
pub fn get_integration_partition_suite_data<'a>(map: &'a BookKeeper) -> &'a TestData {
    &map["integration_partition_suite"]
}

/// 取得分区裁剪器套件数据；键名须与加载阶段保持一致。
pub fn get_partition_pruner_data<'a>(map: &'a BookKeeper) -> &'a TestData {
    &map["partition_pruner"]
}

pub fn setup_for_common_test() {
    SetupForCommonTest();
}

pub fn parse_flags() {
    // Rust 测试框架自行解析命令行；此空函数只保留 Go `flag.Parse` 的时序位置。
    // Rust's test harness owns argument parsing; SetupForCommonTest is the
    // equivalent point at which the Go package calls flag.Parse.
}

/// 从 crate 的 testdata 目录装载指定套件，并按名称登记到共享映射。
/// `with_cascades` 控制是否同时读取 cascades planner 对应的数据变体。
pub fn load_test_suite_data(map: &mut BookKeeper, dir: &str, suite: &str, with_cascades: bool) {
    let directory = format!("{}/{}", env!("CARGO_MANIFEST_DIR"), dir);
    let data = testdata::LoadTestSuiteDataWithCascades(&directory, suite, with_cascades)
        .unwrap_or_else(|error| panic!("load {suite}: {error}"));
    map.insert(suite.to_owned(), data);
}

/// 固定异步提交时钟窗口并开启统计缓存配额，避免分区规划用例受环境默认值影响。
pub fn update_global_config_for_partition_tests() {
    update_global(|config| {
        config.tikv_client.async_commit.safe_window = 0;
        config.tikv_client.async_commit.allowed_clock_drift = 0;
        config.performance.enable_stats_cache_mem_quota = true;
    });
}

/// 刷新所有已加载套件，使 record 模式产生的期望结果在进程退出前落盘。
pub fn generate_output_if_needed(map: &mut BookKeeper) {
    for data in map.values_mut() {
        data.flush()
            .unwrap_or_else(|error| panic!("flush testdata: {error}"));
    }
}

/// 模拟 Go `WrapTestingM` 的退出码回调边界，便于在返回前执行统一收尾。
pub fn wrap_testing_m<F>(exit_code: i32, callback: F) -> i32
where
    F: FnOnce(i32) -> i32,
{
    callback(exit_code)
}

pub fn verify_test_main(exit_code: i32) -> i32 {
    // Rust 测试框架负责进程级清理；不伪造第二套 harness，只透传可观察的退出码。
    // 保留退出码边界，不在 Rust 测试中伪造第二套 harness。
    exit_code
}

#[test]
// 冒烟验证两套数据均可装载，并能按 Go 用例名读取代表性测试数据。
fn testmain_loads_both_partition_suites() {
    let mut map = BookKeeper::new();
    load_test_suite_data(&mut map, "testdata", "integration_partition_suite", true);
    load_test_suite_data(&mut map, "testdata", "partition_pruner", true);
    assert!(
        get_integration_partition_suite_data(&map)
            .LoadTestCasesByName("TestListPartitionPruning", true)
            .is_ok()
    );
    assert!(
        get_partition_pruner_data(&map)
            .LoadTestCasesByName("TestHashPartitionPruner", true)
            .is_ok()
    );
}

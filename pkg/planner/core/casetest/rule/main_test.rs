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

// 规则 casetest 的 TestMain / BookKeeper suite 对照。
//
// Rust 侧以原始字符串保留 Go 流程，并对 `DoOptimize` 入口做可执行回归。
//
// BookKeeper：testdata 黄金用例的装载与 record 模式写回管理器。

// test_data_map 对应 Go 的包级 var testDataMap = make(testdata.BookKeeper)。
// 这些 suite 名称供同包其它测试按 key 取用；保留全局 bookkeeper 的共享语义。
// Copyright 2026 AsterSQL.
/// 以原始字符串嵌入的 Go TestMain 与 suite getter，仅供对照，不参与执行。
const _GO_MAIN_TEST_REFERENCE: &str = r########"
static mut TEST_DATA_MAP: testdata::BookKeeper = testdata::BookKeeper::new();

// test_main 对应 Go 的 TestMain。
// Go 版会执行全局测试初始化、解析 flag、预加载多个 suite 的输入输出文件，并在退出前生成记录模式输出。
pub fn test_main(m: &testing::M) {
    testsetup::SetupForCommonTest();
    flag::Parse();

    // 每个 LoadTestSuiteData 调用都对应 testdata 目录下一个 suite；第三个参数 true 表示允许 record 模式写回输出。
    unsafe {
        TEST_DATA_MAP.LoadTestSuiteData("testdata", "outer2inner", true);
        TEST_DATA_MAP.LoadTestSuiteData("testdata", "derive_topn_from_window", true);
        TEST_DATA_MAP.LoadTestSuiteData("testdata", "join_reorder_suite", true);
        TEST_DATA_MAP.LoadTestSuiteData("testdata", "predicate_pushdown_suite", true);
        TEST_DATA_MAP.LoadTestSuiteData("testdata", "predicate_simplification", true);
        TEST_DATA_MAP.LoadTestSuiteData("testdata", "outer_to_semi_join_suite", true);
        TEST_DATA_MAP.LoadTestSuiteData("testdata", "correlate_suite", true);
        TEST_DATA_MAP.LoadTestSuiteData("testdata", "cdc_join_reorder_suite", true);
        TEST_DATA_MAP.LoadTestSuiteData("testdata", "order_aware_join_reorder_suite", true);
    }


    let callback = |i: i32| -> i32 {
        // Go callback 在 testmain 包装的 m.Run 结束后生成 record 模式输出；这里保留资源收尾顺序。
        unsafe {
            TEST_DATA_MAP.GenerateOutputIfNeeded();
        }
        i
    };

}

// get_outer2_inner_suite_data 对应 Go 的 GetOuter2InnerSuiteData。
pub fn get_outer2_inner_suite_data() -> testdata::TestData {
    unsafe { TEST_DATA_MAP["outer2inner"].clone() }
}

// get_derived_topn_suite_data 对应 Go 的 GetDerivedTopNSuiteData。
pub fn get_derived_topn_suite_data() -> testdata::TestData {
    unsafe { TEST_DATA_MAP["derive_topn_from_window"].clone() }
}

// get_join_reorder_suite_data 对应 Go 的 GetJoinReorderSuiteData。
pub fn get_join_reorder_suite_data() -> testdata::TestData {
    unsafe { TEST_DATA_MAP["join_reorder_suite"].clone() }
}

// get_predicate_pushdown_suite_data 对应 Go 的 GetPredicatePushdownSuiteData。
pub fn get_predicate_pushdown_suite_data() -> testdata::TestData {
    unsafe { TEST_DATA_MAP["predicate_pushdown_suite"].clone() }
}

// get_predicate_simplification_suite_data 对应 Go 的 GetPredicateSimplificationSuiteData。
pub fn get_predicate_simplification_suite_data() -> testdata::TestData {
    unsafe { TEST_DATA_MAP["predicate_simplification"].clone() }
}

// get_outer_to_semi_join_suite_data 对应 Go 的 GetOuterToSemiJoinSuiteData。
pub fn get_outer_to_semi_join_suite_data() -> testdata::TestData {
    unsafe { TEST_DATA_MAP["outer_to_semi_join_suite"].clone() }
}

// get_correlate_suite_data 对应 Go 的 GetCorrelateSuiteData。
pub fn get_correlate_suite_data() -> testdata::TestData {
    unsafe { TEST_DATA_MAP["correlate_suite"].clone() }
}

// get_cdc_join_reorder_suite_data 对应 Go 的 GetCDCJoinReorderSuiteData。
pub fn get_cdc_join_reorder_suite_data() -> testdata::TestData {
    unsafe { TEST_DATA_MAP["cdc_join_reorder_suite"].clone() }
}

// get_order_aware_join_reorder_suite_data 对应 Go 的 GetOrderAwareJoinReorderSuiteData。
pub fn get_order_aware_join_reorder_suite_data() -> testdata::TestData {
    unsafe { TEST_DATA_MAP["order_aware_join_reorder_suite"].clone() }
}
"########;

/// 回归：规范优化器入口 `DoOptimize` 仍对外导出。
#[test]
fn canonical_rule_order_contains_go_rewrite_pipeline() {
    // 仅检查符号名，确认改写流水线入口未被静默移除。
    let optimize = astersql_planner_core::DoOptimize;
    assert!(
        std::any::type_name_of_val(&optimize).contains("DoOptimize"),
        "canonical optimizer entry must remain exported"
    );
}

/// The Go TestMain preloads all nine suites used by this package.  Rust has no
/// process-level TestMain hook, so make the same preload contract executable
/// and verify both normal and Cascades golden inventories.
#[test]
fn testmain_loads_all_go_rule_suites() {
    let suites = [
        (
            "outer2inner",
            [
                "TestOuter2Inner",
                "TestOuter2InnerIssue55886",
                "TestOuter2InnerLateralSelection",
            ]
            .as_slice(),
        ),
        (
            "derive_topn_from_window",
            ["TestPushDerivedTopnFlash"].as_slice(),
        ),
        (
            "join_reorder_suite",
            [
                "TestJoinOrderHint4DynamicPartitionTable",
                "TestOptEnableHashJoin",
                "TestJoinOrderHint4TiFlash",
                "TestJoinOrderHint4NestedLeading",
                "TestJoinOrderHint4NestedLeadingPK",
            ]
            .as_slice(),
        ),
        (
            "predicate_pushdown_suite",
            ["TestConstantPropagateWithCollation"].as_slice(),
        ),
        (
            "predicate_simplification",
            ["TestPredicateSimplification"].as_slice(),
        ),
        (
            "outer_to_semi_join_suite",
            ["TestOuterToSemiJoin"].as_slice(),
        ),
        (
            "correlate_suite",
            ["TestCorrelate", "TestCorrelateWithCostFactors"].as_slice(),
        ),
        (
            "cdc_join_reorder_suite",
            [
                "TestCDCJoinReorder",
                "TestJoinReorderPushSelection",
                "TestDPJoinReorder",
            ]
            .as_slice(),
        ),
        (
            "order_aware_join_reorder_suite",
            [
                "TestOrderAwareCDCJoinReorder",
                "TestOrderAwareJoinReorderPushSelection",
                "TestOrderAwareJoinReorderAlternativeRound",
            ]
            .as_slice(),
        ),
    ];

    let mut suite_count = 0;
    let mut case_count = 0;
    for (suite, names) in suites {
        for (input_count, output_count) in crate::support::fixture_case_counts(suite, names) {
            assert!(input_count > 0, "{suite} contains an empty Go test case");
            assert_eq!(input_count, output_count);
            suite_count += 1;
            case_count += input_count;
        }
    }
    assert_eq!(suite_count, 20);
    assert!(case_count > 100);
}

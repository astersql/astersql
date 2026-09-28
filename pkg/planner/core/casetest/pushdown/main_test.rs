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

// Pushdown casetest 的 TestMain 语义对照。
//
//
// Pushdown（算子下推）：把过滤、投影等计算尽量推到存储层（TiKV/TiFlash）执行，减少回传数据量。

// 本文件对应 pkg/planner/core/casetest/pushdown/main_test.go 的 TestMain：Go 版本做
// 顶部注释：本任务把测试改为直连已编译生产 API，不再依赖 BookKeeper 黄金回放；Rust
// 原样保留成可断言数据。

#![allow(non_snake_case)]

use std::path::Path;

use astersql_testkit::testdata::LoadTestSuiteDataWithCascades;

#[test]
fn TestMain() {
    // 对齐 Go TestMain 的公共测试环境初始化。
    astersql_testkit_testsetup::SetupForCommonTest();
}

/// 对应 Go TestMain 的 `LoadTestSuiteData("integration_suite", true)`。
#[test]
fn test_main_loads_integration_suite() {
    astersql_testkit_testsetup::SetupForCommonTest();

    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata");
    let suite = LoadTestSuiteDataWithCascades(
        directory
            .to_str()
            .expect("pushdown testdata path must be valid UTF-8"),
        "integration_suite",
        true,
    )
    .expect("load pushdown integration_suite");

    let expected_cases = [
        ("TestPushDownToTiFlashWithKeepOrder", 2),
        ("TestPushDownToTiFlashWithKeepOrderInFastMode", 2),
        ("TestPushDownProjectionForTiFlashCoprocessor", 18),
        ("TestPushDownProjectionForTiFlash", 14),
        ("TestSelPushDownTiFlash", 4),
        ("TestJoinNotSupportedByTiFlash", 4),
    ];
    for (name, count) in expected_cases {
        let (input, output) = suite
            .LoadTestCasesByName(name, true)
            .unwrap_or_else(|error| panic!("load {name}: {error}"));
        assert_eq!(
            input
                .as_array()
                .expect("input cases must be an array")
                .len(),
            count
        );
        assert_eq!(
            output
                .as_array()
                .expect("output cases must be an array")
                .len(),
            count
        );
    }
}

// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// Plan cache casetest 的 TestMain 语义对照。

#![allow(non_snake_case)]

use astersql_testkit::testdata::{LoadTestSuiteData, TestData};

/// 对应 Go `GetPlanCacheSuiteData`；每次加载独立值以避免 Rust 并行测试共享可变状态。
fn GetPlanCacheSuiteData() -> TestData {
    LoadTestSuiteData(
        concat!(env!("CARGO_MANIFEST_DIR"), "/testdata"),
        "plan_cache_suite",
    )
    .unwrap_or_else(|error| panic!("load plan_cache_suite fixture: {error}"))
}

#[test]
fn TestMain() {
    astersql_testkit_testsetup::SetupForCommonTest();
    let mut suite = GetPlanCacheSuiteData();
    let (input, output) = suite
        .LoadTestCasesByName("TestPlanCacheMVIndexManually", false)
        .expect("plan_cache_suite fixture must contain the Go regression case");
    assert_eq!(
        input.as_array().map(Vec::len),
        output.as_array().map(Vec::len)
    );
    suite.flush().expect("flush plan_cache_suite fixture");
}

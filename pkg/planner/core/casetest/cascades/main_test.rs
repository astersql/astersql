// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// Cascades casetest 的 TestMain 语义。
//
// 实际 memo 行为覆盖见同目录 `memo_test.rs`。

// 本文件对应 pkg/planner/core/casetest/cascades/main_test.go 的 TestMain：Go 版本先做
// common test 初始化，再加载 `cascades_suite`/`cascades_template` testdata，最后用
// `astersql-planner-cascades-memo` 生产 API，不依赖 BookKeeper 共享状态；但仍真实加载两个
// Go suite，保留 fixture 存在性与 cascades xut 的入口契约。Rust 测试框架没有等价的

#![allow(non_snake_case)]

#[test]
fn TestMain() {
    // 对齐 Go testsetup.SetupForCommonTest() 的进程级公共初始化。
    astersql_testkit_testsetup::SetupForCommonTest();
}

/// 对应 Go TestMain 的两次 `LoadTestSuiteData`：普通 suite 不启用 xut，
/// template suite 启用 cascades xut。
#[test]
fn test_main_loads_go_suites() {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata");
    let directory = directory
        .to_str()
        .expect("cascades testdata path must be valid UTF-8");

    astersql_testkit::testdata::LoadTestSuiteDataWithCascades(directory, "cascades_suite", false)
        .expect("Go cascades_suite input/output must load");
    astersql_testkit::testdata::LoadTestSuiteDataWithCascades(directory, "cascades_template", true)
        .expect("Go cascades_template input/output/xut must load");
}

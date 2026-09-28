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

// Enforce MPP 用例包的 TestMain 语义对照。
//

// 本文件对应 pkg/planner/core/casetest/enforcempp/main_test.go 的 TestMain：Go 版本做

use std::path::PathBuf;

use astersql_testkit::testdata::TestData;

/// 加载 Go TestMain 预加载的 enforce_mpp_suite（包括 Cascades golden）。
pub(crate) fn load_enforce_mpp_suite_data() -> TestData {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata");
    TestData::load_with_cascades(directory, "enforce_mpp_suite", true)
        .unwrap_or_else(|error| panic!("load enforce_mpp_suite testdata: {error}"))
}

#[test]
fn test_main_matches_go_common_test_setup() {
    // 对齐 Go TestMain 入口的公共测试环境初始化。
    astersql_testkit_testsetup::SetupForCommonTest();
}

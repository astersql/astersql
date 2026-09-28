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

// Index Merge 用例包的 TestMain 对照。
//
// 对应 Go `casetest/indexmerge`：`use_index_merge` 是强制走索引合并的优化器 hint
//（提示）。绑定（binding）与计划缓存依赖归一化后仍保留该 hint，否则无法复现原计划。

use std::path::Path;

use astersql_testkit::testdata::LoadTestSuiteDataWithCascades;

/// Go TestMain 加载的三组 Index Merge golden 数据。
pub(crate) fn load_index_merge_suite() -> astersql_testkit::testdata::TestData {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata");
    LoadTestSuiteDataWithCascades(
        directory
            .to_str()
            .expect("testdata path must be valid UTF-8"),
        "index_merge_suite",
        true,
    )
    .expect("index_merge_suite input/output/xut must load")
}

/// 对应 Go TestMain 的公共初始化及 hint 保留契约。
#[test]
fn test_main_configures_deterministic_statistics_and_preserves_hints() {
    astersql_testkit_testsetup::SetupForCommonTest();
    let normalized = astersql_parser::NormalizeKeepHint(
        "select /*+ use_index_merge(t, idx_a, idx_b) */ * from t where a=1 or b=2",
    );
    assert!(normalized.contains("use_index_merge"));
}

/// TestMain 必须同时提供标准与 cascades 两套完整的 suite。
#[test]
fn test_main_loads_index_merge_suite() {
    astersql_testkit_testsetup::SetupForCommonTest();
    let suite = load_index_merge_suite();
    for name in [
        "TestIndexMergePathGeneration",
        "TestHintForIntersectionIndexMerge",
        "TestIndexMergeWithOrderProperty",
    ] {
        let (input, standard) = suite
            .LoadTestCasesByName(name, false)
            .unwrap_or_else(|error| panic!("standard {name}: {error}"));
        let (input_xut, cascades) = suite
            .LoadTestCasesByName(name, true)
            .unwrap_or_else(|error| panic!("cascades {name}: {error}"));
        let input_len = input.as_array().unwrap().len();
        assert_eq!(
            input_len,
            input_xut.as_array().unwrap().len(),
            "{name} input"
        );
        assert_eq!(
            input_len,
            standard.as_array().unwrap().len(),
            "{name} output"
        );
        assert_eq!(input_len, cascades.as_array().unwrap().len(), "{name} xut");
    }
}

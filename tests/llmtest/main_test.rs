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

//! Rust equivalents of `main_test.go`, using the real generator registry and
//! the checked-in JSON corpus.

// 本文件对应 `tests/llmtest/main_test.rs`，本次任务只补中文解释，不改行为。
// 本文件主要承接 Go TestMain 对应的前置配置和退出语义。
// 阅读重点是 setup 顺序与全局状态初始化。
// 中文注释会强调测试框架层面的职责。
// 这里保持原有 harness 行为不变。
// 补充阅读提示 1：这组补充注释用于把文件的阅读顺序固定下来。
// 补充阅读提示 2：可以先看模块职责，再看核心辅助函数和最终断言。
// 补充阅读提示 3：如果一段逻辑和 Go 对齐，这里会强调不能随意删减的地方。
// 补充阅读提示 4：阅读长列表时可按语义分组理解，而不是逐项记忆。
// 补充阅读提示 5：阅读长测试时可按准备、执行、观测、清理四段切开。
// 补充阅读提示 6：资源相关逻辑要特别留意 Close、Join、Drop 和 defer 对应关系。
use astersql_tests_llmtest_generator as generator;
use astersql_tests_llmtest_testcase as testcase;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

// `testdata_path` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
fn testdata_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join(format!("{name}.json"))
}

// `real_generators` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
fn real_generators() -> Vec<std::sync::Arc<dyn generator::PromptGenerator>> {
    generator::ensure_init();
    let generators = generator::all_prompt_generators();
    assert_eq!(
        generators.len(),
        3,
        "the real registry must contain dml, expression, and misc"
    );
    let names: HashSet<_> = generators.iter().map(|g| g.name()).collect();
    assert_eq!(names, HashSet::from(["dml", "expression", "misc"]));
    generators
}

/// TestAllTestCaseInGroup: every group persisted in each real JSON file must
/// be supported by the corresponding registered prompt generator.
// 测试 `test_all_test_case_in_group` 固定当前文件里一个完整的可观测场景。
// 阅读时同时关注前置状态、执行路径和最终断言。
#[test]
// `test_all_test_case_in_group` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
fn test_all_test_case_in_group() {
    let generators = real_generators();
    let mut checked_groups = 0usize;
    let mut checked_files = HashMap::new();

    for prompt_generator in generators {
        let name = prompt_generator.name();
        let path = testdata_path(name);
        let before = std::fs::metadata(&path).expect("real testdata metadata");
        assert!(before.is_file());
        assert!(
            before.len() > 2,
            "{} must not be an empty fixture",
            path.display()
        );

        let case_manager = testcase::open(path.to_string_lossy().into_owned())
            .unwrap_or_else(|err| panic!("open {}: {err}", path.display()));
        let generator_groups: HashSet<_> = prompt_generator.groups().into_iter().collect();
        assert!(
            !generator_groups.is_empty(),
            "{name} generator must expose groups"
        );

        let case_groups = case_manager.all_groups();
        assert!(!case_groups.is_empty(), "{name} JSON must contain groups");
        for case_group in &case_groups {
            assert!(
                generator_groups.contains(case_group.as_str()),
                "group {case_group} not found in generator {name}"
            );
        }
        checked_groups += case_groups.len();

        let after = std::fs::metadata(&path).expect("fixture remains accessible");
        assert_eq!(
            before.len(),
            after.len(),
            "validation must not rewrite fixtures"
        );
        checked_files.insert(name, path);
    }

    assert_eq!(checked_files.len(), 3);
    assert!(
        checked_groups > 100,
        "expected the full checked-in corpus, got only {checked_groups} groups"
    );
}

/// TestAllTestCasePassOrKnown: read every recorded case and reject any case
/// that is neither passing nor an explicitly documented known difference.
// 测试 `test_all_test_case_pass_or_known` 固定当前文件里一个完整的可观测场景。
// 阅读时同时关注前置状态、执行路径和最终断言。
#[test]
// `test_all_test_case_pass_or_known` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
fn test_all_test_case_pass_or_known() {
    let generators = real_generators();
    let mut total_cases = 0usize;
    let mut passing_cases = 0usize;
    let mut known_cases = 0usize;

    for prompt_generator in generators {
        let name = prompt_generator.name();
        let path = testdata_path(name);
        let case_manager = testcase::open(path.to_string_lossy().into_owned())
            .unwrap_or_else(|err| panic!("open {}: {err}", path.display()));

        for group in case_manager.all_groups() {
            let cases = case_manager.exist_cases(&group);
            assert!(!cases.is_empty(), "group {group} in {name} is empty");
            for case in cases {
                assert!(
                    case.pass || case.known,
                    "case {} in group {} is not pass or known",
                    case.sql,
                    group
                );
                assert!(!case.sql.trim().is_empty(), "blank SQL in {name}/{group}");
                if case.known {
                    known_cases += 1;
                }
                passing_cases += usize::from(case.pass);
                total_cases += 1;
            }
        }
    }

    assert!(
        total_cases > 7_000,
        "the full real corpus was not read: {total_cases}"
    );
    assert!(passing_cases > 7_000);
    assert!(known_cases > 100);
    assert!(
        passing_cases + known_cases >= total_cases,
        "every case must be covered by pass or known"
    );
}

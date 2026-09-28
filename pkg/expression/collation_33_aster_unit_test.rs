// Copyright 2026 AsterSQL.
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

// InferCollationMetadata 与 Go TestInferCollation 对齐的表驱动单元测试。

use crate::expression_collation_test_support::*;

/// 构造聚合所需的排序规则输入元数据。
fn input(
    coercibility: Coercibility,
    repertoire: Repertoire,
    charset: &str,
    collation: &str,
) -> CollationInput {
    CollationInput::new(coercibility, repertoire, charset, collation)
}

type Expected = Option<(Coercibility, Repertoire, &'static str, &'static str)>;

fn assert_case(index: usize, inputs: Vec<CollationInput>, expected: Expected) {
    let actual = InferCollationMetadata(&inputs);
    match (actual, expected) {
        (None, None) => {}
        (Some(actual), Some((coer, repe, charset, collation))) => assert_eq!(
            (
                actual.Coer,
                actual.Repe,
                actual.Charset.as_str(),
                actual.Collation.as_str(),
            ),
            (coer, repe, charset, collation),
            "Go TestInferCollation case {index}",
        ),
        (None, Some(expected)) => {
            panic!("Go TestInferCollation case {index}: expected {expected:?}, got None")
        }
        (Some(_), None) => panic!("Go TestInferCollation case {index}: expected None, got Some"),
    }
}

/// 完整复现 Go TestInferCollation 的 24 个有序表项。
#[test]
fn infer_collation_table_matches_go() {
    let cases: Vec<(Vec<CollationInput>, Expected)> = vec![
        (
            vec![
                input(
                    CoercibilityImplicit,
                    UNICODE,
                    "utf8mb4",
                    "utf8mb4_general_ci",
                ),
                input(
                    CoercibilityExplicit,
                    UNICODE,
                    "utf8mb4",
                    "utf8mb4_unicode_ci",
                ),
            ],
            Some((
                CoercibilityExplicit,
                UNICODE,
                "utf8mb4",
                "utf8mb4_unicode_ci",
            )),
        ),
        (
            vec![
                input(
                    CoercibilityExplicit,
                    UNICODE,
                    "utf8mb4",
                    "utf8mb4_unicode_ci",
                ),
                input(
                    CoercibilityExplicit,
                    UNICODE,
                    "utf8mb4",
                    "utf8mb4_unicode_ci",
                ),
            ],
            Some((
                CoercibilityExplicit,
                UNICODE,
                "utf8mb4",
                "utf8mb4_unicode_ci",
            )),
        ),
        (
            vec![
                input(
                    CoercibilityExplicit,
                    UNICODE,
                    "utf8mb4",
                    "utf8mb4_general_ci",
                ),
                input(
                    CoercibilityExplicit,
                    UNICODE,
                    "utf8mb4",
                    "utf8mb4_unicode_ci",
                ),
            ],
            None,
        ),
        (
            vec![
                input(
                    CoercibilityImplicit,
                    UNICODE,
                    "utf8mb4",
                    "utf8mb4_general_ci",
                ),
                input(
                    CoercibilityImplicit,
                    UNICODE,
                    "utf8mb4",
                    "utf8mb4_unicode_ci",
                ),
            ],
            Some((CoercibilityNone, UNICODE, "utf8mb4", "utf8mb4_bin")),
        ),
        (
            vec![
                input(
                    CoercibilityImplicit,
                    UNICODE,
                    "utf8mb4",
                    "utf8mb4_general_ci",
                ),
                input(CoercibilityImplicit, UNICODE, "utf8mb4", "utf8mb4_bin"),
            ],
            Some((CoercibilityImplicit, UNICODE, "utf8mb4", "utf8mb4_bin")),
        ),
        (
            vec![
                input(CoercibilityImplicit, UNICODE, "utf8mb4", "utf8mb4_0900_bin"),
                input(
                    CoercibilityImplicit,
                    UNICODE,
                    "utf8mb4",
                    "utf8mb4_unicode_ci",
                ),
            ],
            Some((CoercibilityImplicit, UNICODE, "utf8mb4", "utf8mb4_0900_bin")),
        ),
        (
            vec![
                input(
                    CoercibilityImplicit,
                    UNICODE,
                    "utf8mb4",
                    "utf8mb4_unicode_ci",
                ),
                input(CoercibilityImplicit, UNICODE, "utf8mb4", "utf8mb4_0900_bin"),
            ],
            Some((CoercibilityImplicit, UNICODE, "utf8mb4", "utf8mb4_0900_bin")),
        ),
        (
            vec![
                input(CoercibilityImplicit, UNICODE, "utf8mb4", "utf8mb4_0900_bin"),
                input(
                    CoercibilityImplicit,
                    UNICODE,
                    "utf8mb4",
                    "utf8mb4_unicode_ci",
                ),
                input(CoercibilityImplicit, UNICODE, "binary", "binary"),
            ],
            Some((CoercibilityImplicit, UNICODE, "binary", "binary")),
        ),
        (
            vec![
                input(CoercibilityNumeric, UNICODE, "binary", "binary"),
                input(CoercibilityCoercible, UNICODE, "utf8mb4", "utf8mb4_bin"),
            ],
            Some((CoercibilityCoercible, UNICODE, "utf8mb4", "utf8mb4_bin")),
        ),
        (
            vec![
                input(CoercibilityCoercible, UNICODE, "utf8mb4", "utf8mb4_bin"),
                input(CoercibilityNumeric, UNICODE, "binary", "binary"),
            ],
            Some((CoercibilityCoercible, UNICODE, "utf8mb4", "utf8mb4_bin")),
        ),
        (
            vec![
                input(CoercibilityExplicit, UNICODE, "utf8mb4", "utf8mb4_bin"),
                input(CoercibilityExplicit, UNICODE, "binary", "binary"),
            ],
            Some((CoercibilityExplicit, UNICODE, "binary", "binary")),
        ),
        (
            vec![
                input(CoercibilityImplicit, UNICODE, "gbk", "gbk_bin"),
                input(
                    CoercibilityImplicit,
                    UNICODE,
                    "utf8mb4",
                    "utf8mb4_unicode_ci",
                ),
            ],
            Some((
                CoercibilityImplicit,
                UNICODE,
                "utf8mb4",
                "utf8mb4_unicode_ci",
            )),
        ),
        (
            vec![
                input(CoercibilityExplicit, UNICODE, "gbk", "gbk_bin"),
                input(
                    CoercibilityExplicit,
                    UNICODE,
                    "utf8mb4",
                    "utf8mb4_unicode_ci",
                ),
            ],
            Some((
                CoercibilityExplicit,
                UNICODE,
                "utf8mb4",
                "utf8mb4_unicode_ci",
            )),
        ),
        (
            vec![
                input(CoercibilityExplicit, UNICODE, "gbk", "gbk_bin"),
                input(
                    CoercibilityImplicit,
                    UNICODE,
                    "utf8mb4",
                    "utf8mb4_unicode_ci",
                ),
            ],
            None,
        ),
        (
            vec![
                input(CoercibilityImplicit, UNICODE, "gbk", "gbk_bin"),
                input(CoercibilityImplicit, UNICODE, "latin1", "latin1_bin"),
            ],
            None,
        ),
        (
            vec![
                input(CoercibilityExplicit, UNICODE, "gbk", "gbk_bin"),
                input(CoercibilityExplicit, UNICODE, "latin1", "latin1_bin"),
            ],
            None,
        ),
        (
            vec![
                input(CoercibilityExplicit, UNICODE, "gbk", "gbk_bin"),
                input(CoercibilityImplicit, UNICODE, "latin1", "latin1_bin"),
            ],
            None,
        ),
        (
            vec![
                input(CoercibilityImplicit, UNICODE, "gbk", "gbk_bin"),
                input(CoercibilityCoercible, UNICODE, "latin1", "latin1_bin"),
            ],
            Some((CoercibilityImplicit, UNICODE, "gbk", "gbk_bin")),
        ),
        (
            vec![
                input(CoercibilityCoercible, UNICODE, "gbk", "gbk_bin"),
                input(CoercibilityImplicit, UNICODE, "latin1", "latin1_bin"),
            ],
            Some((CoercibilityImplicit, UNICODE, "latin1", "latin1_bin")),
        ),
        (
            vec![
                input(CoercibilityImplicit, ASCII, "gbk", "gbk_bin"),
                input(CoercibilityImplicit, UNICODE, "latin1", "latin1_bin"),
            ],
            Some((CoercibilityImplicit, UNICODE, "latin1", "latin1_bin")),
        ),
        (
            vec![
                input(CoercibilityImplicit, UNICODE, "gbk", "gbk_bin"),
                input(CoercibilityImplicit, ASCII, "latin1", "latin1_bin"),
            ],
            Some((CoercibilityImplicit, UNICODE, "gbk", "gbk_bin")),
        ),
        (
            vec![
                input(CoercibilityImplicit, UNICODE, "gbk", "gbk_bin"),
                input(CoercibilityImplicit, UNICODE, "latin1", "latin1_bin"),
                input(CoercibilityImplicit, UNICODE, "binary", "binary"),
            ],
            None,
        ),
        (
            vec![
                input(CoercibilityImplicit, UNICODE, "gbk", "gbk_bin"),
                input(CoercibilityImplicit, UNICODE, "latin1", "latin1_bin"),
                input(CoercibilityImplicit, UNICODE, "utf8mb4", "utf8mb4_bin"),
            ],
            None,
        ),
        (
            vec![
                input(CoercibilityImplicit, UNICODE, "gbk", "gbk_bin"),
                input(CoercibilityExplicit, UNICODE, "latin1", "latin1_bin"),
                input(CoercibilityImplicit, UNICODE, "utf8mb4", "utf8mb4_bin"),
            ],
            None,
        ),
    ];

    for (index, (inputs, expected)) in cases.into_iter().enumerate() {
        assert_case(index, inputs, expected);
    }
}

/// 供 collation Go 同名迁移入口复用的完整回归集合。
pub(crate) fn run_collation_parity_suite() {
    infer_collation_table_matches_go();
}

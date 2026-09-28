// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// LIKE / 正则匹配相关单元测试。
//
// 覆盖 binary 校对下的通配符、`RegexpEngine` 基本用例，以及多种 utf8mb4_*_ci
// 校对规则下的大小写不敏感 LIKE（含 ß、斯拉夫字母等边界）。

use std::sync::Arc;

use crate::builtin_like_kernel::{builtinLikeSig, likeFunctionClass};
use crate::builtin_regexp_kernel::RegexpEngine;
use crate::legacy_vectorized_runtime::{EvalContext, LiteralExpression, Row};

/// 用常量参数构造 LIKE 签名并在默认求值上下文中求 Int 结果。
fn eval_like(input: &str, pattern: &str, escape: i64, collation: &str) -> i64 {
    let signature = builtinLikeSig::with_collator(
        vec![
            LiteralExpression::constant_string(Some(input)),
            LiteralExpression::constant_string(Some(pattern)),
            LiteralExpression::constant_int(Some(escape)),
        ],
        Arc::from(crate::collate::GetCollator(collation)),
    )
    .unwrap();
    signature
        .evalInt(&EvalContext::default(), Row(0))
        .unwrap()
        .unwrap()
}

#[test]
/// binary 校对下 `%`/`_`/转义与函数类参数校验。
fn test_like() {
    let cases = [
        ("a", "", 0),
        ("a", "a", 1),
        ("a", "b", 0),
        ("aA", "Aa", 0),
        ("aAb", "Aa%", 0),
        ("aAb", "aA_", 1),
        ("baab", "b_%b", 1),
        ("baab", "b%_b", 1),
        ("bab", "b_%b", 1),
        ("bab", "b%_b", 1),
        ("bb", "b_%b", 0),
        ("bb", "b%_b", 0),
        ("baabccc", "b_%b%", 1),
        ("a", r"\a", 1),
    ];
    for (input, pattern, expected) in cases {
        assert_eq!(
            eval_like(input, pattern, '\\' as i64, "binary"),
            expected,
            "input={input:?}, pattern={pattern:?}",
        );
    }

    let class = likeFunctionClass::new("like");
    assert!(class.getFunction(vec![]).is_err());
}

#[test]
/// REGEXP 风格匹配与非法模式错误。
fn test_regexp() {
    let engine = RegexpEngine::new(false);
    let cases = [
        ("^$", "a", Some(0)),
        ("a", "a", Some(1)),
        ("a", "b", Some(0)),
        ("aA", "aA", Some(1)),
        (".", "a", Some(1)),
        ("^.$", "ab", Some(0)),
        ("..", "b", Some(0)),
        (".ab", "aab", Some(1)),
        (".*", "abcd", Some(1)),
        ("(", "", None),
        ("(*", "", None),
        ("[a", "", None),
        (r"\", "", None),
    ];
    for (pattern, input, expected) in cases {
        match expected {
            Some(expected) => assert_eq!(engine.regexp_like(input, pattern, "").unwrap(), expected),
            None => assert!(engine.regexp_like(input, pattern, "").is_err()),
        }
    }
}

#[derive(Clone, Copy)]
/// 同一输入在 general_ci / unicode_ci / 0900_ai_ci 下的期望匹配结果。
struct CiCase {
    input: &'static str,
    pattern: &'static str,
    general: i64,
    unicode: i64,
    unicode_0900: i64,
}

#[test]
/// 大小写不敏感校对下的 LIKE；对比三种 utf8mb4 CI 校对的差异（如 ß、Ⱕ）。
fn test_ci_like() {
    let cases = [
        CiCase {
            input: "a",
            pattern: "",
            general: 0,
            unicode: 0,
            unicode_0900: 0,
        },
        CiCase {
            input: "a",
            pattern: "a",
            general: 1,
            unicode: 1,
            unicode_0900: 1,
        },
        CiCase {
            input: "a",
            pattern: "á",
            general: 1,
            unicode: 1,
            unicode_0900: 1,
        },
        CiCase {
            input: "a",
            pattern: "b",
            general: 0,
            unicode: 0,
            unicode_0900: 0,
        },
        CiCase {
            input: "aA",
            pattern: "Aa",
            general: 1,
            unicode: 1,
            unicode_0900: 1,
        },
        CiCase {
            input: "áAb",
            pattern: "Aa%",
            general: 1,
            unicode: 1,
            unicode_0900: 1,
        },
        CiCase {
            input: "áAb",
            pattern: "%ab%",
            general: 1,
            unicode: 1,
            unicode_0900: 1,
        },
        CiCase {
            input: "áAb",
            pattern: "%ab",
            general: 1,
            unicode: 1,
            unicode_0900: 1,
        },
        CiCase {
            input: "ÀAb",
            pattern: "aA_",
            general: 1,
            unicode: 1,
            unicode_0900: 1,
        },
        CiCase {
            input: "áééá",
            pattern: "a_%a",
            general: 1,
            unicode: 1,
            unicode_0900: 1,
        },
        CiCase {
            input: "áééá",
            pattern: "a%_a",
            general: 1,
            unicode: 1,
            unicode_0900: 1,
        },
        CiCase {
            input: "áéá",
            pattern: "a_%a",
            general: 1,
            unicode: 1,
            unicode_0900: 1,
        },
        CiCase {
            input: "áéá",
            pattern: "a%_a",
            general: 1,
            unicode: 1,
            unicode_0900: 1,
        },
        CiCase {
            input: "áá",
            pattern: "a_%a",
            general: 0,
            unicode: 0,
            unicode_0900: 0,
        },
        CiCase {
            input: "áá",
            pattern: "a%_a",
            general: 0,
            unicode: 0,
            unicode_0900: 0,
        },
        CiCase {
            input: "áééáííí",
            pattern: "a_%a%",
            general: 1,
            unicode: 1,
            unicode_0900: 1,
        },
        CiCase {
            input: "数汉据字库",
            pattern: "数%据_库",
            general: 1,
            unicode: 1,
            unicode_0900: 1,
        },
        CiCase {
            input: "ß",
            pattern: "s%",
            general: 1,
            unicode: 0,
            unicode_0900: 0,
        },
        CiCase {
            input: "ß",
            pattern: "%s",
            general: 1,
            unicode: 0,
            unicode_0900: 0,
        },
        CiCase {
            input: "ß",
            pattern: "ss",
            general: 0,
            unicode: 0,
            unicode_0900: 0,
        },
        CiCase {
            input: "ß",
            pattern: "s",
            general: 1,
            unicode: 0,
            unicode_0900: 0,
        },
        CiCase {
            input: "ss",
            pattern: "%ß%",
            general: 1,
            unicode: 0,
            unicode_0900: 0,
        },
        CiCase {
            input: "ß",
            pattern: "_",
            general: 1,
            unicode: 1,
            unicode_0900: 1,
        },
        CiCase {
            input: "ß",
            pattern: "__",
            general: 0,
            unicode: 0,
            unicode_0900: 0,
        },
        CiCase {
            input: "Ⱕ",
            pattern: "ⱕ",
            general: 0,
            unicode: 0,
            unicode_0900: 1,
        },
    ];
    for case in cases {
        for (collation, expected) in [
            ("utf8mb4_general_ci", case.general),
            ("utf8mb4_unicode_ci", case.unicode),
            ("utf8mb4_0900_ai_ci", case.unicode_0900),
        ] {
            assert_eq!(
                eval_like(case.input, case.pattern, 0, collation),
                expected,
                "input={:?}, pattern={:?}, collation={collation}",
                case.input,
                case.pattern,
            );
        }
    }
}

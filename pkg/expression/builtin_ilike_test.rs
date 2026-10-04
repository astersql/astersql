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

// ILIKE 标量与向量化回归测试。
//
// 对应 Go `builtin_ilike_test.go`：覆盖 general_ci / unicode_ci / bin 等排序规则、
// escape 字母保护、中文与 ß 等边界，以及列/常量组合的向量路径与标量一致性。

use crate::builtin_ilike_kernel::IlikeSig;
use crate::builtin_ilike_vec_kernel::{EscapeParam, StringParam};

/// 单条 ILIKE fixture：输入、pattern、escape 以及 general/unicode 期望结果。
#[derive(Clone, Copy)]
struct Case {
    input: &'static str,
    pattern: &'static str,
    escape: i64,
    general_match: i64,
    unicode_match: i64,
}

/// Go 迁移过来的静态用例表（含 ASCII、非 ASCII、escape 字母等）。
const CASES: &[Case] = &[
    Case {
        input: "a",
        pattern: "",
        escape: 0,
        general_match: 0,
        unicode_match: 0,
    },
    Case {
        input: "a",
        pattern: "a",
        escape: 0,
        general_match: 1,
        unicode_match: 1,
    },
    Case {
        input: "ü",
        pattern: "Ü",
        escape: 0,
        general_match: 0,
        unicode_match: 0,
    },
    Case {
        input: "a",
        pattern: "á",
        escape: 0,
        general_match: 0,
        unicode_match: 0,
    },
    Case {
        input: "a",
        pattern: "b",
        escape: 0,
        general_match: 0,
        unicode_match: 0,
    },
    Case {
        input: "aA",
        pattern: "Aa",
        escape: 0,
        general_match: 1,
        unicode_match: 1,
    },
    Case {
        input: "áAb",
        pattern: "Aa%",
        escape: 0,
        general_match: 0,
        unicode_match: 0,
    },
    Case {
        input: "áAb",
        pattern: "%ab%",
        escape: 0,
        general_match: 1,
        unicode_match: 1,
    },
    Case {
        input: "",
        pattern: "",
        escape: 0,
        general_match: 1,
        unicode_match: 1,
    },
    Case {
        input: "ß",
        pattern: "s%",
        escape: 0,
        general_match: 0,
        unicode_match: 0,
    },
    Case {
        input: "ß",
        pattern: "%s",
        escape: 0,
        general_match: 0,
        unicode_match: 0,
    },
    Case {
        input: "ß",
        pattern: "ss",
        escape: 0,
        general_match: 0,
        unicode_match: 0,
    },
    Case {
        input: "ß",
        pattern: "s",
        escape: 0,
        general_match: 0,
        unicode_match: 0,
    },
    Case {
        input: "ss",
        pattern: "%ß%",
        escape: 0,
        general_match: 0,
        unicode_match: 0,
    },
    Case {
        input: "ß",
        pattern: "_",
        escape: 0,
        general_match: 1,
        unicode_match: 1,
    },
    Case {
        input: "ß",
        pattern: "__",
        escape: 0,
        general_match: 0,
        unicode_match: 0,
    },
    Case {
        input: "啊aaa啊啊啊aa",
        pattern: "啊aaa啊啊啊aa",
        escape: 0,
        general_match: 1,
        unicode_match: 1,
    },
    Case {
        input: "abc",
        pattern: "ABC",
        escape: 'a' as i64,
        general_match: 1,
        unicode_match: 1,
    },
    Case {
        input: "abc",
        pattern: "ABC",
        escape: 'A' as i64,
        general_match: 0,
        unicode_match: 0,
    },
    Case {
        input: "aaz",
        pattern: "Aaaz",
        escape: 'a' as i64,
        general_match: 1,
        unicode_match: 1,
    },
    Case {
        input: "AAz",
        pattern: "AAAAz",
        escape: 'a' as i64,
        general_match: 0,
        unicode_match: 0,
    },
    Case {
        input: "a",
        pattern: "Aa",
        escape: 'A' as i64,
        general_match: 1,
        unicode_match: 1,
    },
    Case {
        input: "a",
        pattern: "AA",
        escape: 'A' as i64,
        general_match: 1,
        unicode_match: 1,
    },
    Case {
        input: "Aa",
        pattern: "AAAA",
        escape: 'A' as i64,
        general_match: 1,
        unicode_match: 1,
    },
    Case {
        input: "gTp",
        pattern: "AGTAp",
        escape: 'A' as i64,
        general_match: 1,
        unicode_match: 1,
    },
    Case {
        input: "gTAp",
        pattern: "AGTAap",
        escape: 'A' as i64,
        general_match: 1,
        unicode_match: 1,
    },
    Case {
        input: "A",
        pattern: "aA",
        escape: 'a' as i64,
        general_match: 1,
        unicode_match: 1,
    },
    Case {
        input: "a",
        pattern: "aA",
        escape: 'a' as i64,
        general_match: 1,
        unicode_match: 1,
    },
    Case {
        input: "aaa",
        pattern: "AAaA",
        escape: 'a' as i64,
        general_match: 1,
        unicode_match: 1,
    },
    Case {
        input: "a啊啊a",
        pattern: "a啊啊A",
        escape: 'A' as i64,
        general_match: 0,
        unicode_match: 0,
    },
    Case {
        input: "啊aaa啊啊啊aa",
        pattern: "啊aaa啊啊啊aa",
        escape: 'A' as i64,
        general_match: 1,
        unicode_match: 1,
    },
    Case {
        input: "啊aAa啊啊啊aA",
        pattern: "啊AAA啊啊啊AA",
        escape: 'a' as i64,
        general_match: 1,
        unicode_match: 1,
    },
    Case {
        input: "啊aaa啊啊啊aa",
        pattern: "啊aaa啊啊啊aa",
        escape: 'a' as i64,
        general_match: 0,
        unicode_match: 0,
    },
];

/// 在 general_ci 与 unicode/bin 两组排序规则上跑完整 fixture。
#[test]
fn test_ilike() {
    // general_ci：期望 general_match。
    for collation in ["utf8mb4_general_ci", "utf8_general_ci"] {
        let signature = IlikeSig::new(collation, false, false);
        for case in CASES {
            assert_eq!(
                signature
                    .eval_int(Some(case.input), Some(case.pattern), Some(case.escape))
                    .unwrap(),
                Some(case.general_match),
                "input={:?}, pattern={:?}, escape={}, collation={collation}",
                case.input,
                case.pattern,
                case.escape,
            );
        }
    }
    // unicode_ci / bin：期望 unicode_match。
    for collation in [
        "utf8mb4_bin",
        "utf8mb4_unicode_ci",
        "utf8_bin",
        "utf8_unicode_ci",
    ] {
        let signature = IlikeSig::new(collation, false, false);
        for case in CASES {
            assert_eq!(
                signature
                    .eval_int(Some(case.input), Some(case.pattern), Some(case.escape))
                    .unwrap(),
                Some(case.unicode_match),
                "input={:?}, pattern={:?}, escape={}, collation={collation}",
                case.input,
                case.pattern,
                case.escape,
            );
        }
    }
}

/// 向量路径结果应与逐行标量 `eval_int` 一致（binary 排序规则）。
#[test]
fn test_vectorized_builtin_ilike_func() {
    let inputs = [
        "aaa",
        "abc",
        "aAa",
        "AaA",
        "a啊啊Aa啊",
        "啊啊啊啊",
        "üÜ",
        "Ü",
        "a",
        "A",
    ];
    let patterns = [
        "aaa",
        "ABC",
        "啊啊啊啊",
        "üÜ",
        "ü",
        "a",
        "A",
        "aaa",
        "ABC",
        "啊啊啊啊",
    ];
    // 覆盖大写/小写 escape 字母与反斜杠三种逃逸字符。
    for escape in ['A' as i64, 'a' as i64, '\\' as i64] {
        let signature = IlikeSig::new("binary", false, true);
        let scalar = inputs
            .iter()
            .zip(patterns)
            .map(|(input, pattern)| {
                signature
                    .eval_int(Some(input), Some(pattern), Some(escape))
                    .unwrap()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            signature
                .vec_eval_int(
                    &StringParam::Column(
                        inputs
                            .iter()
                            .map(|value| Some((*value).to_owned()))
                            .collect()
                    ),
                    &StringParam::Column(
                        patterns
                            .iter()
                            .map(|value| Some((*value).to_owned()))
                            .collect()
                    ),
                    EscapeParam::Constant(Some(escape)),
                    inputs.len(),
                )
                .unwrap(),
            scalar,
        );
    }
}

/// 常量 pattern / 常量表达式两条向量路径的期望结果。
#[test]
fn test_vectorized_builtin_ilike_for_constants() {
    let signature = IlikeSig::new("utf8mb4_general_ci", true, true);
    // 列表达式 × 常量 pattern。
    assert_eq!(
        signature
            .vec_eval_int(
                &StringParam::Column(
                    ["a", "A", "aa", "bb"]
                        .into_iter()
                        .map(|value| Some(value.to_owned()))
                        .collect()
                ),
                &StringParam::Constant(Some("A".to_owned())),
                EscapeParam::Constant(Some('\\' as i64)),
                4,
            )
            .unwrap(),
        vec![Some(1), Some(1), Some(0), Some(0)],
    );

    let signature = IlikeSig::new("utf8mb4_general_ci", true, true);
    // 常量表达式 × 列 pattern。
    assert_eq!(
        signature
            .vec_eval_int(
                &StringParam::Constant(Some("Aa".to_owned())),
                &StringParam::Column(
                    ["A", "AA", "B", "%a%"]
                        .into_iter()
                        .map(|value| Some(value.to_owned()))
                        .collect()
                ),
                EscapeParam::Constant(Some('\\' as i64)),
                4,
            )
            .unwrap(),
        vec![Some(0), Some(1), Some(0), Some(1)],
    );
}

#[test]
fn canonical_ilike_factory_preserves_escape_null_and_clone() {
    use crate::Expression;
    let context = exprstatic::NewExprContext(vec![]);
    for (value, pattern, escape, expected) in [
        (Some("foo"), Some("%FOO%"), Some(b'\\'), Some(1)),
        (Some("foo%"), Some("%FOO#%%"), Some(b'#'), Some(1)),
        (Some("fooX"), Some("%FOO#%%"), Some(b'#'), Some(0)),
        (Some("abc_def"), Some("%A_%"), Some(b'A'), Some(1)),
        (None, Some("%FOO%"), Some(b'\\'), None),
        (Some("foo"), None, Some(b'\\'), None),
        (Some("foo"), Some("%FOO%"), None, None),
    ] {
        let string_argument = |value: Option<&str>| -> Box<dyn Expression> {
            value.map_or_else(
                || Box::new(crate::NewNull()) as Box<dyn Expression>,
                |value| Box::new(crate::NewStrConst(value)),
            )
        };
        let escape: Box<dyn Expression> = escape.map_or_else(
            || Box::new(crate::NewNull()) as Box<dyn Expression>,
            |escape| Box::new(crate::NewInt64Const(i64::from(escape))),
        );
        let expression = crate::NewFunctionBase(
            &context,
            "ilike",
            *crate::types::NewFieldType(crate::mysql::TypeLonglong),
            vec![string_argument(value), string_argument(pattern), escape],
        )
        .unwrap();
        let function = expression
            .as_any()
            .downcast_ref::<crate::ScalarFunction>()
            .unwrap();
        assert_eq!(
            function.Function.PbCode(),
            tipb::ScalarFuncSig::IlikeSig as i32
        );
        for expression in [expression.CloneExpr(), expression] {
            let (value, null) = expression
                .EvalInt(context.GetEvalCtx(), crate::chunk::Row::default())
                .unwrap();
            assert_eq!(if null { None } else { Some(value) }, expected);
        }
    }
}

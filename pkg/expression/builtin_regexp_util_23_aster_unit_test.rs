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

// 正则表达式内建工具与 Go 对齐的单元测试。
//
// 覆盖 match_type、上下文级正则缓存、REGEXP_LIKE/SUBSTR/INSTR/REPLACE
// 的标量与向量路径，以及 FIELD 向量辅助函数的 NULL/首匹配规则。
// 正则缓存（memorize）仅在常量参数时可复用编译结果，避免每行重复编译。

use std::collections::HashMap;
use std::sync::Arc;

use crate::builtin_regexp_kernel::*;
use crate::builtin_regexp_util_kernel::*;
use crate::builtin_registry_kernel::registered_builtin_function_names;
use crate::builtin_string_vec_generated_kernel::{field_int, field_real, field_string};

#[test]
/// 验证 match_type 解析：最右大小写标志与排序规则默认值对齐 Go。
fn match_type_rightmost_case_flag_and_collation_default_match_go() {
    assert_eq!(get_regexp_match_type("", true).unwrap(), "i");
    assert_eq!(get_regexp_match_type("ic", true).unwrap(), "");
    assert_eq!(get_regexp_match_type("ci", false).unwrap(), "i");
    assert_eq!(get_regexp_match_type("sm", false).unwrap(), "ms");
    assert_eq!(
        get_regexp_match_type("x", false).unwrap_err(),
        RegexpError::InvalidMatchType
    );
}

#[test]
/// 验证正则缓存按上下文隔离，且仅对常量参数生效。
fn regexp_cache_is_context_scoped_and_only_used_for_constant_arguments() {
    let cached = RegexpBase::new(true, true, false);
    let first = cached.get_regexp(7, "a+", "").unwrap();
    let again = cached.get_regexp(7, "ignored-after-cache", "c").unwrap();
    assert!(Arc::ptr_eq(&first, &again));
    let other_context = cached.get_regexp(8, "b+", "").unwrap();
    assert!(!Arc::ptr_eq(&first, &other_context));
    assert_eq!(cached.cache_len(), 2);

    let dynamic = RegexpBase::new(false, true, false);
    assert!(!dynamic.can_memorize_regexp(true));
    dynamic.get_regexp(7, "a+", "").unwrap();
    assert_eq!(dynamic.cache_len(), 0);

    let failed = RegexpBase::new(true, true, false);
    let first_error = failed.get_regexp(9, "(", "").unwrap_err();
    let cached_error = failed.get_regexp(9, "valid", "").unwrap_err();
    assert_eq!(first_error, cached_error);
    assert_eq!(failed.cache_len(), 1);

    let no_match_type_argument = RegexpBase::new(true, false, false);
    let (_, memorized) = no_match_type_argument
        .try_vec_memorized_regexp(11, "a", "", false, 2)
        .unwrap();
    assert!(memorized);
    assert_eq!(no_match_type_argument.cache_len(), 1);
}

#[test]
/// 验证 REGEXP_LIKE 标量/向量路径正确传播 NULL 与错误。
fn regexp_like_scalar_and_vector_paths_propagate_nulls_and_errors() {
    let engine = RegexpEngine::new(false);
    assert_eq!(engine.regexp_like("AbC", "^abc$", "i").unwrap(), 1);
    assert_eq!(engine.regexp_like("AbC", "^abc$", "c").unwrap(), 0);
    assert_eq!(
        engine
            .regexp_like_vec(&[
                (Some("abc"), Some("a.c"), Some("")),
                (None, Some("a"), Some("")),
                (Some("ABC"), Some("abc"), Some("i")),
            ])
            .unwrap(),
        vec![Some(1), None, Some(1)]
    );
    assert_eq!(
        engine.regexp_like("abc", "", "").unwrap_err(),
        RegexpError::EmptyPattern
    );
}

#[test]
/// 验证 SUBSTR/INSTR 按 Unicode 字符计数，与 Go 一致。
fn substr_and_instr_count_unicode_characters_like_go() {
    let engine = RegexpEngine::new(false);
    assert_eq!(
        engine.regexp_substr("甲a乙a", "a", 2, 2, "").unwrap(),
        Some("a".to_owned())
    );
    assert_eq!(engine.regexp_substr("abc", "z", 1, 1, "").unwrap(), None);
    assert_eq!(engine.regexp_substr("", ".", 1, 1, "").unwrap(), None);
    assert_eq!(
        engine.regexp_substr("abc", ".", 4, 1, "").unwrap_err(),
        RegexpError::InvalidIndex
    );

    assert_eq!(engine.regexp_instr("甲a乙a", "a", 1, 2, 0, "").unwrap(), 4);
    assert_eq!(engine.regexp_instr("甲a乙a", "a", 1, 1, 1, "").unwrap(), 3);
    assert_eq!(engine.regexp_instr("abc", "z", 1, 1, 0, "").unwrap(), 0);
    assert_eq!(
        engine.regexp_instr("abc", "a", 1, 1, 2, "").unwrap_err(),
        RegexpError::InvalidReturnOption
    );
}

#[test]
/// 验证替换指令解析与 occurrence 语义对齐 Go。
fn replacement_instructions_and_occurrence_match_go() {
    assert_eq!(
        get_instructions(br"x\1-\\-\9\"),
        vec![
            Instruction::literal(b"x"),
            Instruction::substitution(1),
            Instruction::literal(b"-\\-"),
            Instruction::substitution(9),
        ]
    );

    let engine = RegexpEngine::new(false);
    assert_eq!(
        engine
            .regexp_replace("abc-123 def-456", r"([a-z]+)-(\d+)", r"\2/\1", 1, 0, "")
            .unwrap(),
        "123/abc 456/def"
    );
    assert_eq!(
        engine
            .regexp_replace("a1 a2 a3", r"a\d", "x", 1, 2, "")
            .unwrap(),
        "a1 x a3"
    );
    assert_eq!(
        engine.regexp_replace("甲a乙a", "a", "X", 3, 0, "").unwrap(),
        "甲a乙X"
    );
    assert_eq!(
        engine
            .regexp_replace("abc", "(a)", r"\2", 1, 0, "")
            .unwrap_err(),
        RegexpError::InvalidSubstitution
    );
}

#[test]
/// 验证列 NULL 判定与 FIELD 向量「首匹配胜出」规则。
fn utility_columns_and_field_vectors_preserve_null_and_first_match_rules() {
    let columns = vec![
        Column::new(vec![Some("a".to_owned()), None]),
        Column::new(vec![Some("b".to_owned()), Some("c".to_owned())]),
    ];
    assert!(!is_result_null(&columns, 0));
    assert!(is_result_null(&columns, 1));
    let mut result = Column::default();
    fill_null_string_into_result(&mut result, 3);
    assert_eq!(result.values(), &[None, None, None]);
    assert!(!check_out_range_pos(0, 1));
    assert!(check_out_range_pos(0, 0));
    assert!(check_out_range_pos(2, 1));

    assert_eq!(
        field_int(
            &[Some(2), Some(3), None],
            &[
                vec![Some(1), Some(3), Some(0)],
                vec![Some(2), Some(3), Some(0)],
            ],
        )
        .unwrap(),
        vec![2, 1, 0]
    );
    assert_eq!(
        field_real(
            &[Some(1.5), Some(f64::NAN)],
            &[vec![Some(1.5), Some(f64::NAN)]]
        )
        .unwrap(),
        vec![1, 0]
    );
    assert_eq!(
        field_string(
            &[Some("A"), Some("x"), None],
            &[vec![Some("a"), Some("y"), Some("z")]],
            |left, right| left.eq_ignore_ascii_case(right),
        )
        .unwrap(),
        vec![1, 0, 0]
    );
}

#[test]
/// 验证注册表快照有序、独立拥有，且不修改源 map。
fn registry_snapshot_is_sorted_owned_and_does_not_mutate_source() {
    let mut funcs = HashMap::new();
    funcs.insert("zeta".to_owned(), 1_u8);
    funcs.insert("alpha".to_owned(), 2_u8);
    let mut snapshot = registered_builtin_function_names(&funcs);
    assert_eq!(snapshot, ["alpha", "zeta"]);
    snapshot[0].push('!');
    assert!(funcs.contains_key("alpha"));
}

#[test]
/// 验证向量与二进制辅助路径与标量 Go 分支一致。
fn regexp_vector_and_binary_helper_paths_match_scalar_go_branches() {
    let engine = RegexpEngine::new(false);
    assert_eq!(
        engine
            .regexp_substr_vec(&[
                Some(RegexpSubstrArgs {
                    expression: "a1 a2",
                    pattern: r"a\d",
                    position: 1,
                    occurrence: 2,
                    match_type: "",
                }),
                None,
            ])
            .unwrap(),
        vec![Some("a2".to_owned()), None]
    );
    assert_eq!(
        engine
            .regexp_instr_vec(&[Some(RegexpInstrArgs {
                expression: "甲a乙",
                pattern: "a",
                position: 1,
                occurrence: 1,
                return_option: 0,
                match_type: "",
            })])
            .unwrap(),
        vec![Some(2)]
    );
    assert_eq!(
        engine
            .regexp_replace_vec(&[Some(RegexpReplaceArgs {
                expression: "a1 a2",
                pattern: r"a\d",
                replacement: "x",
                position: 1,
                occurrence: 0,
                match_type: "",
            })])
            .unwrap(),
        vec![Some("x x".to_owned())]
    );
    assert_eq!(
        engine.regexp_replace("ab", "^", "X", 1, 0, "").unwrap(),
        "Xab"
    );

    let binary = RegexpEngine::new_binary();
    assert_eq!(
        binary
            .regexp_substr_binary(b"\xffa", "a", 1, 1, "")
            .unwrap(),
        Some("0x61".to_owned())
    );
    assert_eq!(
        binary
            .regexp_instr_binary(b"xxa", "a", 2, 1, 0, "")
            .unwrap(),
        3
    );
    assert_eq!(
        binary
            .regexp_replace_binary(b"ab12", r"\d", b"X", 1, 0, "")
            .unwrap(),
        "0x61625858"
    );
}

/// 供正则 Go 同名迁移入口复用的完整回归集合。
pub(crate) fn run_regexp_parity_suite() {
    match_type_rightmost_case_flag_and_collation_default_match_go();
    regexp_cache_is_context_scoped_and_only_used_for_constant_arguments();
    regexp_like_scalar_and_vector_paths_propagate_nulls_and_errors();
    substr_and_instr_count_unicode_characters_like_go();
    replacement_instructions_and_occurrence_match_go();
    utility_columns_and_field_vectors_preserve_null_and_first_match_rules();
    registry_snapshot_is_sorted_owned_and_does_not_mutate_source();
    regexp_vector_and_binary_helper_paths_match_scalar_go_branches();
}

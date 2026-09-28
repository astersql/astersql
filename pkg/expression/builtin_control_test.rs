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

// 控制流内核（CASE / IF / IFNULL / 类型推导）的单元测试。

use crate::builtin_control_kernel::*;

/// 构造整型 SqlValue 测试夹具。
fn int(value: i64) -> SqlValue {
    SqlValue::Int(value)
}

/// CASE：首个匹配、NULL ELSE、条件错误与未达错误语义。
#[test]
fn case_when_preserves_first_match_null_else_and_error_semantics() {
    let cases = [
        (vec![int(1), int(1), int(1), int(2), int(3)], int(1)),
        (vec![int(0), int(1), int(1), int(2), int(3)], int(2)),
        (vec![SqlValue::Null, int(1), int(0), int(2), int(3)], int(3)),
        (vec![int(0), int(1)], SqlValue::Null),
    ];
    for (arguments, expected) in cases {
        assert_eq!(eval_case_when(&arguments).unwrap(), expected);
    }
    assert_eq!(
        eval_case_when(&[int(1), int(7), SqlValue::Error("unreached".into()), int(8)]).unwrap(),
        int(7)
    );
    assert!(eval_case_when(&[SqlValue::Error("condition".into()), int(1)]).is_err());
}

/// IF/IFNULL：真值转换、短路径与 NULL 回退。
#[test]
fn if_and_ifnull_cover_truth_conversion_short_circuit_and_null() {
    for (condition, expected) in [
        (int(1), int(1)),
        (int(0), int(2)),
        (SqlValue::Null, int(2)),
        (SqlValue::String("abc".into()), int(2)),
        (SqlValue::String("1abc".into()), int(1)),
        (SqlValue::String("1e".into()), int(1)),
        (SqlValue::String("1e+".into()), int(1)),
        (SqlValue::Decimal("0.0".into()), int(2)),
        (SqlValue::Duration(1), int(1)),
    ] {
        assert_eq!(eval_if(&condition, &int(1), &int(2)).unwrap(), expected);
    }
    assert_eq!(
        eval_if(&int(1), &int(9), &SqlValue::Error("unreached".into())).unwrap(),
        int(9)
    );
    assert!(eval_if(&SqlValue::Error("condition".into()), &int(1), &int(2)).is_err());
    assert_eq!(
        eval_if_null(&int(1), &SqlValue::Error("unreached".into())).unwrap(),
        int(1)
    );
    assert_eq!(eval_if_null(&SqlValue::Null, &int(2)).unwrap(), int(2));
    assert_eq!(
        eval_if_null(&SqlValue::Null, &SqlValue::Null).unwrap(),
        SqlValue::Null
    );
}

/// 类型推导：宽度、标志与字符串提升；空参数应报错。
#[test]
fn control_type_inference_keeps_width_flags_and_string_fixups() {
    let signed = FieldType::new(FieldKind::Longlong)
        .with_flen(20)
        .with_decimal(0)
        .with_flags(NOT_NULL_FLAG);
    let unsigned = FieldType::new(FieldKind::Longlong)
        .with_flen(20)
        .with_decimal(0)
        .with_flags(NOT_NULL_FLAG | UNSIGNED_FLAG);
    let null = FieldType::new(FieldKind::Null).with_flags(NOT_NULL_FLAG);
    let integer = infer_type_for_control("if", &[signed.clone(), unsigned, null]).unwrap();
    assert_eq!(integer.kind, FieldKind::Longlong);
    assert_eq!(integer.flags & NOT_NULL_FLAG, 0);
    assert!(integer.flen >= 20);

    let enum_string = FieldType::new(FieldKind::Enum)
        .with_flen(12)
        .with_charset("utf8mb4", "utf8mb4_bin");
    let varchar = FieldType::new(FieldKind::Varchar)
        .with_flen(8)
        .with_charset("utf8mb4", "utf8mb4_general_ci");
    let inferred = infer_type_for_control("case", &[enum_string, varchar]).unwrap();
    assert_eq!(inferred.kind, FieldKind::Varchar);
    assert_eq!(inferred.flen, 12);

    // 无参数无法推导控制函数结果类型。
    assert!(infer_type_for_control("if", &[]).is_err());
}

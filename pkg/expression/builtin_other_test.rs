// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// `builtin_other` 标量语义的单元测试。
//
// 覆盖 BIT_COUNT、IN 谓词（含 NULL 三值逻辑、无符号比较与 collation）、
// 用户变量读写、VALUES()/GET_PARAM() 以及 ROW() 的标量契约，
// 与 Go `builtin_other_test.go` 行为对齐。

use crate::expression_other::*;
use serde_json::json;

/// 验证 BIT_COUNT：有符号/无符号位计数、非法字符串转 0、NULL 传播。
#[test]
fn bit_count_covers_signed_unsigned_conversion_and_null() {
    // 用例覆盖正数、全 1 的 -1、无符号最大值、不可解析字符串与 NULL。
    let cases = [
        (Value::Int(8), Some(1)),
        (Value::Int(29), Some(4)),
        (Value::Int(-1), Some(64)),
        (Value::UInt(u64::MAX), Some(64)),
        (Value::String("xxx".into()), Some(0)),
        (Value::Null, None),
    ];
    for (value, expected) in cases {
        assert_eq!(bit_count(&value).unwrap(), expected);
    }
}

/// 验证 IN：命中、仅含 NULL 时返回 NULL、有符号与无符号不相等、ci collation 折叠。
#[test]
fn in_predicate_preserves_match_null_unsigned_and_collation_rules() {
    // 命中列表中的值时应为 true（即使列表含 NULL）。
    assert_eq!(
        eval_in(
            FieldType::new(EvalType::Int),
            Value::Int(2),
            vec![Value::Int(1), Value::Int(2), Value::Null],
        )
        .unwrap(),
        Some(true)
    );
    // 未命中且列表含 NULL：SQL 三值逻辑下结果为 NULL。
    assert_eq!(
        eval_in(
            FieldType::new(EvalType::Int),
            Value::Int(9),
            vec![Value::Int(1), Value::Null],
        )
        .unwrap(),
        None
    );
    // -1 与 u64::MAX 位型不同，按 Go 规则不相等。
    assert_eq!(
        eval_in(
            FieldType::new(EvalType::Int),
            Value::Int(-1),
            vec![Value::UInt(u64::MAX)],
        )
        .unwrap(),
        Some(false)
    );
    // utf8_general_ci 下 "a" 与 "Á" 视为相等。
    assert_eq!(
        eval_in(
            FieldType::string("utf8_general_ci"),
            Value::String("a".into()),
            vec![Value::String("Á".into())],
        )
        .unwrap(),
        Some(true)
    );
}

/// Go `CompareBinaryJSON` compares integer and floating JSON numbers by value.
#[test]
fn in_json_compares_cross_representation_numbers_like_go() {
    assert_eq!(
        eval_in(
            FieldType::new(EvalType::Json),
            Value::Json(json!(1)),
            vec![Value::Json(json!(1.0))],
        )
        .unwrap(),
        Some(true)
    );
    assert_eq!(
        eval_in(
            FieldType::new(EvalType::Json),
            Value::Json(json!([1, {"value": 2}])),
            vec![Value::Json(json!([1.0, {"value": 2.0}]))],
        )
        .unwrap(),
        Some(true)
    );
}

/// 覆盖用户变量大小写不敏感、类型转换、VALUES 偏移错误与 GET_PARAM 越界。
#[test]
fn user_variables_values_and_parameters_cover_state_and_errors() {
    let mut session = Session::default();
    // 名称大小写折叠后仍能取回，并按目标 EvalType 转换。
    assert_eq!(
        session.set_user_var("MiXeD", Value::Int(3)).unwrap(),
        Value::Int(3)
    );
    assert_eq!(
        session.get_user_var("mixed", EvalType::Real).unwrap(),
        Some(Value::Real(3.0))
    );
    assert_eq!(
        session.get_user_var("missing", EvalType::String).unwrap(),
        None
    );
    // 设为 NULL 后仍保留变量名；读取时回落到上一次非 NULL 语义由内核实现决定。
    session.set_user_var("mixed", Value::Null).unwrap();
    assert!(session.contains_user_var("mixed"));
    assert_eq!(
        session.get_user_var("mixed", EvalType::Int).unwrap(),
        Some(Value::Int(3))
    );

    // VALUES(offset)：INSERT 行值不足时报错，足够时按偏移取值。
    let values = ValuesFunction::new(1, FieldType::string("binary"));
    assert_eq!(values.eval(&session).unwrap(), Value::Null);
    session.curr_insert_values = vec![Value::String("1".into())];
    assert!(values.eval(&session).is_err());
    session.curr_insert_values.push(Value::String("2".into()));
    assert_eq!(values.eval(&session).unwrap(), Value::String("2".into()));

    // GET_PARAM：计划缓存参数转字符串，越界与负索引报错。
    session.plan_cache_params = vec![Value::Int(123), Value::String("abc".into())];
    assert_eq!(get_param(&session, 0).unwrap(), Some("123".into()));
    assert_eq!(get_param(&session, 1).unwrap(), Some("abc".into()));
    assert!(get_param(&session, 2).is_err());
    assert!(get_param(&session, -1).is_err());
}

/// ROW() 可构建参数类型元数据，但标量求值应按 Go 契约 panic。
#[test]
fn row_function_builds_metadata_but_rejects_scalar_evaluation() {
    let row = RowFunction::new(vec![
        FieldType::string("binary"),
        FieldType::new(EvalType::Int),
    ]);
    assert_eq!(row.argument_types().len(), 2);
    assert!(std::panic::catch_unwind(|| row.eval()).is_err());
}

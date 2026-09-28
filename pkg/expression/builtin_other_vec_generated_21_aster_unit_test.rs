// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// 生成版向量化 IN 与其它 `builtin_other` 向量路径的 Aster 对齐测试。
//
// 覆盖各类型 `BuiltinIn*Sig` 的哈希/动态比较与 NULL 语义、BIT_COUNT 溢出回退
// 到逐行求值、GET_PARAM/用户变量 SET/GET 向量路径，以及 VALUES/ROW 的向量化契约。

use std::panic::{AssertUnwindSafe, catch_unwind};

use crate::builtin_other_vec_generated_kernel::{
    BuiltinInDecimalSig, BuiltinInDurationSig, BuiltinInIntSig, BuiltinInJsonSig, BuiltinInRealSig,
    BuiltinInStringSig, BuiltinInTimeSig,
};
use crate::builtin_other_vec_kernel::{
    BuiltinBitCountSig, BuiltinGetDecimalVarSig, BuiltinGetIntVarSig, BuiltinGetParamStringSig,
    BuiltinGetRealVarSig, BuiltinGetStringVarSig, BuiltinRowSig, BuiltinSetDecimalVarSig,
    BuiltinSetIntVarSig, BuiltinSetRealVarSig, BuiltinSetStringVarSig, BuiltinValuesDecimalSig,
    BuiltinValuesDurationSig, BuiltinValuesIntSig, BuiltinValuesJsonSig, BuiltinValuesRealSig,
    BuiltinValuesStringSig, BuiltinValuesTimeSig, ColumnExpression, EvalContext, EvalError,
    EvalResult, VectorExpression,
};
use crate::{chunk, types};

/// 通用定长列构造：按 Option 追加值或 NULL。
fn fixed_column<T>(
    values: &[Option<T>],
    mut append: impl FnMut(&mut chunk::Column, &T),
) -> chunk::Column {
    let mut column = *chunk::newFixedLenColumn(std::mem::size_of::<T>(), values.len());
    for value in values {
        if let Some(value) = value {
            append(&mut column, value);
        } else {
            column.AppendNull();
        }
    }
    column
}

/// 构造可空 i64 定长列。
fn int_column(values: &[Option<i64>]) -> chunk::Column {
    fixed_column(values, |column, value| column.AppendInt64(*value))
}

/// 构造可空 f64 定长列。
fn real_column(values: &[Option<f64>]) -> chunk::Column {
    fixed_column(values, |column, value| column.AppendFloat64(*value))
}

/// 构造可空 MyDecimal 定长列。
fn decimal_column(values: &[Option<types::MyDecimal>]) -> chunk::Column {
    let mut column = *chunk::newFixedLenColumn(chunk::sizeMyDecimal, values.len());
    for value in values {
        if let Some(value) = value {
            column.AppendMyDecimal(value);
        } else {
            column.AppendNull();
        }
    }
    column
}

/// 构造可空 Time 定长列。
fn time_column(values: &[Option<types::Time>]) -> chunk::Column {
    let mut column = *chunk::newFixedLenColumn(chunk::sizeTime, values.len());
    for value in values {
        if let Some(value) = value {
            column.AppendTime(*value);
        } else {
            column.AppendNull();
        }
    }
    column
}

/// 构造可空变长字符串列。
fn string_column(values: &[Option<&str>]) -> chunk::Column {
    let mut column = *chunk::newVarLenColumn(values.len());
    for value in values {
        if let Some(value) = value {
            column.AppendString(value);
        } else {
            column.AppendNull();
        }
    }
    column
}

/// 构造可空 JSON 变长列。
fn json_column(values: &[Option<types::BinaryJSON>]) -> chunk::Column {
    let mut column = *chunk::newVarLenColumn(values.len());
    for value in values {
        if let Some(value) = value {
            column.AppendJSON(value.clone());
        } else {
            column.AppendNull();
        }
    }
    column
}

/// 将列列表包装为输入 Chunk。
fn input(columns: Vec<chunk::Column>) -> chunk::Chunk {
    let mut chunk = chunk::Chunk::default();
    chunk.columns = columns;
    chunk
}

/// 构造引用输入第 `index` 列的列表达式。
fn column(index: usize) -> Box<dyn VectorExpression> {
    Box::new(ColumnExpression::new(index))
}

/// 读出可空整数结果向量。
fn int_values(column: &chunk::Column) -> Vec<Option<i64>> {
    (0..column.length)
        .map(|row| (!column.IsNull(row)).then(|| column.GetInt64(row)))
        .collect()
}

/// 读出可空字符串结果向量。
fn string_values(column: &chunk::Column) -> Vec<Option<String>> {
    (0..column.length)
        .map(|row| (!column.IsNull(row)).then(|| column.GetString(row)))
        .collect()
}

/// 从文本解析 MyDecimal。
fn decimal(text: &str) -> types::MyDecimal {
    let mut value = types::MyDecimal::default();
    value.FromString(text.as_bytes()).unwrap();
    value
}

/// 构造固定日期下不同秒数的 Time。
fn time(second: i32) -> types::Time {
    types::Time {
        coreTime: types::FromDate(2019, 11, 2, 22, 0, second, 0),
    }
}

#[test]
/// 各类型 IN 签名：哈希命中、动态比较、有符号区分、collation 与 NULL。
fn in_signatures_preserve_go_hash_comparison_and_null_semantics() {
    let context = EvalContext::new(Vec::new());
    let mut result = chunk::Column::default();

    let int_input = input(vec![
        int_column(&[Some(1), Some(-1), None, Some(7)]),
        int_column(&[Some(0), Some(-1), Some(1), None]),
    ]);
    let int_sig = BuiltinInIntSig::new(vec![column(0), column(1)], [(1, false)], false, vec![1]);
    assert!(int_sig.vectorized());
    int_sig
        .vec_eval_int(&context, &int_input, &mut result)
        .unwrap();
    assert_eq!(int_values(&result), vec![Some(1), Some(1), None, None]);

    let signed_input = input(vec![int_column(&[Some(-1)])]);
    let signed_sig = BuiltinInIntSig::new(
        vec![Box::new(ColumnExpression::new(0))],
        [(-1, true)],
        false,
        Vec::new(),
    );
    signed_sig
        .vec_eval_int(&context, &signed_input, &mut result)
        .unwrap();
    assert_eq!(int_values(&result), vec![Some(0)]);

    let string_input = input(vec![
        string_column(&[Some("A"), Some("x"), None]),
        string_column(&[Some("z"), Some("X"), Some("a")]),
    ]);
    let string_sig = BuiltinInStringSig::new(
        vec![column(0), column(1)],
        ["a"],
        false,
        vec![1],
        "utf8mb4_general_ci",
    );
    assert!(string_sig.vectorized());
    string_sig
        .vec_eval_int(&context, &string_input, &mut result)
        .unwrap();
    assert_eq!(int_values(&result), vec![Some(1), Some(1), None]);

    let decimal_input = input(vec![
        decimal_column(&[Some(decimal("1.0")), Some(decimal("2")), None]),
        decimal_column(&[
            Some(decimal("1.000")),
            Some(decimal("3")),
            Some(decimal("2")),
        ]),
    ]);
    let decimal_sig =
        BuiltinInDecimalSig::new(vec![column(0), column(1)], [decimal("1")], false, vec![1]);
    assert!(decimal_sig.vectorized());
    decimal_sig
        .vec_eval_int(&context, &decimal_input, &mut result)
        .unwrap();
    assert_eq!(int_values(&result), vec![Some(1), Some(0), None]);

    let real_input = input(vec![
        real_column(&[Some(0.1), Some(2.0), Some(f64::NAN)]),
        real_column(&[Some(0.0), Some(2.0), Some(f64::NAN)]),
    ]);
    let real_sig = BuiltinInRealSig::new(vec![column(0), column(1)], [0.1], false, vec![1]);
    assert!(real_sig.vectorized());
    real_sig
        .vec_eval_int(&context, &real_input, &mut result)
        .unwrap();
    assert_eq!(int_values(&result), vec![Some(1), Some(1), Some(1)]);

    let first_time = time(1);
    let second_time = time(2);
    let time_input = input(vec![
        time_column(&[Some(first_time), Some(second_time), None]),
        time_column(&[Some(time(0)), Some(second_time), Some(first_time)]),
    ]);
    let time_sig = BuiltinInTimeSig::new(vec![column(0), column(1)], [first_time], false, vec![1]);
    assert!(time_sig.vectorized());
    time_sig
        .vec_eval_int(&context, &time_input, &mut result)
        .unwrap();
    assert_eq!(int_values(&result), vec![Some(1), Some(1), None]);

    let duration_input = input(vec![
        int_column(&[Some(1_000), Some(2_000), None]),
        int_column(&[Some(0), Some(2_000), Some(0)]),
    ]);
    let duration_sig =
        BuiltinInDurationSig::new(vec![column(0), column(1)], [1_000], false, vec![1]);
    assert!(duration_sig.vectorized());
    duration_sig
        .vec_eval_int(&context, &duration_input, &mut result)
        .unwrap();
    assert_eq!(int_values(&result), vec![Some(1), Some(1), None]);

    let one = types::CreateBinaryJSON(1_i64);
    let two = types::CreateBinaryJSON(2_i64);
    let text = types::CreateBinaryJSON("a".to_owned());
    let json_input = input(vec![
        json_column(&[Some(one.clone()), Some(text.clone()), None]),
        json_column(&[Some(two), Some(text), Some(one)]),
    ]);
    let json_sig = BuiltinInJsonSig::new(vec![column(0), column(1)]);
    assert!(json_sig.vectorized());
    json_sig
        .vec_eval_int(&context, &json_input, &mut result)
        .unwrap();
    assert_eq!(int_values(&result), vec![Some(0), Some(1), None]);
}

#[test]
/// Go 的常量参数包含 NULL 时：未命中为 NULL，命中仍为 1。
fn in_constant_null_is_overridden_only_by_a_match() {
    let context = EvalContext::new(Vec::new());
    let input = input(vec![int_column(&[Some(1), Some(2), None])]);
    let mut result = chunk::Column::default();
    let signature = BuiltinInIntSig::new(vec![column(0)], [(1, false)], true, Vec::new());

    signature
        .vec_eval_int(&context, &input, &mut result)
        .unwrap();
    assert_eq!(int_values(&result), vec![Some(1), None, None]);
}

/// 向量求值故意返回 Overflow，迫使 BIT_COUNT 走逐行回退路径。
struct OverflowExpression;

impl VectorExpression for OverflowExpression {
    fn vec_eval_int(
        &self,
        _context: &EvalContext,
        _input: &chunk::Chunk,
        _result: &mut chunk::Column,
    ) -> EvalResult<()> {
        // 向量路径报溢出，触发签名内逐行 eval_int_row 回退。
        Err(EvalError::Overflow)
    }

    fn eval_int_row(
        &self,
        _context: &EvalContext,
        input: &chunk::Chunk,
        row: usize,
    ) -> EvalResult<Option<i64>> {
        let physical = input.Sel().map_or(row, |selection| selection[row]);
        let column = input.Column(0);
        Ok((!column.IsNull(physical)).then(|| column.GetInt64(physical)))
    }
}

#[test]
/// 正常向量路径与 Overflow 回退后的 BIT_COUNT 结果一致。
fn bit_count_matches_go_and_falls_back_row_by_row_on_overflow() {
    let context = EvalContext::new(Vec::new());
    let input = input(vec![int_column(&[Some(8), Some(29), Some(-1), None])]);
    let mut result = chunk::Column::default();

    let normal = BuiltinBitCountSig::new(column(0));
    normal.vec_eval_int(&context, &input, &mut result).unwrap();
    assert_eq!(int_values(&result), vec![Some(1), Some(4), Some(64), None]);

    let fallback = BuiltinBitCountSig::new(Box::new(OverflowExpression));
    fallback
        .vec_eval_int(&context, &input, &mut result)
        .unwrap();
    assert_eq!(int_values(&result), vec![Some(1), Some(4), Some(64), None]);
}

#[test]
/// GET_PARAM 与各类型 SET/GET 用户变量的向量化路径。
fn parameter_and_user_variable_vector_paths_match_go() {
    let mut context = EvalContext::new(vec![
        types::NewIntDatum(123),
        types::NewStringDatum("abc".to_owned()),
    ]);
    let mut result = chunk::Column::default();

    let params = input(vec![int_column(&[Some(0), Some(1), None])]);
    BuiltinGetParamStringSig::new(column(0))
        .vec_eval_string(&context, &params, &mut result)
        .unwrap();
    assert_eq!(
        string_values(&result),
        vec![Some("123".to_owned()), Some("abc".to_owned()), None]
    );

    let invalid = input(vec![int_column(&[Some(2)])]);
    assert_eq!(
        BuiltinGetParamStringSig::new(column(0)).vec_eval_string(&context, &invalid, &mut result),
        Err(EvalError::ParamIndexExceeds)
    );

    let set_int_input = input(vec![
        string_column(&[Some("MiXeD"), None, Some("empty")]),
        int_column(&[Some(12), Some(13), None]),
    ]);
    BuiltinSetIntVarSig::new(column(0), column(1))
        .vec_eval_int(&mut context, &set_int_input, &mut result)
        .unwrap();
    assert_eq!(int_values(&result), vec![Some(12), None, None]);
    assert_eq!(context.user_var("mixed").unwrap().GetInt64(), 12);

    let set_string_input = input(vec![
        string_column(&[Some("Greeting")]),
        string_column(&[Some("Hello")]),
    ]);
    BuiltinSetStringVarSig::new(column(0), column(1))
        .vec_eval_string(&mut context, &set_string_input, &mut result)
        .unwrap();
    assert_eq!(string_values(&result), vec![Some("Hello".to_owned())]);

    let set_real_input = input(vec![
        string_column(&[Some("ratio")]),
        real_column(&[Some(2.5)]),
    ]);
    BuiltinSetRealVarSig::new(column(0), column(1))
        .vec_eval_real(&mut context, &set_real_input, &mut result)
        .unwrap();

    let five = decimal("5.00");
    let set_decimal_input = input(vec![
        string_column(&[Some("amount")]),
        decimal_column(&[Some(five.clone())]),
    ]);
    BuiltinSetDecimalVarSig::new(column(0), column(1))
        .vec_eval_decimal(&mut context, &set_decimal_input, &mut result)
        .unwrap();

    let names = input(vec![string_column(&[Some("MIXED"), Some("missing"), None])]);
    BuiltinGetIntVarSig::new(column(0))
        .vec_eval_int(&context, &names, &mut result)
        .unwrap();
    assert_eq!(int_values(&result), vec![Some(12), None, None]);

    let names = input(vec![string_column(&[
        Some("greeting"),
        Some("missing"),
        None,
    ])]);
    BuiltinGetStringVarSig::new(column(0))
        .vec_eval_string(&context, &names, &mut result)
        .unwrap();
    assert_eq!(
        string_values(&result),
        vec![Some("Hello".to_owned()), None, None]
    );

    let names = input(vec![string_column(&[Some("ratio"), Some("missing"), None])]);
    BuiltinGetRealVarSig::new(column(0))
        .vec_eval_real(&context, &names, &mut result)
        .unwrap();
    assert_eq!(result.GetFloat64(0), 2.5);
    assert!(result.IsNull(1));
    assert!(result.IsNull(2));

    let names = input(vec![string_column(&[
        Some("amount"),
        Some("missing"),
        None,
    ])]);
    BuiltinGetDecimalVarSig::new(column(0))
        .vec_eval_decimal(&context, &names, &mut result)
        .unwrap();
    assert_eq!(result.GetDecimal(0).Compare(&five), 0);
    assert!(result.IsNull(1));
    assert!(result.IsNull(2));
}

#[test]
/// VALUES 不可向量化；ROW 可向量化但 vec_eval_string 应 panic。
fn values_and_row_vectorization_contracts_match_go() {
    assert!(!BuiltinValuesIntSig.vectorized());
    assert!(!BuiltinValuesDurationSig.vectorized());
    assert!(!BuiltinValuesRealSig.vectorized());
    assert!(!BuiltinValuesStringSig.vectorized());
    assert!(!BuiltinValuesTimeSig.vectorized());
    assert!(!BuiltinValuesJsonSig.vectorized());
    assert!(!BuiltinValuesDecimalSig.vectorized());

    let context = EvalContext::new(Vec::new());
    let input = chunk::Chunk::default();
    let mut result = chunk::Column::default();
    assert!(matches!(
        BuiltinValuesIntSig.vec_eval_int(&context, &input, &mut result),
        Err(EvalError::NotImplemented)
    ));

    let row = BuiltinRowSig;
    assert!(row.vectorized());
    assert!(
        catch_unwind(AssertUnwindSafe(|| {
            let _ = row.vec_eval_string(&context, &input, &mut result);
        }))
        .is_err()
    );
}

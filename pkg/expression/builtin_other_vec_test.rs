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

// `builtin_other` 向量化内核（非生成 IN）的单元测试。
//
// 覆盖 BIT_COUNT 补码边界、GET_PARAM 字符串化与越界、
// VALUES 未实现向量化以及 ROW 声明可向量化等与 Go 一致的契约。

use crate::builtin_other_vec_kernel::{
    BuiltinBitCountSig, BuiltinGetParamStringSig, BuiltinRowSig, BuiltinValuesIntSig,
    BuiltinValuesStringSig, ColumnExpression, EvalContext, EvalError,
};
use crate::{chunk, types};

/// 由可空 i64 列表构造定长整数列。
fn int_column(values: &[Option<i64>]) -> chunk::Column {
    let mut column = *chunk::newFixedLenColumn(chunk::sizeInt64, values.len());
    for value in values {
        match value {
            Some(value) => column.AppendInt64(*value),
            None => column.AppendNull(),
        }
    }
    column
}

/// 将列包装为输入 Chunk。
fn input(columns: Vec<chunk::Column>) -> chunk::Chunk {
    let mut input = chunk::Chunk::default();
    input.columns = columns;
    input
}

/// 从结果列读出可空整数向量。
fn int_values(column: &chunk::Column) -> Vec<Option<i64>> {
    (0..column.length)
        .map(|row| (!column.IsNull(row)).then(|| column.GetInt64(row)))
        .collect()
}

/// 从结果列读出可空字符串向量。
fn string_values(column: &chunk::Column) -> Vec<Option<String>> {
    (0..column.length)
        .map(|row| (!column.IsNull(row)).then(|| column.GetString(row)))
        .collect()
}

/// 验证向量化 BIT_COUNT：正数位计数、-1 补码 64 位全 1、NULL 传播。
#[test]
fn vectorized_bit_count_preserves_null_and_twos_complement_edges() {
    let context = EvalContext::new(Vec::new());
    let input = input(vec![int_column(&[Some(8), Some(29), Some(-1), None])]);
    let mut result = chunk::Column::default();
    BuiltinBitCountSig::new(Box::new(ColumnExpression::new(0)))
        .vec_eval_int(&context, &input, &mut result)
        .unwrap();
    assert_eq!(int_values(&result), vec![Some(1), Some(4), Some(64), None]);
}

/// 验证 GET_PARAM：Datum 转字符串、索引 NULL、越界报 `ParamIndexExceeds`。
#[test]
fn vectorized_get_param_preserves_string_conversion_null_and_bounds() {
    // 计划缓存中的参数列表，索引 0/1 分别对应 int 与 string。
    let context = EvalContext::new(vec![
        types::NewIntDatum(123),
        types::NewStringDatum("abc".to_owned()),
    ]);
    let params = input(vec![int_column(&[Some(0), Some(1), None])]);
    let mut result = chunk::Column::default();
    BuiltinGetParamStringSig::new(Box::new(ColumnExpression::new(0)))
        .vec_eval_string(&context, &params, &mut result)
        .unwrap();
    assert_eq!(
        string_values(&result),
        vec![Some("123".into()), Some("abc".into()), None]
    );

    // 索引 2 超出参数个数。
    let invalid = input(vec![int_column(&[Some(2)])]);
    assert_eq!(
        BuiltinGetParamStringSig::new(Box::new(ColumnExpression::new(0))).vec_eval_string(
            &context,
            &invalid,
            &mut result
        ),
        Err(EvalError::ParamIndexExceeds)
    );
}

/// VALUES 标记不可向量化且求值返回 NotImplemented；ROW 标记可向量化。
#[test]
fn values_and_row_vectorization_report_the_go_contract() {
    assert!(!BuiltinValuesIntSig.vectorized());
    assert!(!BuiltinValuesStringSig.vectorized());
    let context = EvalContext::new(Vec::new());
    let input = chunk::Chunk::default();
    let mut result = chunk::Column::default();
    assert_eq!(
        BuiltinValuesIntSig.vec_eval_int(&context, &input, &mut result),
        Err(EvalError::NotImplemented)
    );
    assert!(BuiltinRowSig.vectorized());
}

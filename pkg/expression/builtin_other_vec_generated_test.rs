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

// 生成版向量化 IN（整数）签名的单元测试。
//
// 验证 `BuiltinInIntSig` 的常量哈希路径、动态参数比较与 NULL 三值逻辑，
// 以及有符号/无符号常量与探测值不相等的边界。

use crate::builtin_other_vec_generated_kernel::BuiltinInIntSig;
use crate::builtin_other_vec_kernel::{ColumnExpression, EvalContext};
use crate::chunk;

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

/// 从结果列读出可空整数向量，便于断言。
fn values(column: &chunk::Column) -> Vec<Option<i64>> {
    (0..column.length)
        .map(|row| (!column.IsNull(row)).then(|| column.GetInt64(row)))
        .collect()
}

/// 覆盖哈希命中、动态参数命中、未命中含 NULL、左值为 NULL 等路径。
#[test]
fn generated_int_in_signature_covers_hash_dynamic_and_null_paths() {
    let context = EvalContext::new(Vec::new());
    // 左列：探测值；右列：动态 IN 参数；hash 常量含有符号 1。
    let input = input(vec![
        int_column(&[Some(1), Some(2), Some(9), None]),
        int_column(&[Some(0), Some(2), None, Some(1)]),
    ]);
    let signature = BuiltinInIntSig::new(
        vec![
            Box::new(ColumnExpression::new(0)),
            Box::new(ColumnExpression::new(1)),
        ],
        [(1, false)],
        false,
        vec![1],
    );
    let mut result = chunk::Column::default();
    signature
        .vec_eval_int(&context, &input, &mut result)
        .unwrap();
    // 1 命中哈希；2 命中动态列；9 未命中且动态为 NULL → NULL；左 NULL → NULL。
    assert_eq!(values(&result), vec![Some(1), Some(1), None, None]);
    assert!(signature.vectorized());
}

/// 确认哈希中无符号 -1 不会与有符号探测值 -1/1 匹配。
#[test]
fn generated_int_in_signature_keeps_signed_unsigned_distinction() {
    let context = EvalContext::new(Vec::new());
    let input = input(vec![int_column(&[Some(-1), Some(1)])]);
    // hash_values 中 (-1, true) 表示无符号常量，与有符号左值不相等。
    let signature = BuiltinInIntSig::new(
        vec![Box::new(ColumnExpression::new(0))],
        [(-1, true)],
        false,
        Vec::new(),
    );
    let mut result = chunk::Column::default();
    signature
        .vec_eval_int(&context, &input, &mut result)
        .unwrap();
    assert_eq!(values(&result), vec![Some(0), Some(0)]);
}

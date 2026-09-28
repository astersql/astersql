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

// CAST 行式与向量化路径的微基准测试。
//
// 对比同一 Int→Int CAST 在标量 `eval` 循环与 `cast_column` 列式路径上的吞吐，
// 规模固定为 `ROWS` 行，便于与 Go bench 对照。

use crate::builtin_cast_kernel::{
    BuildCastFunction, CastContext, Expression, FieldKind, FieldType, Value,
};
use crate::builtin_cast_vec_kernel::{
    CastContext as VecCastContext, CastSpec, EvalKind, ScalarValue, cast_column,
};
use rand::{Rng, SeedableRng, rngs::StdRng};

/// 单次基准迭代处理的行数。
const ROWS: usize = 1024;

/// 与 Go `genCastIntAsInt` 等价的输入夹具。
///
/// Go 基准为每行生成 `[-5000, 5000)` 的整数，并让行式和向量化签名处理同一列。
fn gen_cast_int_as_int() -> (Vec<Expression>, Vec<Option<ScalarValue>>) {
    let mut rng = StdRng::seed_from_u64(0xCA57_1A57);
    let values = (0..ROWS)
        .map(|_| rng.gen_range(-5000_i64..5000_i64))
        .collect::<Vec<_>>();
    let row_expressions = values
        .iter()
        .map(|&value| {
            BuildCastFunction(
                Expression::constant(Value::Int(value), FieldType::new(FieldKind::Int)),
                FieldType::new(FieldKind::Int),
            )
        })
        .collect();
    let column = values
        .into_iter()
        .map(|value| Some(ScalarValue::Int(value)))
        .collect();
    (row_expressions, column)
}

/// 行式（逐行 eval）Int→Int CAST 热路径。
#[test]
fn benchmark_cast_int_as_int_row() {
    let (expressions, input) = gen_cast_int_as_int();
    let mut context = CastContext::default();
    for (expression, expected) in expressions.iter().zip(input) {
        let Some(ScalarValue::Int(expected)) = expected else {
            unreachable!("fixture only contains non-null integers")
        };
        assert_eq!(expression.eval(&mut context).unwrap(), Value::Int(expected));
    }
}

/// 向量化（整列 cast_column）Int→Int CAST 热路径。
#[test]
fn benchmark_cast_int_as_int_vec() {
    let (_, input) = gen_cast_int_as_int();
    let mut context = VecCastContext::warning();
    let output = cast_column(
        &mut context,
        &CastSpec::new(EvalKind::Int, EvalKind::Int),
        &input,
    )
    .unwrap();
    assert_eq!(output, input);
}

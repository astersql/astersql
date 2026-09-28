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

// 生成式向量比较与 COALESCE 的 Go 语义对齐单元测试。
//
// 覆盖各比较算子的 NULL 传播、`<=>`、NaN 序、多类型比较器，
// 以及 COALESCE 的向量填空、警告回滚与标量短路径回退。

use std::cell::Cell;

use crate::expression_compare_vec_generated::{
    BinaryJSON, CompareOp, EvalContext, EvalError, MyDecimal, Time, VectorExpression,
    bool_to_int64, vec_coalesce, vec_coalesce_time, vec_compare_decimal, vec_compare_duration,
    vec_compare_json, vec_compare_real, vec_compare_string, vec_compare_time, vectorized,
};
use types_decimal::mydecimal::NewDecFromStringForTest;
use types_json_functions::CreateBinaryJSON;
use types_time::{FromDate, NewTime, mysql};

/// 按算子与 `Ordering` 计算期望的 0/1 比较结果。
fn expected(op: CompareOp, ordering: std::cmp::Ordering) -> i64 {
    bool_to_int64(match op {
        CompareOp::Lt => ordering.is_lt(),
        CompareOp::Le => ordering.is_le(),
        CompareOp::Gt => ordering.is_gt(),
        CompareOp::Ge => ordering.is_ge(),
        CompareOp::Eq => ordering.is_eq(),
        CompareOp::Ne => ordering.is_ne(),
        CompareOp::NullEq => ordering.is_eq(),
    })
}

/// 普通比较算子应对三种序关系输出正确 0/1，并在任一侧 NULL 时传播 NULL。
#[test]
fn comparison_operators_propagate_nulls_for_every_ordering() {
    let left = [Some(1_i64), Some(2), Some(3), None];
    let right = [Some(2_i64), Some(2), Some(2), Some(4)];
    let orderings = [
        std::cmp::Ordering::Less,
        std::cmp::Ordering::Equal,
        std::cmp::Ordering::Greater,
    ];

    for op in [
        CompareOp::Lt,
        CompareOp::Le,
        CompareOp::Gt,
        CompareOp::Ge,
        CompareOp::Eq,
        CompareOp::Ne,
    ] {
        let got = vec_compare_duration(op, &left, &right).unwrap();
        assert_eq!(
            got,
            vec![
                Some(expected(op, orderings[0])),
                Some(expected(op, orderings[1])),
                Some(expected(op, orderings[2])),
                None,
            ]
        );
    }
    assert!(vectorized());
}

/// `<=>`（NULL-safe 相等）永不返回 SQL NULL。
#[test]
fn null_safe_equality_never_returns_sql_null() {
    let left = [None, None, Some(7_i64), Some(7), Some(8)];
    let right = [None, Some(7_i64), None, Some(7), Some(9)];
    assert_eq!(
        vec_compare_duration(CompareOp::NullEq, &left, &right).unwrap(),
        vec![Some(1), Some(0), Some(0), Some(1), Some(0)]
    );
}

/// 浮点比较应对齐 Go `cmp.Compare` 的 NaN 序：NaN==NaN，NaN < 非 NaN。
#[test]
fn real_comparison_matches_go_cmp_nan_ordering() {
    let nan = f64::NAN;
    let left = [Some(nan), Some(1.0), Some(nan), Some(-0.0)];
    let right = [Some(1.0), Some(nan), Some(nan), Some(0.0)];

    assert_eq!(
        vec_compare_real(CompareOp::Lt, &left, &right).unwrap(),
        vec![Some(1), Some(0), Some(0), Some(0)]
    );
    assert_eq!(
        vec_compare_real(CompareOp::Eq, &left, &right).unwrap(),
        vec![Some(0), Some(0), Some(1), Some(1)]
    );
}

/// DECIMAL / 字符串（校对）/ 时间 / JSON 应走各自源类型比较器。
#[test]
fn decimal_string_time_duration_and_json_use_source_comparators() {
    let decimals: Vec<Option<MyDecimal>> = vec![
        Some(NewDecFromStringForTest("1.00")),
        Some(NewDecFromStringForTest("-9.5")),
    ];
    let decimal_rhs = vec![
        Some(NewDecFromStringForTest("1")),
        Some(NewDecFromStringForTest("-10")),
    ];
    assert_eq!(
        vec_compare_decimal(CompareOp::Ge, &decimals, &decimal_rhs).unwrap(),
        vec![Some(1), Some(1)]
    );

    // general_ci：不区分大小写且尾部空格在部分校对下可等价。
    assert_eq!(
        vec_compare_string(
            CompareOp::Eq,
            &[Some("A ".to_owned()), Some("a".to_owned())],
            &[Some("A".to_owned()), Some("A".to_owned())],
            "utf8mb4_general_ci",
        )
        .unwrap(),
        vec![Some(1), Some(1)]
    );
    assert_eq!(
        vec_compare_string(
            CompareOp::Ne,
            &[Some("a".to_owned())],
            &[Some("A".to_owned())],
            "utf8mb4_bin",
        )
        .unwrap(),
        vec![Some(1)]
    );

    let early = NewTime(FromDate(2024, 1, 1, 0, 0, 0, 0), mysql::TypeDatetime, 0);
    let late = NewTime(FromDate(2024, 1, 1, 0, 0, 0, 1), mysql::TypeDatetime, 6);
    assert_eq!(
        vec_compare_time(CompareOp::Lt, &[Some(early)], &[Some(late)]).unwrap(),
        vec![Some(1)]
    );

    let json_left: Vec<Option<BinaryJSON>> = vec![
        Some(CreateBinaryJSON(1_i64.into()).unwrap()),
        Some(CreateBinaryJSON(vec![1_i64, 2_i64].into()).unwrap()),
    ];
    let json_right = vec![
        Some(CreateBinaryJSON(2_i64.into()).unwrap()),
        Some(CreateBinaryJSON(vec![1_i64, 3_i64].into()).unwrap()),
    ];
    assert_eq!(
        vec_compare_json(CompareOp::Lt, &json_left, &json_right).unwrap(),
        vec![Some(1), Some(1)]
    );
}

/// 列长不一致应拒绝，而非 panic。
#[test]
fn mismatched_columns_are_rejected_instead_of_truncated() {
    let error = vec_compare_duration(CompareOp::Eq, &[Some(1)], &[]).unwrap_err();
    assert_eq!(
        error,
        EvalError::ColumnLengthMismatch {
            expected: 1,
            actual: 0
        }
    );
}

/// 可脚本化的 mock 表达式：分别控制向量结果、标量结果与向量侧警告。
struct MockExpression<T> {
    vector: Result<Vec<Option<T>>, &'static str>,
    scalar: Vec<Result<Option<T>, &'static str>>,
    vector_warning: Option<&'static str>,
    vector_calls: Cell<usize>,
    scalar_calls: Cell<usize>,
}

impl<T> MockExpression<T> {
    /// 构造无警告、向量与标量均成功的 mock。
    fn values(vector: Vec<Option<T>>, scalar: Vec<Option<T>>) -> Self {
        Self {
            vector: Ok(vector),
            scalar: scalar.into_iter().map(Ok).collect(),
            vector_warning: None,
            vector_calls: Cell::new(0),
            scalar_calls: Cell::new(0),
        }
    }
}

impl<T: Clone> VectorExpression<T> for MockExpression<T> {
    fn vec_eval(
        &self,
        context: &mut EvalContext,
        _rows: usize,
    ) -> Result<Vec<Option<T>>, EvalError> {
        self.vector_calls.set(self.vector_calls.get() + 1);
        if let Some(warning) = self.vector_warning {
            context.append_warning(warning);
        }
        self.vector
            .as_ref()
            .map(Clone::clone)
            .map_err(|message| EvalError::Evaluation((*message).to_owned()))
    }

    fn eval_row(&self, _context: &mut EvalContext, row: usize) -> Result<Option<T>, EvalError> {
        self.scalar_calls.set(self.scalar_calls.get() + 1);
        self.scalar[row]
            .as_ref()
            .map(Clone::clone)
            .map_err(|message| EvalError::Evaluation((*message).to_owned()))
    }
}

/// 向量路径应求值每个参数，但只填结果中仍为 NULL 的行。
#[test]
fn coalesce_vector_path_fills_only_unset_rows_but_evaluates_every_argument() {
    let first = MockExpression::values(
        vec![Some(1_i64), None, Some(3)],
        vec![Some(1), None, Some(3)],
    );
    let second = MockExpression::values(
        vec![Some(9_i64), Some(2), Some(9)],
        vec![Some(9), Some(2), Some(9)],
    );
    let mut context = EvalContext::default();

    let result = vec_coalesce(&mut context, 3, &[&first, &second]).unwrap();
    assert_eq!(result, vec![Some(1), Some(2), Some(3)]);
    assert_eq!(first.vector_calls.get(), 1);
    assert_eq!(second.vector_calls.get(), 1);
    assert_eq!(first.scalar_calls.get() + second.scalar_calls.get(), 0);
}

/// 向量路径产生警告时应回滚并退回逐行短路径。
#[test]
fn coalesce_warning_rolls_back_vector_warnings_and_falls_back_row_by_row() {
    let first = MockExpression::values(vec![Some(10_i64), None], vec![Some(10), None]);
    let mut second = MockExpression::values(vec![Some(20_i64), Some(20)], vec![Some(20), Some(20)]);
    second.vector_warning = Some("vector-only warning");
    let mut context = EvalContext::default();
    context.append_warning("existing warning");

    let result = vec_coalesce(&mut context, 2, &[&first, &second]).unwrap();
    assert_eq!(result, vec![Some(10), Some(20)]);
    // 投机性向量警告被截断，仅保留既有警告。
    assert_eq!(context.warnings(), &["existing warning"]);
    assert_eq!(first.scalar_calls.get(), 2);
    assert_eq!(second.scalar_calls.get(), 1);
}

/// 向量侧错误可在标量短路径永不触达该参数时被“绕过”并成功。
#[test]
fn coalesce_vector_error_can_succeed_when_scalar_short_circuits_it() {
    let first = MockExpression::values(vec![Some(7_i64)], vec![Some(7)]);
    let failing = MockExpression {
        vector: Err("vector evaluated an unreachable expression"),
        scalar: vec![Err("scalar must not reach this expression")],
        vector_warning: None,
        vector_calls: Cell::new(0),
        scalar_calls: Cell::new(0),
    };
    let mut context = EvalContext::default();

    assert_eq!(
        vec_coalesce(&mut context, 1, &[&first, &failing]).unwrap(),
        vec![Some(7)]
    );
    assert_eq!(failing.vector_calls.get(), 1);
    assert_eq!(failing.scalar_calls.get(), 0);
}

/// 时间 COALESCE 应把结果 fsp（小数秒精度）统一到指定值。
#[test]
fn coalesce_time_applies_result_fractional_seconds_precision() {
    let source = NewTime(
        FromDate(2024, 2, 3, 4, 5, 6, 123_456),
        mysql::TypeDatetime,
        6,
    );
    let expression = MockExpression::values(vec![Some(source)], vec![Some(source)]);
    let mut context = EvalContext::default();

    let result: Vec<Option<Time>> = vec_coalesce_time(&mut context, 1, &[&expression], 3).unwrap();
    assert_eq!(result[0].unwrap().Fsp(), 3);
}

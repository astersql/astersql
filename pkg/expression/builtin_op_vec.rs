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

// Vector kernels for TiDB's logical, bit, truth, NULL and unary operators.
//
// The expression signatures that own argument evaluation are wired by the
// package integration task.  This file owns the row-for-row behavior from
// `builtin_op_vec.go` and deliberately operates on the real chunk `Column`.

// 逻辑/位运算/真值/NULL/一元运算符的向量化内核（对应 Go `builtin_op_vec.go`）。
// 在真实 chunk `Column` 上按行实现与 Go 一致的语义；参数求值由上层签名装配。

pub use chunk_dependency::Column;
use types_dependency::decimal::mydecimal::DecimalNeg;
pub use types_dependency::decimal::mydecimal::MyDecimal;

/// 向量化求值结果别名。
pub type EvalResult<T> = Result<T, EvalError>;

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
/// 列长不匹配、BIGINT 溢出或通用求值失败。
pub enum EvalError {
    #[error("column length mismatch: left has {left} rows, right has {right} rows")]
    ColumnLengthMismatch { left: usize, right: usize },
    #[error("BIGINT value -({value}) is out of range")]
    BigIntOverflow { value: String },
    #[error("{0}")]
    Evaluation(String),
}

/// Minimal warning surface needed by LogicAnd/LogicOr's vector fallback.
/// LogicAnd/LogicOr 向量回退路径所需的最小 warning 上下文。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WarningContext {
    warnings: Vec<String>,
}

impl WarningContext {
    /// 追加一条 warning。
    pub fn append_warning(&mut self, warning: impl Into<String>) {
        self.warnings.push(warning.into());
    }

    /// 当前 warning 条数。
    pub fn warning_count(&self) -> usize {
        self.warnings.len()
    }

    /// 截断到指定条数（回退前丢弃转换产生的 warning）。
    pub fn truncate_warnings(&mut self, count: usize) {
        self.warnings.truncate(count);
    }

    /// 只读访问 warning 列表。
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }
}

/// All operator signatures in the Go file opt into vectorized evaluation.
/// Go 本文件中的运算符签名均启用向量化。
pub const fn vectorized() -> bool {
    true
}

/// 校验左右列行数一致，返回公共行数。
fn check_binary_rows(left: &Column, right: &Column) -> EvalResult<usize> {
    let left_rows = left.Rows();
    let right_rows = right.Rows();
    if left_rows != right_rows {
        return Err(EvalError::ColumnLengthMismatch {
            left: left_rows,
            right: right_rows,
        });
    }
    Ok(left_rows)
}

/// 预分配指定行数的 Int64 结果列。
fn int_result(rows: usize) -> Column {
    let mut result = Column::default();
    result.ResizeInt64(rows, false);
    result
}

/// 向结果列指定行写入 i64（原地覆盖）。
fn write_i64(column: &mut Column, row: usize, value: i64) {
    let offset = row * size_of::<i64>();
    column.data[offset..offset + size_of::<i64>()].copy_from_slice(&value.to_ne_bytes());
}

/// 追加 i64 或 NULL 到结果列。
fn push_i64(column: &mut Column, value: Option<i64>) {
    match value {
        Some(value) => column.AppendInt64(value),
        None => column.AppendNull(),
    }
}

/// 将整型单元格转为三值布尔：NULL / 非零真 / 零假。
fn logical_value(column: &Column, row: usize) -> Option<bool> {
    (!column.IsNull(row)).then(|| column.GetInt64(row) != 0)
}

/// 向量逻辑二元运算公共循环。
fn vec_logic_binary(
    left: &Column,
    right: &Column,
    op: fn(Option<bool>, Option<bool>) -> Option<bool>,
) -> EvalResult<Column> {
    let rows = check_binary_rows(left, right)?;
    let mut result = Column::default();
    result.ResizeInt64(0, false);
    for row in 0..rows {
        push_i64(
            &mut result,
            op(logical_value(left, row), logical_value(right, row)).map(i64::from),
        );
    }
    Ok(result)
}

/// SQL OR 三值真值表。
fn sql_or(left: Option<bool>, right: Option<bool>) -> Option<bool> {
    match (left, right) {
        (Some(true), _) | (_, Some(true)) => Some(true),
        (None, _) | (_, None) => None,
        (Some(false), Some(false)) => Some(false),
    }
}

/// SQL AND 三值真值表。
fn sql_and(left: Option<bool>, right: Option<bool>) -> Option<bool> {
    match (left, right) {
        (Some(false), _) | (_, Some(false)) => Some(false),
        (None, _) | (_, None) => None,
        (Some(true), Some(true)) => Some(true),
    }
}

/// SQL XOR：任一侧 NULL 则结果 NULL。
fn sql_xor(left: Option<bool>, right: Option<bool>) -> Option<bool> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left != right),
        _ => None,
    }
}

/// 向量化逻辑或。
pub fn vec_logic_or(left: &Column, right: &Column) -> EvalResult<Column> {
    vec_logic_binary(left, right, sql_or)
}

/// 向量化逻辑与。
pub fn vec_logic_and(left: &Column, right: &Column) -> EvalResult<Column> {
    vec_logic_binary(left, right, sql_and)
}

/// 向量化逻辑异或。
pub fn vec_logic_xor(left: &Column, right: &Column) -> EvalResult<Column> {
    vec_logic_binary(left, right, sql_xor)
}

/// 右操作数转换若产生 warning/错误，则丢弃两侧转换 warning 并回退逐行标量求值。
fn vec_logic_with_fallback<L, R, F, V>(
    ctx: &mut WarningContext,
    left_eval: L,
    right_eval: R,
    fallback_eval: F,
    vector_eval: V,
) -> EvalResult<Column>
where
    L: FnOnce(&mut WarningContext) -> EvalResult<Column>,
    R: FnOnce(&mut WarningContext) -> EvalResult<Column>,
    F: FnOnce(&mut WarningContext) -> EvalResult<Column>,
    V: FnOnce(&Column, &Column) -> EvalResult<Column>,
{
    let before_left_warnings = ctx.warning_count();
    let left = left_eval(ctx)?;
    let before_right_warnings = ctx.warning_count();
    let right = right_eval(ctx);
    let right_added_warning = ctx.warning_count() > before_right_warnings;
    match right {
        Ok(right) if !right_added_warning => vector_eval(&left, &right),
        Ok(_) | Err(_) => {
            // Go discards conversion warnings from both vector arguments before
            // repeating the operation row by row.
            // Go 在逐行重算前丢弃两侧向量参数转换产生的 warning。
            ctx.truncate_warnings(before_left_warnings);
            fallback_eval(ctx)
        }
    }
}

/// 带回退的向量化 OR。
pub fn vec_logic_or_with_fallback<L, R, F>(
    ctx: &mut WarningContext,
    left_eval: L,
    right_eval: R,
    fallback_eval: F,
) -> EvalResult<Column>
where
    L: FnOnce(&mut WarningContext) -> EvalResult<Column>,
    R: FnOnce(&mut WarningContext) -> EvalResult<Column>,
    F: FnOnce(&mut WarningContext) -> EvalResult<Column>,
{
    vec_logic_with_fallback(ctx, left_eval, right_eval, fallback_eval, vec_logic_or)
}

/// 带回退的向量化 AND。
pub fn vec_logic_and_with_fallback<L, R, F>(
    ctx: &mut WarningContext,
    left_eval: L,
    right_eval: R,
    fallback_eval: F,
) -> EvalResult<Column>
where
    L: FnOnce(&mut WarningContext) -> EvalResult<Column>,
    R: FnOnce(&mut WarningContext) -> EvalResult<Column>,
    F: FnOnce(&mut WarningContext) -> EvalResult<Column>,
{
    vec_logic_with_fallback(ctx, left_eval, right_eval, fallback_eval, vec_logic_and)
}

#[derive(Clone, Copy)]
/// 向量化位运算种类。
enum BitOperation {
    Or,
    Xor,
    And,
    LeftShift,
    RightShift,
}

/// 位运算/移位公共循环：按无符号语义计算，任一侧 NULL 则结果 NULL。
fn vec_bit_binary(left: &Column, right: &Column, op: BitOperation) -> EvalResult<Column> {
    let rows = check_binary_rows(left, right)?;
    let mut result = int_result(rows);
    for row in 0..rows {
        if left.IsNull(row) || right.IsNull(row) {
            result.SetNull(row, true);
            continue;
        }
        let left = left.GetInt64(row) as u64;
        let right = right.GetInt64(row) as u64;
        let value = match op {
            BitOperation::Or => left | right,
            BitOperation::Xor => left ^ right,
            BitOperation::And => left & right,
            BitOperation::LeftShift => {
                if right >= i64::BITS as u64 {
                    0
                } else {
                    left << right
                }
            }
            BitOperation::RightShift => {
                if right >= i64::BITS as u64 {
                    0
                } else {
                    left >> right
                }
            }
        };
        write_i64(&mut result, row, value as i64);
    }
    Ok(result)
}

/// 向量化按位或。
pub fn vec_bit_or(left: &Column, right: &Column) -> EvalResult<Column> {
    vec_bit_binary(left, right, BitOperation::Or)
}

/// 向量化按位异或。
pub fn vec_bit_xor(left: &Column, right: &Column) -> EvalResult<Column> {
    vec_bit_binary(left, right, BitOperation::Xor)
}

/// 向量化按位与。
pub fn vec_bit_and(left: &Column, right: &Column) -> EvalResult<Column> {
    vec_bit_binary(left, right, BitOperation::And)
}

/// 向量化逻辑左移。
pub fn vec_left_shift(left: &Column, right: &Column) -> EvalResult<Column> {
    vec_bit_binary(left, right, BitOperation::LeftShift)
}

/// 向量化逻辑右移。
pub fn vec_right_shift(left: &Column, right: &Column) -> EvalResult<Column> {
    vec_bit_binary(left, right, BitOperation::RightShift)
}

/// 向量化按位取反。
pub fn vec_bit_neg(input: &Column) -> Column {
    let mut result = int_result(input.Rows());
    for row in 0..input.Rows() {
        if input.IsNull(row) {
            result.SetNull(row, true);
        } else {
            write_i64(&mut result, row, !input.GetInt64(row));
        }
    }
    result
}

/// 整型一元逻辑非。
pub fn vec_unary_not_int(input: &Column) -> Column {
    let mut result = int_result(input.Rows());
    for row in 0..input.Rows() {
        if input.IsNull(row) {
            result.SetNull(row, true);
        } else {
            write_i64(&mut result, row, i64::from(input.GetInt64(row) == 0));
        }
    }
    result
}

/// 浮点一元逻辑非。
pub fn vec_unary_not_real(input: &Column) -> Column {
    let mut result = int_result(input.Rows());
    for row in 0..input.Rows() {
        if input.IsNull(row) {
            result.SetNull(row, true);
        } else {
            write_i64(&mut result, row, i64::from(input.GetFloat64(row) == 0.0));
        }
    }
    result
}

/// DECIMAL 一元逻辑非。
pub fn vec_unary_not_decimal(input: &Column) -> Column {
    let mut result = int_result(input.Rows());
    for row in 0..input.Rows() {
        if input.IsNull(row) {
            result.SetNull(row, true);
        } else {
            write_i64(&mut result, row, i64::from(input.GetDecimal(row).IsZero()));
        }
    }
    result
}

/// 浮点一元负号。
pub fn vec_unary_minus_real(input: &Column) -> Column {
    let mut result = Column::default();
    result.ResizeFloat64(0, false);
    for row in 0..input.Rows() {
        if input.IsNull(row) {
            result.AppendNull();
        } else {
            result.AppendFloat64(-input.GetFloat64(row));
        }
    }
    result
}

/// DECIMAL 一元负号。
pub fn vec_unary_minus_decimal(input: &Column) -> Column {
    let mut result = Column::default();
    result.ResizeDecimal(0, false);
    for row in 0..input.Rows() {
        if input.IsNull(row) {
            result.AppendNull();
        } else {
            result.AppendMyDecimal(&DecimalNeg(&input.GetDecimal(row)));
        }
    }
    result
}

/// 整数一元负号；`unsigned` 控制按有符号或无符号溢出规则。
pub fn vec_unary_minus_int(input: &Column, unsigned: bool) -> EvalResult<Column> {
    let mut result = int_result(input.Rows());
    for row in 0..input.Rows() {
        if input.IsNull(row) {
            result.SetNull(row, true);
            continue;
        }
        let negated = if unsigned {
            let value = input.GetUint64(row);
            if value > (1u64 << 63) {
                return Err(EvalError::BigIntOverflow {
                    value: value.to_string(),
                });
            }
            (value as i64).wrapping_neg()
        } else {
            let value = input.GetInt64(row);
            value
                .checked_neg()
                .ok_or_else(|| EvalError::BigIntOverflow {
                    value: value.to_string(),
                })?
        };
        write_i64(&mut result, row, negated);
    }
    Ok(result)
}

/// 整型 IS TRUE/FALSE 公共实现。
fn vec_is_truth_int(input: &Column, keep_null: bool, want_true: bool) -> Column {
    let mut result = int_result(input.Rows());
    for row in 0..input.Rows() {
        if input.IsNull(row) {
            if keep_null {
                result.SetNull(row, true);
            }
            continue;
        }
        let truth = input.GetInt64(row) != 0;
        write_i64(&mut result, row, i64::from(truth == want_true));
    }
    result
}

/// 浮点 IS TRUE/FALSE 公共实现。
fn vec_is_truth_real(input: &Column, keep_null: bool, want_true: bool) -> Column {
    let mut result = int_result(input.Rows());
    for row in 0..input.Rows() {
        if input.IsNull(row) {
            if keep_null {
                result.SetNull(row, true);
            }
            continue;
        }
        let truth = input.GetFloat64(row) != 0.0;
        write_i64(&mut result, row, i64::from(truth == want_true));
    }
    result
}

/// DECIMAL IS TRUE/FALSE 公共实现。
fn vec_is_truth_decimal(input: &Column, keep_null: bool, want_true: bool) -> Column {
    let mut result = int_result(input.Rows());
    for row in 0..input.Rows() {
        if input.IsNull(row) {
            if keep_null {
                result.SetNull(row, true);
            }
            continue;
        }
        let truth = !input.GetDecimal(row).IsZero();
        write_i64(&mut result, row, i64::from(truth == want_true));
    }
    result
}

/// 整型 IS TRUE。
pub fn vec_int_is_true(input: &Column, keep_null: bool) -> Column {
    vec_is_truth_int(input, keep_null, true)
}

/// 整型 IS FALSE。
pub fn vec_int_is_false(input: &Column, keep_null: bool) -> Column {
    vec_is_truth_int(input, keep_null, false)
}

/// 浮点 IS TRUE。
pub fn vec_real_is_true(input: &Column, keep_null: bool) -> Column {
    vec_is_truth_real(input, keep_null, true)
}

/// 浮点 IS FALSE。
pub fn vec_real_is_false(input: &Column, keep_null: bool) -> Column {
    vec_is_truth_real(input, keep_null, false)
}

/// DECIMAL IS TRUE。
pub fn vec_decimal_is_true(input: &Column, keep_null: bool) -> Column {
    vec_is_truth_decimal(input, keep_null, true)
}

/// DECIMAL IS FALSE。
pub fn vec_decimal_is_false(input: &Column, keep_null: bool) -> Column {
    vec_is_truth_decimal(input, keep_null, false)
}

/// IS NULL is identical for Time, Int, Real, Decimal and Duration because it
/// only inspects the chunk bitmap and always produces a non-NULL integer.
/// IS NULL 对各类型相同：只看 chunk NULL 位图，结果恒为非 NULL 整数。
pub fn vec_is_null(input: &Column) -> Column {
    let mut result = int_result(input.Rows());
    for row in 0..input.Rows() {
        write_i64(&mut result, row, i64::from(input.IsNull(row)));
    }
    result
}

/// Time 类型 IS NULL（委托通用实现）。
pub fn vec_time_is_null(input: &Column) -> Column {
    vec_is_null(input)
}

/// Int 类型 IS NULL。
pub fn vec_int_is_null(input: &Column) -> Column {
    vec_is_null(input)
}

/// Real 类型 IS NULL。
pub fn vec_real_is_null(input: &Column) -> Column {
    vec_is_null(input)
}

/// Decimal 类型 IS NULL。
pub fn vec_decimal_is_null(input: &Column) -> Column {
    vec_is_null(input)
}

/// Duration 类型 IS NULL。
pub fn vec_duration_is_null(input: &Column) -> Column {
    vec_is_null(input)
}

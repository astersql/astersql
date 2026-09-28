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

// Vectorized MySQL arithmetic from `builtin_arithmetic_vec.go`.
//
// TiDB stores unsigned integers in the same 64-bit lane as signed integers.
// `IntVector` keeps that representation and an unsigned flag, so the four Go
// signedness branches retain their bit-for-bit behavior.  NULL rows are
// merged before arithmetic and therefore never turn an otherwise irrelevant
// overflow into an error.
//
// 向量化算术实现（对应 Go `builtin_arithmetic_vec.go`）。
//
// 以列为单位批量计算加减乘除、整除与取模。无符号整数与有符号共用 64 位通道，
// `IntVector` 用 `unsigned` 标志区分解释方式。NULL 行先合并，避免无关溢出变成错误。
// 向量化（vectorized execution）指对 chunk 中一整列批量求值，而非逐行标量调用。

#![allow(non_snake_case)]

pub use types_dependency::decimal::mydecimal::{
    DecimalAdd, DecimalDiv, DecimalError, DecimalMod, DecimalMul, DecimalSub, ModeHalfUp,
    MyDecimal, NewMaxOrMinDec,
};
use types_dependency::file_group::overflow::{
    DivInt64, DivIntWithUint, DivUintWithInt, OverflowError,
};

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
/// 向量化算术错误：溢出、除零、DECIMAL 截断、列长不一致等。
pub enum ArithmeticError {
    #[error("{target_type} value is out of range in '{expression}'")]
    Overflow {
        target_type: &'static str,
        expression: String,
    },
    #[error("division by zero")]
    DivisionByZero,
    #[error("truncated wrong DECIMAL value")]
    TruncatedDecimal,
    #[error("decimal arithmetic failed: {0}")]
    Decimal(DecimalError),
    #[error("vector length mismatch: left={left}, right={right}")]
    LengthMismatch { left: usize, right: usize },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 向量化求值上下文：精度增量、除零/截断策略、无符号减法开关。
pub struct EvalContext {
    pub div_precision_increment: isize,
    pub result_decimal: isize,
    pub division_by_zero_as_error: bool,
    pub truncation_as_error: bool,
    pub no_unsigned_subtraction: bool,
}

impl Default for EvalContext {
    fn default() -> Self {
        Self {
            div_precision_increment: 4,
            result_decimal: 0,
            division_by_zero_as_error: false,
            truncation_as_error: false,
            no_unsigned_subtraction: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
/// 带 NULL 位图的通用列向量容器。
pub struct Vector<T> {
    values: Vec<T>,
    nulls: Vec<bool>,
}

impl<T: Default> Vector<T> {
    /// 由 `Option<T>` 列表构造向量，None 记为 NULL。
    pub fn from_options(values: Vec<Option<T>>) -> Self {
        let mut data = Vec::with_capacity(values.len());
        let mut nulls = Vec::with_capacity(values.len());
        for value in values {
            match value {
                Some(value) => {
                    data.push(value);
                    nulls.push(false);
                }
                None => {
                    data.push(T::default());
                    nulls.push(true);
                }
            }
        }
        Self {
            values: data,
            nulls,
        }
    }
}

impl<T: Clone> Vector<T> {
    /// 导出为 `Option<T>` 列表。
    pub fn options(&self) -> Vec<Option<T>> {
        self.values
            .iter()
            .zip(&self.nulls)
            .map(|(value, is_null)| (!*is_null).then(|| value.clone()))
            .collect()
    }
}

impl<T> Vector<T> {
    /// 行数。
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// 是否为空列。
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// 判断指定行是否为 NULL。
    pub fn is_null(&self, row: usize) -> bool {
        self.nulls[row]
    }
}

/// f64 实数列别名。
pub type RealVector = Vector<f64>;
/// `MyDecimal` 定点数列别名。
pub type DecimalVector = Vector<MyDecimal>;

#[derive(Clone, Debug, Eq, PartialEq)]
/// 整型列：底层存 i64 位模式，并用 `unsigned` 标志解释。
pub struct IntVector {
    values: Vec<i64>,
    nulls: Vec<bool>,
    unsigned: bool,
}

impl IntVector {
    /// 构造有符号整型列。
    pub fn signed(values: Vec<Option<i64>>) -> Self {
        let mut data = Vec::with_capacity(values.len());
        let mut nulls = Vec::with_capacity(values.len());
        for value in values {
            match value {
                Some(value) => {
                    data.push(value);
                    nulls.push(false);
                }
                None => {
                    data.push(0);
                    nulls.push(true);
                }
            }
        }
        Self {
            values: data,
            nulls,
            unsigned: false,
        }
    }

    /// 构造无符号整型列（值以 i64 位模式存放）。
    pub fn unsigned(values: Vec<Option<u64>>) -> Self {
        let mut data = Vec::with_capacity(values.len());
        let mut nulls = Vec::with_capacity(values.len());
        for value in values {
            match value {
                Some(value) => {
                    data.push(value as i64);
                    nulls.push(false);
                }
                None => {
                    data.push(0);
                    nulls.push(true);
                }
            }
        }
        Self {
            values: data,
            nulls,
            unsigned: true,
        }
    }

    /// 由原始值、NULL 位图与无符号标志构造。
    fn from_raw(values: Vec<i64>, nulls: Vec<bool>, unsigned: bool) -> Self {
        Self {
            values,
            nulls,
            unsigned,
        }
    }

    /// 行数。
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// 是否为空列。
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// 是否为无符号列。
    pub fn is_unsigned(&self) -> bool {
        self.unsigned
    }

    /// 按有符号导出可选值。
    pub fn options_i64(&self) -> Vec<Option<i64>> {
        self.values
            .iter()
            .zip(&self.nulls)
            .map(|(value, is_null)| (!*is_null).then_some(*value))
            .collect()
    }

    /// 按无符号导出可选值。
    pub fn options_u64(&self) -> Vec<Option<u64>> {
        self.values
            .iter()
            .zip(&self.nulls)
            .map(|(value, is_null)| (!*is_null).then_some(*value as u64))
            .collect()
    }
}

/// 向量化签名标记 trait；默认 `vectorized()` 为 true。
pub trait Vectorized {
    /// 是否声明支持向量化执行。
    fn vectorized(&self) -> bool {
        true
    }
}

// 批量声明空结构体签名，并为每个签名实现 `Vectorized`。
// 具体逐列运算在下方各自的 `impl ...Sig` 中提供。
macro_rules! vectorized_signatures {
    ($($name:ident),+ $(,)?) => {
        $(
            #[derive(Clone, Copy, Debug, Default)]
            pub struct $name;
            impl Vectorized for $name {}
        )+
    };
}

// Go 中每个算术签名对应一个空结构体；此处用宏生成同名类型以保持分派表一致。
vectorized_signatures!(
    BuiltinArithmeticMultiplyRealSig,
    BuiltinArithmeticDivideDecimalSig,
    BuiltinArithmeticModIntUnsignedUnsignedSig,
    BuiltinArithmeticModIntUnsignedSignedSig,
    BuiltinArithmeticModIntSignedUnsignedSig,
    BuiltinArithmeticModIntSignedSignedSig,
    BuiltinArithmeticMinusRealSig,
    BuiltinArithmeticMinusDecimalSig,
    BuiltinArithmeticMinusIntSig,
    BuiltinArithmeticModRealSig,
    BuiltinArithmeticModDecimalSig,
    BuiltinArithmeticPlusRealSig,
    BuiltinArithmeticMultiplyDecimalSig,
    BuiltinArithmeticIntDivideDecimalSig,
    BuiltinArithmeticMultiplyIntSig,
    BuiltinArithmeticDivideRealSig,
    BuiltinArithmeticIntDivideIntSig,
    BuiltinArithmeticPlusIntSig,
    BuiltinArithmeticPlusDecimalSig,
    BuiltinArithmeticMultiplyIntUnsignedSig,
);

/// 确保左右列长度一致，否则返回 LengthMismatch。
fn ensure_same_len(left: usize, right: usize) -> Result<(), ArithmeticError> {
    if left == right {
        Ok(())
    } else {
        Err(ArithmeticError::LengthMismatch { left, right })
    }
}

/// 合并左右 NULL 位图（任一侧为 NULL 则结果为 NULL）。
fn merged_nulls(left: &[bool], right: &[bool]) -> Vec<bool> {
    left.iter()
        .zip(right)
        .map(|(left, right)| *left || *right)
        .collect()
}

/// 构造溢出错误，带目标类型与运算符文本。
fn overflow(target_type: &'static str, operator: &str) -> ArithmeticError {
    ArithmeticError::Overflow {
        target_type,
        expression: format!("(lhs {operator} rhs)"),
    }
}

/// 将底层 OverflowError 映射为 ArithmeticError。
fn map_integer_overflow(error: OverflowError, operator: &str) -> ArithmeticError {
    ArithmeticError::Overflow {
        target_type: error.target_type,
        expression: format!("(lhs {operator} rhs)"),
    }
}

/// 按上下文处理除零。
fn handle_division_by_zero(context: &EvalContext) -> Result<(), ArithmeticError> {
    if context.division_by_zero_as_error {
        Err(ArithmeticError::DivisionByZero)
    } else {
        Ok(())
    }
}

/// 根据 DECIMAL 运算状态决定截断错误或继续。
fn handle_decimal_status(
    context: &EvalContext,
    status: DecimalError,
    operator: &str,
) -> Result<(), ArithmeticError> {
    match status {
        DecimalError::Truncated if !context.truncation_as_error => Ok(()),
        DecimalError::Truncated | DecimalError::TruncatedWrongValue => {
            Err(ArithmeticError::TruncatedDecimal)
        }
        DecimalError::Overflow => Err(overflow("DECIMAL", operator)),
        other => Err(ArithmeticError::Decimal(other)),
    }
}

/// 对两列 REAL 做逐行二元运算的公共骨架。
fn real_binary<F>(
    left: &RealVector,
    right: &RealVector,
    operator: &str,
    reject_non_finite: bool,
    operation: F,
) -> Result<RealVector, ArithmeticError>
where
    F: Fn(f64, f64) -> f64,
{
    // 先对齐左右列长度，再合并 NULL，最后对非 NULL 行做逐元素运算。
    ensure_same_len(left.len(), right.len())?;
    let nulls = merged_nulls(&left.nulls, &right.nulls);
    let mut values = left.values.clone();
    for row in 0..values.len() {
        if nulls[row] {
            continue;
        }
        let value = operation(left.values[row], right.values[row]);
        if (reject_non_finite && !value.is_finite()) || (!reject_non_finite && value.is_infinite())
        {
            return Err(overflow("DOUBLE", operator));
        }
        values[row] = value;
    }
    Ok(RealVector { values, nulls })
}

impl BuiltinArithmeticMultiplyRealSig {
    pub fn vec_eval_real(
        &self,
        _context: &EvalContext,
        left: &RealVector,
        right: &RealVector,
    ) -> Result<RealVector, ArithmeticError> {
        // 与 Go 一致：乘用 IsInf（保留 NaN）；加减用 IsFinite 拒绝非有限值。
        // Go checks IsInf here (rather than IsFinite), so NaN remains a value.
        real_binary(left, right, "*", false, |left, right| left * right)
    }
}

impl BuiltinArithmeticMinusRealSig {
    pub fn vec_eval_real(
        &self,
        _context: &EvalContext,
        left: &RealVector,
        right: &RealVector,
    ) -> Result<RealVector, ArithmeticError> {
        real_binary(left, right, "-", true, |left, right| left - right)
    }
}

impl BuiltinArithmeticPlusRealSig {
    pub fn vec_eval_real(
        &self,
        _context: &EvalContext,
        left: &RealVector,
        right: &RealVector,
    ) -> Result<RealVector, ArithmeticError> {
        real_binary(left, right, "+", true, |left, right| left + right)
    }
}

impl BuiltinArithmeticDivideRealSig {
    pub fn vec_eval_real(
        &self,
        context: &EvalContext,
        left: &RealVector,
        right: &RealVector,
    ) -> Result<RealVector, ArithmeticError> {
        ensure_same_len(left.len(), right.len())?;
        let mut nulls = merged_nulls(&left.nulls, &right.nulls);
        let mut values = left.values.clone();
        for row in 0..values.len() {
            if nulls[row] {
                continue;
            }
            if right.values[row] == 0.0 {
                handle_division_by_zero(context)?;
                nulls[row] = true;
                continue;
            }
            let value = left.values[row] / right.values[row];
            if value.is_infinite() {
                return Err(overflow("DOUBLE", "/"));
            }
            values[row] = value;
        }
        Ok(RealVector { values, nulls })
    }
}

impl BuiltinArithmeticModRealSig {
    pub fn vec_eval_real(
        &self,
        context: &EvalContext,
        left: &RealVector,
        right: &RealVector,
    ) -> Result<RealVector, ArithmeticError> {
        ensure_same_len(left.len(), right.len())?;
        let mut nulls = merged_nulls(&left.nulls, &right.nulls);
        let mut values = left.values.clone();
        for row in 0..values.len() {
            if nulls[row] {
                continue;
            }
            if right.values[row] == 0.0 {
                handle_division_by_zero(context)?;
                nulls[row] = true;
                continue;
            }
            values[row] = left.values[row] % right.values[row];
        }
        Ok(RealVector { values, nulls })
    }
}

/// 对两列 DECIMAL 做逐行二元运算的公共骨架。
fn decimal_binary<F>(
    context: &EvalContext,
    left: &DecimalVector,
    right: &DecimalVector,
    operator: &str,
    operation: F,
) -> Result<DecimalVector, ArithmeticError>
where
    F: Fn(&MyDecimal, &MyDecimal, &mut MyDecimal) -> Result<(), DecimalError>,
{
    ensure_same_len(left.len(), right.len())?;
    let nulls = merged_nulls(&left.nulls, &right.nulls);
    let mut values = left.values.clone();
    for row in 0..values.len() {
        if nulls[row] {
            continue;
        }
        let mut value = MyDecimal::default();
        if let Err(error) = operation(&left.values[row], &right.values[row], &mut value) {
            handle_decimal_status(context, error, operator)?;
        }
        values[row] = value;
    }
    Ok(DecimalVector { values, nulls })
}

impl BuiltinArithmeticPlusDecimalSig {
    pub fn vec_eval_decimal(
        &self,
        context: &EvalContext,
        left: &DecimalVector,
        right: &DecimalVector,
    ) -> Result<DecimalVector, ArithmeticError> {
        decimal_binary(context, left, right, "+", DecimalAdd)
    }
}

impl BuiltinArithmeticMinusDecimalSig {
    pub fn vec_eval_decimal(
        &self,
        context: &EvalContext,
        left: &DecimalVector,
        right: &DecimalVector,
    ) -> Result<DecimalVector, ArithmeticError> {
        decimal_binary(context, left, right, "-", DecimalSub)
    }
}

impl BuiltinArithmeticMultiplyDecimalSig {
    pub fn vec_eval_decimal(
        &self,
        context: &EvalContext,
        left: &DecimalVector,
        right: &DecimalVector,
    ) -> Result<DecimalVector, ArithmeticError> {
        // DecimalMul's Truncated status is explicitly ignored by the Go code.
        let relaxed = EvalContext {
            truncation_as_error: false,
            ..*context
        };
        decimal_binary(&relaxed, left, right, "*", DecimalMul)
    }
}

impl BuiltinArithmeticDivideDecimalSig {
    pub fn vec_eval_decimal(
        &self,
        context: &EvalContext,
        left: &DecimalVector,
        right: &DecimalVector,
    ) -> Result<DecimalVector, ArithmeticError> {
        ensure_same_len(left.len(), right.len())?;
        let mut nulls = merged_nulls(&left.nulls, &right.nulls);
        let mut values = left.values.clone();
        for row in 0..values.len() {
            if nulls[row] {
                continue;
            }
            let mut value = MyDecimal::default();
            match DecimalDiv(
                &left.values[row],
                &right.values[row],
                &mut value,
                context.div_precision_increment,
            ) {
                Ok(()) => {
                    if value.PrecisionAndFrac().1 < context.result_decimal.max(0) as usize {
                        let source = value.clone();
                        source
                            .Round(&mut value, context.result_decimal, ModeHalfUp)
                            .map_err(|error| ArithmeticError::Decimal(error))?;
                    }
                }
                Err(DecimalError::DivByZero) => {
                    handle_division_by_zero(context)?;
                    nulls[row] = true;
                    continue;
                }
                Err(error) => handle_decimal_status(context, error, "/")?,
            }
            values[row] = value;
        }
        Ok(DecimalVector { values, nulls })
    }
}

impl BuiltinArithmeticModDecimalSig {
    pub fn vec_eval_decimal(
        &self,
        context: &EvalContext,
        left: &DecimalVector,
        right: &DecimalVector,
    ) -> Result<DecimalVector, ArithmeticError> {
        ensure_same_len(left.len(), right.len())?;
        let mut nulls = merged_nulls(&left.nulls, &right.nulls);
        let mut values = left.values.clone();
        for row in 0..values.len() {
            if nulls[row] {
                continue;
            }
            let mut value = MyDecimal::default();
            match DecimalMod(&left.values[row], &right.values[row], &mut value) {
                Ok(()) => values[row] = value,
                Err(DecimalError::DivByZero) => {
                    handle_division_by_zero(context)?;
                    nulls[row] = true;
                }
                Err(error) => return Err(ArithmeticError::Decimal(error)),
            }
        }
        Ok(DecimalVector { values, nulls })
    }
}

/// 准备整型结果列（长度、NULL 合并、无符号标志）。
fn prepare_int_result(
    left: &IntVector,
    right: &IntVector,
    unsigned: bool,
) -> Result<IntVector, ArithmeticError> {
    ensure_same_len(left.len(), right.len())?;
    Ok(IntVector::from_raw(
        right.values.clone(),
        merged_nulls(&left.nulls, &right.nulls),
        unsigned,
    ))
}

impl BuiltinArithmeticModIntUnsignedUnsignedSig {
    pub fn vec_eval_int(
        &self,
        context: &EvalContext,
        left: &IntVector,
        right: &IntVector,
    ) -> Result<IntVector, ArithmeticError> {
        let mut result = prepare_int_result(left, right, true)?;
        for row in 0..result.len() {
            if result.nulls[row] {
                continue;
            }
            let divisor = right.values[row] as u64;
            if divisor == 0 {
                handle_division_by_zero(context)?;
                result.nulls[row] = true;
                continue;
            }
            result.values[row] = ((left.values[row] as u64) % divisor) as i64;
        }
        Ok(result)
    }
}

impl BuiltinArithmeticModIntUnsignedSignedSig {
    pub fn vec_eval_int(
        &self,
        context: &EvalContext,
        left: &IntVector,
        right: &IntVector,
    ) -> Result<IntVector, ArithmeticError> {
        let mut result = prepare_int_result(left, right, true)?;
        for row in 0..result.len() {
            if result.nulls[row] {
                continue;
            }
            let divisor = right.values[row];
            if divisor == 0 {
                handle_division_by_zero(context)?;
                result.nulls[row] = true;
                continue;
            }
            result.values[row] = ((left.values[row] as u64) % divisor.unsigned_abs()) as i64;
        }
        Ok(result)
    }
}

impl BuiltinArithmeticModIntSignedUnsignedSig {
    pub fn vec_eval_int(
        &self,
        context: &EvalContext,
        left: &IntVector,
        right: &IntVector,
    ) -> Result<IntVector, ArithmeticError> {
        let mut result = prepare_int_result(left, right, false)?;
        for row in 0..result.len() {
            if result.nulls[row] {
                continue;
            }
            let divisor = right.values[row] as u64;
            if divisor == 0 {
                handle_division_by_zero(context)?;
                result.nulls[row] = true;
                continue;
            }
            let left_value = left.values[row];
            let magnitude = left_value.unsigned_abs() % divisor;
            result.values[row] = if left_value < 0 {
                (magnitude as i64).wrapping_neg()
            } else {
                magnitude as i64
            };
        }
        Ok(result)
    }
}

impl BuiltinArithmeticModIntSignedSignedSig {
    pub fn vec_eval_int(
        &self,
        context: &EvalContext,
        left: &IntVector,
        right: &IntVector,
    ) -> Result<IntVector, ArithmeticError> {
        let mut result = prepare_int_result(left, right, false)?;
        for row in 0..result.len() {
            if result.nulls[row] {
                continue;
            }
            let divisor = right.values[row];
            if divisor == 0 {
                handle_division_by_zero(context)?;
                result.nulls[row] = true;
                continue;
            }
            result.values[row] = if left.values[row] == i64::MIN && divisor == -1 {
                0
            } else {
                left.values[row] % divisor
            };
        }
        Ok(result)
    }
}

// 整型加法：按左右有无符号四组合分别做溢出检查，NULL 行跳过。
impl BuiltinArithmeticPlusIntSig {
    pub fn vec_eval_int(
        &self,
        _context: &EvalContext,
        left: &IntVector,
        right: &IntVector,
    ) -> Result<IntVector, ArithmeticError> {
        let unsigned = left.unsigned || right.unsigned;
        let mut result = prepare_int_result(left, right, unsigned)?;
        for row in 0..result.len() {
            if result.nulls[row] {
                continue;
            }
            let left_raw = left.values[row];
            let right_raw = right.values[row];
            result.values[row] = match (left.unsigned, right.unsigned) {
                (true, true) => (left_raw as u64)
                    .checked_add(right_raw as u64)
                    .ok_or_else(|| overflow("BIGINT UNSIGNED", "+"))?
                    as i64,
                (true, false) => {
                    let left_value = left_raw as u64;
                    if right_raw < 0 {
                        left_value
                            .checked_sub(right_raw.unsigned_abs())
                            .ok_or_else(|| overflow("BIGINT UNSIGNED", "+"))?
                            as i64
                    } else {
                        left_value
                            .checked_add(right_raw as u64)
                            .ok_or_else(|| overflow("BIGINT UNSIGNED", "+"))?
                            as i64
                    }
                }
                (false, true) => {
                    let right_value = right_raw as u64;
                    if left_raw < 0 {
                        right_value
                            .checked_sub(left_raw.unsigned_abs())
                            .ok_or_else(|| overflow("BIGINT UNSIGNED", "+"))?
                            as i64
                    } else {
                        right_value
                            .checked_add(left_raw as u64)
                            .ok_or_else(|| overflow("BIGINT UNSIGNED", "+"))?
                            as i64
                    }
                }
                (false, false) => left_raw
                    .checked_add(right_raw)
                    .ok_or_else(|| overflow("BIGINT", "+"))?,
            };
        }
        Ok(result)
    }
}

/// 判断整型减法在给定符号组合下是否溢出。
// 按有/无符号组合判断减法溢出；配合 `no_unsigned_subtraction` SQL 模式。
fn minus_overflows(
    left_unsigned: bool,
    right_unsigned: bool,
    signed: bool,
    left: i64,
    right: i64,
) -> bool {
    // This is the Go overflowCheck function, including its wrapped int64 result.
    let result = left.wrapping_sub(right);
    let unsigned_left = left as u64;
    let unsigned_right = right as u64;
    let mut result_unsigned = false;
    if left_unsigned {
        if right_unsigned {
            if unsigned_left < unsigned_right {
                if result >= 0 {
                    return true;
                }
            } else {
                result_unsigned = true;
            }
        } else if right >= 0 {
            if unsigned_left > unsigned_right {
                result_unsigned = true;
            }
        } else {
            if unsigned_left.checked_add(right.unsigned_abs()).is_none() {
                return true;
            }
            result_unsigned = true;
        }
    } else if right_unsigned {
        if (left.wrapping_sub(i64::MIN) as u64) < unsigned_right {
            return true;
        }
    } else if left > 0 && right < 0 {
        result_unsigned = true;
    } else if left < 0 && right > 0 && result >= 0 {
        return true;
    }

    (!signed && !result_unsigned && result < 0)
        || (signed && result_unsigned && result as u64 > i64::MAX as u64)
}

impl BuiltinArithmeticMinusIntSig {
    pub fn vec_eval_int(
        &self,
        context: &EvalContext,
        left: &IntVector,
        right: &IntVector,
    ) -> Result<IntVector, ArithmeticError> {
        let signed = context.no_unsigned_subtraction || (!left.unsigned && !right.unsigned);
        let target_type = if signed { "BIGINT" } else { "BIGINT UNSIGNED" };
        let mut result = prepare_int_result(left, right, !signed)?;
        for row in 0..result.len() {
            if result.nulls[row] {
                continue;
            }
            if minus_overflows(
                left.unsigned,
                right.unsigned,
                signed,
                left.values[row],
                right.values[row],
            ) {
                return Err(overflow(target_type, "-"));
            }
            result.values[row] = left.values[row].wrapping_sub(right.values[row]);
        }
        Ok(result)
    }
}

impl BuiltinArithmeticMultiplyIntSig {
    pub fn vec_eval_int(
        &self,
        _context: &EvalContext,
        left: &IntVector,
        right: &IntVector,
    ) -> Result<IntVector, ArithmeticError> {
        let mut result = prepare_int_result(left, right, false)?;
        for row in 0..result.len() {
            if result.nulls[row] {
                continue;
            }
            result.values[row] = left.values[row]
                .checked_mul(right.values[row])
                .ok_or_else(|| overflow("BIGINT", "*"))?;
        }
        Ok(result)
    }
}

impl BuiltinArithmeticMultiplyIntUnsignedSig {
    pub fn vec_eval_int(
        &self,
        _context: &EvalContext,
        left: &IntVector,
        right: &IntVector,
    ) -> Result<IntVector, ArithmeticError> {
        let mut result = prepare_int_result(left, right, true)?;
        for row in 0..result.len() {
            if result.nulls[row] {
                continue;
            }
            result.values[row] = (left.values[row] as u64)
                .checked_mul(right.values[row] as u64)
                .ok_or_else(|| overflow("BIGINT UNSIGNED", "*"))?
                as i64;
        }
        Ok(result)
    }
}

impl BuiltinArithmeticIntDivideIntSig {
    pub fn vec_eval_int(
        &self,
        context: &EvalContext,
        left: &IntVector,
        right: &IntVector,
    ) -> Result<IntVector, ArithmeticError> {
        let unsigned = left.unsigned || right.unsigned;
        let mut result = prepare_int_result(left, right, unsigned)?;
        for row in 0..result.len() {
            if result.nulls[row] {
                continue;
            }
            if right.values[row] == 0 {
                handle_division_by_zero(context)?;
                result.nulls[row] = true;
                continue;
            }
            result.values[row] = match (left.unsigned, right.unsigned) {
                (true, true) => ((left.values[row] as u64) / (right.values[row] as u64)) as i64,
                (true, false) => DivUintWithInt(left.values[row] as u64, right.values[row])
                    .map_err(|error| map_integer_overflow(error, "DIV"))?
                    as i64,
                (false, true) => DivIntWithUint(left.values[row], right.values[row] as u64)
                    .map_err(|error| map_integer_overflow(error, "DIV"))?
                    as i64,
                (false, false) => DivInt64(left.values[row], right.values[row])
                    .map_err(|error| map_integer_overflow(error, "DIV"))?,
            };
        }
        Ok(result)
    }
}

// DECIMAL 整除：先做定点除法再取整；无符号路径对小数部分有 Go 特有规则。
impl BuiltinArithmeticIntDivideDecimalSig {
    #[allow(clippy::too_many_arguments)]
    pub fn vec_eval_int(
        &self,
        context: &EvalContext,
        left: &DecimalVector,
        right: &DecimalVector,
        left_unsigned: bool,
        right_unsigned: bool,
    ) -> Result<IntVector, ArithmeticError> {
        ensure_same_len(left.len(), right.len())?;
        let unsigned = left_unsigned || right_unsigned;
        let mut result = IntVector::from_raw(
            vec![0; left.len()],
            merged_nulls(&left.nulls, &right.nulls),
            unsigned,
        );
        for row in 0..result.len() {
            if result.nulls[row] {
                continue;
            }
            let mut quotient = MyDecimal::default();
            match DecimalDiv(
                &left.values[row],
                &right.values[row],
                &mut quotient,
                context.div_precision_increment,
            ) {
                Ok(()) => {}
                Err(DecimalError::DivByZero) => {
                    handle_division_by_zero(context)?;
                    result.nulls[row] = true;
                    continue;
                }
                Err(DecimalError::Truncated | DecimalError::Overflow)
                    if !context.truncation_as_error => {}
                Err(error) => handle_decimal_status(context, error, "DIV")?,
            }

            if unsigned {
                let (value, conversion) = quotient.ToUint();
                if conversion == Err(DecimalError::Overflow) {
                    let (signed_value, signed_conversion) = quotient.ToInt();
                    // Go special case: a final value in (-1, 0] becomes zero.
                    if signed_value == 0 && signed_conversion == Err(DecimalError::Truncated) {
                        result.values[row] = 0;
                        continue;
                    }
                    return Err(overflow("BIGINT UNSIGNED", "DIV"));
                }
                result.values[row] = value as i64;
            } else {
                let (value, conversion) = quotient.ToInt();
                if conversion == Err(DecimalError::Overflow) {
                    return Err(overflow("BIGINT", "DIV"));
                }
                result.values[row] = value;
            }
        }
        Ok(result)
    }
}

#[cfg(test)]
#[path = "builtin_arithmetic_vec_1_aster_unit_test.rs"]
/// 条件编译挂载 Go 对等单元测试模块。
mod builtin_arithmetic_vec_aster_unit_test;

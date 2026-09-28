// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

// Scalar operator semantics migrated from `builtin_op.go`.
//
// The surrounding expression executor is migrated by separate tasks.  This module therefore
// exposes the complete, context-free part of the Go implementation: three-valued logic and its
// evaluation order, bit operations, truth tests, unary NOT/minus, IS NULL, and signature/type
// selection.  `EvalResult<T>` retains Go's `(value, isNull, error)` contract without inventing a
// sentinel value for SQL NULL.

// 标量运算符语义（对应 Go `builtin_op.go`）。
// 提供与上下文无关的完整行为：MySQL 三值逻辑及求值顺序、位运算、IS TRUE/FALSE、
// 一元 NOT/负号、IS NULL，以及函数类签名/类型选择；`EvalResult<T>` 中 `Ok(None)` 表示 SQL NULL。

use rust_decimal::Decimal;
use serde_json::Value as JsonValue;
use thiserror::Error;

const MYSQL_MAX_DECIMAL_WIDTH: i32 = 65;

/// Result shape used by scalar evaluation: `Ok(None)` is SQL NULL.
/// 标量求值结果形态：`Ok(None)` 表示 SQL NULL。
pub type EvalResult<T> = Result<Option<T>, EvalError>;

/// Errors produced by the operator layer.
/// 运算符层产生的错误。
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum EvalError {
    #[error("failed to evaluate {0}")]
    Input(&'static str),
    #[error("{type_name} value {value} is out of range")]
    Overflow {
        type_name: &'static str,
        value: String,
    },
    #[error("{0}")]
    Unsupported(String),
    #[error("expected {expected} argument(s), got {actual}")]
    InvalidArity { expected: usize, actual: usize },
}

impl EvalError {
    /// 构造输入求值失败错误。
    pub const fn input(name: &'static str) -> Self {
        Self::Input(name)
    }
}

/// MySQL's integer values retain whether the source field is unsigned.
/// MySQL 整数保留源字段是否无符号。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntValue {
    Signed(i64),
    Unsigned(u64),
}

/// Value families accepted by IS TRUE, IS FALSE, and unary NOT.
/// IS TRUE / IS FALSE / 一元 NOT 接受的值族。
#[derive(Clone, Debug, PartialEq)]
pub enum BooleanValue {
    Int(i64),
    Real(f64),
    Decimal(Decimal),
    VectorFloat32(Vec<f32>),
    Json(JsonValue),
}

impl BooleanValue {
    fn is_zero(&self) -> bool {
        match self {
            Self::Int(value) => *value == 0,
            Self::Real(value) => *value == 0.0,
            Self::Decimal(value) => value.is_zero(),
            Self::VectorFloat32(value) => value.iter().all(|component| *component == 0.0),
            // CompareBinaryJSON is used by Go here rather than general JSON truth conversion.
            Self::Json(value) => {
                value.as_i64() == Some(0)
                    || value.as_u64() == Some(0)
                    || value.as_f64() == Some(0.0)
            }
        }
    }
}

/// Evaluation types used by the Go function-class dispatch.
/// Go 函数类派发使用的求值类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvalType {
    Int,
    Real,
    Decimal,
    Timestamp,
    Datetime,
    Duration,
    Json,
    String,
    VectorFloat32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// IS TRUE / IS FALSE 操作选择。
pub enum TruthOp {
    IsTruth,
    IsFalsity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// 二元逻辑/位运算种类，用于构造函数类签名。
pub enum BinaryOp {
    LogicAnd,
    LogicOr,
    LogicXor,
    BitAnd,
    BitOr,
    BitXor,
    LeftShift,
    RightShift,
}

/// Push-down signature identifiers selected by `builtin_op.go`.
/// `builtin_op.go` 选择的下推签名标识。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScalarFuncSig {
    LogicalAnd,
    LogicalOr,
    LogicalXor,
    BitAnd,
    BitOr,
    BitXor,
    LeftShift,
    RightShift,
    RealIsTrue,
    RealIsTrueWithNull,
    DecimalIsTrue,
    DecimalIsTrueWithNull,
    IntIsTrue,
    IntIsTrueWithNull,
    RealIsFalse,
    RealIsFalseWithNull,
    DecimalIsFalse,
    DecimalIsFalseWithNull,
    IntIsFalse,
    IntIsFalseWithNull,
    BitNeg,
    UnaryNotReal,
    UnaryNotDecimal,
    UnaryNotInt,
    UnaryNotJson,
    UnaryMinusInt,
    UnaryMinusDecimal,
    UnaryMinusReal,
    IntIsNull,
    DecimalIsNull,
    RealIsNull,
    TimeIsNull,
    DurationIsNull,
    StringIsNull,
    VectorFloat32IsNull,
}

/// Metadata needed to reproduce function-class signature construction.
/// 复现函数类签名构造所需的元数据。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signature {
    pub argument_type: EvalType,
    pub return_type: EvalType,
    pub pb_code: Option<ScalarFuncSig>,
    pub flen: i32,
    pub decimal: i32,
    pub unsigned_result: bool,
    pub keep_null: bool,
    pub constant_arg_overflow: bool,
    pub warning: Option<&'static str>,
}

impl Signature {
    fn new(argument_type: EvalType, return_type: EvalType, pb_code: ScalarFuncSig) -> Self {
        Self {
            argument_type,
            return_type,
            pb_code: Some(pb_code),
            flen: -1,
            decimal: -1,
            unsigned_result: false,
            keep_null: false,
            constant_arg_overflow: false,
            warning: None,
        }
    }
}

/// Source-expression metadata used by unary-minus return type inference.
/// 一元负号返回类型推断所用的源表达式元数据。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExprMetadata {
    pub eval_type: EvalType,
    pub flen: i32,
    pub decimal: i32,
    pub unsigned: bool,
    pub is_column: bool,
}

impl ExprMetadata {
    pub const fn column(eval_type: EvalType, flen: i32, decimal: i32, unsigned: bool) -> Self {
        Self {
            eval_type,
            flen,
            decimal,
            unsigned,
            is_column: true,
        }
    }

    pub const fn constant(eval_type: EvalType, flen: i32, decimal: i32, unsigned: bool) -> Self {
        Self {
            eval_type,
            flen,
            decimal,
            unsigned,
            is_column: false,
        }
    }
}

/// 校验实参个数是否等于期望值。
pub fn validate_arity(actual: usize, expected: usize) -> Result<(), EvalError> {
    if actual == expected {
        Ok(())
    } else {
        Err(EvalError::InvalidArity { expected, actual })
    }
}

/// Logical AND preserves Go's left-to-right short circuit and SQL NULL truth table.
/// 逻辑与：保持 Go 从左到右短路与 SQL NULL 真值表。
pub fn logical_and<F>(lhs: EvalResult<i64>, rhs: F) -> EvalResult<i64>
where
    F: FnOnce() -> EvalResult<i64>,
{
    let lhs = lhs?;
    if matches!(lhs, Some(0)) {
        return Ok(Some(0));
    }

    let rhs = rhs()?;
    if matches!(rhs, Some(0)) {
        return Ok(Some(0));
    }
    if lhs.is_none() || rhs.is_none() {
        Ok(None)
    } else {
        Ok(Some(1))
    }
}

/// Logical OR preserves Go's left-to-right short circuit and SQL NULL truth table.
/// 逻辑或：保持 Go 从左到右短路与 SQL NULL 真值表。
pub fn logical_or<F>(lhs: EvalResult<i64>, rhs: F) -> EvalResult<i64>
where
    F: FnOnce() -> EvalResult<i64>,
{
    let lhs = lhs?;
    if matches!(lhs, Some(value) if value != 0) {
        return Ok(Some(1));
    }

    let rhs = rhs()?;
    if matches!(rhs, Some(value) if value != 0) {
        return Ok(Some(1));
    }
    if lhs.is_none() || rhs.is_none() {
        Ok(None)
    } else {
        Ok(Some(0))
    }
}

/// 逻辑异或：任一侧为 NULL 则结果为 NULL；两侧均非 NULL 时按布尔异或。
pub fn logical_xor<F>(lhs: EvalResult<i64>, rhs: F) -> EvalResult<i64>
where
    F: FnOnce() -> EvalResult<i64>,
{
    let Some(lhs) = lhs? else {
        return Ok(None);
    };
    let Some(rhs) = rhs()? else {
        return Ok(None);
    };
    Ok(Some(i64::from((lhs != 0) ^ (rhs != 0))))
}

/// 位运算公共路径：任一侧 NULL 则结果 NULL，否则对两整数应用 `operation`。
fn bit_binary<R, F>(lhs: EvalResult<i64>, rhs: R, operation: F) -> EvalResult<i64>
where
    R: FnOnce() -> EvalResult<i64>,
    F: FnOnce(i64, i64) -> i64,
{
    let Some(lhs) = lhs? else {
        return Ok(None);
    };
    let Some(rhs) = rhs()? else {
        return Ok(None);
    };
    Ok(Some(operation(lhs, rhs)))
}

/// 按位与。
pub fn bit_and<F>(lhs: EvalResult<i64>, rhs: F) -> EvalResult<i64>
where
    F: FnOnce() -> EvalResult<i64>,
{
    bit_binary(lhs, rhs, |lhs, rhs| lhs & rhs)
}

/// 按位或。
pub fn bit_or<F>(lhs: EvalResult<i64>, rhs: F) -> EvalResult<i64>
where
    F: FnOnce() -> EvalResult<i64>,
{
    bit_binary(lhs, rhs, |lhs, rhs| lhs | rhs)
}

/// 按位异或。
pub fn bit_xor<F>(lhs: EvalResult<i64>, rhs: F) -> EvalResult<i64>
where
    F: FnOnce() -> EvalResult<i64>,
{
    bit_binary(lhs, rhs, |lhs, rhs| lhs ^ rhs)
}

/// 按位取反。
pub fn bit_neg(value: EvalResult<i64>) -> EvalResult<i64> {
    value.map(|value| value.map(|value| !value))
}

/// Go shifts by `uint64(rhs)` and yields zero when the count is at least the word width.
/// 左移：按 `uint64(rhs)` 移位，移位计数达到字宽及以上时结果为 0。
pub fn left_shift<F>(lhs: EvalResult<i64>, rhs: F) -> EvalResult<i64>
where
    F: FnOnce() -> EvalResult<i64>,
{
    bit_binary(lhs, rhs, |lhs, rhs| {
        let shift = rhs as u64;
        if shift >= u64::BITS as u64 {
            0
        } else {
            ((lhs as u64) << shift) as i64
        }
    })
}

/// 逻辑右移：按无符号计数，达到字宽及以上时结果为 0。
pub fn right_shift<F>(lhs: EvalResult<i64>, rhs: F) -> EvalResult<i64>
where
    F: FnOnce() -> EvalResult<i64>,
{
    bit_binary(lhs, rhs, |lhs, rhs| {
        let shift = rhs as u64;
        if shift >= u64::BITS as u64 {
            0
        } else {
            ((lhs as u64) >> shift) as i64
        }
    })
}

/// IS TRUE/FALSE 公共实现：`keep_null` 控制 NULL 是否保留；`invert` 表示测假。
fn truth_test(value: EvalResult<BooleanValue>, keep_null: bool, invert: bool) -> EvalResult<i64> {
    match value? {
        None if keep_null => Ok(None),
        None => Ok(Some(0)),
        Some(value) => {
            let truth = !value.is_zero();
            Ok(Some(i64::from(if invert { !truth } else { truth })))
        }
    }
}

/// IS TRUE：非零为真；NULL 依 `keep_null` 返回 NULL 或 0。
pub fn is_true(value: EvalResult<BooleanValue>, keep_null: bool) -> EvalResult<i64> {
    truth_test(value, keep_null, false)
}

/// IS FALSE：零为真（返回 1）；NULL 依 `keep_null`。
pub fn is_false(value: EvalResult<BooleanValue>, keep_null: bool) -> EvalResult<i64> {
    truth_test(value, keep_null, true)
}

/// 一元逻辑非：零 -> 1，非零 -> 0；NULL 传播。
pub fn unary_not(value: EvalResult<BooleanValue>) -> EvalResult<i64> {
    match value? {
        None => Ok(None),
        Some(value) => Ok(Some(i64::from(value.is_zero()))),
    }
}

/// 判断一元负号是否会溢出：有符号 MIN，或无符号大于 2^63。
pub fn handle_int_overflow(value: IntValue) -> bool {
    match value {
        IntValue::Signed(value) => value == i64::MIN,
        IntValue::Unsigned(value) => value > (1_u64 << 63),
    }
}

/// 整数一元负号；溢出返回 BIGINT out of range。
pub fn unary_minus_int(value: EvalResult<IntValue>) -> EvalResult<i64> {
    let Some(value) = value? else {
        return Ok(None);
    };
    match value {
        IntValue::Signed(i64::MIN) => Err(EvalError::Overflow {
            type_name: "BIGINT",
            value: format!("-{value}", value = i64::MIN),
        }),
        IntValue::Signed(value) => Ok(Some(-value)),
        IntValue::Unsigned(value) if value > (1_u64 << 63) => Err(EvalError::Overflow {
            type_name: "BIGINT",
            value: format!("-{value}"),
        }),
        IntValue::Unsigned(value) if value == (1_u64 << 63) => Ok(Some(i64::MIN)),
        IntValue::Unsigned(value) => Ok(Some(-(value as i64))),
    }
}

/// DECIMAL 一元负号。
pub fn unary_minus_decimal(value: EvalResult<Decimal>) -> EvalResult<Decimal> {
    match value? {
        None => Ok(None),
        Some(value) => value
            .checked_mul(Decimal::NEGATIVE_ONE)
            .map(Some)
            .ok_or_else(|| EvalError::Overflow {
                type_name: "DECIMAL",
                value: format!("-{value}"),
            }),
    }
}

/// 浮点一元负号。
pub fn unary_minus_real(value: EvalResult<f64>) -> EvalResult<f64> {
    value.map(|value| value.map(|value| -value))
}

/// IS NULL always returns a non-NULL integer unless evaluating its argument failed.
/// IS NULL：参数求值成功时结果恒为非 NULL 整数。
pub fn is_null<T>(value: EvalResult<T>) -> EvalResult<i64> {
    value.map(|value| Some(i64::from(value.is_none())))
}

// The following function-class and signature types intentionally mirror the Go declarations.
// They keep the migration usable before the package-wide expression executor is wired: a class
// performs arity/type selection, while a signature owns immutable execution options and delegates
// to the context-free evaluators above.  `Clone` is derived because all retained state is immutable,
// matching each Go `Clone` method's `cloneFrom` plus field-copy behavior.

// 以下函数类与签名类型刻意镜像 Go 声明：类负责参数个数/类型选择，签名持有不可变执行选项
// 并委托到上文无上下文求值器；全部状态不可变故可 `Clone`，对齐 Go 的 cloneFrom 行为。

/// 生成二元运算符函数类：校验二元参数并返回对应签名。
macro_rules! binary_function_class {
    ($name:ident, $operation:expr) => {
        #[derive(Clone, Debug, Default, PartialEq, Eq)]
        pub struct $name;

        impl $name {
            pub fn get_function(&self, actual_args: usize) -> Result<Signature, EvalError> {
                validate_arity(actual_args, 2)?;
                Ok(binary_signature($operation))
            }
        }
    };
}

binary_function_class!(LogicAndFunctionClass, BinaryOp::LogicAnd);
binary_function_class!(LogicOrFunctionClass, BinaryOp::LogicOr);
binary_function_class!(LogicXorFunctionClass, BinaryOp::LogicXor);
binary_function_class!(BitAndFunctionClass, BinaryOp::BitAnd);
binary_function_class!(BitOrFunctionClass, BinaryOp::BitOr);
binary_function_class!(BitXorFunctionClass, BinaryOp::BitXor);
binary_function_class!(LeftShiftFunctionClass, BinaryOp::LeftShift);
binary_function_class!(RightShiftFunctionClass, BinaryOp::RightShift);

#[derive(Clone, Debug, PartialEq, Eq)]
/// IS TRUE / IS FALSE 函数类。
pub struct IsTrueOrFalseFunctionClass {
    pub operation: TruthOp,
    pub keep_null: bool,
}

impl IsTrueOrFalseFunctionClass {
    /// 显示名："IS TRUE" 或 "IS FALSE"。
    pub const fn get_display_name(&self) -> &'static str {
        truth_display_name(self.operation)
    }

    /// 按参数类型与 keep_null 选择真值签名。
    pub fn get_function(
        &self,
        actual_args: usize,
        argument_type: EvalType,
    ) -> Result<Signature, EvalError> {
        validate_arity(actual_args, 1)?;
        truth_signature(self.operation, argument_type, self.keep_null)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 按位取反函数类。
pub struct BitNegFunctionClass;

impl BitNegFunctionClass {
    pub fn get_function(&self, actual_args: usize) -> Result<Signature, EvalError> {
        validate_arity(actual_args, 1)?;
        Ok(bit_neg_signature())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 一元逻辑非函数类。
pub struct UnaryNotFunctionClass;

impl UnaryNotFunctionClass {
    pub fn get_function(
        &self,
        actual_args: usize,
        argument_type: EvalType,
    ) -> Result<Signature, EvalError> {
        validate_arity(actual_args, 1)?;
        unary_not_signature(argument_type)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 一元负号函数类：含类型推断与溢出常量处理。
pub struct UnaryMinusFunctionClass;

impl UnaryMinusFunctionClass {
    pub fn handle_int_overflow(&self, value: IntValue) -> bool {
        handle_int_overflow(value)
    }

    /// Matches Go `typeInfer`: non-int/non-decimal inputs first infer REAL, while overflowing
    /// integer constants infer DECIMAL.
    /// 对齐 Go `typeInfer`：非 int/decimal 先推断为 REAL；溢出整型常量推断为 DECIMAL。
    pub fn type_infer(
        &self,
        metadata: &ExprMetadata,
        constant: Option<IntValue>,
    ) -> (EvalType, bool) {
        let mut inferred = match metadata.eval_type {
            EvalType::Int | EvalType::Decimal => metadata.eval_type,
            _ => EvalType::Real,
        };
        let overflow = inferred == EvalType::Int && constant.is_some_and(handle_int_overflow);
        if overflow {
            inferred = EvalType::Decimal;
        }
        (inferred, overflow)
    }

    pub fn get_function(
        &self,
        actual_args: usize,
        metadata: &ExprMetadata,
        constant: Option<IntValue>,
    ) -> Result<Signature, EvalError> {
        validate_arity(actual_args, 1)?;
        unary_minus_signature(metadata, constant)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// IS NULL 函数类。
pub struct IsNullFunctionClass;

impl IsNullFunctionClass {
    pub fn get_function(
        &self,
        actual_args: usize,
        argument_type: EvalType,
    ) -> Result<Signature, EvalError> {
        validate_arity(actual_args, 1)?;
        is_null_signature(argument_type)
    }
}

/// 生成返回 Int 的二元签名包装，委托到对应求值函数。
macro_rules! binary_int_signature {
    ($name:ident, $evaluate:ident) => {
        #[derive(Clone, Debug, Default, PartialEq, Eq)]
        pub struct $name;

        impl $name {
            pub fn eval_int<F>(&self, lhs: EvalResult<i64>, rhs: F) -> EvalResult<i64>
            where
                F: FnOnce() -> EvalResult<i64>,
            {
                $evaluate(lhs, rhs)
            }
        }
    };
}

binary_int_signature!(BuiltinLogicAndSig, logical_and);
binary_int_signature!(BuiltinLogicOrSig, logical_or);
binary_int_signature!(BuiltinLogicXorSig, logical_xor);
binary_int_signature!(BuiltinBitAndSig, bit_and);
binary_int_signature!(BuiltinBitOrSig, bit_or);
binary_int_signature!(BuiltinBitXorSig, bit_xor);
binary_int_signature!(BuiltinLeftShiftSig, left_shift);
binary_int_signature!(BuiltinRightShiftSig, right_shift);

/// 将求值结果映射为 BooleanValue 族，供真值/NOT 使用。
fn map_boolean_value<T, F>(value: EvalResult<T>, map: F) -> EvalResult<BooleanValue>
where
    F: FnOnce(T) -> BooleanValue,
{
    value.map(|value| value.map(map))
}

/// 生成 IS TRUE/FALSE 签名结构体。
macro_rules! truth_signature {
    ($name:ident, $input:ty, $variant:ident, $evaluate:ident) => {
        #[derive(Clone, Debug, PartialEq, Eq)]
        pub struct $name {
            pub keep_null: bool,
        }

        impl $name {
            pub fn eval_int(&self, value: EvalResult<$input>) -> EvalResult<i64> {
                $evaluate(
                    map_boolean_value(value, BooleanValue::$variant),
                    self.keep_null,
                )
            }
        }
    };
}

truth_signature!(BuiltinRealIsTrueSig, f64, Real, is_true);
truth_signature!(BuiltinDecimalIsTrueSig, Decimal, Decimal, is_true);
truth_signature!(BuiltinIntIsTrueSig, i64, Int, is_true);
truth_signature!(
    BuiltinVectorFloat32IsTrueSig,
    Vec<f32>,
    VectorFloat32,
    is_true
);
truth_signature!(BuiltinRealIsFalseSig, f64, Real, is_false);
truth_signature!(BuiltinDecimalIsFalseSig, Decimal, Decimal, is_false);
truth_signature!(BuiltinIntIsFalseSig, i64, Int, is_false);
truth_signature!(
    BuiltinVectorFloat32IsFalseSig,
    Vec<f32>,
    VectorFloat32,
    is_false
);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 按位取反签名。
pub struct BuiltinBitNegSig;

impl BuiltinBitNegSig {
    pub fn eval_int(&self, value: EvalResult<i64>) -> EvalResult<i64> {
        bit_neg(value)
    }
}

/// 生成一元 NOT 签名结构体。
macro_rules! unary_not_signature {
    ($name:ident, $input:ty, $variant:ident) => {
        #[derive(Clone, Debug, Default, PartialEq, Eq)]
        pub struct $name;

        impl $name {
            pub fn eval_int(&self, value: EvalResult<$input>) -> EvalResult<i64> {
                unary_not(map_boolean_value(value, BooleanValue::$variant))
            }
        }
    };
}

unary_not_signature!(BuiltinUnaryNotRealSig, f64, Real);
unary_not_signature!(BuiltinUnaryNotDecimalSig, Decimal, Decimal);
unary_not_signature!(BuiltinUnaryNotIntSig, i64, Int);
unary_not_signature!(BuiltinUnaryNotJsonSig, JsonValue, Json);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 整数一元负号签名。
pub struct BuiltinUnaryMinusIntSig;

impl BuiltinUnaryMinusIntSig {
    pub fn eval_int(&self, value: EvalResult<IntValue>) -> EvalResult<i64> {
        unary_minus_int(value)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// DECIMAL 一元负号签名；可标记常量参数溢出。
pub struct BuiltinUnaryMinusDecimalSig {
    pub constant_arg_overflow: bool,
}

impl BuiltinUnaryMinusDecimalSig {
    pub fn eval_decimal(&self, value: EvalResult<Decimal>) -> EvalResult<Decimal> {
        unary_minus_decimal(value)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 浮点一元负号签名。
pub struct BuiltinUnaryMinusRealSig;

impl BuiltinUnaryMinusRealSig {
    pub fn eval_real(&self, value: EvalResult<f64>) -> EvalResult<f64> {
        unary_minus_real(value)
    }
}

/// 生成各类型 IS NULL 签名。
macro_rules! is_null_signature {
    ($name:ident) => {
        #[derive(Clone, Debug, Default, PartialEq, Eq)]
        pub struct $name;

        impl $name {
            pub fn eval_int<T>(&self, value: EvalResult<T>) -> EvalResult<i64> {
                is_null(value)
            }
        }
    };
}

is_null_signature!(BuiltinDecimalIsNullSig);
is_null_signature!(BuiltinDurationIsNullSig);
is_null_signature!(BuiltinIntIsNullSig);
is_null_signature!(BuiltinRealIsNullSig);
is_null_signature!(BuiltinStringIsNullSig);
is_null_signature!(BuiltinVectorFloat32IsNullSig);
is_null_signature!(BuiltinTimeIsNullSig);

/// 真值操作的显示名称。
pub const fn truth_display_name(operation: TruthOp) -> &'static str {
    match operation {
        TruthOp::IsTruth => "IS TRUE",
        TruthOp::IsFalsity => "IS FALSE",
    }
}

/// 构造二元逻辑/位运算签名（逻辑结果 flen=1，位运算标记 unsigned）。
pub fn binary_signature(operation: BinaryOp) -> Signature {
    let (pb_code, logical) = match operation {
        BinaryOp::LogicAnd => (ScalarFuncSig::LogicalAnd, true),
        BinaryOp::LogicOr => (ScalarFuncSig::LogicalOr, true),
        BinaryOp::LogicXor => (ScalarFuncSig::LogicalXor, true),
        BinaryOp::BitAnd => (ScalarFuncSig::BitAnd, false),
        BinaryOp::BitOr => (ScalarFuncSig::BitOr, false),
        BinaryOp::BitXor => (ScalarFuncSig::BitXor, false),
        BinaryOp::LeftShift => (ScalarFuncSig::LeftShift, false),
        BinaryOp::RightShift => (ScalarFuncSig::RightShift, false),
    };
    let mut signature = Signature::new(EvalType::Int, EvalType::Int, pb_code);
    signature.flen = if logical { 1 } else { -1 };
    signature.unsigned_result = !logical;
    signature
}

/// 构造按位取反签名。
pub fn bit_neg_signature() -> Signature {
    let mut signature = Signature::new(EvalType::Int, EvalType::Int, ScalarFuncSig::BitNeg);
    signature.unsigned_result = true;
    signature
}

/// 按操作/参数类型/keep_null 选择 IS TRUE/FALSE 的 PB 码与签名。
pub fn truth_signature(
    operation: TruthOp,
    argument_type: EvalType,
    keep_null: bool,
) -> Result<Signature, EvalError> {
    let argument_type = match argument_type {
        EvalType::Timestamp
        | EvalType::Datetime
        | EvalType::Duration
        | EvalType::Json
        | EvalType::String => EvalType::Real,
        other => other,
    };
    let pb_code = match (operation, argument_type, keep_null) {
        (TruthOp::IsTruth, EvalType::Real, false) => Some(ScalarFuncSig::RealIsTrue),
        (TruthOp::IsTruth, EvalType::Real, true) => Some(ScalarFuncSig::RealIsTrueWithNull),
        (TruthOp::IsTruth, EvalType::Decimal, false) => Some(ScalarFuncSig::DecimalIsTrue),
        (TruthOp::IsTruth, EvalType::Decimal, true) => Some(ScalarFuncSig::DecimalIsTrueWithNull),
        (TruthOp::IsTruth, EvalType::Int, false) => Some(ScalarFuncSig::IntIsTrue),
        (TruthOp::IsTruth, EvalType::Int, true) => Some(ScalarFuncSig::IntIsTrueWithNull),
        (TruthOp::IsFalsity, EvalType::Real, false) => Some(ScalarFuncSig::RealIsFalse),
        (TruthOp::IsFalsity, EvalType::Real, true) => Some(ScalarFuncSig::RealIsFalseWithNull),
        (TruthOp::IsFalsity, EvalType::Decimal, false) => Some(ScalarFuncSig::DecimalIsFalse),
        (TruthOp::IsFalsity, EvalType::Decimal, true) => {
            Some(ScalarFuncSig::DecimalIsFalseWithNull)
        }
        (TruthOp::IsFalsity, EvalType::Int, false) => Some(ScalarFuncSig::IntIsFalse),
        (TruthOp::IsFalsity, EvalType::Int, true) => Some(ScalarFuncSig::IntIsFalseWithNull),
        (_, EvalType::VectorFloat32, _) => None,
        (_, unsupported, _) => {
            return Err(EvalError::Unsupported(format!(
                "unexpected EvalType {unsupported:?}"
            )));
        }
    };

    Ok(Signature {
        argument_type,
        return_type: EvalType::Int,
        pb_code,
        flen: 1,
        decimal: -1,
        unsigned_result: false,
        keep_null,
        constant_arg_overflow: false,
        warning: None,
    })
}

/// 一元 NOT 的类型提升与 PB 码选择；JSON 附带 boolean context 警告。
pub fn unary_not_signature(argument_type: EvalType) -> Result<Signature, EvalError> {
    let argument_type = match argument_type {
        EvalType::Timestamp | EvalType::Datetime | EvalType::Duration => EvalType::Int,
        EvalType::String => EvalType::Real,
        other => other,
    };
    let (pb_code, warning) = match argument_type {
        EvalType::Real => (ScalarFuncSig::UnaryNotReal, None),
        EvalType::Decimal => (ScalarFuncSig::UnaryNotDecimal, None),
        EvalType::Int => (ScalarFuncSig::UnaryNotInt, None),
        EvalType::Json => (
            ScalarFuncSig::UnaryNotJson,
            Some("JSON value used in a boolean context"),
        ),
        unsupported => {
            return Err(EvalError::Unsupported(format!(
                "{unsupported:?} is not supported for unary not operator"
            )));
        }
    };
    let mut signature = Signature::new(argument_type, EvalType::Int, pb_code);
    signature.flen = 1;
    signature.warning = warning;
    Ok(signature)
}

/// 一元负号返回类型推断：溢出整型常量提升为 DECIMAL，并调整 flen。
pub fn unary_minus_signature(
    metadata: &ExprMetadata,
    constant: Option<IntValue>,
) -> Result<Signature, EvalError> {
    let int_overflow =
        metadata.eval_type == EvalType::Int && constant.is_some_and(handle_int_overflow);

    let (return_type, pb_code, constant_arg_overflow) = match metadata.eval_type {
        EvalType::Int if int_overflow => {
            (EvalType::Decimal, ScalarFuncSig::UnaryMinusDecimal, true)
        }
        EvalType::Int => (EvalType::Int, ScalarFuncSig::UnaryMinusInt, false),
        EvalType::Real => (EvalType::Real, ScalarFuncSig::UnaryMinusReal, false),
        EvalType::Decimal | EvalType::Timestamp | EvalType::Datetime | EvalType::Duration => {
            (EvalType::Decimal, ScalarFuncSig::UnaryMinusDecimal, false)
        }
        EvalType::Json | EvalType::String | EvalType::VectorFloat32 => {
            (EvalType::Real, ScalarFuncSig::UnaryMinusReal, false)
        }
    };

    let keep_column_width = metadata.is_column
        && (metadata.eval_type == EvalType::Decimal
            || (metadata.eval_type == EvalType::Int && !int_overflow && !metadata.unsigned));
    let flen = if keep_column_width {
        metadata.flen
    } else {
        metadata.flen.saturating_add(1)
    };
    let flen = if return_type == EvalType::Decimal {
        flen.min(MYSQL_MAX_DECIMAL_WIDTH)
    } else {
        flen
    };

    Ok(Signature {
        argument_type: return_type,
        return_type,
        pb_code: Some(pb_code),
        flen,
        decimal: if metadata.eval_type == EvalType::Int {
            0
        } else {
            metadata.decimal
        },
        unsigned_result: false,
        keep_null: false,
        constant_arg_overflow,
        warning: None,
    })
}

/// IS NULL 签名：Timestamp 归并到 Datetime，Json 归并到 String。
pub fn is_null_signature(argument_type: EvalType) -> Result<Signature, EvalError> {
    let argument_type = match argument_type {
        EvalType::Timestamp => EvalType::Datetime,
        EvalType::Json => EvalType::String,
        other => other,
    };
    let pb_code = match argument_type {
        EvalType::Int => ScalarFuncSig::IntIsNull,
        EvalType::Decimal => ScalarFuncSig::DecimalIsNull,
        EvalType::Real => ScalarFuncSig::RealIsNull,
        EvalType::Datetime => ScalarFuncSig::TimeIsNull,
        EvalType::Duration => ScalarFuncSig::DurationIsNull,
        EvalType::String => ScalarFuncSig::StringIsNull,
        EvalType::VectorFloat32 => ScalarFuncSig::VectorFloat32IsNull,
        unsupported => {
            return Err(EvalError::Unsupported(format!(
                "{unsupported:?} is not supported for ISNULL()"
            )));
        }
    };
    let mut signature = Signature::new(argument_type, EvalType::Int, pb_code);
    signature.flen = 1;
    Ok(signature)
}

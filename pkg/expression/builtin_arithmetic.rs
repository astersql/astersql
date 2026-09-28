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

// Rust 2024 implementation of `builtin_arithmetic.go`.
//
// The package-level expression traits are migrated in parallel, so this file exposes the
// complete arithmetic kernel behind a small owned `Expression` boundary.  It retains Go's
// type dispatch, unsigned bit-pattern semantics, overflow checks, NULL propagation, division
// policy, DECIMAL precision rules, and vector dimension checks without depending on unfinished
// package-local modules.
//
// 算术内置函数内核（对应 Go `builtin_arithmetic.go`）。
//
// 在独立的小型 `Expression` 边界上实现完整算术求值：类型分派、无符号位模式、
// 溢出检查、NULL 传播、除零策略、DECIMAL 精度规则与向量维度校验。
// EvalType 是求值时使用的抽象类型（Int/Real/Decimal 等），与存储层 FieldType 相对。

use std::fmt;

use bigdecimal::{BigDecimal, FromPrimitive, RoundingMode, ToPrimitive};
use num_traits::Zero;
use thiserror::Error;

/// 未指定显示宽度/精度时的占位值（与 MySQL `UnspecifiedLength` 一致）。
pub const UNSPECIFIED_LENGTH: i32 = -1;
/// 整数类型最大显示宽度。
pub const MAX_INT_WIDTH: i32 = 20;
/// 浮点类型最大显示宽度。
pub const MAX_REAL_WIDTH: i32 = 23;
/// DECIMAL 最大小数位数。
pub const MAX_DECIMAL_SCALE: i32 = 30;
/// DECIMAL 最大总精度（位数）。
pub const MAX_DECIMAL_WIDTH: i32 = 65;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// 表达式求值类型：决定按整数、小数、浮点或向量等路径计算。
pub enum EvalType {
    Int,
    Decimal,
    Real,
    VectorFloat32,
    String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// 更细的源类型编码，含时间、BIT、二进制串、混合类型等。
pub enum TypeCode {
    Int,
    Decimal,
    Real,
    Temporal,
    BinaryString,
    Bit,
    String,
    VectorFloat32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// 字段类型描述：求值类型、显示宽度 flen、小数位、无符号与混合标志。
pub struct FieldType {
    pub eval_type: EvalType,
    pub type_code: TypeCode,
    pub flen: i32,
    pub decimal: i32,
    pub unsigned: bool,
    pub hybrid: bool,
}

impl FieldType {
    /// 构造有符号或无符号整型字段类型。
    pub fn int(unsigned: bool) -> Self {
        Self {
            eval_type: EvalType::Int,
            type_code: TypeCode::Int,
            flen: MAX_INT_WIDTH,
            decimal: 0,
            unsigned,
            hybrid: false,
        }
    }

    /// 构造 DECIMAL 字段类型。
    pub fn decimal(flen: i32, decimal: i32) -> Self {
        Self {
            eval_type: EvalType::Decimal,
            type_code: TypeCode::Decimal,
            flen,
            decimal,
            unsigned: false,
            hybrid: false,
        }
    }

    /// 构造 REAL（DOUBLE）字段类型。
    pub fn real(flen: i32, decimal: i32) -> Self {
        Self {
            eval_type: EvalType::Real,
            type_code: TypeCode::Real,
            flen,
            decimal,
            unsigned: false,
            hybrid: false,
        }
    }

    /// 构造 VectorFloat32 字段类型。
    pub fn vector() -> Self {
        Self {
            eval_type: EvalType::VectorFloat32,
            type_code: TypeCode::VectorFloat32,
            flen: UNSPECIFIED_LENGTH,
            decimal: UNSPECIFIED_LENGTH,
            unsigned: false,
            hybrid: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
/// 一次求值得到的具体值，含 NULL 与多种数值/向量/字符串形态。
pub enum EvalValue {
    Null,
    Int(i64),
    UInt(u64),
    Real(f64),
    Decimal(BigDecimal),
    Vector(Vec<f32>),
    Bytes(Vec<u8>),
    String(String),
}

#[derive(Debug, Clone, PartialEq)]
/// 算术内核使用的常量表达式：值、类型、显示文本与二进制字面量标志。
pub struct Expression {
    value: EvalValue,
    field_type: FieldType,
    display: String,
    constant_binary_literal: bool,
}

impl Expression {
    /// 有符号整型常量。
    pub fn signed(value: i64) -> Self {
        Self::named_signed(value, value.to_string())
    }

    /// 带自定义显示名的有符号整型常量（用于错误信息）。
    pub fn named_signed(value: i64, name: impl Into<String>) -> Self {
        Self {
            value: EvalValue::Int(value),
            field_type: FieldType::int(false),
            display: name.into(),
            constant_binary_literal: false,
        }
    }

    /// 无符号整型常量。
    pub fn unsigned(value: u64) -> Self {
        Self {
            value: EvalValue::UInt(value),
            field_type: FieldType::int(true),
            display: value.to_string(),
            constant_binary_literal: false,
        }
    }

    /// 构造 REAL（DOUBLE）字段类型。
    pub fn real(value: f64) -> Self {
        Self {
            value: EvalValue::Real(value),
            field_type: FieldType::real(UNSPECIFIED_LENGTH, UNSPECIFIED_LENGTH),
            display: value.to_string(),
            constant_binary_literal: false,
        }
    }

    /// 构造 DECIMAL 字段类型。
    pub fn decimal(value: BigDecimal, flen: i32, decimal: i32) -> Self {
        let display = value.to_string();
        Self {
            value: EvalValue::Decimal(value),
            field_type: FieldType::decimal(flen, decimal),
            display,
            constant_binary_literal: false,
        }
    }

    /// 无符号 DECIMAL 常量。
    pub fn unsigned_decimal(value: &str, flen: i32, decimal: i32) -> Self {
        let mut expression =
            Self::decimal(value.parse().expect("valid decimal literal"), flen, decimal);
        expression.field_type.unsigned = true;
        expression
    }

    /// 构造 VectorFloat32 字段类型。
    pub fn vector(value: Vec<f32>) -> Self {
        Self {
            display: format!("{value:?}"),
            value: EvalValue::Vector(value),
            field_type: FieldType::vector(),
            constant_binary_literal: false,
        }
    }

    /// 指定类型的 NULL 常量。
    pub fn null(field_type: FieldType) -> Self {
        Self {
            value: EvalValue::Null,
            field_type,
            display: "NULL".into(),
            constant_binary_literal: false,
        }
    }

    /// 时间类型占位常量；`decimal` 表示小数秒精度（FSP）。
    pub fn temporal(decimal: i32) -> Self {
        Self {
            value: EvalValue::Null,
            field_type: FieldType {
                eval_type: EvalType::String,
                type_code: TypeCode::Temporal,
                flen: UNSPECIFIED_LENGTH,
                decimal,
                unsigned: false,
                hybrid: false,
            },
            display: "temporal".into(),
            constant_binary_literal: false,
        }
    }

    /// 二进制字面量（如 `_binary`），数值上下文常按整数处理。
    pub fn binary_literal(value: u64) -> Self {
        Self {
            value: EvalValue::UInt(value),
            field_type: FieldType {
                eval_type: EvalType::String,
                type_code: TypeCode::BinaryString,
                flen: UNSPECIFIED_LENGTH,
                decimal: 0,
                unsigned: true,
                hybrid: false,
            },
            display: format!("0x{value:x}"),
            constant_binary_literal: true,
        }
    }

    /// BIT 字面量。
    pub fn bit_literal(value: u64) -> Self {
        Self {
            value: EvalValue::UInt(value),
            field_type: FieldType {
                eval_type: EvalType::Int,
                type_code: TypeCode::Bit,
                flen: MAX_INT_WIDTH,
                decimal: 0,
                unsigned: true,
                hybrid: false,
            },
            display: format!("0b{value:b}"),
            constant_binary_literal: false,
        }
    }

    /// 二进制字符串常量。
    pub fn binary_string(value: Vec<u8>) -> Self {
        Self {
            display: format!("{value:?}"),
            value: EvalValue::Bytes(value),
            field_type: FieldType {
                eval_type: EvalType::String,
                type_code: TypeCode::BinaryString,
                flen: UNSPECIFIED_LENGTH,
                decimal: UNSPECIFIED_LENGTH,
                unsigned: false,
                hybrid: false,
            },
            constant_binary_literal: false,
        }
    }

    /// 混合类型字符串（ENUM/SET 一类，数值上下文可转数字）。
    pub fn hybrid_string(value: impl Into<String>) -> Self {
        let value = value.into();
        Self {
            display: value.clone(),
            value: EvalValue::String(value),
            field_type: FieldType {
                eval_type: EvalType::String,
                type_code: TypeCode::String,
                flen: UNSPECIFIED_LENGTH,
                decimal: UNSPECIFIED_LENGTH,
                unsigned: false,
                hybrid: true,
            },
            constant_binary_literal: false,
        }
    }

    /// 返回表达式的字段类型。
    pub fn field_type(&self) -> &FieldType {
        &self.field_type
    }

    /// 取出底层 i64 位模式（无符号也按位解释）。
    fn raw_i64(&self) -> Result<i64, ArithmeticError> {
        match &self.value {
            EvalValue::Int(value) => Ok(*value),
            EvalValue::UInt(value) => Ok(*value as i64),
            other => Err(ArithmeticError::InvalidType(format!(
                "expected integer, got {other:?}"
            ))),
        }
    }

    /// 转换为 f64，失败则报错。
    fn as_real(&self) -> Result<f64, ArithmeticError> {
        match &self.value {
            EvalValue::Int(value) => Ok(*value as f64),
            EvalValue::UInt(value) => Ok(*value as f64),
            EvalValue::Real(value) => Ok(*value),
            EvalValue::Decimal(value) => value.to_f64().ok_or_else(|| {
                ArithmeticError::InvalidType("DECIMAL cannot be represented as DOUBLE".into())
            }),
            other => Err(ArithmeticError::InvalidType(format!(
                "expected numeric value, got {other:?}"
            ))),
        }
    }

    /// 转换为 `BigDecimal`。
    fn as_decimal(&self) -> Result<BigDecimal, ArithmeticError> {
        match &self.value {
            EvalValue::Int(value) => Ok(BigDecimal::from(*value)),
            EvalValue::UInt(value) => Ok(BigDecimal::from(*value)),
            EvalValue::Real(value) => BigDecimal::from_f64(*value).ok_or_else(|| {
                ArithmeticError::InvalidType("non-finite DOUBLE cannot become DECIMAL".into())
            }),
            EvalValue::Decimal(value) => Ok(value.clone()),
            other => Err(ArithmeticError::InvalidType(format!(
                "expected numeric value, got {other:?}"
            ))),
        }
    }

    /// 取出 float32 向量切片。
    fn as_vector(&self) -> Result<&[f32], ArithmeticError> {
        match &self.value {
            EvalValue::Vector(value) => Ok(value),
            other => Err(ArithmeticError::InvalidType(format!(
                "expected VECTOR FLOAT32, got {other:?}"
            ))),
        }
    }
}

/// 判断表达式是否为常量二进制字面量。
pub fn is_constant_binary_literal(expression: &Expression) -> bool {
    expression.field_type.type_code == TypeCode::BinaryString && expression.constant_binary_literal
}

/// Go's numeric-context coercion order: temporal, constant binary/bit, normal numeric, hybrid.
/// 在数值运算上下文中推导操作数的求值类型（时间/BIT/混合等特殊规则）。
// 数值上下文类型推导：时间按 FSP 选 Int/Decimal，BIT/二进制字面量等走特殊规则。
pub fn numeric_context_result_type(expression: &Expression) -> EvalType {
    let field_type = expression.field_type();
    if field_type.type_code == TypeCode::Temporal {
        return if field_type.decimal > 0 {
            EvalType::Decimal
        } else {
            EvalType::Int
        };
    }
    if is_constant_binary_literal(expression) || field_type.type_code == TypeCode::Bit {
        return EvalType::Int;
    }
    if field_type.hybrid {
        return EvalType::Real;
    }
    match field_type.eval_type {
        EvalType::Int | EvalType::Decimal => field_type.eval_type,
        EvalType::VectorFloat32 => EvalType::VectorFloat32,
        _ => EvalType::Real,
    }
}

/// 将 decimal 精度夹紧到合法范围。
fn clamp_decimal(value: i32) -> i32 {
    value.clamp(0, MAX_DECIMAL_SCALE)
}

/// Mirrors `setFlenDecimal4RealOrDecimal` for plus, minus, and multiply.
/// 按加减乘规则设置 REAL/DECIMAL 结果的 flen 与 decimal。
pub fn set_flen_decimal_for_real_or_decimal(
    a: &FieldType,
    b: &FieldType,
    is_real: bool,
    is_multiply: bool,
) -> FieldType {
    let mut result = if is_real {
        FieldType::real(UNSPECIFIED_LENGTH, UNSPECIFIED_LENGTH)
    } else {
        FieldType::decimal(MAX_DECIMAL_WIDTH, MAX_DECIMAL_SCALE)
    };

    if a.decimal == UNSPECIFIED_LENGTH || b.decimal == UNSPECIFIED_LENGTH {
        if is_real {
            result.flen = UNSPECIFIED_LENGTH;
            result.decimal = UNSPECIFIED_LENGTH;
        }
        return result;
    }

    result.decimal = if is_multiply {
        a.decimal + b.decimal
    } else {
        a.decimal.max(b.decimal)
    };
    result.decimal = clamp_decimal(result.decimal);
    if a.flen == UNSPECIFIED_LENGTH || b.flen == UNSPECIFIED_LENGTH {
        result.flen = UNSPECIFIED_LENGTH;
        return result;
    }

    result.flen = if is_multiply {
        a.flen - a.decimal + b.flen - b.decimal + result.decimal
    } else {
        (a.flen - a.decimal).max(b.flen - b.decimal) + result.decimal + 1
    };
    result.flen = result.flen.min(if is_real {
        MAX_REAL_WIDTH
    } else {
        MAX_DECIMAL_WIDTH
    });
    result
}

/// 由显示长度与 scale 推算 DECIMAL 精度。
fn decimal_length_to_precision(length: i32, scale: i32, unsigned: bool) -> i32 {
    let mut precision = length;
    if scale > 0 {
        precision -= 1;
    }
    if unsigned || precision > 0 {
        precision -= 1;
    }
    precision
}

/// 由精度与 scale 反推显示长度（不截断）。
fn precision_to_length_no_truncation(precision: i32, scale: i32, unsigned: bool) -> i32 {
    let mut length = precision;
    if scale > 0 {
        length += 1;
    }
    if unsigned || length > 0 {
        length += 1;
    }
    length
}

/// 推导 DECIMAL 除法结果类型；`increment` 为精度增量。
pub fn set_type_for_div_decimal(a: &FieldType, b: &FieldType, increment: i32) -> FieldType {
    let dec_a = if a.decimal == UNSPECIFIED_LENGTH {
        0
    } else {
        a.decimal
    };
    let dec_b = if b.decimal == UNSPECIFIED_LENGTH {
        0
    } else {
        b.decimal
    };
    let mut result = FieldType::decimal(MAX_DECIMAL_WIDTH, clamp_decimal(dec_a + increment));
    if a.flen == UNSPECIFIED_LENGTH {
        return result;
    }
    let precision = decimal_length_to_precision(a.flen, a.decimal, a.unsigned);
    let result_precision = (precision + dec_b + increment).min(MAX_DECIMAL_WIDTH);
    result.flen =
        precision_to_length_no_truncation(result_precision, result.decimal, result.unsigned)
            .min(MAX_DECIMAL_WIDTH);
    result
}

/// 推导 REAL 除法结果类型。
pub fn set_type_for_div_real() -> FieldType {
    FieldType::real(MAX_REAL_WIDTH, UNSPECIFIED_LENGTH)
}

/// 推导取模结果类型。
pub fn set_type_for_mod(a: &FieldType, b: &FieldType, is_decimal: bool) -> FieldType {
    let mut result = if is_decimal {
        FieldType::decimal(UNSPECIFIED_LENGTH, UNSPECIFIED_LENGTH)
    } else {
        FieldType::real(UNSPECIFIED_LENGTH, UNSPECIFIED_LENGTH)
    };
    result.decimal = if a.decimal == UNSPECIFIED_LENGTH || b.decimal == UNSPECIFIED_LENGTH {
        UNSPECIFIED_LENGTH
    } else {
        clamp_decimal(a.decimal.max(b.decimal))
    };
    result.flen = if a.flen == UNSPECIFIED_LENGTH || b.flen == UNSPECIFIED_LENGTH {
        UNSPECIFIED_LENGTH
    } else {
        a.flen.max(b.flen).min(if is_decimal {
            MAX_DECIMAL_WIDTH
        } else {
            MAX_REAL_WIDTH
        })
    };
    result.unsigned = a.unsigned;
    result
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// 算术运算符：加、减、乘、除、整除、取模。
pub enum ArithmeticOp {
    Plus,
    Minus,
    Multiply,
    Divide,
    IntDivide,
    Mod,
}

impl ArithmeticOp {
    /// 返回运算符的 SQL 符号文本。
    fn symbol(self) -> &'static str {
        match self {
            Self::Plus => "+",
            Self::Minus => "-",
            Self::Multiply => "*",
            Self::Divide => "/",
            Self::IntDivide => "DIV",
            Self::Mod => "%",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// 具体求值签名（按操作数/结果类型细分的实现入口）。
pub enum Signature {
    PlusInt,
    PlusDecimal,
    PlusReal,
    PlusVectorFloat32,
    MinusInt,
    MinusDecimal,
    MinusReal,
    MinusVectorFloat32,
    MultiplyInt,
    MultiplyIntUnsigned,
    MultiplyDecimal,
    MultiplyReal,
    MultiplyVectorFloat32,
    DivideReal,
    DivideDecimal,
    IntDivideInt,
    IntDivideDecimal,
    ModReal,
    ModDecimal,
    ModIntUnsignedUnsigned,
    ModIntUnsignedSigned,
    ModIntSignedUnsigned,
    ModIntSignedSigned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// 算术过程中的警告（如除零转 NULL）。
pub enum ArithmeticWarning {
    DivisionByZero,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
/// 算术错误：溢出、除零、截断、维度不匹配等。
pub enum ArithmeticError {
    #[error("{ty} value is out of range in '{expression}'")]
    Overflow {
        ty: &'static str,
        expression: String,
    },
    #[error("division by zero")]
    DivisionByZero,
    #[error("vector dimensions differ: {left} and {right}")]
    VectorDimension { left: usize, right: usize },
    #[error("invalid arithmetic type: {0}")]
    InvalidType(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// 求值上下文：除法精度增量、除零是否报错、是否禁止无符号减法等会话级开关。
pub struct EvalContext {
    pub no_unsigned_subtraction: bool,
    pub div_precision_increment: i32,
    pub division_by_zero_as_error: bool,
    pub warnings: Vec<ArithmeticWarning>,
}

impl Default for EvalContext {
    fn default() -> Self {
        Self {
            no_unsigned_subtraction: false,
            div_precision_increment: 4,
            division_by_zero_as_error: false,
            warnings: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
/// 已分派签名的二元算术表达式，可对两个操作数求值。
pub struct ArithmeticExpr {
    op: ArithmeticOp,
    lhs: Expression,
    rhs: Expression,
    signature: Signature,
    result_type: FieldType,
}

impl ArithmeticExpr {
    /// 根据运算符与操作数类型选择签名并构造 `ArithmeticExpr`。
    pub fn build(
        op: ArithmeticOp,
        lhs: Expression,
        rhs: Expression,
        context: &EvalContext,
    ) -> Result<Self, ArithmeticError> {
        let left_eval = numeric_context_result_type(&lhs);
        let right_eval = numeric_context_result_type(&rhs);
        let vector = left_eval == EvalType::VectorFloat32 || right_eval == EvalType::VectorFloat32;
        let real = left_eval == EvalType::Real || right_eval == EvalType::Real;
        let decimal = left_eval == EvalType::Decimal || right_eval == EvalType::Decimal;
        let left_unsigned = lhs.field_type.unsigned;
        let right_unsigned = rhs.field_type.unsigned;

        let (signature, result_type) = match op {
            ArithmeticOp::Plus if vector => (Signature::PlusVectorFloat32, FieldType::vector()),
            ArithmeticOp::Plus if real => (
                Signature::PlusReal,
                set_flen_decimal_for_real_or_decimal(&lhs.field_type, &rhs.field_type, true, false),
            ),
            ArithmeticOp::Plus if decimal => (
                Signature::PlusDecimal,
                set_flen_decimal_for_real_or_decimal(
                    &lhs.field_type,
                    &rhs.field_type,
                    false,
                    false,
                ),
            ),
            ArithmeticOp::Plus => (
                Signature::PlusInt,
                FieldType::int(left_unsigned || right_unsigned),
            ),

            ArithmeticOp::Minus if vector => (Signature::MinusVectorFloat32, FieldType::vector()),
            ArithmeticOp::Minus if real => (
                Signature::MinusReal,
                set_flen_decimal_for_real_or_decimal(&lhs.field_type, &rhs.field_type, true, false),
            ),
            ArithmeticOp::Minus if decimal => (
                Signature::MinusDecimal,
                set_flen_decimal_for_real_or_decimal(
                    &lhs.field_type,
                    &rhs.field_type,
                    false,
                    false,
                ),
            ),
            ArithmeticOp::Minus => (
                Signature::MinusInt,
                FieldType::int(
                    (left_unsigned || right_unsigned) && !context.no_unsigned_subtraction,
                ),
            ),

            ArithmeticOp::Multiply if vector => {
                (Signature::MultiplyVectorFloat32, FieldType::vector())
            }
            ArithmeticOp::Multiply if real => (
                Signature::MultiplyReal,
                set_flen_decimal_for_real_or_decimal(&lhs.field_type, &rhs.field_type, true, true),
            ),
            ArithmeticOp::Multiply if decimal => (
                Signature::MultiplyDecimal,
                set_flen_decimal_for_real_or_decimal(&lhs.field_type, &rhs.field_type, false, true),
            ),
            ArithmeticOp::Multiply if left_unsigned || right_unsigned => {
                (Signature::MultiplyIntUnsigned, FieldType::int(true))
            }
            ArithmeticOp::Multiply => (Signature::MultiplyInt, FieldType::int(false)),

            ArithmeticOp::Divide if real => (Signature::DivideReal, set_type_for_div_real()),
            ArithmeticOp::Divide => (
                Signature::DivideDecimal,
                set_type_for_div_decimal(
                    &lhs.field_type,
                    &rhs.field_type,
                    context.div_precision_increment,
                ),
            ),

            ArithmeticOp::IntDivide
                if left_eval == EvalType::Int && right_eval == EvalType::Int =>
            {
                (
                    Signature::IntDivideInt,
                    FieldType::int(left_unsigned || right_unsigned),
                )
            }
            ArithmeticOp::IntDivide => (
                Signature::IntDivideDecimal,
                FieldType::int(left_unsigned || right_unsigned),
            ),

            ArithmeticOp::Mod if real => (
                Signature::ModReal,
                set_type_for_mod(&lhs.field_type, &rhs.field_type, false),
            ),
            ArithmeticOp::Mod if decimal => (
                Signature::ModDecimal,
                set_type_for_mod(&lhs.field_type, &rhs.field_type, true),
            ),
            // 整型取模按左右无符号标志分成四种签名，结果符号跟随左操作数。
            ArithmeticOp::Mod => {
                let signature = match (left_unsigned, right_unsigned) {
                    (true, true) => Signature::ModIntUnsignedUnsigned,
                    (true, false) => Signature::ModIntUnsignedSigned,
                    (false, true) => Signature::ModIntSignedUnsigned,
                    (false, false) => Signature::ModIntSignedSigned,
                };
                (signature, FieldType::int(left_unsigned))
            }
        };

        Ok(Self {
            op,
            lhs,
            rhs,
            signature,
            result_type,
        })
    }

    /// 返回已选择的求值签名。
    pub fn signature(&self) -> Signature {
        self.signature
    }

    /// 返回结果字段类型。
    pub fn result_type(&self) -> &FieldType {
        &self.result_type
    }

    /// 在给定上下文中执行算术运算。
    pub fn eval(&self, context: &mut EvalContext) -> Result<EvalValue, ArithmeticError> {
        if self.lhs.value == EvalValue::Null || self.rhs.value == EvalValue::Null {
            return Ok(EvalValue::Null);
        }
        match self.signature {
            Signature::PlusInt => self.eval_plus_int(),
            Signature::MinusInt => self.eval_minus_int(context.no_unsigned_subtraction),
            Signature::MultiplyInt => self.eval_multiply_int(false),
            Signature::MultiplyIntUnsigned => self.eval_multiply_int(true),
            Signature::PlusReal
            | Signature::MinusReal
            | Signature::MultiplyReal
            | Signature::DivideReal
            | Signature::ModReal => self.eval_real(context),
            Signature::PlusDecimal
            | Signature::MinusDecimal
            | Signature::MultiplyDecimal
            | Signature::DivideDecimal
            | Signature::ModDecimal => self.eval_decimal(context),
            Signature::IntDivideInt => self.eval_int_divide(context),
            Signature::IntDivideDecimal => self.eval_decimal_int_divide(context),
            Signature::ModIntUnsignedUnsigned
            | Signature::ModIntUnsignedSigned
            | Signature::ModIntSignedUnsigned
            | Signature::ModIntSignedSigned => self.eval_int_mod(context),
            Signature::PlusVectorFloat32
            | Signature::MinusVectorFloat32
            | Signature::MultiplyVectorFloat32 => self.eval_vector(),
        }
    }

    /// 生成用于错误信息的表达式文本。
    fn expression_text(&self) -> String {
        format!(
            "({} {} {})",
            self.lhs.display,
            self.op.symbol(),
            self.rhs.display
        )
    }

    /// 构造溢出错误。
    fn overflow(&self, ty: &'static str) -> ArithmeticError {
        ArithmeticError::Overflow {
            ty,
            expression: self.expression_text(),
        }
    }

    /// 按上下文策略处理除零（报错或警告并返回 NULL）。
    // 除零：若上下文要求报错则返回错误，否则记警告并返回 NULL（MySQL 兼容）。
    fn division_by_zero(&self, context: &mut EvalContext) -> Result<EvalValue, ArithmeticError> {
        if context.division_by_zero_as_error {
            Err(ArithmeticError::DivisionByZero)
        } else {
            context.warnings.push(ArithmeticWarning::DivisionByZero);
            Ok(EvalValue::Null)
        }
    }

    /// 按结果有无符号包装整型结果。
    fn integer_result(&self, raw: i64) -> EvalValue {
        if self.result_type.unsigned {
            EvalValue::UInt(raw as u64)
        } else {
            EvalValue::Int(raw)
        }
    }

    /// 整型加法路径。
    // 整型加法：用位模式做有/无符号组合，溢出时返回 ArithmeticError::Overflow。
    fn eval_plus_int(&self) -> Result<EvalValue, ArithmeticError> {
        let a = self.lhs.raw_i64()?;
        let b = self.rhs.raw_i64()?;
        let ua = a as u64;
        let ub = b as u64;
        let left_unsigned = self.lhs.field_type.unsigned;
        let right_unsigned = self.rhs.field_type.unsigned;

        let overflow = match (left_unsigned, right_unsigned) {
            (true, true) => ua.checked_add(ub).is_none(),
            (true, false) => {
                (b < 0 && b.wrapping_neg() as u64 > ua) || (b > 0 && ua > u64::MAX - b as u64)
            }
            (false, true) => {
                (a < 0 && a.wrapping_neg() as u64 > ub) || (a > 0 && ub > u64::MAX - a as u64)
            }
            (false, false) => a.checked_add(b).is_none(),
        };
        if overflow {
            return Err(self.overflow(if left_unsigned || right_unsigned {
                "BIGINT UNSIGNED"
            } else {
                "BIGINT"
            }));
        }
        Ok(self.integer_result(a.wrapping_add(b)))
    }

    /// 整型减法路径；`force_signed` 用于 no_unsigned_subtraction。
    fn eval_minus_int(&self, force_signed: bool) -> Result<EvalValue, ArithmeticError> {
        let a = self.lhs.raw_i64()?;
        let b = self.rhs.raw_i64()?;
        let left_unsigned = self.lhs.field_type.unsigned;
        let right_unsigned = self.rhs.field_type.unsigned;
        let signed = force_signed || (!left_unsigned && !right_unsigned);
        if subtraction_overflows(left_unsigned, right_unsigned, signed, a, b) {
            return Err(self.overflow(if signed { "BIGINT" } else { "BIGINT UNSIGNED" }));
        }
        let raw = a.wrapping_sub(b);
        Ok(if signed {
            EvalValue::Int(raw)
        } else {
            EvalValue::UInt(raw as u64)
        })
    }

    /// 整型乘法路径。
    fn eval_multiply_int(&self, unsigned: bool) -> Result<EvalValue, ArithmeticError> {
        let a = self.lhs.raw_i64()?;
        let b = self.rhs.raw_i64()?;
        if unsigned {
            let result = (a as u64)
                .checked_mul(b as u64)
                .ok_or_else(|| self.overflow("BIGINT UNSIGNED"))?;
            Ok(EvalValue::UInt(result))
        } else {
            let result = a.checked_mul(b).ok_or_else(|| self.overflow("BIGINT"))?;
            Ok(EvalValue::Int(result))
        }
    }

    /// 浮点路径。
    fn eval_real(&self, context: &mut EvalContext) -> Result<EvalValue, ArithmeticError> {
        let a = self.lhs.as_real()?;
        let b = self.rhs.as_real()?;
        if matches!(self.op, ArithmeticOp::Divide | ArithmeticOp::Mod) && b == 0.0 {
            return self.division_by_zero(context);
        }
        let value = match self.op {
            ArithmeticOp::Plus => a + b,
            ArithmeticOp::Minus => a - b,
            ArithmeticOp::Multiply => a * b,
            ArithmeticOp::Divide => a / b,
            ArithmeticOp::Mod => a % b,
            ArithmeticOp::IntDivide => unreachable!(),
        };
        let overflow = match self.op {
            ArithmeticOp::Plus | ArithmeticOp::Minus => !value.is_finite(),
            ArithmeticOp::Multiply | ArithmeticOp::Divide => value.is_infinite(),
            ArithmeticOp::Mod => false,
            ArithmeticOp::IntDivide => unreachable!(),
        };
        if overflow {
            Err(self.overflow("DOUBLE"))
        } else {
            Ok(EvalValue::Real(value))
        }
    }

    /// DECIMAL 路径。
    fn eval_decimal(&self, context: &mut EvalContext) -> Result<EvalValue, ArithmeticError> {
        let a = self.lhs.as_decimal()?;
        let b = self.rhs.as_decimal()?;
        if matches!(self.op, ArithmeticOp::Divide | ArithmeticOp::Mod) && b.is_zero() {
            return self.division_by_zero(context);
        }
        let value = match self.op {
            ArithmeticOp::Plus => a + b,
            ArithmeticOp::Minus => a - b,
            ArithmeticOp::Multiply => a * b,
            ArithmeticOp::Divide => {
                let scale = self.result_type.decimal.max(0) as i64;
                (a / b).with_scale_round(scale, RoundingMode::HalfUp)
            }
            ArithmeticOp::Mod => a % b,
            ArithmeticOp::IntDivide => unreachable!(),
        };
        if decimal_precision(&value) > MAX_DECIMAL_WIDTH as usize {
            Err(self.overflow("DECIMAL"))
        } else {
            Ok(EvalValue::Decimal(value))
        }
    }

    /// 整型 DIV（整除）路径。
    fn eval_int_divide(&self, context: &mut EvalContext) -> Result<EvalValue, ArithmeticError> {
        let a = self.lhs.raw_i64()?;
        let b = self.rhs.raw_i64()?;
        if b == 0 {
            return self.division_by_zero(context);
        }
        let left_unsigned = self.lhs.field_type.unsigned;
        let right_unsigned = self.rhs.field_type.unsigned;
        let result = match (left_unsigned, right_unsigned) {
            (true, true) => (a as u64 / b as u64) as i64,
            (true, false) if b < 0 => {
                let magnitude = b.wrapping_neg() as u64;
                if a as u64 != 0 && magnitude <= a as u64 {
                    return Err(self.overflow("BIGINT UNSIGNED"));
                }
                0
            }
            (true, false) => ((a as u64) / b as u64) as i64,
            (false, true) if a < 0 => {
                let magnitude = a.wrapping_neg() as u64;
                if magnitude >= b as u64 {
                    return Err(self.overflow("BIGINT UNSIGNED"));
                }
                0
            }
            (false, true) => (a as u64 / b as u64) as i64,
            (false, false) if a == i64::MIN && b == -1 => return Err(self.overflow("BIGINT")),
            (false, false) => a / b,
        };
        Ok(self.integer_result(result))
    }

    /// DECIMAL 操作数上的整除路径。
    fn eval_decimal_int_divide(
        &self,
        context: &mut EvalContext,
    ) -> Result<EvalValue, ArithmeticError> {
        let a = self.lhs.as_decimal()?;
        let b = self.rhs.as_decimal()?;
        if b.is_zero() {
            return self.division_by_zero(context);
        }
        let quotient = a / b;
        if self.result_type.unsigned {
            if quotient < BigDecimal::zero() {
                if quotient > BigDecimal::from(-1) {
                    return Ok(EvalValue::UInt(0));
                }
                return Err(self.overflow("BIGINT UNSIGNED"));
            }
            quotient
                .to_u64()
                .map(EvalValue::UInt)
                .ok_or_else(|| self.overflow("BIGINT UNSIGNED"))
        } else {
            quotient
                .to_i64()
                .map(EvalValue::Int)
                .ok_or_else(|| self.overflow("BIGINT"))
        }
    }

    /// 整型取模路径。
    fn eval_int_mod(&self, context: &mut EvalContext) -> Result<EvalValue, ArithmeticError> {
        let a = self.lhs.raw_i64()?;
        let b = self.rhs.raw_i64()?;
        if b == 0 {
            return self.division_by_zero(context);
        }
        let value = match self.signature {
            Signature::ModIntUnsignedUnsigned => EvalValue::UInt((a as u64) % (b as u64)),
            Signature::ModIntUnsignedSigned => {
                let divisor = if b < 0 {
                    b.wrapping_neg() as u64
                } else {
                    b as u64
                };
                EvalValue::UInt((a as u64) % divisor)
            }
            Signature::ModIntSignedUnsigned => {
                let magnitude = if a < 0 {
                    let remainder = (a.wrapping_neg() as u64) % (b as u64);
                    (remainder as i64).wrapping_neg()
                } else {
                    ((a as u64) % (b as u64)) as i64
                };
                EvalValue::Int(magnitude)
            }
            Signature::ModIntSignedSigned => {
                EvalValue::Int(if a == i64::MIN && b == -1 { 0 } else { a % b })
            }
            _ => unreachable!(),
        };
        Ok(value)
    }

    /// 向量逐维算术路径。
    fn eval_vector(&self) -> Result<EvalValue, ArithmeticError> {
        let left = self.lhs.as_vector()?;
        let right = self.rhs.as_vector()?;
        if left.len() != right.len() {
            return Err(ArithmeticError::VectorDimension {
                left: left.len(),
                right: right.len(),
            });
        }
        let value = left
            .iter()
            .zip(right)
            .map(|(left, right)| match self.op {
                ArithmeticOp::Plus => left + right,
                ArithmeticOp::Minus => left - right,
                ArithmeticOp::Multiply => left * right,
                _ => unreachable!(),
            })
            .collect();
        Ok(EvalValue::Vector(value))
    }
}

/// Exact translation of `builtinArithmeticMinusIntSig.overflowCheck`.
/// 判断有/无符号组合下减法是否溢出。
pub fn subtraction_overflows(
    left_unsigned: bool,
    right_unsigned: bool,
    signed_result: bool,
    a: i64,
    b: i64,
) -> bool {
    let result = a.wrapping_sub(b);
    let (ua, ub) = (a as u64, b as u64);
    let mut result_unsigned = false;

    if left_unsigned {
        if right_unsigned {
            if ua < ub {
                if result >= 0 {
                    return true;
                }
            } else {
                result_unsigned = true;
            }
        } else if b >= 0 {
            if ua > ub {
                result_unsigned = true;
            }
        } else {
            if test_if_sum_overflows_ull(ua, b.wrapping_neg() as u64) {
                return true;
            }
            result_unsigned = true;
        }
    } else if right_unsigned {
        if (a.wrapping_sub(i64::MIN) as u64) < ub {
            return true;
        }
    } else if a > 0 && b < 0 {
        result_unsigned = true;
    } else if a < 0 && b > 0 && result >= 0 {
        return true;
    }

    (!signed_result && !result_unsigned && result < 0)
        || (signed_result && result_unsigned && result as u64 > i64::MAX as u64)
}

/// 判断两个 u64 相加是否溢出。
pub fn test_if_sum_overflows_ull(a: u64, b: u64) -> bool {
    u64::MAX - a < b
}

/// 计算 `BigDecimal` 的有效精度。
fn decimal_precision(value: &BigDecimal) -> usize {
    let (integer, scale) = value.as_bigint_and_exponent();
    let digits = integer
        .to_str_radix(10)
        .trim_start_matches('-')
        .len()
        .max(1);
    if scale >= 0 {
        digits.max(scale as usize)
    } else {
        digits + (-scale) as usize
    }
}

impl fmt::Display for EvalValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

#[cfg(test)]
#[path = "builtin_arithmetic_2_aster_unit_test.rs"]
/// 条件编译挂载 Go 对等单元测试模块。
mod builtin_arithmetic_aster_unit_test;

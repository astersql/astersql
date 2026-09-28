// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// Copyright 2013 The ql Authors. All rights reserved.
// Use of this source code is governed by a BSD-style
// license that can be found in the LICENSES/QL-LICENSE file.

// 数学类 SQL 内建函数的标量实现，对应 Go `builtin_math.go`。
//
// 包含 ABS/ROUND/CEIL/FLOOR、对数族、RAND、POW/CONV/CRC32、三角函数，
// 以及按参数 FieldType（字段类型元数据）选择具体 ScalarFuncSignature 的分发逻辑。
// DECIMAL 使用 MyDecimal（MySQL 风格定点数）；溢出映射为与 Go 一致的 SQL 错误文案。

#![allow(non_snake_case)]

use mathutil::{MysqlRng, NewWithSeed};
use std::fmt;
use std::sync::Arc;
use types_dependency::decimal::mydecimal::{
    DecimalAdd, DecimalSub, ModeHalfUp, ModeTruncate, NewDecFromInt, NewDecFromUint,
};
pub use types_dependency::decimal::mydecimal::{DecimalError, MyDecimal};

/// MySQL DECIMAL 允许的最大小数位数上限。
pub const MAX_DECIMAL_SCALE: i32 = 30;
/// BIGINT 显示宽度上界，用于 FLOOR/CEIL 决定返回 Int 还是 Decimal。
const MAX_INT_WIDTH: i32 = 20;

/// 数学求值错误：数值溢出或 DECIMAL 运算失败。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MathError {
    Overflow {
        data_type: &'static str,
        expression: String,
    },
    Decimal(DecimalError),
}

impl MathError {
    /// 构造与 TiDB/MySQL `[types:1690]` 一致的溢出错误。
    fn overflow(data_type: &'static str, expression: impl Into<String>) -> Self {
        Self::Overflow {
            data_type,
            expression: expression.into(),
        }
    }
}

impl fmt::Display for MathError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Overflow {
                data_type,
                expression,
            } => {
                write!(
                    formatter,
                    "[types:1690]{data_type} value is out of range in '{expression}'"
                )
            }
            Self::Decimal(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for MathError {}

impl From<DecimalError> for MathError {
    fn from(value: DecimalError) -> Self {
        Self::Decimal(value)
    }
}

/// 数学函数通用结果别名。
pub type MathResult<T> = Result<T, MathError>;

/// 表达式求值类型，决定走整数/小数/实数/字符串实现分支。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EvalType {
    Int,
    Decimal,
    Real,
    String,
}

/// 精简的字段类型元数据，用于签名选择（flen/decimal/unsigned）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FieldTypeMeta {
    pub eval_type: EvalType,
    pub flen: i32,
    pub decimal: i32,
    pub unsigned: bool,
}

impl FieldTypeMeta {
    /// 构造整数字段元数据。
    pub const fn int(unsigned: bool) -> Self {
        Self {
            eval_type: EvalType::Int,
            flen: MAX_INT_WIDTH,
            decimal: 0,
            unsigned,
        }
    }

    /// 构造 DECIMAL 字段元数据。
    pub const fn decimal(flen: i32, decimal: i32) -> Self {
        Self {
            eval_type: EvalType::Decimal,
            flen,
            decimal,
            unsigned: false,
        }
    }

    /// 构造 DOUBLE/REAL 字段元数据。
    pub const fn real() -> Self {
        Self {
            eval_type: EvalType::Real,
            flen: 23,
            decimal: -1,
            unsigned: false,
        }
    }
}

/// ROUND/TRUNCATE 第二参数（小数位数）在常量折叠时的形态。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FractionMetadata {
    Missing,
    Dynamic,
    Constant(Option<i64>),
}

/// 与 TiDB tipb/Go 对齐的数学函数签名枚举，供执行器按 pb code 分发。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScalarFuncSignature {
    AbsReal,
    AbsInt,
    AbsUInt,
    AbsDecimal,
    RoundReal,
    RoundInt,
    RoundDecimal,
    RoundWithFracReal,
    RoundWithFracInt,
    RoundWithFracDecimal,
    CeilReal,
    CeilIntToDecimal,
    CeilIntToInt,
    CeilDecimalToInt,
    CeilDecimalToDecimal,
    FloorReal,
    FloorIntToDecimal,
    FloorIntToInt,
    FloorDecimalToInt,
    FloorDecimalToDecimal,
    Log1Arg,
    Log2Args,
    Log2,
    Log10,
    Rand,
    RandWithSeedFirstGen,
    Pow,
    Conv,
    Crc32,
    Sign,
    Sqrt,
    Acos,
    Asin,
    Atan1Arg,
    Atan2Args,
    Cos,
    Cot,
    Degrees,
    Exp,
    Pi,
    Radians,
    Sin,
    Tan,
    TruncateInt,
    TruncateReal,
    TruncateDecimal,
    TruncateUInt,
}

/// 参数形态固定的数学函数集合，直接映射到单一签名。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FixedMathFunction {
    Log2,
    Log10,
    Pow,
    Conv,
    Crc32,
    Sign,
    Sqrt,
    Acos,
    Asin,
    Cos,
    Cot,
    Degrees,
    Exp,
    Pi,
    Radians,
    Sin,
    Tan,
}

/// 将固定形态函数映射到 ScalarFuncSignature。
pub fn fixed_signature(function: FixedMathFunction) -> ScalarFuncSignature {
    match function {
        FixedMathFunction::Log2 => ScalarFuncSignature::Log2,
        FixedMathFunction::Log10 => ScalarFuncSignature::Log10,
        FixedMathFunction::Pow => ScalarFuncSignature::Pow,
        FixedMathFunction::Conv => ScalarFuncSignature::Conv,
        FixedMathFunction::Crc32 => ScalarFuncSignature::Crc32,
        FixedMathFunction::Sign => ScalarFuncSignature::Sign,
        FixedMathFunction::Sqrt => ScalarFuncSignature::Sqrt,
        FixedMathFunction::Acos => ScalarFuncSignature::Acos,
        FixedMathFunction::Asin => ScalarFuncSignature::Asin,
        FixedMathFunction::Cos => ScalarFuncSignature::Cos,
        FixedMathFunction::Cot => ScalarFuncSignature::Cot,
        FixedMathFunction::Degrees => ScalarFuncSignature::Degrees,
        FixedMathFunction::Exp => ScalarFuncSignature::Exp,
        FixedMathFunction::Pi => ScalarFuncSignature::Pi,
        FixedMathFunction::Radians => ScalarFuncSignature::Radians,
        FixedMathFunction::Sin => ScalarFuncSignature::Sin,
        FixedMathFunction::Tan => ScalarFuncSignature::Tan,
    }
}

/// 按参数类型选择 ABS 的具体实现签名（有符号/无符号整数、DECIMAL、REAL）。
pub fn abs_signature(argument: FieldTypeMeta) -> ScalarFuncSignature {
    match argument.eval_type {
        EvalType::Int if argument.unsigned => ScalarFuncSignature::AbsUInt,
        EvalType::Int => ScalarFuncSignature::AbsInt,
        EvalType::Decimal => ScalarFuncSignature::AbsDecimal,
        _ => ScalarFuncSignature::AbsReal,
    }
}

/// 按参数类型与是否带小数位参数选择 ROUND 签名。
pub fn round_signature(argument: FieldTypeMeta, with_fraction: bool) -> ScalarFuncSignature {
    match (argument.eval_type, with_fraction) {
        (EvalType::Int, false) => ScalarFuncSignature::RoundInt,
        (EvalType::Decimal, false) => ScalarFuncSignature::RoundDecimal,
        (_, false) => ScalarFuncSignature::RoundReal,
        (EvalType::Int, true) => ScalarFuncSignature::RoundWithFracInt,
        (EvalType::Decimal, true) => ScalarFuncSignature::RoundWithFracDecimal,
        (_, true) => ScalarFuncSignature::RoundWithFracReal,
    }
}

/// 按 FLOOR/CEIL 类型推导结果选择 CEIL 签名。
pub fn ceil_signature(argument: FieldTypeMeta) -> ScalarFuncSignature {
    match get_eval_type_for_floor_and_ceil(argument) {
        (EvalType::Int, EvalType::Int) => ScalarFuncSignature::CeilIntToInt,
        (EvalType::Decimal, EvalType::Int) => ScalarFuncSignature::CeilIntToDecimal,
        (EvalType::Int, EvalType::Decimal) => ScalarFuncSignature::CeilDecimalToInt,
        (EvalType::Decimal, EvalType::Decimal) => ScalarFuncSignature::CeilDecimalToDecimal,
        _ => ScalarFuncSignature::CeilReal,
    }
}

/// 按 FLOOR/CEIL 类型推导结果选择 FLOOR 签名。
pub fn floor_signature(argument: FieldTypeMeta) -> ScalarFuncSignature {
    match get_eval_type_for_floor_and_ceil(argument) {
        (EvalType::Int, EvalType::Int) => ScalarFuncSignature::FloorIntToInt,
        (EvalType::Decimal, EvalType::Int) => ScalarFuncSignature::FloorIntToDecimal,
        (EvalType::Int, EvalType::Decimal) => ScalarFuncSignature::FloorDecimalToInt,
        (EvalType::Decimal, EvalType::Decimal) => ScalarFuncSignature::FloorDecimalToDecimal,
        _ => ScalarFuncSignature::FloorReal,
    }
}

/// 单参数 LOG 与双参数 LOG(base, x) 的签名选择。
pub const fn log_signature(two_arguments: bool) -> ScalarFuncSignature {
    if two_arguments {
        ScalarFuncSignature::Log2Args
    } else {
        ScalarFuncSignature::Log1Arg
    }
}

/// ATAN(x) 与 ATAN(y, x) 的签名选择。
pub const fn atan_signature(two_arguments: bool) -> ScalarFuncSignature {
    if two_arguments {
        ScalarFuncSignature::Atan2Args
    } else {
        ScalarFuncSignature::Atan1Arg
    }
}

/// RAND：无参/常量种子走会话 RNG；非常量种子走“每行首值”路径。
pub const fn rand_signature(has_argument: bool, argument_is_constant: bool) -> ScalarFuncSignature {
    if has_argument && !argument_is_constant {
        ScalarFuncSignature::RandWithSeedFirstGen
    } else {
        ScalarFuncSignature::Rand
    }
}

/// 按参数类型选择 TRUNCATE 签名。
pub fn truncate_signature(argument: FieldTypeMeta) -> ScalarFuncSignature {
    match argument.eval_type {
        EvalType::Int if argument.unsigned => ScalarFuncSignature::TruncateUInt,
        EvalType::Int => ScalarFuncSignature::TruncateInt,
        EvalType::Decimal => ScalarFuncSignature::TruncateDecimal,
        _ => ScalarFuncSignature::TruncateReal,
    }
}

/// 计算 ROUND/TRUNCATE 结果 DECIMAL 的小数位数，受 MAX_DECIMAL_SCALE 钳制。
pub fn calculate_decimal_for_round_and_truncate(
    return_type: EvalType,
    argument_decimal: i32,
    fraction: FractionMetadata,
) -> i32 {
    if return_type == EvalType::Int || fraction == FractionMetadata::Missing {
        return 0;
    }
    match fraction {
        FractionMetadata::Dynamic => argument_decimal,
        FractionMetadata::Constant(Some(value)) if value >= 0 => {
            value.min(i64::from(MAX_DECIMAL_SCALE)) as i32
        }
        _ => 0,
    }
}

/// 返回 (结果类型, 参数类型)：整数部分过宽时 CEIL/FLOOR 保持 DECIMAL。
pub fn get_eval_type_for_floor_and_ceil(field: FieldTypeMeta) -> (EvalType, EvalType) {
    match field.eval_type {
        EvalType::Int => (EvalType::Int, EvalType::Int),
        EvalType::Decimal if field.flen - field.decimal > MAX_INT_WIDTH - 2 => {
            (EvalType::Decimal, EvalType::Decimal)
        }
        EvalType::Decimal => (EvalType::Int, EvalType::Decimal),
        _ => (EvalType::Real, EvalType::Real),
    }
}

/// FLOOR/CEIL 结果是否应按无符号整数处理。
pub const fn floor_and_ceil_unsigned(field: FieldTypeMeta) -> bool {
    field.unsigned && matches!(field.eval_type, EvalType::Int | EvalType::Decimal)
}

/// ABS(REAL)。
pub fn abs_real(value: f64) -> f64 {
    value.abs()
}

/// ABS(有符号整数)；`i64::MIN` 无法取反时返回溢出。
pub fn abs_int(value: i64) -> MathResult<i64> {
    value
        .checked_abs()
        .ok_or_else(|| MathError::overflow("BIGINT", format!("abs({value})")))
}

/// ABS(无符号整数)：恒等返回。
pub const fn abs_uint(value: u64) -> u64 {
    value
}

/// ABS(DECIMAL)：负值通过 `0 - value` 取反。
pub fn abs_decimal(value: &MyDecimal) -> MathResult<MyDecimal> {
    if !value.IsNegative() {
        return Ok(value.clone());
    }
    let mut result = MyDecimal::default();
    DecimalSub(&MyDecimal::default(), value, &mut result)?;
    Ok(result)
}

/// ROUND(REAL) 到整数位，使用 types 包的银行家舍入规则封装。
pub fn round_real(value: f64) -> f64 {
    types_dependency::field::Round(value, 0)
}

/// ROUND(整数，无小数位参数)：原样返回。
pub const fn round_int(value: i64) -> i64 {
    value
}

/// ROUND(DECIMAL) 到 0 位小数。
pub fn round_decimal(value: &MyDecimal) -> MathResult<MyDecimal> {
    round_with_frac_decimal(value, 0, 0)
}

/// ROUND(REAL, d)：d 可为负，表示向整数位左侧舍入。
pub fn round_with_frac_real(value: f64, fraction: i64) -> f64 {
    types_dependency::field::Round(value, clamp_i64_to_i32(fraction))
}

/// ROUND(整数, d)：经 f64 舍入后再转回整数。
pub fn round_with_frac_int(value: i64, fraction: i64) -> i64 {
    round_with_frac_real(value as f64, fraction) as i64
}

/// ROUND(DECIMAL, d)：半入（ModeHalfUp），位数不超过返回类型的 decimal。
pub fn round_with_frac_decimal(
    value: &MyDecimal,
    fraction: i64,
    return_decimal: i32,
) -> MathResult<MyDecimal> {
    let mut result = MyDecimal::default();
    value.Round(
        &mut result,
        fraction.min(i64::from(return_decimal)) as isize,
        ModeHalfUp,
    )?;
    Ok(result)
}

/// CEIL(REAL)。
pub fn ceil_real(value: f64) -> f64 {
    value.ceil()
}

/// CEIL(整数)：恒等。
pub const fn ceil_int(value: i64) -> i64 {
    value
}

/// 将整数提升为 DECIMAL，供 CEIL Int→Decimal 签名使用。
pub fn ceil_int_to_decimal(value: i64, unsigned: bool) -> MyDecimal {
    if unsigned || value >= 0 {
        NewDecFromUint(value as u64)
    } else {
        NewDecFromInt(value)
    }
}

/// CEIL(DECIMAL)→整数：向正无穷方向进位。
pub fn ceil_decimal_to_int(value: &MyDecimal) -> MathResult<i64> {
    let (mut result, status) = value.ToInt();
    match status {
        Ok(()) => Ok(result),
        Err(DecimalError::Truncated) => {
            if !value.IsNegative() {
                result += 1;
            }
            Ok(result)
        }
        Err(error) => Err(error.into()),
    }
}

/// CEIL(DECIMAL)→DECIMAL：截断后若为正且仍有小数则加一。
pub fn ceil_decimal(value: &MyDecimal) -> MathResult<MyDecimal> {
    let mut result = MyDecimal::default();
    value.Round(&mut result, 0, ModeTruncate)?;
    if value.IsNegative() || result.Compare(value) == 0 {
        return Ok(result);
    }
    let truncated = result.clone();
    DecimalAdd(&truncated, &NewDecFromInt(1), &mut result)?;
    Ok(result)
}

/// FLOOR(REAL)。
pub fn floor_real(value: f64) -> f64 {
    value.floor()
}

/// FLOOR(整数)：恒等。
pub const fn floor_int(value: i64) -> i64 {
    value
}

/// FLOOR Int→Decimal：与 CEIL 共用整数转 DECIMAL。
pub fn floor_int_to_decimal(value: i64, unsigned: bool) -> MyDecimal {
    ceil_int_to_decimal(value, unsigned)
}

/// FLOOR(DECIMAL)→整数：向负无穷方向取整。
pub fn floor_decimal_to_int(value: &MyDecimal) -> MathResult<i64> {
    let (mut result, status) = value.ToInt();
    match status {
        Ok(()) => Ok(result),
        Err(DecimalError::Truncated) => {
            if value.IsNegative() {
                result -= 1;
            }
            Ok(result)
        }
        Err(error) => Err(error.into()),
    }
}

/// FLOOR(DECIMAL)→DECIMAL：截断后若为负且仍有小数则减一。
pub fn floor_decimal(value: &MyDecimal) -> MathResult<MyDecimal> {
    let mut result = MyDecimal::default();
    value.Round(&mut result, 0, ModeTruncate)?;
    if !value.IsNegative() || result.Compare(value) == 0 {
        return Ok(result);
    }
    let truncated = result.clone();
    DecimalSub(&truncated, &NewDecFromInt(1), &mut result)?;
    Ok(result)
}

/// 数学求值告警；当前仅覆盖非法对数参数。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MathWarning {
    InvalidArgumentForLogarithm,
}

/// 带可选告警列表的求值结果；`value == None` 表示 SQL NULL。
#[derive(Clone, Debug, PartialEq)]
pub struct MathOutcome<T> {
    pub value: Option<T>,
    pub warnings: Vec<MathWarning>,
}

impl<T> MathOutcome<T> {
    fn value(value: T) -> Self {
        Self {
            value: Some(value),
            warnings: Vec::new(),
        }
    }

    fn invalid_logarithm() -> Self {
        Self {
            value: None,
            warnings: vec![MathWarning::InvalidArgumentForLogarithm],
        }
    }
}

/// 自然对数求值，非正数产生告警并返回 NULL。
pub fn eval_log(value: f64) -> MathOutcome<f64> {
    if value <= 0.0 {
        MathOutcome::invalid_logarithm()
    } else {
        MathOutcome::value(value.ln())
    }
}

/// LOG(x) 便捷封装，丢弃告警只返回可选值。
pub fn log(value: f64) -> Option<f64> {
    eval_log(value).value
}

/// LOG(base, x)：底数/真数非法时告警并 NULL。
pub fn eval_log_base(base: f64, value: f64) -> MathOutcome<f64> {
    if base <= 0.0 || base == 1.0 || value <= 0.0 {
        MathOutcome::invalid_logarithm()
    } else {
        MathOutcome::value(value.ln() / base.ln())
    }
}

/// 双参数 LOG 便捷封装。
pub fn log_base(base: f64, value: f64) -> Option<f64> {
    eval_log_base(base, value).value
}

/// LOG2 求值。
pub fn eval_log2(value: f64) -> MathOutcome<f64> {
    if value <= 0.0 {
        MathOutcome::invalid_logarithm()
    } else {
        MathOutcome::value(value.log2())
    }
}

/// LOG2 便捷封装。
pub fn log2(value: f64) -> Option<f64> {
    eval_log2(value).value
}

/// LOG10 求值。
pub fn eval_log10(value: f64) -> MathOutcome<f64> {
    if value <= 0.0 {
        MathOutcome::invalid_logarithm()
    } else {
        MathOutcome::value(value.log10())
    }
}

/// LOG10 便捷封装。
pub fn log10(value: f64) -> Option<f64> {
    eval_log10(value).value
}

/// 包装 MySQL 兼容伪随机数生成器（MysqlRng），可在会话间共享。
#[derive(Clone)]
pub struct MysqlRand {
    rng: Arc<MysqlRng>,
}

impl MysqlRand {
    /// 使用给定种子构造独立 RNG。
    pub fn with_seed(seed: i64) -> Self {
        Self {
            rng: Arc::from(NewWithSeed(seed)),
        }
    }

    /// 从已有共享 MysqlRng 构造包装。
    pub fn from_shared(rng: Arc<MysqlRng>) -> Self {
        Self { rng }
    }

    /// 生成下一个 [0,1) 随机数。
    pub fn generate(&self) -> f64 {
        self.rng.Gen()
    }
}

/// 非常量种子路径：每行用种子新建 RNG 并取首个随机值；NULL 种子视为 0。
pub fn rand_with_seed_first_gen(seed: Option<i64>) -> f64 {
    NewWithSeed(seed.unwrap_or(0)).Gen()
}

/// POW(x, y)；非有限结果视为 DOUBLE 溢出。
pub fn pow(x: f64, y: f64) -> MathResult<f64> {
    let result = x.powf(y);
    if !result.is_finite() {
        return Err(MathError::overflow(
            "DOUBLE",
            format!("pow({}, {})", format_float(x), format_float(y)),
        ));
    }
    Ok(result)
}

/// 从字符串中截取 CONV 可用的最长合法前缀（含可选符号）。
pub fn get_valid_prefix(value: &str, base: i64) -> &str {
    let upper = match base {
        2..=9 => b'0' + base as u8,
        10..=36 => b'A' + (base - 10) as u8,
        _ => return "",
    };
    let bytes = value.as_bytes();
    let mut valid_len = 0;
    for (index, byte) in bytes.iter().copied().enumerate() {
        if byte.is_ascii_alphanumeric() {
            if byte.to_ascii_uppercase() >= upper {
                break;
            }
            valid_len = index + 1;
        } else if byte == b'+' || byte == b'-' {
            if index != 0 {
                break;
            }
        } else {
            break;
        }
    }
    if valid_len > 1 && bytes.first() == Some(&b'+') {
        &value[1..valid_len]
    } else {
        &value[..valid_len]
    }
}

/// CONV(N, from_base, to_base)：负基数表示有符号解释；非法基数返回 NULL。
pub fn conv(value: &str, mut from_base: i64, mut to_base: i64) -> MathResult<Option<String>> {
    // 负的 from_base/to_base 在 MySQL 中表示按有符号数解释/输出。
    let signed = from_base < 0;
    if signed {
        from_base = -from_base;
    }
    let ignore_sign = to_base < 0;
    if ignore_sign {
        to_base = -to_base;
    }
    if !(2..=36).contains(&from_base) || !(2..=36).contains(&to_base) {
        return Ok(None);
    }

    let prefix = get_valid_prefix(value.trim(), from_base);
    if prefix.is_empty() {
        return Ok(Some("0".to_owned()));
    }
    let (negative_input, digits) = match prefix.strip_prefix('-') {
        Some(digits) => (true, digits),
        None => (false, prefix),
    };
    let mut number = parse_radix_u64(digits, from_base as u32)?;
    if signed {
        let min_magnitude = i64::MAX as u64 + 1;
        if negative_input && number > min_magnitude {
            number = min_magnitude;
        }
        if !negative_input && number > i64::MAX as u64 {
            number = i64::MAX as u64;
        }
    }
    if negative_input {
        number = number.wrapping_neg();
    }

    let negative_result = (number as i64) < 0;
    if ignore_sign && negative_result {
        number = number.wrapping_neg();
    }
    let mut result = format_radix_u64(number, to_base as u32);
    if negative_result && ignore_sign {
        result.insert(0, '-');
    }
    Ok(Some(result))
}

/// 二进制字面量先展开为比特串，再经中间进制转到目标进制。
pub fn conv_binary_literal(
    value: &[u8],
    from_base: i64,
    to_base: i64,
) -> MathResult<Option<String>> {
    let mut binary = String::with_capacity(value.len() * 8);
    for (index, byte) in value.iter().enumerate() {
        if index == 0 {
            binary.push_str(&format!("{byte:b}"));
        } else {
            binary.push_str(&format!("{byte:08b}"));
        }
    }
    if binary.is_empty() {
        binary.push('0');
    }
    let intermediate = match conv(&binary, 2, from_base)? {
        Some(value) => value,
        None => return Ok(None),
    };
    conv(&intermediate, from_base, to_base)
}

/// CRC32 校验，返回值以有符号 i64 承载无符号 32 位结果。
pub fn crc32(value: &[u8]) -> i64 {
    i64::from(crc32fast::hash(value))
}

/// SIGN：正→1、零→0、负→-1。
pub fn sign(value: f64) -> i64 {
    if value > 0.0 {
        1
    } else if value == 0.0 {
        0
    } else {
        -1
    }
}

/// SQRT；负值返回 NULL。
pub fn sqrt(value: f64) -> Option<f64> {
    (value >= 0.0 || value.is_nan()).then(|| value.sqrt())
}

/// ACOS；超出 [-1,1] 返回 NULL。
pub fn acos(value: f64) -> Option<f64> {
    (!(value < -1.0 || value > 1.0)).then(|| value.acos())
}

/// ASIN；超出 [-1,1] 返回 NULL。
pub fn asin(value: f64) -> Option<f64> {
    (!(value < -1.0 || value > 1.0)).then(|| value.asin())
}

/// ATAN(x)。
pub fn atan(value: f64) -> f64 {
    value.atan()
}

/// ATAN(y, x)，保留象限信息。
pub fn atan2(y: f64, x: f64) -> f64 {
    y.atan2(x)
}

/// COS。
pub fn cos(value: f64) -> f64 {
    value.cos()
}

/// COT = 1/tan；tan 为 0 或结果非有限时溢出。
pub fn cot(value: f64) -> MathResult<f64> {
    let tangent = value.tan();
    if tangent != 0.0 {
        let result = 1.0 / tangent;
        if result.is_finite() {
            return Ok(result);
        }
    }
    Err(MathError::overflow(
        "DOUBLE",
        format!("cot({})", format_float(value)),
    ))
}

/// 弧度转角度。
pub fn degrees(value: f64) -> f64 {
    value * 180.0 / std::f64::consts::PI
}

/// EXP；非有限结果视为溢出。
pub fn exp(value: f64) -> MathResult<f64> {
    let result = value.exp();
    if !result.is_finite() {
        return Err(MathError::overflow(
            "DOUBLE",
            format!("exp({})", format_float(value)),
        ));
    }
    Ok(result)
}

/// 返回圆周率常量。
pub const fn pi() -> f64 {
    std::f64::consts::PI
}

/// 角度转弧度。
pub fn radians(value: f64) -> f64 {
    value * (std::f64::consts::PI / 180.0)
}

/// SIN。
pub fn sin(value: f64) -> f64 {
    value.sin()
}

/// TAN。
pub fn tan(value: f64) -> f64 {
    value.tan()
}

/// TRUNCATE(DECIMAL, d)：向零截断，不做四舍五入。
pub fn truncate_decimal(
    value: &MyDecimal,
    fraction: i64,
    return_decimal: i32,
) -> MathResult<MyDecimal> {
    let mut result = MyDecimal::default();
    value.Round(
        &mut result,
        fraction.min(i64::from(return_decimal)) as isize,
        ModeTruncate,
    )?;
    Ok(result)
}

/// TRUNCATE(REAL, d)。
pub fn truncate_real(value: f64, fraction: i64) -> f64 {
    types_dependency::field::Truncate(value, clamp_i64_to_i32(fraction))
}

/// TRUNCATE(有符号整数, d)：仅当 d 为负时截断低位；无符号 d 短路返回原值。
pub fn truncate_int(value: i64, fraction: i64, fraction_is_unsigned: bool) -> i64 {
    if fraction_is_unsigned || fraction >= 0 {
        return value;
    }
    if fraction == i64::MIN {
        return 0;
    }
    let places = fraction.unsigned_abs();
    let Some(shift) = power_of_ten_i64(places) else {
        return 0;
    };
    value / shift * shift
}

/// TRUNCATE(无符号整数, d)。
pub fn truncate_uint(value: u64, fraction: i64, fraction_is_unsigned: bool) -> u64 {
    if fraction_is_unsigned || fraction >= 0 {
        return value;
    }
    if fraction == i64::MIN {
        return 0;
    }
    let places = fraction.unsigned_abs();
    let Some(shift) = power_of_ten_u64(places) else {
        return 0;
    };
    value / shift * shift
}

/// 将小数位参数钳制到 i32，避免下游 powi/Round 溢出。
fn clamp_i64_to_i32(value: i64) -> i32 {
    value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

/// 计算 10^n（有符号），溢出返回 None。
fn power_of_ten_i64(exponent: u64) -> Option<i64> {
    let exponent = u32::try_from(exponent).ok()?;
    10_i64.checked_pow(exponent)
}

/// 计算 10^n（无符号），溢出返回 None。
fn power_of_ten_u64(exponent: u64) -> Option<u64> {
    let exponent = u32::try_from(exponent).ok()?;
    10_u64.checked_pow(exponent)
}

/// 按给定进制解析无符号整数，中间溢出报 BIGINT UNSIGNED。
fn parse_radix_u64(value: &str, radix: u32) -> MathResult<u64> {
    let mut result = 0_u64;
    for byte in value.bytes() {
        let digit = byte.to_ascii_uppercase();
        let digit = if digit.is_ascii_digit() {
            u32::from(digit - b'0')
        } else {
            u32::from(digit - b'A') + 10
        };
        result = result
            .checked_mul(u64::from(radix))
            .and_then(|current| current.checked_add(u64::from(digit)))
            .ok_or_else(|| MathError::overflow("BIGINT UNSIGNED", value))?;
    }
    Ok(result)
}

/// 将无符号整数格式化为指定进制的大写字母数字串。
fn format_radix_u64(mut value: u64, radix: u32) -> String {
    const DIGITS: &[u8; 36] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    if value == 0 {
        return "0".to_owned();
    }
    let mut output = Vec::new();
    while value != 0 {
        output.push(DIGITS[(value % u64::from(radix)) as usize]);
        value /= u64::from(radix);
    }
    output.reverse();
    String::from_utf8(output).expect("radix digits are ASCII")
}

/// 溢出错误消息中的浮点格式化：整数形态去掉小数点。
fn format_float(value: f64) -> String {
    if value.is_finite() && value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        value.to_string()
    }
}

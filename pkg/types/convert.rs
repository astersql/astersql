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

// Copyright 2014 The ql Authors. All rights reserved.
// Use of this source code is governed by a BSD-style
// license that can be found in the LICENSES/QL-LICENSE file.

// MySQL 类型转换：整数/浮点/字符串互转与截断策略。
//
// 按 Context Flags 处理溢出裁剪、科学计数法展开与合法数字前缀提取，
// 对齐 Go `types` 包 Convert* / StrTo* 语义。

use std::any::Any;
use std::fmt::Display;

#[cfg(feature = "types-integration")]
use crate::{BinaryJSON, Duration, Enum, MyDecimal, Set, Time};
use crate::{BinaryLiteral, Context, ErrorWithValue, Flags, ValueResult, errors, mysql};

/// 未指定字段长度（flen）时的哨兵值。
pub const UnspecifiedLength: i32 = -1;
/// u64 最大值的十进制字符串，用于溢出裁剪。
pub const maxUintStr: &str = "18446744073709551615";
/// i64 最小值的十进制字符串，用于溢出裁剪。
pub const minIntStr: &str = "-9223372036854775808";

/// MySQL 类型字节码到可读类型名，用于溢出错误信息。
fn mysql_type_name(tp: u8) -> &'static str {
    match tp {
        mysql::TypeTiny => "TINYINT",
        mysql::TypeShort => "SMALLINT",
        mysql::TypeInt24 => "MEDIUMINT",
        mysql::TypeLong => "INT",
        mysql::TypeLonglong => "BIGINT",
        mysql::TypeBit => "BIT",
        mysql::TypeEnum => "ENUM",
        mysql::TypeSet => "SET",
        _ => "INTEGER",
    }
}

/// 构造“值超出类型范围”的共享错误。
fn overflow(value: impl Display, tp: u8) -> errors::SharedError {
    errors::New(format!(
        "value {value} is out of range for {}",
        mysql_type_name(tp)
    ))
}

/// 将值与错误包装为 ValueResult 失败分支。
fn ErrWithValue<T>(value: T, error: errors::SharedError) -> ValueResult<T> {
    Err(ErrorWithValue::new(value, error))
}

/// 委托 Context.HandleTruncate 处理截断。
fn apply_truncate<T>(ctx: &Context, value: T, error: errors::SharedError) -> ValueResult<T> {
    ctx.HandleTruncate(value, error)
}

/// 若截断被忽略/转 warning 则返回 None，否则返回错误。
fn handled_truncate(ctx: &Context, error: errors::SharedError) -> Option<errors::SharedError> {
    match ctx.HandleTruncate((), error) {
        Ok(()) => None,
        Err(error) => Some(error.error),
    }
}

/// 按 flen 截断字符串，并回退到合法 UTF-8 字符边界。
pub fn truncateStr(mut value: String, flen: i32) -> String {
    if flen != UnspecifiedLength && value.len() > flen as usize {
        let mut end = flen as usize;
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        value.truncate(end);
    }
    value
}

/// 给定 MySQL 整数类型的无符号上界。
pub fn IntegerUnsignedUpperBound(intType: u8) -> u64 {
    match intType {
        mysql::TypeTiny => u8::MAX as u64,
        mysql::TypeShort => u16::MAX as u64,
        mysql::TypeInt24 => mysql::MaxUint24 as u64,
        mysql::TypeLong => u32::MAX as u64,
        mysql::TypeLonglong | mysql::TypeBit | mysql::TypeSet => u64::MAX,
        mysql::TypeEnum => 65_535,
        _ => panic!("Input byte is not a mysql type"),
    }
}

/// 给定 MySQL 整数类型的有符号上界。
pub fn IntegerSignedUpperBound(intType: u8) -> i64 {
    match intType {
        mysql::TypeTiny => i8::MAX as i64,
        mysql::TypeShort => i16::MAX as i64,
        mysql::TypeInt24 => mysql::MaxInt24 as i64,
        mysql::TypeLong => i32::MAX as i64,
        mysql::TypeLonglong => i64::MAX,
        mysql::TypeEnum => 65_535,
        _ => panic!("Input byte is not a mysql int type"),
    }
}

/// 给定 MySQL 整数类型的有符号下界。
pub fn IntegerSignedLowerBound(intType: u8) -> i64 {
    match intType {
        mysql::TypeTiny => i8::MIN as i64,
        mysql::TypeShort => i16::MIN as i64,
        mysql::TypeInt24 => mysql::MinInt24 as i64,
        mysql::TypeLong => i32::MIN as i64,
        mysql::TypeLonglong => i64::MIN,
        mysql::TypeEnum => 0,
        _ => panic!("Input byte is not a mysql type"),
    }
}

/// 银行家舍入浮点（对齐 Go math.RoundToEven）。
pub fn RoundFloat(value: f64) -> f64 {
    value.round_ties_even()
}

/// 浮点转有符号整数：先四舍五入，越界则裁剪并报错。
pub fn ConvertFloatToInt(fval: f64, lowerBound: i64, upperBound: i64, tp: u8) -> ValueResult<i64> {
    // 先四舍五入再与有符号上下界比较
    let value = RoundFloat(fval);
    if value < lowerBound as f64 {
        return ErrWithValue(lowerBound, overflow(value, tp));
    }
    if value >= upperBound as f64 {
        if value == upperBound as f64 {
            return Ok(upperBound);
        }
        return ErrWithValue(upperBound, overflow(value, tp));
    }
    Ok(value as i64)
}

/// 有符号整数范围检查，越界裁剪到上下界。
pub fn ConvertIntToInt(value: i64, lowerBound: i64, upperBound: i64, tp: u8) -> ValueResult<i64> {
    if value < lowerBound {
        return ErrWithValue(lowerBound, overflow(value, tp));
    }
    if value > upperBound {
        return ErrWithValue(upperBound, overflow(value, tp));
    }
    Ok(value)
}

/// 无符号转有符号，超过上界则裁剪。
pub fn ConvertUintToInt(value: u64, upperBound: i64, tp: u8) -> ValueResult<i64> {
    if value > upperBound as u64 {
        return ErrWithValue(upperBound, overflow(value, tp));
    }
    Ok(value as i64)
}

/// 有符号转无符号；负值是否允许由 Flags 控制。
pub fn ConvertIntToUint(flags: Flags, value: i64, upperBound: u64, tp: u8) -> ValueResult<u64> {
    if value < 0 && !flags.AllowNegativeToUnsigned() {
        return ErrWithValue(0, overflow(value, tp));
    }
    let unsigned = value as u64;
    if unsigned > upperBound {
        return ErrWithValue(upperBound, overflow(value, tp));
    }
    Ok(unsigned)
}

/// 无符号范围检查。
pub fn ConvertUintToUint(value: u64, upperBound: u64, tp: u8) -> ValueResult<u64> {
    if value > upperBound {
        return ErrWithValue(upperBound, overflow(value, tp));
    }
    Ok(value)
}

/// 浮点转无符号：处理负值、无穷与上界。
pub fn ConvertFloatToUint(flags: Flags, fval: f64, upperBound: u64, tp: u8) -> ValueResult<u64> {
    let value = RoundFloat(fval);
    if value < 0.0 {
        if !flags.AllowNegativeToUnsigned() {
            return ErrWithValue(0, overflow(value, tp));
        }
        return ErrWithValue((value as i64) as u64, overflow(value, tp));
    }
    if !value.is_finite() || value >= 18_446_744_073_709_551_616.0 {
        return ErrWithValue(upperBound, overflow(value, tp));
    }
    let result = value as u64;
    if result > upperBound {
        return ErrWithValue(upperBound, overflow(value, tp));
    }
    Ok(result)
}

/// 将科学计数法字符串展开为普通十进制形式。
pub fn convertScientificNotation(value: &str) -> ValueResult<String> {
    // 无指数则原样返回；有指数则按尾数/小数点位置重排数字
    let Some(e_index) = value.find(['e', 'E']) else {
        return Ok(value.to_owned());
    };
    let exponent = match value[e_index + 1..].parse::<i64>() {
        Ok(exponent) => exponent,
        Err(error) => return ErrWithValue(String::new(), errors::New(error.to_string())),
    };
    let mantissa = &value[..e_index];
    let (sign, unsigned) = match mantissa.as_bytes().first() {
        Some(b'+') => ("", &mantissa[1..]),
        Some(b'-') => ("-", &mantissa[1..]),
        _ => ("", mantissa),
    };
    let point = unsigned.find('.').unwrap_or(unsigned.len());
    let digits = unsigned.replace('.', "");
    let Some(decimal_position) = (point as i64).checked_add(exponent) else {
        return ErrWithValue(
            String::new(),
            errors::New(format!("BIGINT value is out of range: {value}")),
        );
    };
    let rendered = if decimal_position <= 0 {
        format!("0.{}{}", "0".repeat((-decimal_position) as usize), digits)
    } else if decimal_position as usize >= digits.len() {
        format!(
            "{}{}",
            digits,
            "0".repeat(decimal_position as usize - digits.len())
        )
    } else {
        let position = decimal_position as usize;
        format!("{}.{}", &digits[..position], &digits[position..])
    };
    Ok(format!("{sign}{rendered}"))
}

/// 十进制字符串转 u64，按小数首位四舍五入。
pub fn convertDecimalStrToUint(value: &str, upperBound: u64, tp: u8) -> ValueResult<u64> {
    let value = match convertScientificNotation(value) {
        Ok(value) => value,
        Err(error) => return ErrWithValue(0, error.error),
    };
    let (integer, fraction) = value.split_once('.').unwrap_or((&value, ""));
    let integer = integer.trim_start_matches('0');
    let integer = if integer.is_empty() { "0" } else { integer };
    if integer.starts_with('-') {
        return ErrWithValue(0, overflow(&value, tp));
    }
    let round = u64::from(
        fraction
            .as_bytes()
            .first()
            .is_some_and(|digit| *digit >= b'5'),
    );
    let Some(limit) = upperBound.checked_sub(round) else {
        return ErrWithValue(upperBound, overflow(&value, tp));
    };
    let limit_text = limit.to_string();
    if integer.len() > limit_text.len()
        || integer.len() == limit_text.len() && integer > limit_text.as_str()
    {
        return ErrWithValue(upperBound, overflow(&value, tp));
    }
    match integer.parse::<u64>() {
        Ok(parsed) => Ok(parsed + round),
        Err(_) => ErrWithValue(0, overflow(&value, tp)),
    }
}

/// 字符串转 i64：提取合法整数前缀，溢出裁剪到 MIN/MAX。
pub fn StrToInt(ctx: Context, value: &str, isFuncCast: bool) -> ValueResult<i64> {
    let value = value.trim();
    let (prefix, warning) = getValidIntPrefix(ctx, value, isFuncCast);
    let parsed = match prefix.parse::<i64>() {
        Ok(parsed) => parsed,
        Err(_) => {
            let clipped = if prefix.starts_with('-') {
                i64::MIN
            } else {
                i64::MAX
            };
            return ErrWithValue(clipped, overflow(&prefix, mysql::TypeLonglong));
        }
    };
    match warning {
        Some(warning) => ErrWithValue(parsed, warning),
        None => Ok(parsed),
    }
}

/// 字符串转 u64：负零视为 0，其它负数溢出。
pub fn StrToUint(ctx: Context, value: &str, isFuncCast: bool) -> ValueResult<u64> {
    let value = value.trim();
    let (mut prefix, warning) = getValidIntPrefix(ctx, value, isFuncCast);
    let parsed = if prefix.starts_with('-') {
        if prefix[1..].bytes().all(|byte| byte == b'0') {
            0
        } else {
            return ErrWithValue(0, overflow(&prefix, mysql::TypeLonglong));
        }
    } else {
        if prefix.starts_with('+') {
            prefix.remove(0);
        }
        match prefix.parse::<u64>() {
            Ok(parsed) => parsed,
            Err(_) => return ErrWithValue(u64::MAX, overflow(&prefix, mysql::TypeLonglong)),
        }
    };
    match warning {
        Some(warning) => ErrWithValue(parsed, warning),
        None => Ok(parsed),
    }
}

/// 提取可用于整数解析的合法前缀；CAST 路径只接受纯整数。
pub fn getValidIntPrefix(
    ctx: Context,
    value: &str,
    isFuncCast: bool,
) -> (String, Option<errors::SharedError>) {
    // 非 CAST：先按浮点前缀再四舍五入为整数；CAST：只接受可选符号+数字
    if !isFuncCast {
        let (float_prefix, warning) = getValidFloatPrefix(ctx, value, false);
        if warning.is_some() {
            return (float_prefix, warning);
        }
        return floatStrToIntStr(&float_prefix, value);
    }

    let mut valid_length = 0;
    for (index, byte) in value.bytes().enumerate() {
        if index == 0 && matches!(byte, b'+' | b'-') {
            continue;
        }
        if byte.is_ascii_digit() {
            valid_length = index + 1;
        } else {
            break;
        }
    }
    let valid = if valid_length == 0 {
        "0".to_owned()
    } else {
        value[..valid_length].to_owned()
    };
    let warning = if valid_length == 0 || valid_length != value.len() {
        handled_truncate(
            &ctx,
            errors::New(format!("truncated incorrect INTEGER value: {value}")),
        )
    } else {
        None
    };
    (valid, warning)
}

/// 根据小数点后第一位是否 >=5 对整数字符串四舍五入。
pub fn roundIntStr(numNextDot: u8, intStr: &str) -> String {
    if numNextDot < b'5' {
        return intStr.to_owned();
    }
    let negative = intStr.starts_with('-');
    let positive = intStr.starts_with('+');
    let sign_length = usize::from(negative || positive);
    let mut digits = intStr.as_bytes()[sign_length..].to_vec();
    let mut carry = true;
    for digit in digits.iter_mut().rev() {
        if *digit == b'9' {
            *digit = b'0';
        } else {
            *digit += 1;
            carry = false;
            break;
        }
    }
    if carry {
        digits.insert(0, b'1');
    }
    let mut result = String::new();
    if negative {
        result.push('-');
    } else if positive {
        result.push('+');
    }
    result.push_str(std::str::from_utf8(&digits).expect("integer prefix is ASCII"));
    result
}

/// 浮点/科学计数法字符串转为整数字符串，必要时裁剪到 BIGINT 边界。
pub fn floatStrToIntStr(validFloat: &str, oriStr: &str) -> (String, Option<errors::SharedError>) {
    let Some(e_index) = validFloat.find(['e', 'E']) else {
        let Some(dot_index) = validFloat.find('.') else {
            return (validFloat.to_owned(), None);
        };
        let negative = validFloat.starts_with('-');
        let signed = negative || validFloat.starts_with('+');
        let unsigned = if signed { &validFloat[1..] } else { validFloat };
        let dot = unsigned.find('.').unwrap_or(dot_index);
        let digits = unsigned.as_bytes();
        let mut integer = if dot == 0 {
            "0".to_owned()
        } else {
            unsigned[..dot].to_owned()
        };
        if digits.len() > dot + 1 {
            integer = roundIntStr(digits[dot + 1], &integer);
        }
        if negative && integer != "0" {
            integer.insert(0, '-');
        }
        return (integer, None);
    };

    let (expanded, error) = match convertScientificNotation(validFloat) {
        Ok(expanded) => (expanded, None),
        Err(error) => {
            let clipped = if validFloat.starts_with('-') {
                minIntStr
            } else {
                maxUintStr
            };
            return (clipped.to_owned(), Some(error.error));
        }
    };
    let exponent = validFloat[e_index + 1..].parse::<i64>();
    if exponent.is_err() || expanded.len() > 22 {
        let clipped = if validFloat.starts_with('-') {
            minIntStr
        } else {
            maxUintStr
        };
        return (
            clipped.to_owned(),
            Some(errors::New(format!(
                "BIGINT value is out of range: {oriStr}"
            ))),
        );
    }
    let (integer, fraction) = expanded.split_once('.').unwrap_or((&expanded, ""));
    let next = fraction.as_bytes().first().copied().unwrap_or(b'0');
    let mut rounded = roundIntStr(next, if integer.is_empty() { "0" } else { integer });
    if validFloat.starts_with('+') && !rounded.starts_with('+') {
        rounded.insert(0, '+');
    }
    let unsigned = rounded
        .trim_start_matches(['+', '-'])
        .trim_start_matches('0');
    let limit = if rounded.starts_with('-') {
        "9223372036854775808"
    } else {
        maxUintStr
    };
    if unsigned.len() > limit.len() || (unsigned.len() == limit.len() && unsigned > limit) {
        let clipped = if rounded.starts_with('-') {
            minIntStr
        } else {
            maxUintStr
        };
        return (
            clipped.to_owned(),
            Some(errors::New(format!(
                "BIGINT value is out of range: {oriStr}"
            ))),
        );
    }
    (rounded, error)
}

/// 字符串转 f64：提取合法浮点前缀，无穷裁剪为 ±MAX。
pub fn StrToFloat(ctx: Context, value: &str, isFuncCast: bool) -> ValueResult<f64> {
    let value = value.trim();
    let (valid, warning) = getValidFloatPrefix(ctx.clone(), value, isFuncCast);
    let mut parsed = match valid.parse::<f64>() {
        Ok(parsed) => parsed,
        Err(error) => return ErrWithValue(0.0, errors::New(error.to_string())),
    };
    // 无穷大按 DOUBLE 截断语义裁剪到 ±f64::MAX
    if parsed.is_infinite() {
        parsed = if parsed.is_sign_positive() {
            f64::MAX
        } else {
            -f64::MAX
        };
        return apply_truncate(
            &ctx,
            parsed,
            errors::New(format!("truncated incorrect DOUBLE value: {value}")),
        );
    }
    match warning {
        Some(warning) => ErrWithValue(parsed, warning),
        None => Ok(parsed),
    }
}

/// 扫描字符串得到合法浮点前缀，非法后缀触发截断处理。
pub fn getValidFloatPrefix(
    ctx: Context,
    mut value: &str,
    isFuncCast: bool,
) -> (String, Option<errors::SharedError>) {
    if isFuncCast && value.is_empty() {
        return ("0".to_owned(), None);
    }
    let mut saw_dot = false;
    let mut saw_digit = false;
    let mut valid_length = 0;
    let mut exponent_index = None;
    for (index, byte) in value.bytes().enumerate() {
        match byte {
            b'+' | b'-' if index == 0 || exponent_index == Some(index - 1) => {}
            b'+' | b'-' => break,
            b'.' if !saw_dot && exponent_index.is_none() => {
                saw_dot = true;
                if saw_digit {
                    valid_length = index + 1;
                }
            }
            b'.' => break,
            b'e' | b'E' if saw_digit && exponent_index.is_none() => {
                exponent_index = Some(index);
                if index + 1 == value.len() {
                    return (value[..index].to_owned(), None);
                }
            }
            b'e' | b'E' => break,
            0 => {
                value = &value[..valid_length];
                break;
            }
            byte if byte.is_ascii_digit() => {
                saw_digit = true;
                valid_length = index + 1;
            }
            _ => break,
        }
    }
    let valid = if valid_length == 0 {
        "0".to_owned()
    } else {
        value[..valid_length].to_owned()
    };
    let warning = if valid_length == 0 || valid_length != value.len() {
        handled_truncate(
            &ctx,
            errors::New(format!("truncated incorrect DOUBLE value: {value}")),
        )
    } else {
        None
    };
    (valid, warning)
}

/// 将常见标量类型转为字符串表示。
pub fn ToString(value: &dyn Any) -> Result<String, errors::SharedError> {
    if let Some(value) = value.downcast_ref::<bool>() {
        return Ok(if *value { "1" } else { "0" }.to_owned());
    }
    if let Some(value) = value.downcast_ref::<i32>() {
        return Ok(value.to_string());
    }
    if let Some(value) = value.downcast_ref::<i64>() {
        return Ok(value.to_string());
    }
    if let Some(value) = value.downcast_ref::<u64>() {
        return Ok(value.to_string());
    }
    if let Some(value) = value.downcast_ref::<f32>() {
        let mut buffer = ryu::Buffer::new();
        let shortest = buffer.format_finite(*value);
        return Ok(if shortest.contains(['e', 'E']) {
            convertScientificNotation(shortest).unwrap_or_else(|_| shortest.to_owned())
        } else {
            shortest.to_owned()
        });
    }
    if let Some(value) = value.downcast_ref::<f64>() {
        return Ok(format_float(*value));
    }
    if let Some(value) = value.downcast_ref::<String>() {
        return Ok(value.clone());
    }
    if let Some(value) = value.downcast_ref::<Vec<u8>>() {
        return Ok(String::from_utf8_lossy(value).into_owned());
    }
    if let Some(value) = value.downcast_ref::<BinaryLiteral>() {
        return Ok(value.ToString());
    }
    #[cfg(feature = "types-integration")]
    {
        if let Some(value) = value.downcast_ref::<Time>() {
            return Ok(value.String());
        }
        if let Some(value) = value.downcast_ref::<Duration>() {
            return Ok(value.String());
        }
        if let Some(value) = value.downcast_ref::<MyDecimal>() {
            return Ok(value.String());
        }
        if let Some(value) = value.downcast_ref::<Enum>() {
            return Ok(value.String());
        }
        if let Some(value) = value.downcast_ref::<Set>() {
            return Ok(value.String());
        }
        if let Some(value) = value.downcast_ref::<BinaryJSON>() {
            return Ok(value.String());
        }
    }
    Err(errors::New(format!(
        "cannot convert value of type {:?} to string",
        value.type_id()
    )))
}

/// 最短浮点格式化；若含指数则再展开为十进制。
fn format_float(value: f64) -> String {
    let mut buffer = ryu::Buffer::new();
    let shortest = buffer.format_finite(value);
    if shortest.contains(['e', 'E']) {
        convertScientificNotation(shortest).unwrap_or_else(|_| shortest.to_owned())
    } else {
        shortest.to_owned()
    }
}

// These entry points depend on Time, Duration, MyDecimal and BinaryJSON, which are
// owned by other exclusive tasks in this batch. Their Go-equivalent implementations
// remain enabled by the package integration crate once those source files are wired.
#[cfg(feature = "types-integration")]
mod integrated {
    use super::*;
    use crate::*;

    /// DECIMAL 转 u64（经字符串路径）。
    pub fn ConvertDecimalToUint(decimal: &MyDecimal, upper_bound: u64, tp: u8) -> ValueResult<u64> {
        convertDecimalStrToUint(
            &String::from_utf8_lossy(&decimal.ToString()),
            upper_bound,
            tp,
        )
    }

    /// 字符串解析为 DATETIME。
    pub fn StrToDateTime(ctx: Context, value: &str, fsp: i32) -> ValueResult<Time> {
        ParseTime(&ctx, value, mysql::TypeDatetime, fsp)
            .map_err(|error| ErrorWithValue::new(Time::default(), errors::SharedError::new(error)))
    }

    /// 字符串解析为 Duration；较长数字串优先尝试 DATETIME。
    pub fn StrToDuration(
        ctx: Context,
        value: &str,
        fsp: i32,
    ) -> ValueResult<(Duration, Time, bool)> {
        let value = value.trim();
        let mut length = value.len();
        if value.starts_with('-') {
            length -= 1;
        }
        if let Some(point) = value.find('.') {
            length -= value[point..].len();
        }
        if length >= 12 {
            if let Ok(time) = StrToDateTime(ctx.clone(), value, fsp) {
                return Ok((Duration::default(), time, false));
            }
        }
        match ParseDuration(&ctx, value, fsp) {
            Ok((duration, _is_null)) => Ok((duration, Time::default(), true)),
            Err(error) => ctx.HandleTruncate(
                (ZeroDuration, Time::default(), true),
                errors::SharedError::new(error),
            ),
        }
    }

    /// 整数编码（HHMMSS 风格）转为 Duration，越界返回最大时长。
    pub fn NumberToDuration(mut number: i64, fsp: i32) -> ValueResult<Duration> {
        if number > i64::from(TimeMaxValue) {
            if number >= 10_000_000_000 {
                if let Ok(time) = ParseDatetimeFromNum(&*DefaultStmtNoWarningContext, number) {
                    return time.ConvertToDuration().map_err(|error| {
                        ErrorWithValue::new(ZeroDuration, errors::SharedError::new(error))
                    });
                }
            }
            let duration = MaxMySQLDuration(fsp);
            return ErrWithValue(
                duration,
                errors::New(format!("Duration overflow: {number}")),
            );
        }
        if number < -i64::from(TimeMaxValue) {
            let mut duration = MaxMySQLDuration(fsp);
            duration.Duration = -duration.Duration;
            return ErrWithValue(
                duration,
                errors::New(format!("Duration overflow: {number}")),
            );
        }
        let negative = number < 0;
        if negative {
            number = -number;
        }
        if number / 10_000 > i64::from(TimeMaxHour)
            || number % 100 >= 60
            || number / 100 % 100 >= 60
        {
            return ErrWithValue(
                ZeroDuration,
                errors::New(format!("truncated incorrect time value: {number}")),
            );
        }
        let mut duration = NewDuration(
            (number / 10_000) as i32,
            (number / 100 % 100) as i32,
            (number % 100) as i32,
            0,
            fsp,
        );
        if negative {
            duration.Duration = -duration.Duration;
        }
        Ok(duration)
    }

    /// JSON 转 i64（默认 BIGINT）。
    pub fn ConvertJSONToInt64(ctx: Context, json: BinaryJSON, unsigned: bool) -> ValueResult<i64> {
        ConvertJSONToInt(ctx, json, unsigned, mysql::TypeLonglong)
    }

    pub fn ConvertJSONToInt(
        ctx: Context,
        json: BinaryJSON,
        unsigned: bool,
        tp: u8,
    ) -> ValueResult<i64> {
        match json.TypeCode {
            JSONTypeCodeObject
            | JSONTypeCodeArray
            | JSONTypeCodeOpaque
            | JSONTypeCodeDate
            | JSONTypeCodeDatetime
            | JSONTypeCodeTimestamp
            | JSONTypeCodeDuration => ctx.HandleTruncate(
                0,
                errors::New(format!(
                    "truncated incorrect INTEGER value: {}",
                    json.String()
                )),
            ),
            JSONTypeCodeLiteral => match json.Value[0] {
                JSONLiteralFalse => Ok(0),
                JSONLiteralNil => ctx.HandleTruncate(
                    0,
                    errors::New(format!(
                        "truncated incorrect INTEGER value: {}",
                        json.String()
                    )),
                ),
                _ => Ok(1),
            },
            JSONTypeCodeInt64 => {
                let value = json.GetInt64();
                if unsigned {
                    ConvertIntToUint(ctx.Flags(), value, IntegerUnsignedUpperBound(tp), tp)
                        .map(|value| value as i64)
                        .map_err(|error| ErrorWithValue::new(error.value as i64, error.error))
                } else {
                    ConvertIntToInt(
                        value,
                        IntegerSignedLowerBound(tp),
                        IntegerSignedUpperBound(tp),
                        tp,
                    )
                }
            }
            JSONTypeCodeUint64 => {
                let value = json.GetUint64();
                if unsigned {
                    ConvertUintToUint(value, IntegerUnsignedUpperBound(tp), tp)
                        .map(|value| value as i64)
                        .map_err(|error| ErrorWithValue::new(error.value as i64, error.error))
                } else {
                    ConvertUintToInt(value, IntegerSignedUpperBound(tp), tp)
                }
            }
            JSONTypeCodeFloat64 => {
                let value = json.GetFloat64();
                if unsigned {
                    ConvertFloatToUint(ctx.Flags(), value, IntegerUnsignedUpperBound(tp), tp)
                        .map(|value| value as i64)
                        .map_err(|error| ErrorWithValue::new(error.value as i64, error.error))
                } else {
                    ConvertFloatToInt(
                        value,
                        IntegerSignedLowerBound(tp),
                        IntegerSignedUpperBound(tp),
                        tp,
                    )
                }
            }
            JSONTypeCodeString => {
                let value = String::from_utf8_lossy(&json.GetString()).into_owned();
                if value.len() > 1 && value.starts_with('-') {
                    StrToInt(ctx, &value, false)
                } else {
                    StrToUint(ctx, &value, false)
                        .map(|value| value as i64)
                        .map_err(|error| ErrorWithValue::new(error.value as i64, error.error))
                }
            }
            _ => ErrWithValue(0, errors::New("Unknown type code in JSON")),
        }
    }

    /// JSON 转 f64。
    pub fn ConvertJSONToFloat(ctx: Context, json: BinaryJSON) -> ValueResult<f64> {
        match json.TypeCode {
            JSONTypeCodeObject
            | JSONTypeCodeArray
            | JSONTypeCodeOpaque
            | JSONTypeCodeDate
            | JSONTypeCodeDatetime
            | JSONTypeCodeTimestamp
            | JSONTypeCodeDuration => ctx.HandleTruncate(
                0.0,
                errors::New(format!(
                    "truncated incorrect FLOAT value: {}",
                    json.String()
                )),
            ),
            JSONTypeCodeLiteral => match json.Value[0] {
                JSONLiteralFalse => Ok(0.0),
                JSONLiteralNil => ctx.HandleTruncate(
                    0.0,
                    errors::New(format!(
                        "truncated incorrect FLOAT value: {}",
                        json.String()
                    )),
                ),
                _ => Ok(1.0),
            },
            JSONTypeCodeInt64 => Ok(json.GetInt64() as f64),
            JSONTypeCodeUint64 => Ok(json.GetUint64() as f64),
            JSONTypeCodeFloat64 => Ok(json.GetFloat64()),
            JSONTypeCodeString => {
                StrToFloat(ctx, &String::from_utf8_lossy(&json.GetString()), false)
            }
            _ => ErrWithValue(0.0, errors::New("Unknown type code in JSON")),
        }
    }

    /// JSON 转 MyDecimal。
    pub fn ConvertJSONToDecimal(ctx: Context, json: BinaryJSON) -> ValueResult<MyDecimal> {
        let mut result = MyDecimal::default();
        let error = match json.TypeCode {
            JSONTypeCodeObject
            | JSONTypeCodeArray
            | JSONTypeCodeOpaque
            | JSONTypeCodeDate
            | JSONTypeCodeDatetime
            | JSONTypeCodeTimestamp
            | JSONTypeCodeDuration => Some(errors::New(format!(
                "truncated incorrect DECIMAL value: {}",
                json.String()
            ))),
            JSONTypeCodeLiteral => match json.Value[0] {
                JSONLiteralFalse => {
                    result.FromInt(0);
                    None
                }
                JSONLiteralNil => Some(errors::New(format!(
                    "truncated incorrect DECIMAL value: {}",
                    json.String()
                ))),
                _ => {
                    result.FromInt(1);
                    None
                }
            },
            JSONTypeCodeInt64 => {
                result.FromInt(json.GetInt64());
                None
            }
            JSONTypeCodeUint64 => {
                result.FromUint(json.GetUint64());
                None
            }
            JSONTypeCodeFloat64 => result
                .FromFloat64(json.GetFloat64())
                .err()
                .map(errors::SharedError::new),
            JSONTypeCodeString => result
                .FromString(&json.GetString())
                .err()
                .map(errors::SharedError::new),
            _ => None,
        };
        match error {
            Some(error) => ctx.HandleTruncate(result, error),
            None => Ok(result),
        }
    }
}

#[cfg(feature = "types-integration")]
pub use integrated::*;

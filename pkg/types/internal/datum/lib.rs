// Copyright 2026 AsterSQL.
// Formal implementation owner: pkg/types.

// Datum（标量值）内部 crate 聚合入口。
//
// 再导出 decimal/field/time/json 等依赖，提供错误桥接、JSON 比较与转换，
// 以及 Go 标准库兼容桩；最终 `include!` 生产 `datum.rs`。正式实现归属 `pkg/types`。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    unused_imports,
    unused_mut,
    unused_variables
)]

use std::any;

pub use types_decimal::mydecimal::{
    DecimalAdd, ModeHalfUp, MyDecimal, MyDecimalStructSize, NewMaxOrMinDec,
};
pub use types_field_group::{
    ErrOverflow, ErrTruncated, FieldType, GetMaxFloat, IsBinaryStr, IsTypeChar, NewFieldType,
    TruncateFloat,
};
pub use types_file_group::set::{ParseSet, ParseSetValue, Set};
pub use types_group_1::{
    BinaryLiteral, Context, ConvertFloatToInt, ConvertFloatToUint, ConvertIntToInt,
    ConvertIntToUint, ConvertUintToInt, ConvertUintToUint, DefaultStmtNoWarningContext,
    ErrorWithValue, Flags, IntegerSignedLowerBound, IntegerSignedUpperBound,
    IntegerUnsignedUpperBound, NewBinaryLiteralFromUint, StrToFloat, StrToInt, StrToUint,
    UnspecifiedLength, ValueResult, truncateStr,
};
pub use types_group_4::{
    DateStr, DateTimeStr, Enum, ErrDataTooLong, ErrMBiggerThanD, ErrTruncatedWrongVal,
    ErrWrongValue, KindStr, ParseEnum, ParseEnumValue, TimeStr, TimestampStr, TypeStr,
};
pub use types_json::{
    BinaryJSON, CreateBinaryJSON, ErrInvalidJSONCharset, JSONLiteralFalse, JSONLiteralNil,
    JSONTypeCodeArray, JSONTypeCodeDate, JSONTypeCodeDatetime, JSONTypeCodeDuration,
    JSONTypeCodeFloat64, JSONTypeCodeInt64, JSONTypeCodeLiteral, JSONTypeCodeObject,
    JSONTypeCodeOpaque, JSONTypeCodeString, JSONTypeCodeTimestamp, JSONTypeCodeUint64,
    JsonDuration, JsonError, JsonTime, JsonValue, ParseBinaryJSONFromString,
};
pub use types_time::{
    AdjustYear, BasicTimeContext, CoreTime, DefaultFsp, Duration, FromDate, MaxDatetime,
    MaxDuration, MaxFsp, MaxTime, MaxTimestamp, MinDatetime, MinTime, MinTimestamp, NewTime,
    ParseDatetime, ParseDatetimeFromNum, ParseDuration, ParseTime, ParseTimeFromFloatString,
    ParseTimeFromNum, Time, TimeFlags, ZeroDuration,
};
pub use types_vector::{ParseVectorFloat32, VectorFloat32, ZeroCopyDeserializeVectorFloat32};

/// Datum 转换用轻量错误类型及与 SharedError 的桥接。
pub mod errors {
    use std::fmt;

    #[derive(Clone)]
    /// 带可选 cause 的错误消息包装。
    pub struct Error {
        pub(super) message: String,
        pub(super) cause: Option<contextutil::errors::SharedError>,
    }

    impl fmt::Debug for Error {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.debug_tuple("Error").field(&self.message).finish()
        }
    }

    impl PartialEq for Error {
        fn eq(&self, other: &Self) -> bool {
            self.message == other.message
        }
    }

    impl Eq for Error {}

    impl fmt::Display for Error {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str(&self.message)
        }
    }

    impl std::error::Error for Error {}

    /// 由消息构造错误。
    pub fn New(message: impl Into<String>) -> Error {
        Error {
            message: message.into(),
            cause: None,
        }
    }

    /// 格式化风格构造（当前等同 New）。
    pub fn Errorf(message: impl Into<String>) -> Error {
        New(message)
    }

    /// 透传错误（对齐 Go errors.Trace 形态）。
    pub fn Trace(error: Error) -> Error {
        error
    }

    impl Error {
        /// Preserve TiDB's typed error identity across Datum conversion so
        /// callers such as ranger can apply the same tolerated-error branches
        /// as Go instead of matching rendered messages.
        /// 与 typed SharedError 比较，供 ranger 等容忍错误分支。
        pub fn Equal(&self, expected: &contextutil::errors::Error) -> bool {
            expected.Equal(self.cause.as_ref())
        }
    }
}

/// SharedError → Datum Error。
impl From<contextutil::errors::SharedError> for errors::Error {
    fn from(error: contextutil::errors::SharedError) -> Self {
        errors::Error {
            message: error.to_string(),
            cause: Some(error),
        }
    }
}

/// 带值错误 → Datum Error。
impl<T> From<types_group_1::ErrorWithValue<T>> for errors::Error {
    fn from(error: types_group_1::ErrorWithValue<T>) -> Self {
        errors::Error {
            message: error.to_string(),
            cause: Some(error.error),
        }
    }
}

/// 时间错误转换。
impl From<types_time::TimeError> for errors::Error {
    fn from(error: types_time::TimeError) -> Self {
        errors::New(error.to_string())
    }
}

/// DECIMAL 错误转换。
impl From<types_decimal::mydecimal::DecimalError> for errors::Error {
    fn from(error: types_decimal::mydecimal::DecimalError) -> Self {
        errors::New(error.to_string())
    }
}

/// JSON 错误转换。
impl From<types_json::JsonError> for errors::Error {
    fn from(error: types_json::JsonError) -> Self {
        errors::New(error.to_string())
    }
}

/// SET 类型错误转换。
impl From<types_file_group::set::SetError> for errors::Error {
    fn from(error: types_file_group::set::SetError) -> Self {
        errors::New(error.to_string())
    }
}

/// serde_json 错误转换。
impl From<serde_json::Error> for errors::Error {
    fn from(error: serde_json::Error) -> Self {
        errors::New(error.to_string())
    }
}

/// base64 解码错误转换。
impl From<base64::DecodeError> for errors::Error {
    fn from(error: base64::DecodeError) -> Self {
        errors::New(error.to_string())
    }
}

/// 从语句 Context 提取时间解析标志与时区。
pub fn time_context(context: &Context) -> BasicTimeContext {
    BasicTimeContext {
        flags: TimeFlags {
            ignore_zero_in_date: context.Flags().IgnoreZeroInDate(),
            ignore_invalid_date: context.Flags().IgnoreInvalidDateErr(),
            ignore_zero_date: context.Flags().IgnoreZeroDateErr(),
            cast_time_to_year_through_concat: context.Flags().CastTimeToYearThroughConcat(),
        },
        location: context.Location(),
    }
}

/// Duration（纳秒）换算为秒。
pub fn duration_seconds(duration: Duration) -> f64 {
    duration.Duration as f64 / 1_000_000_000.0
}

/// rust_decimal → f64。
pub fn rust_decimal_to_f64(value: rust_decimal::Decimal) -> f64 {
    use rust_decimal::prelude::ToPrimitive;
    value.to_f64().unwrap_or(0.0)
}

/// rust_decimal 四舍五入后 → i64。
pub fn rust_decimal_to_i64(value: rust_decimal::Decimal) -> i64 {
    use rust_decimal::prelude::ToPrimitive;
    value.round().to_i64().unwrap_or(0)
}

/// 经字符串中转构造 MyDecimal。
pub fn mydecimal_from_rust_decimal(
    value: rust_decimal::Decimal,
) -> Result<MyDecimal, errors::Error> {
    let mut result = MyDecimal::default();
    result.FromString(value.to_string().as_bytes())?;
    Ok(result)
}

/// JSON 二进制比较：先按类型优先级，再按值；返回 -1/0/1。
pub fn CompareBinaryJSON(left: &BinaryJSON, right: &BinaryJSON) -> i32 {
    /// Rust Ordering → Go 风格 -1/0/1。
    fn ordering(value: std::cmp::Ordering) -> i32 {
        match value {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
        }
    }

    /// 浮点近似相等比较（阈值 1e-8）。
    fn float_compare(left: f64, right: f64) -> i32 {
        if (left - right).abs() < 1.0e-8 {
            0
        } else if left < right {
            -1
        } else {
            1
        }
    }

    /// MySQL JSON 类型比较优先级（数值越小优先级越低）。
    fn precedence(value: &BinaryJSON) -> i32 {
        match value.Type() {
            "NULL" => -12,
            "INTEGER" | "UNSIGNED INTEGER" | "DOUBLE" => -11,
            "STRING" => -10,
            "OBJECT" => -9,
            "ARRAY" => -8,
            "BOOLEAN" => -7,
            "DATE" => -6,
            "TIME" => -5,
            "DATETIME" => -4,
            "OPAQUE" => -3,
            _ => -3,
        }
    }

    // 不同类型：仅比较类型优先级
    let left_precedence = precedence(left);
    let right_precedence = precedence(right);
    if left_precedence != right_precedence {
        return (left_precedence - right_precedence).signum();
    }

    match (left.GetValue(), right.GetValue()) {
        (JsonValue::Null, JsonValue::Null) => 0,
        (JsonValue::Bool(left), JsonValue::Bool(right)) => ordering(left.cmp(&right)),
        (JsonValue::I64(left), JsonValue::I64(right)) => ordering(left.cmp(&right)),
        (JsonValue::U64(left), JsonValue::U64(right)) => ordering(left.cmp(&right)),
        (JsonValue::I64(left), JsonValue::U64(right)) => {
            if left < 0 {
                -1
            } else {
                ordering((left as u64).cmp(&right))
            }
        }
        (JsonValue::U64(left), JsonValue::I64(right)) => {
            -CompareBinaryJSON(&CreateBinaryJSON(right), &CreateBinaryJSON(left))
        }
        (JsonValue::F64(left), JsonValue::I64(right)) => float_compare(left, right as f64),
        (JsonValue::F64(left), JsonValue::U64(right)) => float_compare(left, right as f64),
        (JsonValue::I64(left), JsonValue::F64(right)) => -float_compare(right, left as f64),
        (JsonValue::U64(left), JsonValue::F64(right)) => -float_compare(right, left as f64),
        (JsonValue::F64(left), JsonValue::F64(right)) => {
            if left < right {
                -1
            } else if left == right {
                0
            } else {
                1
            }
        }
        (JsonValue::String(left), JsonValue::String(right)) => {
            ordering(left.as_bytes().cmp(right.as_bytes()))
        }
        // 数组：逐元素比较，再比长度
        (JsonValue::Array(left), JsonValue::Array(right)) => {
            for (left, right) in left.iter().zip(&right) {
                let comparison = CompareBinaryJSON(
                    &CreateBinaryJSON(left.clone()),
                    &CreateBinaryJSON(right.clone()),
                );
                if comparison != 0 {
                    return comparison;
                }
            }
            ordering(left.len().cmp(&right.len()))
        }
        // 对象：先比键，再比值
        (JsonValue::Object(left), JsonValue::Object(right)) => {
            if left.len() != right.len() {
                return ordering(left.len().cmp(&right.len()));
            }
            for ((left_key, left), (right_key, right)) in left.iter().zip(&right) {
                let comparison = ordering(left_key.as_bytes().cmp(right_key.as_bytes()));
                if comparison != 0 {
                    return comparison;
                }
                let comparison = CompareBinaryJSON(
                    &CreateBinaryJSON(left.clone()),
                    &CreateBinaryJSON(right.clone()),
                );
                if comparison != 0 {
                    return comparison;
                }
            }
            0
        }
        (JsonValue::Opaque(left), JsonValue::Opaque(right)) => ordering(left.Buf.cmp(&right.Buf)),
        (JsonValue::Time(left), JsonValue::Time(right)) => {
            ordering(left.CoreTime.cmp(&right.CoreTime))
        }
        (JsonValue::Duration(left), JsonValue::Duration(right)) => {
            ordering(left.Duration.cmp(&right.Duration))
        }
        (JsonValue::Binary(left), JsonValue::Binary(right)) => CompareBinaryJSON(&left, &right),
        (left, right) => ordering(format!("{left:?}").cmp(&format!("{right:?}"))),
    }
}

/// JSON 去引号：字符串取原文，其它类型走 String()。
pub fn json_unquote(value: &BinaryJSON) -> String {
    match value.GetValue() {
        JsonValue::String(text) => text,
        _ => value.String(),
    }
}

/// 构造 JSON 标量转换的截断错误。
fn json_truncated_error(kind: &str, value: &BinaryJSON) -> contextutil::errors::SharedError {
    contextutil::errors::New(format!(
        "truncated incorrect {kind} value: {}",
        value.String()
    ))
}

/// JSON 值转为 f64，保留合法数字前缀与截断错误。
pub fn ConvertJSONToFloat(context: Context, value: BinaryJSON) -> ValueResult<f64> {
    match value.TypeCode {
        JSONTypeCodeObject
        | JSONTypeCodeArray
        | JSONTypeCodeOpaque
        | JSONTypeCodeDate
        | JSONTypeCodeDatetime
        | JSONTypeCodeTimestamp
        | JSONTypeCodeDuration => {
            context.HandleTruncate(0.0, json_truncated_error("FLOAT", &value))
        }
        JSONTypeCodeLiteral => match value.Value[0] {
            JSONLiteralFalse => Ok(0.0),
            JSONLiteralNil => context.HandleTruncate(0.0, json_truncated_error("FLOAT", &value)),
            _ => Ok(1.0),
        },
        JSONTypeCodeInt64 => Ok(value.GetInt64() as f64),
        JSONTypeCodeUint64 => Ok(value.GetUint64() as f64),
        JSONTypeCodeFloat64 => Ok(value.GetFloat64()),
        JSONTypeCodeString => {
            StrToFloat(context, &String::from_utf8_lossy(&value.GetString()), false)
        }
        _ => Err(ErrorWithValue::new(
            0.0,
            contextutil::errors::New("Unknown type code in JSON"),
        )),
    }
}

/// JSON 转整数，按来源类型执行 Go 的裁剪、舍入与字符串前缀规则。
pub fn ConvertJSONToInt(
    context: Context,
    value: BinaryJSON,
    unsigned: bool,
    tp: u8,
) -> ValueResult<i64> {
    match value.TypeCode {
        JSONTypeCodeObject
        | JSONTypeCodeArray
        | JSONTypeCodeOpaque
        | JSONTypeCodeDate
        | JSONTypeCodeDatetime
        | JSONTypeCodeTimestamp
        | JSONTypeCodeDuration => {
            context.HandleTruncate(0, json_truncated_error("INTEGER", &value))
        }
        JSONTypeCodeLiteral => match value.Value[0] {
            JSONLiteralFalse => Ok(0),
            JSONLiteralNil => context.HandleTruncate(0, json_truncated_error("INTEGER", &value)),
            _ => Ok(1),
        },
        JSONTypeCodeInt64 => {
            let integer = value.GetInt64();
            if unsigned {
                ConvertIntToUint(context.Flags(), integer, IntegerUnsignedUpperBound(tp), tp)
                    .map(|value| value as i64)
                    .map_err(|error| ErrorWithValue::new(error.value as i64, error.error))
            } else {
                ConvertIntToInt(
                    integer,
                    IntegerSignedLowerBound(tp),
                    IntegerSignedUpperBound(tp),
                    tp,
                )
            }
        }
        JSONTypeCodeUint64 => {
            let integer = value.GetUint64();
            if unsigned {
                ConvertUintToUint(integer, IntegerUnsignedUpperBound(tp), tp)
                    .map(|value| value as i64)
                    .map_err(|error| ErrorWithValue::new(error.value as i64, error.error))
            } else {
                ConvertUintToInt(integer, IntegerSignedUpperBound(tp), tp)
            }
        }
        JSONTypeCodeFloat64 => {
            let float = value.GetFloat64();
            if unsigned {
                ConvertFloatToUint(context.Flags(), float, IntegerUnsignedUpperBound(tp), tp)
                    .map(|value| value as i64)
                    .map_err(|error| ErrorWithValue::new(error.value as i64, error.error))
            } else {
                types_group_1::ConvertFloatToInt(
                    float,
                    IntegerSignedLowerBound(tp),
                    IntegerSignedUpperBound(tp),
                    tp,
                )
            }
        }
        JSONTypeCodeString => {
            let string = String::from_utf8_lossy(&value.GetString()).into_owned();
            if string.len() > 1 && string.starts_with('-') {
                StrToInt(context, &string, false)
            } else {
                StrToUint(context, &string, false)
                    .map(|value| value as i64)
                    .map_err(|error| ErrorWithValue::new(error.value as i64, error.error))
            }
        }
        _ => Err(ErrorWithValue::new(
            0,
            contextutil::errors::New("Unknown type code in JSON"),
        )),
    }
}

/// JSON → i64（按 LONGLONG 上下界）。
pub fn ConvertJSONToInt64(context: Context, value: BinaryJSON, unsigned: bool) -> ValueResult<i64> {
    ConvertJSONToInt(context, value, unsigned, mysql::TypeLonglong)
}

/// JSON → MyDecimal，截断时保留已经构造的十进制值。
pub fn ConvertJSONToDecimal(context: Context, value: BinaryJSON) -> ValueResult<MyDecimal> {
    let mut decimal = MyDecimal::default();
    let error = match value.TypeCode {
        JSONTypeCodeObject
        | JSONTypeCodeArray
        | JSONTypeCodeOpaque
        | JSONTypeCodeDate
        | JSONTypeCodeDatetime
        | JSONTypeCodeTimestamp
        | JSONTypeCodeDuration => Some(json_truncated_error("DECIMAL", &value)),
        JSONTypeCodeLiteral => match value.Value[0] {
            JSONLiteralFalse => {
                decimal.FromInt(0);
                None
            }
            JSONLiteralNil => Some(json_truncated_error("DECIMAL", &value)),
            _ => {
                decimal.FromInt(1);
                None
            }
        },
        JSONTypeCodeInt64 => {
            decimal.FromInt(value.GetInt64());
            None
        }
        JSONTypeCodeUint64 => {
            decimal.FromUint(value.GetUint64());
            None
        }
        JSONTypeCodeFloat64 => decimal
            .FromFloat64(value.GetFloat64())
            .err()
            .map(|error| context_error(&error)),
        JSONTypeCodeString => decimal
            .FromString(&value.GetString())
            .err()
            .map(|error| context_error(&error)),
        _ => None,
    };
    match error {
        Some(error) => context.HandleTruncate(decimal, error),
        None => Ok(decimal),
    }
}

/// MyDecimal 整数部分转为 u64，再做类型上界检查。
pub fn ConvertDecimalToUint(decimal: &MyDecimal, upper_bound: u64, tp: u8) -> ValueResult<u64> {
    let rendered = decimal.ToString();
    let text = String::from_utf8_lossy(&rendered);
    let (integer, fraction) = text.split_once('.').unwrap_or((&text, ""));
    let integer = integer.trim_start_matches('0');
    let integer = if integer.is_empty() { "0" } else { integer };
    if integer.starts_with('-') {
        return Err(ErrorWithValue::new(0, context_error(&overflow(&text, tp))));
    }

    // Go rounds from the first fractional digit when converting DECIMAL to
    // an integer. Later digits do not affect this half-up decision.
    let round = u64::from(
        fraction
            .as_bytes()
            .first()
            .is_some_and(|digit| *digit >= b'5'),
    );
    let Some(upper_without_round) = upper_bound.checked_sub(round) else {
        return Err(ErrorWithValue::new(
            upper_bound,
            context_error(&overflow(&text, tp)),
        ));
    };
    let upper_without_round = upper_without_round.to_string();
    if integer.len() > upper_without_round.len()
        || (integer.len() == upper_without_round.len()
            && integer.as_bytes() > upper_without_round.as_bytes())
    {
        return Err(ErrorWithValue::new(
            upper_bound,
            context_error(&overflow(&text, tp)),
        ));
    }

    match integer.parse::<u64>() {
        Ok(parsed) => Ok(parsed + round),
        Err(_) => Err(ErrorWithValue::new(0, context_error(&overflow(&text, tp)))),
    }
}

/// 构造常量溢出错误消息。
pub fn overflow(value: &dyn std::fmt::Debug, tp: u8) -> errors::Error {
    errors::New(format!("constant {value:?} overflows {}", TypeStr(tp)))
}

/// 包装为 SharedError。
pub fn context_error(error: &impl std::fmt::Display) -> contextutil::errors::SharedError {
    contextutil::errors::New(error.to_string())
}

/// terror 兼容桩：日志与错误相等比较。
pub mod terror {
    pub fn Log<T: std::fmt::Display>(error: Option<T>) {
        if let Some(error) = error {
            eprintln!("{error}");
        }
    }

    pub fn ErrorEqual(left: &crate::errors::Error, right: &contextutil::errors::Error) -> bool {
        left.Equal(right) || left.to_string() == right.to_string()
    }
}

/// MySQL 类型/字符集常量再导出。
pub mod mysql {
    pub use parser_mysql::charset::{DefaultCharset, DefaultCollationName};
    pub use parser_mysql::r#type::*;
}

/// parser types 再导出。
pub mod types {
    pub use parser_types::types::*;
}

/// 字符集与编码再导出。
pub mod charset {
    pub use parser_charset::charset::{CollationBin, GetCollationByName};
    pub use parser_charset::encoding::{EncodingRef, EncodingTpASCII, EncodingTpUTF8};
    pub use parser_charset::{
        CharsetASCII, CharsetBin, CharsetGB18030, CharsetGBK, CharsetLatin1, CharsetUTF8,
        CharsetUTF8MB4, CountValidBytes, CountValidBytesDecode, EncodingUTF8MB3StrictImpl,
        FindEncoding, FindEncodingTakeUTF8AsNoop, OpDecode, OpEncodeNoErr, OpReplace,
    };
}

/// 排序规则再导出。
pub mod collate {
    pub use ::collate::*;
}

/// planner base 再导出。
pub mod base {
    pub use planner_base::base::*;
}

/// 字节/字符串零拷贝风格互转桩。
pub mod hack {
    pub fn String(bytes: &[u8]) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(bytes)
    }

    pub fn Slice(value: &str) -> Vec<u8> {
        value.as_bytes().to_vec()
    }
}

/// PartialOrd → -1/0/1 比较。
pub mod cmp {
    pub fn Compare<T: PartialOrd>(left: T, right: T) -> i32 {
        match left.partial_cmp(&right) {
            Some(std::cmp::Ordering::Less) => -1,
            Some(std::cmp::Ordering::Greater) => 1,
            _ => 0,
        }
    }
}

/// 数值上下界常量。
pub mod math {
    pub const MaxFloat32: f64 = f32::MAX as f64;
    pub const MaxFloat64: f64 = f64::MAX;
    pub const MaxInt16: i16 = i16::MAX;
    pub const MaxInt32: i32 = i32::MAX;
    pub const MaxInt64: i64 = i64::MAX;
    pub const MaxUint16: u16 = u16::MAX;
    pub const MaxUint32: u32 = u32::MAX;
    pub const MaxUint64: u64 = u64::MAX;
}

/// Go strconv 兼容再导出。
pub mod strconv {
    pub use goish::strconv::*;
}

/// Unicode 控制字符判断。
pub mod unicode {
    pub fn IsControl(character: char) -> bool {
        character.is_control()
    }
}

/// UTF-8 rune 计数与合法性桩。
pub mod utf8 {
    pub fn RuneCountInString(value: &str) -> usize {
        value.chars().count()
    }

    pub fn ValidString(value: impl AsRef<[u8]>) -> bool {
        std::str::from_utf8(value.as_ref()).is_ok()
    }
}

/// 简易 Duration 包装。
pub mod time {
    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    pub struct Duration(pub i64);
}

// 挂接生产 Datum 实现
include!("../../datum.rs");

#[cfg(test)]
mod migration_aster_unit_test;

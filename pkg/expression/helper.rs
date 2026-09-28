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

// Default-time helpers from `helper.go`. Chrono and chrono-tz are
// used for calendar validation, fractional-second truncation and timezone
// conversion instead of local replacements for timezone data.
//
// 默认时间值辅助逻辑（对应 Go `helper.go`）。
//
// 负责列默认值中的 `CURRENT_TIMESTAMP` / `CURRENT_DATE` 判定，
// 以及将文本/整型/函数形式输入解析为 MySQL 时间值。
// fsp（fractional seconds precision，小数秒精度）范围为 0..=6。

use std::fmt;

use chrono::{DateTime, NaiveDate, NaiveDateTime, Timelike, Utc};
use chrono_tz::Tz;
use thiserror::Error;

/// 布尔转 i64：true→1，false→0（与 MySQL 布尔语义一致）。
pub fn bool_to_int64(value: bool) -> i64 {
    i64::from(value)
}

/// 时间相关表达式节点：字面量或函数调用。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TimeExpr {
    /// 普通时间字面量。
    Value,
    /// 命名函数及其整型参数（如 `current_timestamp(3)`）。
    Function { name: String, arguments: Vec<i64> },
}

impl TimeExpr {
    /// 构造 `current_timestamp` 函数表达式。
    pub fn current_timestamp(arguments: Vec<i64>) -> Self {
        Self::Function {
            name: "current_timestamp".into(),
            arguments,
        }
    }
}

/// 时间字段类型元信息；`decimal` 即 fsp。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TimeFieldType {
    /// 小数秒精度（0–6）。
    pub decimal: i32,
}

/// 校验表达式是否可作为合法的 `CURRENT_TIMESTAMP` 默认值。
///
/// 规则：无参且字段无 fsp，或单参且与字段 `decimal` 一致。
pub fn is_valid_current_timestamp_expr(
    expression: &TimeExpr,
    field_type: Option<&TimeFieldType>,
) -> bool {
    let TimeExpr::Function { name, arguments } = expression else {
        return false;
    };
    if !name.eq_ignore_ascii_case("current_timestamp") {
        return false;
    }
    // 参数个数与字段 fsp 必须一致：有参则唯一且等于 decimal；无参则 decimal 不得 > 0。
    let contains_argument = !arguments.is_empty();
    let contains_fsp = field_type.is_some_and(|field| field.decimal > 0);
    let consistent = contains_argument
        && field_type.is_some_and(|field| arguments[0] == i64::from(field.decimal));
    (contains_argument && consistent) || (!contains_argument && !contains_fsp)
}

/// MySQL 时间类型：TIMESTAMP / DATETIME / DATE。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimeType {
    Timestamp,
    Datetime,
    Date,
}

/// MySQL 时间值内部表示：墙钟时间 + 类型 + fsp。
///
/// `value == None` 表示零日期 `0000-00-00 00:00:00`。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MysqlTime {
    value: Option<NaiveDateTime>,
    time_type: TimeType,
    fsp: u32,
}

impl MysqlTime {
    /// 构造零时间（零日期）。
    fn zero(time_type: TimeType, fsp: u32) -> Self {
        Self {
            value: None,
            time_type,
            fsp,
        }
    }

    /// 构造非空墙钟时间。
    fn new(value: NaiveDateTime, time_type: TimeType, fsp: u32) -> Self {
        Self {
            value: Some(value),
            time_type,
            fsp,
        }
    }
}

impl fmt::Display for MysqlTime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 零日期按 MySQL 惯例输出固定字符串。
        let Some(value) = self.value else {
            if self.time_type == TimeType::Date {
                return formatter.write_str("0000-00-00");
            }
            if self.fsp == 0 {
                return formatter.write_str("0000-00-00 00:00:00");
            }
            return write!(
                formatter,
                "0000-00-00 00:00:00.{:0width$}",
                0,
                width = self.fsp as usize
            );
        };
        let base = match self.time_type {
            TimeType::Date => value.format("%Y-%m-%d").to_string(),
            TimeType::Timestamp | TimeType::Datetime => {
                value.format("%Y-%m-%d %H:%M:%S").to_string()
            }
        };
        if self.fsp == 0 || self.time_type == TimeType::Date {
            formatter.write_str(&base)
        } else {
            // 按 fsp 截断纳秒并左补零输出小数秒。
            let divisor = 10_u32.pow(9 - self.fsp);
            write!(
                formatter,
                "{base}.{:0width$}",
                value.nanosecond() / divisor,
                width = self.fsp as usize
            )
        }
    }
}

/// 解析后的时间默认值：NULL、具体时间或延迟求值函数名。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TimeValue {
    Null,
    MysqlTime(MysqlTime),
    /// 如 `CURRENT_TIMESTAMP`，插入时再求值。
    DeferredFunction(String),
}

impl fmt::Display for TimeValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Null => formatter.write_str("NULL"),
            Self::MysqlTime(value) => value.fmt(formatter),
            Self::DeferredFunction(value) => formatter.write_str(value),
        }
    }
}

/// 默认值解析的输入形态（AST / 常量折叠结果的抽象）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TimeInput {
    Text(String),
    Integer(i64),
    Null,
    Function(String),
    /// 一元加减后的整型字面量。
    UnaryInteger(i64),
    Other,
}

/// 语句级时间上下文：冻结的 statement 时间与会话时区。
///
/// statement 时间在同一语句内保持不变，保证 `CURRENT_TIMESTAMP` 稳定。
#[derive(Clone, Debug)]
pub struct TimeContext {
    statement_time: DateTime<Utc>,
    location: Tz,
}

impl TimeContext {
    /// 用 UTC 时刻与会话时区构造上下文。
    pub fn new(statement_time: DateTime<Utc>, location: Tz) -> Self {
        Self {
            statement_time,
            location,
        }
    }

    /// 返回本语句冻结的 UTC 时间。
    pub fn statement_time(&self) -> DateTime<Utc> {
        self.statement_time
    }

    /// 返回会话时区。
    pub fn location(&self) -> Tz {
        self.location
    }
}

/// 时间解析错误。
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum TimeError {
    #[error("fsp must be between 0 and 6, got {0}")]
    InvalidFsp(i32),
    #[error("invalid time value: {0}")]
    InvalidTime(String),
    #[error("invalid default value")]
    InvalidDefaultValue,
}

/// 取当前 statement 时间并包装为 `TimeValue::MysqlTime`。
pub fn get_time_current_timestamp(
    context: &TimeContext,
    time_type: TimeType,
    fsp: i32,
) -> Result<TimeValue, TimeError> {
    Ok(TimeValue::MysqlTime(current_mysql_time(
        context, time_type, fsp,
    )?))
}

/// 将 statement 时间转换到会话时区，按 fsp 截断小数秒；DATE 类型清零时分秒。
fn current_mysql_time(
    context: &TimeContext,
    time_type: TimeType,
    fsp: i32,
) -> Result<MysqlTime, TimeError> {
    let fsp = validate_fsp(fsp)?;
    let localized = context.statement_time.with_timezone(&context.location);
    // 按 10^(9-fsp) 纳秒量子向下取整，模拟 MySQL 小数秒截断。
    let quantum = 10_u32.pow(9 - fsp);
    let nanos = localized.nanosecond() / quantum * quantum;
    let mut value = localized
        .naive_local()
        .with_nanosecond(nanos)
        .ok_or_else(|| TimeError::InvalidTime(localized.to_string()))?;
    if time_type == TimeType::Date {
        value = value
            .date()
            .and_hms_opt(0, 0, 0)
            .expect("midnight is valid");
    }
    Ok(MysqlTime::new(value, time_type, fsp))
}

/// 将各类默认值输入解析为 `TimeValue`。
///
/// 函数形式的 `current_timestamp`/`current_date` 延迟求值；
/// 文本同名则立即求值；零日期与 0 整型映射为零时间。
pub fn get_time_value(
    context: &TimeContext,
    input: TimeInput,
    time_type: TimeType,
    fsp: i32,
    explicit_timezone: Option<Tz>,
) -> Result<TimeValue, TimeError> {
    let fsp = validate_fsp(fsp)?;
    match input {
        TimeInput::Null => Ok(TimeValue::Null),
        TimeInput::Other => Ok(TimeValue::Null),
        TimeInput::Function(name)
            if name.eq_ignore_ascii_case("current_timestamp")
                || name.eq_ignore_ascii_case("current_date") =>
        {
            // 函数节点：保留大写函数名，插入行时再求值。
            Ok(TimeValue::DeferredFunction(name.to_ascii_uppercase()))
        }
        TimeInput::Function(_) => Err(TimeError::InvalidDefaultValue),
        TimeInput::Text(text) if text.eq_ignore_ascii_case("current_timestamp") => Ok(
            TimeValue::MysqlTime(current_mysql_time(context, time_type, fsp as i32)?),
        ),
        TimeInput::Text(text) if text.eq_ignore_ascii_case("current_date") => Ok(
            TimeValue::MysqlTime(current_mysql_time(context, TimeType::Date, fsp as i32)?),
        ),
        TimeInput::Text(text) if text == "0000-00-00 00:00:00" => {
            Ok(TimeValue::MysqlTime(MysqlTime::zero(time_type, fsp)))
        }
        TimeInput::Text(text) => {
            // Parsing uses the explicit timezone when supplied, matching Go's TypeCtx override.
            // The stored MySQL value is wall-clock data, so its display remains unchanged.
            // 显式时区仅影响解析上下文；存储的仍是墙钟时间，显示不变。
            let _parse_location = explicit_timezone.unwrap_or(context.location);
            parse_text_time(&text, time_type, fsp).map(TimeValue::MysqlTime)
        }
        TimeInput::Integer(0) | TimeInput::UnaryInteger(0) => {
            Ok(TimeValue::MysqlTime(MysqlTime::zero(time_type, fsp)))
        }
        TimeInput::Integer(value) | TimeInput::UnaryInteger(value) => {
            parse_numeric_time(value, time_type, fsp).map(TimeValue::MysqlTime)
        }
    }
}

/// 校验 fsp 落在 MySQL 允许的 0..=6。
fn validate_fsp(fsp: i32) -> Result<u32, TimeError> {
    if (0..=6).contains(&fsp) {
        Ok(fsp as u32)
    } else {
        Err(TimeError::InvalidFsp(fsp))
    }
}

/// 解析 `YYYY-MM-DD[ HH:MM:SS[.f]]` 文本；仅日期时补零时分秒。
fn parse_text_time(text: &str, time_type: TimeType, fsp: u32) -> Result<MysqlTime, TimeError> {
    let parsed = NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S%.f")
        .or_else(|_| {
            NaiveDate::parse_from_str(text, "%Y-%m-%d")
                .map(|date| date.and_hms_opt(0, 0, 0).expect("midnight is valid"))
        })
        .map_err(|_| TimeError::InvalidTime(text.into()))?;
    Ok(MysqlTime::new(parsed, time_type, fsp))
}

/// 按 MySQL 规则将 YYMMDD、YYYYMMDD、YYMMDDHHmmss 或 YYYYMMDDHHmmss
/// 数字时间扩展为 14 位格式后解析。
fn parse_numeric_time(value: i64, time_type: TimeType, fsp: u32) -> Result<MysqlTime, TimeError> {
    if value < 101 {
        return Err(TimeError::InvalidTime(value.to_string()));
    }
    let original = value;
    let value = if value <= 691_231 {
        (value + 20_000_000) * 1_000_000
    } else if value < 700_101 {
        return Err(TimeError::InvalidTime(original.to_string()));
    } else if value <= 991_231 {
        (value + 19_000_000) * 1_000_000
    } else if value <= 99_991_231 {
        value * 1_000_000
    } else if value < 101_000_000 {
        return Err(TimeError::InvalidTime(original.to_string()));
    } else if value <= 691_231_235_959 {
        value + 20_000_000_000_000
    } else if value < 700_101_000_000 {
        return Err(TimeError::InvalidTime(original.to_string()));
    } else if value <= 991_231_235_959 {
        value + 19_000_000_000_000
    } else {
        value
    };
    let text = value.to_string();
    if text.len() != 14 {
        return Err(TimeError::InvalidTime(original.to_string()));
    }
    let display = format!(
        "{}-{}-{} {}:{}:{}",
        &text[0..4],
        &text[4..6],
        &text[6..8],
        &text[8..10],
        &text[10..12],
        &text[12..14]
    );
    parse_text_time(&display, time_type, fsp)
}

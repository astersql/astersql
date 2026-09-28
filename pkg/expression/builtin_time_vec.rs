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
// Copyright 2026 AsterSQL.

// Native vector kernels for TiDB time builtins.
//
// The scalar time builtin and the expression/chunk integration are migrated by
// separate file groups.  This module therefore owns the behavior that belongs
// specifically to the vector layer: row order, SQL NULL propagation, warning
// versus error handling, and the direct per-row transformations from
// `builtin_time_vec.go`.  Operations owned by the scalar layer can be supplied
// through [`vec_map`] and [`vec_zip_map`] without weakening their errors.
//
// 时间类内置函数的原生向量化内核（对应 Go `builtin_time_vec.go`）。
// 向量化指一次对 Chunk/列批量求值，而非逐行标量调用。本模块负责行序、SQL NULL 传播、
// 警告与错误分流，以及日期/时间抽取、构造、时区转换与 TSO 解析等按行变换。

use chrono::{
    DateTime, Datelike, Duration, FixedOffset, NaiveDate, NaiveDateTime, Offset, TimeZone,
    Timelike, Utc,
};
use chrono_tz::Tz;

const WEEKDAY_NAMES: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];
const MONTH_NAMES: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const MAX_TIME_HOUR: i64 = 838;
const MICROS_PER_SECOND: i64 = 1_000_000;
const MICROS_PER_MINUTE: i64 = 60 * MICROS_PER_SECOND;
const MICROS_PER_HOUR: i64 = 60 * MICROS_PER_MINUTE;

/// 时间向量化求值错误：非法时间、参数错误、FSP 越界、溢出、时区问题、列长不一致等。
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TimeVecError {
    #[error("invalid time: {0}")]
    InvalidTime(String),
    #[error("incorrect arguments to {0}")]
    IncorrectArgs(&'static str),
    #[error("invalid fractional-seconds precision: {0}")]
    InvalidFsp(i64),
    #[error("time overflow: {0}")]
    Overflow(String),
    #[error("invalid time zone: {0}")]
    InvalidTimeZone(String),
    #[error("vector columns have different lengths")]
    LengthMismatch,
    #[error("ambiguous local time: {0}")]
    AmbiguousLocalTime(String),
    #[error("nonexistent local time: {0}")]
    NonexistentLocalTime(String),
}

/// 求值上下文：控制零日期模式、严格模式，并收集非严格路径下的警告。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EvalContext {
    pub no_zero_date: bool,
    pub no_zero_in_date: bool,
    pub strict: bool,
    pub warnings: Vec<TimeVecError>,
}

impl EvalContext {
    fn handle_invalid(&mut self, value: impl Into<String>) -> Result<(), TimeVecError> {
        let error = TimeVecError::InvalidTime(value.into());
        if self.strict {
            Err(error)
        } else {
            self.warnings.push(error);
            Ok(())
        }
    }

    fn handle_truncate(&mut self, value: impl Into<String>) -> Result<(), TimeVecError> {
        let error = TimeVecError::Overflow(value.into());
        if self.strict {
            Err(error)
        } else {
            self.warnings.push(error);
            Ok(())
        }
    }
}

/// 可空列向量：按行存放 `Option<T>`，NULL 对应 `None`，保持与输入相同行序。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NullableVec<T> {
    values: Vec<Option<T>>,
}

impl<T> NullableVec<T> {
    pub fn new(values: Vec<Option<T>>) -> Self {
        Self { values }
    }

    pub fn values(&self) -> &[Option<T>] {
        &self.values
    }

    pub fn into_inner(self) -> Vec<Option<T>> {
        self.values
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

/// 对单列做逐行映射；输入为 NULL 时输出 NULL，求值错误按 `Result` 上抛。
pub fn vec_map<T, U, F>(input: &NullableVec<T>, mut eval: F) -> Result<NullableVec<U>, TimeVecError>
where
    F: FnMut(&T) -> Result<U, TimeVecError>,
{
    let mut output = Vec::with_capacity(input.len());
    for value in input.values() {
        output.push(match value {
            Some(value) => Some(eval(value)?),
            None => None,
        });
    }
    Ok(NullableVec::new(output))
}

/// 对两列按行 zip 后求值；任一侧 NULL 则输出 NULL，列长不一致报 `LengthMismatch`。
pub fn vec_zip_map<A, B, U, F>(
    left: &NullableVec<A>,
    right: &NullableVec<B>,
    mut eval: F,
) -> Result<NullableVec<U>, TimeVecError>
where
    F: FnMut(&A, &B) -> Result<Option<U>, TimeVecError>,
{
    if left.len() != right.len() {
        return Err(TimeVecError::LengthMismatch);
    }
    let mut output = Vec::with_capacity(left.len());
    for (left, right) in left.values().iter().zip(right.values()) {
        output.push(match (left, right) {
            (Some(left), Some(right)) => eval(left, right)?,
            _ => None,
        });
    }
    Ok(NullableVec::new(output))
}

/// MySQL 风格日期时间分量（年/月/日/时/分/秒/微秒），供向量内核直接变换。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct MysqlTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub microsecond: u32,
}

impl MysqlTime {
    pub fn new(
        year: i32,
        month: u32,
        day: u32,
        hour: u32,
        minute: u32,
        second: u32,
        microsecond: u32,
    ) -> Result<Self, TimeVecError> {
        let value = Self::from_parts_unchecked(year, month, day, hour, minute, second, microsecond);
        value.to_naive()?;
        Ok(value)
    }

    pub const fn from_parts_unchecked(
        year: i32,
        month: u32,
        day: u32,
        hour: u32,
        minute: u32,
        second: u32,
        microsecond: u32,
    ) -> Self {
        Self {
            year,
            month,
            day,
            hour,
            minute,
            second,
            microsecond,
        }
    }

    pub const fn zero() -> Self {
        Self::from_parts_unchecked(0, 0, 0, 0, 0, 0, 0)
    }

    pub fn is_zero(self) -> bool {
        self.year == 0
            && self.month == 0
            && self.day == 0
            && self.hour == 0
            && self.minute == 0
            && self.second == 0
            && self.microsecond == 0
    }

    pub fn invalid_zero(self) -> bool {
        self.year == 0 || self.month == 0 || self.day == 0
    }

    pub fn to_naive(self) -> Result<NaiveDateTime, TimeVecError> {
        let date = NaiveDate::from_ymd_opt(self.year, self.month, self.day)
            .ok_or_else(|| TimeVecError::InvalidTime(self.to_string()))?;
        date.and_hms_micro_opt(self.hour, self.minute, self.second, self.microsecond)
            .ok_or_else(|| TimeVecError::InvalidTime(self.to_string()))
    }

    fn date_only(self) -> Self {
        Self::from_parts_unchecked(self.year, self.month, self.day, 0, 0, 0, 0)
    }
}

impl std::fmt::Display for MysqlTime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:06}",
            self.year, self.month, self.day, self.hour, self.minute, self.second, self.microsecond
        )
    }
}

/// MySQL TIME/DURATION，内部以微秒存放，保留符号。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MysqlDuration {
    micros: i64,
}

impl MysqlDuration {
    pub const fn from_micros(micros: i64) -> Self {
        Self { micros }
    }

    pub const fn as_micros(self) -> i64 {
        self.micros
    }

    pub fn hour(self) -> i64 {
        self.micros.unsigned_abs() as i64 / MICROS_PER_HOUR
    }

    pub fn minute(self) -> i64 {
        (self.micros.unsigned_abs() as i64 / MICROS_PER_MINUTE) % 60
    }

    pub fn second(self) -> i64 {
        (self.micros.unsigned_abs() as i64 / MICROS_PER_SECOND) % 60
    }

    pub fn microsecond(self) -> i64 {
        self.micros.unsigned_abs() as i64 % MICROS_PER_SECOND
    }
}

/// 向量化 MONTH：抽取月份，NULL 传播。
pub fn vec_month(input: &NullableVec<MysqlTime>) -> NullableVec<i64> {
    NullableVec::new(
        input
            .values()
            .iter()
            .map(|v| v.map(|t| i64::from(t.month)))
            .collect(),
    )
}

/// 向量化 YEAR：抽取年份，NULL 传播。
pub fn vec_year(input: &NullableVec<MysqlTime>) -> NullableVec<i64> {
    NullableVec::new(
        input
            .values()
            .iter()
            .map(|v| v.map(|t| i64::from(t.year)))
            .collect(),
    )
}

/// 向量化 DAYOFMONTH：抽取日，NULL 传播。
pub fn vec_day_of_month(input: &NullableVec<MysqlTime>) -> NullableVec<i64> {
    NullableVec::new(
        input
            .values()
            .iter()
            .map(|v| v.map(|t| i64::from(t.day)))
            .collect(),
    )
}

/// 向量化 QUARTER：按月份计算季度（1–4），NULL 传播。
pub fn vec_quarter(input: &NullableVec<MysqlTime>) -> NullableVec<i64> {
    NullableVec::new(
        input
            .values()
            .iter()
            .map(|v| v.map(|t| i64::from((t.month + 2) / 3)))
            .collect(),
    )
}

/// 向量化 DATE：截断为日期；在 no_zero_* 模式下把非法零日期转为 NULL（或严格报错）。
pub fn vec_date(
    input: &NullableVec<MysqlTime>,
    ctx: &mut EvalContext,
) -> Result<NullableVec<MysqlTime>, TimeVecError> {
    let mut output = Vec::with_capacity(input.len());
    for value in input.values() {
        let Some(value) = value else {
            output.push(None);
            continue;
        };
        // 零日期与零分量日期按 SQL 模式拒绝：非严格记警告并输出 NULL，严格则上抛。
        let rejected = (value.is_zero() && ctx.no_zero_date)
            || (!value.is_zero() && value.invalid_zero() && ctx.no_zero_in_date);
        if rejected {
            ctx.handle_invalid(value.to_string())?;
            output.push(None);
        } else {
            output.push(Some(value.date_only()));
        }
    }
    Ok(NullableVec::new(output))
}

fn valid_date_or_null(
    value: MysqlTime,
    ctx: &mut EvalContext,
) -> Result<Option<NaiveDate>, TimeVecError> {
    if value.invalid_zero() {
        ctx.handle_invalid(value.to_string())?;
        return Ok(None);
    }
    Ok(Some(value.to_naive()?.date()))
}

/// 向量化 WEEKDAY：周一为 0，与 MySQL 一致；非法零日期走警告/错误。
pub fn vec_weekday(
    input: &NullableVec<MysqlTime>,
    ctx: &mut EvalContext,
) -> Result<NullableVec<i64>, TimeVecError> {
    vec_map_nullable_time(input, ctx, |date| {
        i64::from(date.weekday().num_days_from_monday())
    })
}

/// 向量化 DAYOFWEEK：周日为 1，与 MySQL 一致。
pub fn vec_day_of_week(
    input: &NullableVec<MysqlTime>,
    ctx: &mut EvalContext,
) -> Result<NullableVec<i64>, TimeVecError> {
    vec_map_nullable_time(input, ctx, |date| {
        i64::from(date.weekday().num_days_from_sunday() + 1)
    })
}

/// 向量化 DAYOFYEAR：一年中的第几天（1–366）。
pub fn vec_day_of_year(
    input: &NullableVec<MysqlTime>,
    ctx: &mut EvalContext,
) -> Result<NullableVec<i64>, TimeVecError> {
    vec_map_nullable_time(input, ctx, |date| i64::from(date.ordinal()))
}

fn vec_map_nullable_time<U, F>(
    input: &NullableVec<MysqlTime>,
    ctx: &mut EvalContext,
    mut eval: F,
) -> Result<NullableVec<U>, TimeVecError>
where
    F: FnMut(NaiveDate) -> U,
{
    let mut output = Vec::with_capacity(input.len());
    for value in input.values() {
        output.push(match value {
            None => None,
            Some(value) => valid_date_or_null(*value, ctx)?.map(&mut eval),
        });
    }
    Ok(NullableVec::new(output))
}

/// 向量化 DAYNAME：返回英文星期名。
pub fn vec_day_name(
    input: &NullableVec<MysqlTime>,
    ctx: &mut EvalContext,
) -> Result<NullableVec<String>, TimeVecError> {
    vec_map_nullable_time(input, ctx, |date| {
        WEEKDAY_NAMES[date.weekday().num_days_from_monday() as usize].to_owned()
    })
}

/// 向量化 MONTHNAME：返回英文月份名；非法月份按模式警告或报错。
pub fn vec_month_name(
    input: &NullableVec<MysqlTime>,
    ctx: &mut EvalContext,
) -> Result<NullableVec<String>, TimeVecError> {
    let mut output = Vec::with_capacity(input.len());
    for value in input.values() {
        output.push(match value {
            None => None,
            Some(value) if value.month == 0 || value.month > 12 || value.is_zero() => {
                if value.is_zero() && !ctx.no_zero_date {
                    None
                } else {
                    ctx.handle_invalid(value.to_string())?;
                    None
                }
            }
            Some(value) => Some(MONTH_NAMES[value.month as usize - 1].to_owned()),
        });
    }
    Ok(NullableVec::new(output))
}

/// 向量化 LAST_DAY：返回当月最后一天的日期。
pub fn vec_last_day(
    input: &NullableVec<MysqlTime>,
    ctx: &mut EvalContext,
) -> Result<NullableVec<MysqlTime>, TimeVecError> {
    let mut output = Vec::with_capacity(input.len());
    for value in input.values() {
        output.push(match value {
            None => None,
            Some(value) if value.month == 0 || (value.day == 0 && ctx.no_zero_date) => {
                ctx.handle_invalid(value.to_string())?;
                None
            }
            Some(value) => {
                let first = NaiveDate::from_ymd_opt(value.year, value.month, 1)
                    .ok_or_else(|| TimeVecError::InvalidTime(value.to_string()))?;
                let next = if value.month == 12 {
                    NaiveDate::from_ymd_opt(value.year + 1, 1, 1)
                } else {
                    NaiveDate::from_ymd_opt(value.year, value.month + 1, 1)
                }
                .ok_or_else(|| TimeVecError::InvalidTime(value.to_string()))?;
                let last = next - Duration::days(1);
                debug_assert_eq!(first.month(), last.month());
                Some(MysqlTime::from_parts_unchecked(
                    last.year(),
                    last.month(),
                    last.day(),
                    0,
                    0,
                    0,
                    0,
                ))
            }
        });
    }
    Ok(NullableVec::new(output))
}

/// 向量化 MAKEDATE：由年与年内天数构造日期；两位数年份按 MySQL 规则扩展。
pub fn vec_make_date(
    years: &NullableVec<i64>,
    days: &NullableVec<i64>,
) -> Result<NullableVec<MysqlTime>, TimeVecError> {
    vec_zip_map(years, days, |year, day| {
        if *day <= 0 || *year < 0 || *year > 9999 {
            return Ok(None);
        }
        // MySQL 两位数年份：00–69 → 2000–2069，70–99 → 1970–1999。
        let year = if *year < 70 {
            *year + 2000
        } else if *year < 100 {
            *year + 1900
        } else {
            *year
        };
        let Some(start) = NaiveDate::from_ymd_opt(year as i32, 1, 1) else {
            return Ok(None);
        };
        let Some(date) = start.checked_add_signed(Duration::days(*day - 1)) else {
            return Ok(None);
        };
        if date.year() > 9999 {
            return Ok(None);
        }
        Ok(Some(MysqlTime::from_parts_unchecked(
            date.year(),
            date.month(),
            date.day(),
            0,
            0,
            0,
            0,
        )))
    })
}

/// 向量化 DATEDIFF：两侧日期天数差；非法日期传播为 NULL。
pub fn vec_date_diff(
    left: &NullableVec<MysqlTime>,
    right: &NullableVec<MysqlTime>,
    ctx: &mut EvalContext,
) -> Result<NullableVec<i64>, TimeVecError> {
    vec_zip_map(left, right, |left, right| {
        let Some(left) = valid_date_or_null(*left, ctx)? else {
            return Ok(None);
        };
        let Some(right) = valid_date_or_null(*right, ctx)? else {
            return Ok(None);
        };
        Ok(Some(left.signed_duration_since(right).num_days()))
    })
}

/// GET_FORMAT：按格式类型与地区返回 strftime 风格格式串。
pub fn get_format(format: &str, location: &str) -> &'static str {
    match (format, location.to_ascii_uppercase().as_str()) {
        ("DATE", "USA") => "%m.%d.%Y",
        ("DATE", "JIS" | "ISO") => "%Y-%m-%d",
        ("DATE", "EUR") => "%d.%m.%Y",
        ("DATE", "INTERNAL") => "%Y%m%d",
        ("DATETIME" | "TIMESTAMP", "USA" | "EUR") => "%Y-%m-%d %H.%i.%s",
        ("DATETIME" | "TIMESTAMP", "JIS" | "ISO") => "%Y-%m-%d %H:%i:%s",
        ("DATETIME" | "TIMESTAMP", "INTERNAL") => "%Y%m%d%H%i%s",
        ("TIME", "USA") => "%h:%i:%s %p",
        ("TIME", "JIS" | "ISO") => "%H:%i:%s",
        ("TIME", "EUR") => "%H.%i.%s",
        ("TIME", "INTERNAL") => "%H%i%s",
        _ => "",
    }
}

fn valid_period(period: i64) -> bool {
    !(period < 0 || period % 100 == 0 || period % 100 > 12)
}

fn period_to_month(period: u64) -> u64 {
    if period == 0 {
        return 0;
    }
    let (mut year, month) = (period / 100, period % 100);
    if year < 70 {
        year += 2000;
    } else if year < 100 {
        year += 1900;
    }
    year * 12 + month - 1
}

fn month_to_period(month: u64) -> u64 {
    if month == 0 {
        return 0;
    }
    let mut year = month / 12;
    if year < 70 {
        year += 2000;
    } else if year < 100 {
        year += 1900;
    }
    year * 100 + month % 12 + 1
}

/// 向量化 PERIOD_DIFF：计算两个 YYYYMM/YYMM 会计期之间的月差。
pub fn vec_period_diff(
    left: &NullableVec<i64>,
    right: &NullableVec<i64>,
) -> Result<NullableVec<i64>, TimeVecError> {
    vec_zip_map(left, right, |left, right| {
        if !valid_period(*left) || !valid_period(*right) {
            return Err(TimeVecError::IncorrectArgs("period_diff"));
        }
        Ok(Some(
            period_to_month(*left as u64) as i64 - period_to_month(*right as u64) as i64,
        ))
    })
}

/// 向量化 PERIOD_ADD：在会计期上加减月数。
pub fn vec_period_add(
    periods: &NullableVec<i64>,
    offsets: &NullableVec<i64>,
) -> Result<NullableVec<i64>, TimeVecError> {
    vec_zip_map(periods, offsets, |period, offset| {
        if !valid_period(*period) {
            return Err(TimeVecError::IncorrectArgs("period_add"));
        }
        let month = period_to_month(*period as u64) as i64 + offset;
        if month < 0 {
            return Err(TimeVecError::Overflow("period_add".to_owned()));
        }
        Ok(Some(month_to_period(month as u64) as i64))
    })
}

/// 向量化 HOUR：取 TIME 的小时绝对值分量。
pub fn vec_hour(input: &NullableVec<MysqlDuration>) -> NullableVec<i64> {
    NullableVec::new(
        input
            .values()
            .iter()
            .map(|v| v.map(MysqlDuration::hour))
            .collect(),
    )
}

/// 向量化 MINUTE：取 TIME 的分钟分量。
pub fn vec_minute(input: &NullableVec<MysqlDuration>) -> NullableVec<i64> {
    NullableVec::new(
        input
            .values()
            .iter()
            .map(|v| v.map(MysqlDuration::minute))
            .collect(),
    )
}

/// 向量化 SECOND：取 TIME 的秒分量。
pub fn vec_second(input: &NullableVec<MysqlDuration>) -> NullableVec<i64> {
    NullableVec::new(
        input
            .values()
            .iter()
            .map(|v| v.map(MysqlDuration::second))
            .collect(),
    )
}

/// 向量化 MICROSECOND：取 TIME 的微秒分量。
pub fn vec_microsecond(input: &NullableVec<MysqlDuration>) -> NullableVec<i64> {
    NullableVec::new(
        input
            .values()
            .iter()
            .map(|v| v.map(MysqlDuration::microsecond))
            .collect(),
    )
}

/// 向量化 TIME_TO_SEC：将 TIME 转为带符号的总秒数。
pub fn vec_time_to_sec(input: &NullableVec<MysqlDuration>) -> NullableVec<i64> {
    NullableVec::new(
        input
            .values()
            .iter()
            .map(|value| value.map(|duration| duration.as_micros() / MICROS_PER_SECOND))
            .collect(),
    )
}

fn checked_fsp(fsp: i32) -> Result<i32, TimeVecError> {
    if (0..=6).contains(&fsp) {
        Ok(fsp)
    } else {
        Err(TimeVecError::InvalidFsp(i64::from(fsp)))
    }
}

fn seconds_to_micros(seconds: f64, fsp: i32) -> Result<i64, TimeVecError> {
    checked_fsp(fsp)?;
    if !seconds.is_finite() {
        return Err(TimeVecError::InvalidTime(seconds.to_string()));
    }
    let quantum = 10_i64.pow((6 - fsp) as u32);
    let micros = (seconds * MICROS_PER_SECOND as f64).round();
    if micros > i64::MAX as f64 || micros < i64::MIN as f64 {
        return Err(TimeVecError::Overflow(seconds.to_string()));
    }
    Ok(((micros / quantum as f64).round() as i64) * quantum)
}

/// 向量化 SEC_TO_TIME：秒数转 TIME；超过 ±838:59:59 时截断并告警。
pub fn vec_sec_to_time(
    input: &NullableVec<f64>,
    fsp: i32,
    ctx: &mut EvalContext,
) -> Result<NullableVec<MysqlDuration>, TimeVecError> {
    checked_fsp(fsp)?;
    let mut output = Vec::with_capacity(input.len());
    for value in input.values() {
        output.push(match value {
            None => None,
            Some(value) => {
                let negative = value.is_sign_negative();
                let mut absolute = value.abs();
                let mut hours = (absolute / 3600.0).floor() as i64;
                // MySQL TIME 最大范围为 ±838:59:59，超出则截断并记 truncate 警告。
                if hours > MAX_TIME_HOUR {
                    ctx.handle_truncate(value.to_string())?;
                    hours = MAX_TIME_HOUR;
                    absolute = 59.0 * 60.0 + 59.0;
                } else {
                    absolute -= hours as f64 * 3600.0;
                }
                let micros = hours * MICROS_PER_HOUR + seconds_to_micros(absolute, fsp)?;
                Some(MysqlDuration::from_micros(if negative {
                    -micros
                } else {
                    micros
                }))
            }
        });
    }
    Ok(NullableVec::new(output))
}

/// 向量化 MAKETIME：由时/分/秒构造 TIME；越界分量置 NULL 或钳位到 MySQL 上限。
pub fn vec_make_time(
    hours: &NullableVec<i64>,
    minutes: &NullableVec<i64>,
    seconds: &NullableVec<f64>,
    hour_unsigned: bool,
    fsp: i32,
    _ctx: &mut EvalContext,
) -> Result<NullableVec<MysqlDuration>, TimeVecError> {
    if hours.len() != minutes.len() || hours.len() != seconds.len() {
        return Err(TimeVecError::LengthMismatch);
    }
    checked_fsp(fsp)?;
    let mut output = Vec::with_capacity(hours.len());
    for index in 0..hours.len() {
        let (Some(mut hour), Some(minute), Some(mut second)) = (
            hours.values()[index],
            minutes.values()[index],
            seconds.values()[index],
        ) else {
            output.push(None);
            continue;
        };
        if minute < 0 || minute >= 60 || second < 0.0 || second >= 60.0 {
            output.push(None);
            continue;
        }
        let mut overflow = false;
        if hour < 0 && hour_unsigned {
            hour = MAX_TIME_HOUR;
            overflow = true;
        }
        if hour < -MAX_TIME_HOUR {
            hour = -MAX_TIME_HOUR;
            overflow = true;
        } else if hour > MAX_TIME_HOUR {
            hour = MAX_TIME_HOUR;
            overflow = true;
        }
        if hour.unsigned_abs() == MAX_TIME_HOUR as u64 && minute == 59 && second > 59.0 {
            overflow = true;
        }
        let mut minute = minute;
        if overflow {
            minute = 59;
            second = 59.0;
        }
        let sign = if hour < 0 { -1 } else { 1 };
        let micros = hour.abs() * MICROS_PER_HOUR
            + minute * MICROS_PER_MINUTE
            + seconds_to_micros(second, fsp)?;
        output.push(Some(MysqlDuration::from_micros(sign * micros)));
    }
    Ok(NullableVec::new(output))
}

fn parse_tz(name: &str) -> Result<Tz, TimeVecError> {
    name.parse()
        .map_err(|_| TimeVecError::InvalidTimeZone(name.to_owned()))
}

#[derive(Clone, Copy, Debug)]
enum ConvertTimeZone {
    Named(Tz),
    Fixed(FixedOffset),
}

fn parse_convert_tz(name: &str) -> Option<ConvertTimeZone> {
    if name.is_empty() {
        return None;
    }
    if let Ok(timezone) = name.parse::<Tz>() {
        return Some(ConvertTimeZone::Named(timezone));
    }
    let (sign, rest) = match name.as_bytes().first() {
        Some(b'+') => (1, &name[1..]),
        Some(b'-') => (-1, &name[1..]),
        _ => return None,
    };
    let (hour, minute) = rest.split_once(':')?;
    if hour.len() != 2 || minute.len() != 2 {
        return None;
    }
    let hour: i32 = hour.parse().ok()?;
    let minute: i32 = minute.parse().ok()?;
    if hour > 14 || minute > 59 || (hour == 14 && minute != 0) {
        return None;
    }
    FixedOffset::east_opt(sign * (hour * 3600 + minute * 60)).map(ConvertTimeZone::Fixed)
}

fn local_to_utc(
    timezone: ConvertTimeZone,
    value: NaiveDateTime,
) -> Result<DateTime<Utc>, TimeVecError> {
    let local = match timezone {
        ConvertTimeZone::Named(timezone) => {
            timezone.from_local_datetime(&value).map(|v| v.to_utc())
        }
        ConvertTimeZone::Fixed(timezone) => {
            timezone.from_local_datetime(&value).map(|v| v.to_utc())
        }
    };
    match local {
        chrono::LocalResult::Single(value) => Ok(value),
        chrono::LocalResult::Ambiguous(_, _) => {
            Err(TimeVecError::AmbiguousLocalTime(value.to_string()))
        }
        chrono::LocalResult::None => Err(TimeVecError::NonexistentLocalTime(value.to_string())),
    }
}

fn utc_in_timezone(value: DateTime<Utc>, timezone: ConvertTimeZone) -> MysqlTime {
    let offset = match timezone {
        ConvertTimeZone::Named(timezone) => {
            timezone.offset_from_utc_datetime(&value.naive_utc()).fix()
        }
        ConvertTimeZone::Fixed(timezone) => timezone,
    };
    let local = value.with_timezone(&offset);
    MysqlTime::from_parts_unchecked(
        local.year(),
        local.month(),
        local.day(),
        local.hour(),
        local.minute(),
        local.second(),
        local.nanosecond() / 1_000,
    )
}

fn from_datetime<T: TimeZone>(value: DateTime<T>, fsp: i32) -> Result<MysqlTime, TimeVecError> {
    checked_fsp(fsp)?;
    let quantum = 10_u32.pow((6 - fsp) as u32);
    let micros = value.nanosecond() / 1_000 / quantum * quantum;
    MysqlTime::new(
        value.year(),
        value.month(),
        value.day(),
        value.hour(),
        value.minute(),
        value.second(),
        micros,
    )
}

/// 向量化 STATEMENT_TIMESTAMP：将语句开始时刻按会话时区广播到所有行。
pub fn vec_statement_timestamp(
    rows: usize,
    instant: DateTime<Utc>,
    timezone: &str,
    fsp: i32,
) -> Result<NullableVec<MysqlTime>, TimeVecError> {
    let timezone = parse_tz(timezone)?;
    let value = from_datetime(instant.with_timezone(&timezone), fsp)?;
    Ok(NullableVec::new(vec![Some(value); rows]))
}

/// 向量化 UTC_TIMESTAMP：将 UTC 时刻广播到所有行。
pub fn vec_utc_timestamp(
    rows: usize,
    instant: DateTime<Utc>,
    fsp: i32,
) -> Result<NullableVec<MysqlTime>, TimeVecError> {
    let value = from_datetime(instant, fsp)?;
    Ok(NullableVec::new(vec![Some(value); rows]))
}

/// 向量化 CURRENT_DATE：按会话时区取当日日期并广播。
pub fn vec_current_date(
    rows: usize,
    instant: DateTime<Utc>,
    timezone: &str,
) -> Result<NullableVec<MysqlTime>, TimeVecError> {
    let timezone = parse_tz(timezone)?;
    let local = instant.with_timezone(&timezone);
    let value = MysqlTime::new(local.year(), local.month(), local.day(), 0, 0, 0, 0)?;
    Ok(NullableVec::new(vec![Some(value); rows]))
}

/// 向量化解析 TSO（Timestamp Oracle，物理时间左移 18 位加逻辑计数）为日期时间。
pub fn vec_parse_tso(
    input: &NullableVec<i64>,
    timezone: &str,
) -> Result<NullableVec<MysqlTime>, TimeVecError> {
    let timezone = parse_tz(timezone)?;
    let mut output = Vec::with_capacity(input.len());
    for value in input.values() {
        output.push(match value {
            None | Some(0) => None,
            Some(value) if *value < 0 => None,
            Some(value) => {
                // TSO 高位为物理毫秒时间戳，低 18 位为逻辑计数。
                let millis = (*value as u64 >> 18) as i64;
                let instant = Utc
                    .timestamp_millis_opt(millis)
                    .single()
                    .ok_or_else(|| TimeVecError::InvalidTime(value.to_string()))?;
                Some(from_datetime(instant.with_timezone(&timezone), 6)?)
            }
        });
    }
    Ok(NullableVec::new(output))
}

/// 向量化 CONVERT_TZ：在源/目标时区之间转换；歧义或空洞本地时间报错。
pub fn vec_convert_tz(
    input: &NullableVec<MysqlTime>,
    from_timezone: &NullableVec<String>,
    to_timezone: &NullableVec<String>,
) -> Result<NullableVec<MysqlTime>, TimeVecError> {
    if input.len() != from_timezone.len() || input.len() != to_timezone.len() {
        return Err(TimeVecError::LengthMismatch);
    }
    let mut output = Vec::with_capacity(input.len());
    for index in 0..input.len() {
        let (Some(value), Some(from), Some(to)) = (
            input.values()[index],
            from_timezone.values()[index].as_deref(),
            to_timezone.values()[index].as_deref(),
        ) else {
            output.push(None);
            continue;
        };
        let (Some(from), Some(to)) = (parse_convert_tz(from), parse_convert_tz(to)) else {
            output.push(None);
            continue;
        };
        let Ok(naive) = value.to_naive() else {
            output.push(None);
            continue;
        };
        let instant = local_to_utc(from, naive)?;
        output.push(Some(utc_in_timezone(instant, to)));
    }
    Ok(NullableVec::new(output))
}

/// 向量化有界陈旧读时间钳位：把安全时间限制在 [min, max] 区间内。
pub fn vec_bounded_staleness(
    minimum: &NullableVec<MysqlTime>,
    maximum: &NullableVec<MysqlTime>,
    minimum_safe: MysqlTime,
) -> Result<NullableVec<MysqlTime>, TimeVecError> {
    vec_zip_map(minimum, maximum, |minimum, maximum| {
        minimum.to_naive()?;
        maximum.to_naive()?;
        if minimum > maximum {
            return Ok(None);
        }
        Ok(Some(if minimum_safe < *minimum {
            *minimum
        } else if minimum_safe > *maximum {
            *maximum
        } else {
            minimum_safe
        }))
    })
}

/// 将常量字面量广播为指定行数的可空列。
pub fn vec_literal<T: Clone>(rows: usize, literal: T) -> NullableVec<T> {
    NullableVec::new(vec![Some(literal); rows])
}

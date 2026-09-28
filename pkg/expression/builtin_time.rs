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

// Rust implementation of the scalar time helpers in `builtin_time.go`.
//
// Expression construction and row evaluation are wired by the package integration task. This
// file owns the deterministic MySQL/TiDB time semantics so scalar and vectorized signatures can
// share one implementation.
//
// 本文件实现 `builtin_time.go` 中的标量时间辅助语义（中文说明）：
// 解析/格式化日期时间与 duration、加减时间、PERIOD 换算、格式化掩码、
// 日期部件、时区转换、TSO（Timestamp Oracle）与有界陈旧读辅助等，
// 供标量与向量化签名共享同一确定性实现。

use std::fmt;
use std::sync::OnceLock;

use chrono::{
    DateTime, Datelike, Duration, FixedOffset, LocalResult, NaiveDate, NaiveDateTime, NaiveTime,
    TimeZone, Timelike, Utc, Weekday,
};
use chrono_tz::Tz;
use regex::Regex;

/// 一秒对应的微秒数。
const MICROS_PER_SECOND: i64 = 1_000_000;
/// 一分钟对应的微秒数。
const MICROS_PER_MINUTE: i64 = 60 * MICROS_PER_SECOND;
/// 一小时对应的微秒数。
const MICROS_PER_HOUR: i64 = 60 * MICROS_PER_MINUTE;
/// 一天对应的微秒数。
const MICROS_PER_DAY: i64 = 24 * MICROS_PER_HOUR;
/// MySQL TIME 类型允许的最大绝对值（838:59:59.999999）。
const MAX_TIME_MICROS: i64 = (838 * 3600 + 59 * 60 + 59) * MICROS_PER_SECOND + 999_999;
/// TSO 低位逻辑时钟所占比特数。
const TSO_LOGICAL_BITS: u32 = 18;

#[derive(Clone, Debug, Eq, PartialEq)]
/// 时间解析/运算错误。
pub struct TimeError(String);

impl TimeError {
    /// 构造带种类与值的非法输入错误。
    fn invalid(kind: &str, value: impl fmt::Display) -> Self {
        Self(format!("invalid {kind}: {value}"))
    }

    /// 构造溢出错误。
    fn overflow(kind: &str) -> Self {
        Self(format!("{kind} overflow"))
    }
}

impl fmt::Display for TimeError {
    /// Display 实现。
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for TimeError {}

/// 时间运算统一 Result 别名。
pub type TimeResult<T> = Result<T, TimeError>;

/// 与 Go isDuration 一致的 duration 词法正则（懒加载）。
fn duration_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(r"^\s*[-]?(((\d{1,2}\s+)?0*\d{0,3}(:0*\d{1,2}){0,2})|(\d{1,7}))?(\.\d*)?\s*$")
            .expect("the Go duration pattern is valid")
    })
}

/// Matches the same lexical duration family as the Go `isDuration` helper.
/// 与 Go `isDuration` 相同的 duration 词法族判定。
pub fn is_duration(value: &str) -> bool {
    duration_pattern().is_match(value)
}

/// ADDTIME/SUBTIME use either no fractional precision or maximum precision for string operands.
/// ADDTIME/SUBTIME 字符串操作数的小数精度选取。
pub fn get_fsp_for_time_add_sub(value: &str) -> u8 {
    match value.split_once('.') {
        None => 0,
        Some((_, fraction)) if fraction.chars().all(|ch| ch == '0') => 0,
        Some(_) => 6,
    }
}

/// 将小数秒文本规范化为微秒，超过 6 位时与 Go 一样截断。
fn parse_fraction(value: &str) -> TimeResult<u32> {
    if value.is_empty() {
        return Ok(0);
    }
    if !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(TimeError::invalid("fractional seconds", value));
    }
    let precision = value.len().min(6);
    let number = value[..precision]
        .parse::<u32>()
        .map_err(|_| TimeError::invalid("fractional seconds", value))?;
    Ok(number * 10_u32.pow(6 - precision as u32))
}

/// 拆分整数部分与小数秒。
fn split_fraction(value: &str) -> TimeResult<(&str, u32)> {
    match value.split_once('.') {
        Some((whole, fraction)) => Ok((whole, parse_fraction(fraction)?)),
        None => Ok((value, 0)),
    }
}

/// 两位数年份：00–69→2000+，70–99→1900+。
fn two_digit_year(year: i32) -> i32 {
    match year {
        0..=69 => year + 2000,
        70..=99 => year + 1900,
        _ => year,
    }
}

/// 解析紧凑数字形式的日期时间。
fn parse_compact_datetime(value: &str, micros: u32) -> TimeResult<NaiveDateTime> {
    // 按数字长度 6/8/12/14 切分年月日时分秒字段。
    let digits = value.as_bytes();
    if !digits.iter().all(u8::is_ascii_digit) {
        return Err(TimeError::invalid("datetime", value));
    }
    let field = |start: usize, end: usize| -> TimeResult<u32> {
        value[start..end]
            .parse()
            .map_err(|_| TimeError::invalid("datetime", value))
    };
    let (year, month, day, hour, minute, second) = match digits.len() {
        6 => (
            two_digit_year(field(0, 2)? as i32),
            field(2, 4)?,
            field(4, 6)?,
            0,
            0,
            0,
        ),
        8 => (field(0, 4)? as i32, field(4, 6)?, field(6, 8)?, 0, 0, 0),
        12 => (
            two_digit_year(field(0, 2)? as i32),
            field(2, 4)?,
            field(4, 6)?,
            field(6, 8)?,
            field(8, 10)?,
            field(10, 12)?,
        ),
        14 => (
            field(0, 4)? as i32,
            field(4, 6)?,
            field(6, 8)?,
            field(8, 10)?,
            field(10, 12)?,
            field(12, 14)?,
        ),
        _ => return Err(TimeError::invalid("datetime", value)),
    };
    let date = NaiveDate::from_ymd_opt(year, month, day)
        .ok_or_else(|| TimeError::invalid("date", value))?;
    date.and_hms_micro_opt(hour, minute, second, micros)
        .ok_or_else(|| TimeError::invalid("datetime", value))
}

/// Parses TiDB's common date, datetime, two-digit-year, and compact numeric forms.
/// 解析 TiDB 常见日期、日期时间、两位数年与紧凑数字形式。
pub fn parse_datetime(value: &str) -> TimeResult<NaiveDateTime> {
    let value = value.trim();
    // A dot in the date portion is a legal separator in Go/TiDB.  Treat only
    // the suffix of a compact numeric datetime or a clock as fractional seconds.
    let (whole, micros) = match value.rsplit_once('.') {
        Some((whole, fraction))
            if whole.contains(':') || whole.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            (whole, parse_fraction(fraction)?)
        }
        _ => (value, 0),
    };
    if whole.bytes().all(|byte| byte.is_ascii_digit()) {
        return parse_compact_datetime(whole, micros);
    }

    let (date_text, time_text) = whole.split_once(char::is_whitespace).unwrap_or((whole, ""));
    if date_text
        .chars()
        .any(|ch| !ch.is_ascii_digit() && !ch.is_ascii_punctuation())
    {
        return Err(TimeError::invalid("date", value));
    }
    let mut date_fields = date_text
        .split(|ch: char| ch.is_ascii_punctuation())
        .filter(|field| !field.is_empty());
    let year_text = date_fields.next().unwrap_or_default();
    let year = two_digit_year(
        year_text
            .parse::<i32>()
            .map_err(|_| TimeError::invalid("date", value))?,
    );
    let month = date_fields
        .next()
        .ok_or_else(|| TimeError::invalid("date", value))?
        .parse::<u32>()
        .map_err(|_| TimeError::invalid("date", value))?;
    let day = date_fields
        .next()
        .ok_or_else(|| TimeError::invalid("date", value))?
        .parse::<u32>()
        .map_err(|_| TimeError::invalid("date", value))?;
    if date_fields.next().is_some() {
        return Err(TimeError::invalid("date", value));
    }
    let date = NaiveDate::from_ymd_opt(year, month, day)
        .ok_or_else(|| TimeError::invalid("date", value))?;

    let (hour, minute, second) = if time_text.is_empty() {
        (0, 0, 0)
    } else {
        let mut fields = time_text.trim().split(':');
        let parse = |field: Option<&str>| -> TimeResult<u32> {
            field
                .ok_or_else(|| TimeError::invalid("datetime", value))?
                .parse()
                .map_err(|_| TimeError::invalid("datetime", value))
        };
        let result = (
            parse(fields.next())?,
            parse(fields.next())?,
            parse(fields.next())?,
        );
        if fields.next().is_some() {
            return Err(TimeError::invalid("datetime", value));
        }
        result
    };
    date.and_hms_micro_opt(hour, minute, second, micros)
        .ok_or_else(|| TimeError::invalid("datetime", value))
}

/// 格式化为 `YYYY-MM-DD HH:MM:SS[.ffffff]`。
pub fn format_datetime(value: NaiveDateTime) -> String {
    if value.and_utc().timestamp_subsec_micros() == 0 {
        value.format("%Y-%m-%d %H:%M:%S").to_string()
    } else {
        value.format("%Y-%m-%d %H:%M:%S%.6f").to_string()
    }
}

/// 将 duration 字符串解析为带符号微秒。
fn parse_duration_micros(value: &str) -> TimeResult<i64> {
    // 解析天、时:分:秒与小数；紧凑数字按秒/分/时从右切分，并检查溢出。
    if !is_duration(value) || value.trim().is_empty() {
        return Err(TimeError::invalid("duration", value));
    }
    let mut value = value.trim();
    let negative = value.starts_with('-');
    if negative {
        value = &value[1..];
    }
    let (whole, micros) = split_fraction(value)?;
    let (days, clock) = match whole.split_once(char::is_whitespace) {
        Some((days, clock)) => (
            days.trim()
                .parse::<i64>()
                .map_err(|_| TimeError::invalid("duration", value))?,
            clock.trim(),
        ),
        None => (0, whole),
    };
    let fields: Vec<&str> = clock.split(':').collect();
    let number = |text: &str| -> TimeResult<i64> {
        if text.is_empty() {
            return Ok(0);
        }
        text.parse()
            .map_err(|_| TimeError::invalid("duration", value))
    };
    let (hour, minute, second) = match fields.as_slice() {
        [compact] => {
            let compact = compact.trim_start_matches('0');
            let compact = if compact.is_empty() { "0" } else { compact };
            if compact.len() <= 2 {
                (0, 0, number(compact)?)
            } else {
                let split_second = compact.len() - 2;
                let split_minute = split_second.saturating_sub(2);
                (
                    number(&compact[..split_minute])?,
                    number(&compact[split_minute..split_second])?,
                    number(&compact[split_second..])?,
                )
            }
        }
        [hour, minute] => (number(hour)?, number(minute)?, 0),
        [hour, minute, second] => (number(hour)?, number(minute)?, number(second)?),
        _ => return Err(TimeError::invalid("duration", value)),
    };
    if minute >= 60 || second >= 60 {
        return Err(TimeError::invalid("duration", value));
    }
    let total = days
        .checked_mul(MICROS_PER_DAY)
        .and_then(|total| total.checked_add(hour * MICROS_PER_HOUR))
        .and_then(|total| total.checked_add(minute * MICROS_PER_MINUTE))
        .and_then(|total| total.checked_add(second * MICROS_PER_SECOND))
        .and_then(|total| total.checked_add(micros as i64))
        .ok_or_else(|| TimeError::overflow("duration"))?;
    Ok(if negative { -total } else { total })
}

/// 将微秒格式化为 MySQL duration 字符串。
fn format_duration_micros(micros: i64) -> String {
    let negative = micros < 0;
    let value = micros.unsigned_abs();
    let total_seconds = value / MICROS_PER_SECOND as u64;
    let fraction = value % MICROS_PER_SECOND as u64;
    let hour = total_seconds / 3600;
    let minute = total_seconds / 60 % 60;
    let second = total_seconds % 60;
    let sign = if negative { "-" } else { "" };
    if fraction == 0 {
        format!("{sign}{hour:02}:{minute:02}:{second:02}")
    } else {
        format!("{sign}{hour:02}:{minute:02}:{second:02}.{fraction:06}")
    }
}

/// Implements the string branches of ADDTIME. A datetime left operand remains a datetime;
/// otherwise both operands are interpreted as MySQL durations.
/// ADDTIME 字符串分支：datetime 左操作数保持 datetime，否则按 duration。
pub fn add_time_strings(left: &str, right: &str) -> TimeResult<String> {
    // 含空白与日期分隔符的左操作数按 datetime 处理，否则按 duration。
    let delta = parse_duration_micros(right)?;
    if left.contains('-') && left.contains(char::is_whitespace) {
        let datetime = parse_datetime(left)?;
        let result = datetime
            .checked_add_signed(Duration::microseconds(delta))
            .ok_or_else(|| TimeError::overflow("datetime"))?;
        Ok(format_datetime(result))
    } else {
        let left = parse_duration_micros(left)?;
        let result = left
            .checked_add(delta)
            .ok_or_else(|| TimeError::overflow("duration"))?;
        if result.unsigned_abs() > MAX_TIME_MICROS as u64 {
            return Err(TimeError::overflow("duration"));
        }
        Ok(format_duration_micros(result))
    }
}

/// SUBTIME：对右操作数取负后复用 ADDTIME。
pub fn sub_time_strings(left: &str, right: &str) -> TimeResult<String> {
    let right = parse_duration_micros(right)?;
    add_time_strings(left, &format_duration_micros(-right))
}

/// TIME_TO_SEC：duration 转秒。
pub fn time_to_sec(value: &str) -> TimeResult<i64> {
    Ok(parse_duration_micros(value)? / MICROS_PER_SECOND)
}

/// SEC_TO_TIME：秒（可小数）转 duration，并钳制到 TIME 范围。
pub fn sec_to_time(seconds: f64) -> TimeResult<String> {
    if !seconds.is_finite() {
        return Err(TimeError::invalid("seconds", seconds));
    }
    let micros = (seconds * MICROS_PER_SECOND as f64).round() as i64;
    let clamped = micros.clamp(-MAX_TIME_MICROS, MAX_TIME_MICROS);
    Ok(format_duration_micros(clamped))
}

/// 校验 MySQL PERIOD 值（年月，月∈1..12）。
pub fn valid_period(period: i64) -> bool {
    period >= 0 && period % 100 != 0 && period % 100 <= 12
}

/// PERIOD 转为绝对月数。
pub fn period_to_month(period: u64) -> u64 {
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

/// 绝对月数转回 PERIOD。
pub fn month_to_period(month: u64) -> u64 {
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

/// PERIOD_ADD：在 period 上加月数。
pub fn period_add(period: i64, months: i64) -> TimeResult<i64> {
    if !valid_period(period) {
        return Err(TimeError::invalid("period_add argument", period));
    }
    let month = period_to_month(period as u64) as i64 + months;
    if month <= 0 {
        return Err(TimeError::overflow("period_add"));
    }
    Ok(month_to_period(month as u64) as i64)
}

/// PERIOD_DIFF：两 period 相差月数。
pub fn period_diff(left: i64, right: i64) -> TimeResult<i64> {
    if !valid_period(left) || !valid_period(right) {
        return Err(TimeError::invalid(
            "period_diff arguments",
            format_args!("{left}, {right}"),
        ));
    }
    Ok(period_to_month(left as u64) as i64 - period_to_month(right as u64) as i64)
}

/// GET_FORMAT：按类型与地区返回格式掩码。
pub fn get_format(unit: &str, location: &str) -> Option<&'static str> {
    match (
        unit.to_ascii_uppercase().as_str(),
        location.to_ascii_uppercase().as_str(),
    ) {
        ("DATE", "USA") => Some("%m.%d.%Y"),
        ("DATE", "JIS" | "ISO") => Some("%Y-%m-%d"),
        ("DATE", "EUR") => Some("%d.%m.%Y"),
        ("DATE", "INTERNAL") => Some("%Y%m%d"),
        ("DATETIME" | "TIMESTAMP", "USA" | "EUR") => Some("%Y-%m-%d %H.%i.%s"),
        ("DATETIME" | "TIMESTAMP", "JIS" | "ISO") => Some("%Y-%m-%d %H:%i:%s"),
        ("DATETIME" | "TIMESTAMP", "INTERNAL") => Some("%Y%m%d%H%i%s"),
        ("TIME", "USA") => Some("%h:%i:%s %p"),
        ("TIME", "JIS" | "ISO") => Some("%H:%i:%s"),
        ("TIME", "EUR") => Some("%H.%i.%s"),
        ("TIME", "INTERNAL") => Some("%H%i%s"),
        _ => None,
    }
}

/// 按 TIME_FORMAT 风格掩码展开时分秒与微秒。
fn format_mask(mask: &str, hour: u64, minute: u64, second: u64, micros: u64) -> String {
    let hour12 = match hour % 12 {
        0 => 12,
        hour => hour,
    };
    let mut result = String::with_capacity(mask.len() + 16);
    let mut chars = mask.chars();
    while let Some(ch) = chars.next() {
        if ch != '%' {
            result.push(ch);
            continue;
        }
        let Some(code) = chars.next() else {
            result.push('%');
            break;
        };
        match code {
            '%' => result.push('%'),
            'H' => result.push_str(&format!("{hour:02}")),
            'k' => result.push_str(&hour.to_string()),
            'h' | 'I' => result.push_str(&format!("{hour12:02}")),
            'l' => result.push_str(&hour12.to_string()),
            'i' => result.push_str(&format!("{minute:02}")),
            's' | 'S' => result.push_str(&format!("{second:02}")),
            'f' => result.push_str(&format!("{micros:06}")),
            'p' => result.push_str(if hour % 24 < 12 { "AM" } else { "PM" }),
            'r' => result.push_str(&format!(
                "{hour12:02}:{minute:02}:{second:02} {}",
                if hour % 24 < 12 { "AM" } else { "PM" }
            )),
            'T' => result.push_str(&format!("{hour:02}:{minute:02}:{second:02}")),
            other => {
                result.push('%');
                result.push(other);
            }
        }
    }
    result
}

/// TIME_FORMAT：格式化 duration。
pub fn time_format(value: &str, mask: &str) -> TimeResult<String> {
    if mask.is_empty() {
        return Err(TimeError::invalid("time format", mask));
    }
    let micros = parse_duration_micros(value)?;
    let value = micros.unsigned_abs();
    let total_seconds = value / MICROS_PER_SECOND as u64;
    Ok(format_mask(
        mask,
        total_seconds / 3600,
        total_seconds / 60 % 60,
        total_seconds % 60,
        value % MICROS_PER_SECOND as u64,
    ))
}

/// MAKEDATE：年 + 一年中的第几天。
pub fn make_date(year: i64, day_of_year: i64) -> TimeResult<NaiveDate> {
    if day_of_year <= 0 || !(0..=9999).contains(&year) {
        return Err(TimeError::invalid(
            "make_date arguments",
            format_args!("{year}, {day_of_year}"),
        ));
    }
    let year = two_digit_year(year as i32);
    let start =
        NaiveDate::from_ymd_opt(year, 1, 1).ok_or_else(|| TimeError::invalid("year", year))?;
    let result = start
        .checked_add_signed(Duration::days(day_of_year - 1))
        .ok_or_else(|| TimeError::overflow("make_date"))?;
    if result.year() > 9999 {
        return Err(TimeError::overflow("make_date"));
    }
    Ok(result)
}

/// MAKETIME：构造 TIME，可钳制到 838:59:59。
pub fn make_time(
    mut hour: i64,
    minute: i64,
    second: f64,
    unsigned_hour: bool,
) -> TimeResult<String> {
    if !(0..60).contains(&minute) || !(0.0..60.0).contains(&second) || !second.is_finite() {
        return Err(TimeError::invalid(
            "make_time arguments",
            format_args!("{hour}, {minute}, {second}"),
        ));
    }
    let overflow = (hour < 0 && unsigned_hour) || !(-838..=838).contains(&hour);
    if hour < 0 && unsigned_hour {
        hour = 838;
    }
    hour = hour.clamp(-838, 838);
    let (minute, second) =
        if overflow || (hour.unsigned_abs() == 838 && minute == 59 && second > 59.0) {
            (59, 59.0)
        } else {
            (minute, second)
        };
    let sign = if hour < 0 { -1 } else { 1 };
    let micros = (hour.unsigned_abs() as i64 * MICROS_PER_HOUR
        + minute * MICROS_PER_MINUTE
        + (second * MICROS_PER_SECOND as f64).round() as i64)
        * sign;
    Ok(format_duration_micros(micros))
}

/// QUARTER：返回季度 1–4。
pub fn quarter(value: NaiveDateTime) -> u32 {
    (value.month() + 2) / 3
}

/// MONTHNAME：英文月份名。
pub fn month_name(value: NaiveDateTime) -> &'static str {
    const NAMES: [&str; 12] = [
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
    NAMES[value.month0() as usize]
}

/// DAYNAME：英文星期名。
pub fn day_name(value: NaiveDateTime) -> &'static str {
    match value.weekday() {
        Weekday::Mon => "Monday",
        Weekday::Tue => "Tuesday",
        Weekday::Wed => "Wednesday",
        Weekday::Thu => "Thursday",
        Weekday::Fri => "Friday",
        Weekday::Sat => "Saturday",
        Weekday::Sun => "Sunday",
    }
}

/// MySQL DAYOFWEEK numbering is Sunday=1 through Saturday=7.
/// DAYOFWEEK：1=Sunday … 7=Saturday。
pub fn day_of_week(value: NaiveDateTime) -> u32 {
    value.weekday().num_days_from_sunday() + 1
}

/// WEEKDAY：0=Monday … 6=Sunday。
pub fn week_day(value: NaiveDateTime) -> u32 {
    value.weekday().num_days_from_monday()
}

/// DAYOFYEAR：一年中的第几天。
pub fn day_of_year(value: NaiveDateTime) -> u32 {
    value.ordinal()
}

/// DATEDIFF：按日期差（忽略时间）计天数。
pub fn date_diff(left: NaiveDateTime, right: NaiveDateTime) -> i64 {
    left.date().signed_duration_since(right.date()).num_days()
}

/// 计算指定年月的最后一天。
fn last_day_of_month(year: i32, month: u32) -> TimeResult<u32> {
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let next = NaiveDate::from_ymd_opt(next_year, next_month, 1)
        .ok_or_else(|| TimeError::overflow("month"))?;
    Ok(next
        .pred_opt()
        .ok_or_else(|| TimeError::overflow("month"))?
        .day())
}

/// LAST_DAY：返回当月最后一天。
pub fn last_day(value: NaiveDateTime) -> NaiveDate {
    let day =
        last_day_of_month(value.year(), value.month()).expect("a valid date has a valid month");
    NaiveDate::from_ymd_opt(value.year(), value.month(), day).expect("last day is valid")
}

/// 按月加减并钳制到目标月末日。
fn add_months(value: NaiveDateTime, months: i64) -> TimeResult<NaiveDateTime> {
    let total = value.year() as i64 * 12 + value.month0() as i64 + months;
    if !(12..=119_999).contains(&total) {
        return Err(TimeError::overflow("datetime"));
    }
    let year = (total.div_euclid(12)) as i32;
    let month = total.rem_euclid(12) as u32 + 1;
    let day = value.day().min(last_day_of_month(year, month)?);
    NaiveDate::from_ymd_opt(year, month, day)
        .and_then(|date| {
            date.and_hms_micro_opt(
                value.hour(),
                value.minute(),
                value.second(),
                value.and_utc().timestamp_subsec_micros(),
            )
        })
        .ok_or_else(|| TimeError::overflow("datetime"))
}

/// Implements TIMESTAMPADD, including TiDB's truncation for SECOND fractions, rounding for other
/// numeric units, overflow checks, and end-of-month clamping.
/// TIMESTAMPADD：按单位加间隔，月/年末日钳制。
pub fn timestamp_add(unit: &str, interval: f64, value: NaiveDateTime) -> TimeResult<NaiveDateTime> {
    // MONTH/YEAR 走月末钳制；其余单位按微秒换算后加减。
    if !interval.is_finite() {
        return Err(TimeError::invalid("interval", interval));
    }
    let rounded = interval.round() as i64;
    match unit.to_ascii_uppercase().as_str() {
        "MICROSECOND" => value
            .checked_add_signed(Duration::microseconds(rounded))
            .ok_or_else(|| TimeError::overflow("datetime")),
        "SECOND" => value
            .checked_add_signed(Duration::microseconds(
                (interval * 1_000_000.0).trunc() as i64
            ))
            .ok_or_else(|| TimeError::overflow("datetime")),
        "MINUTE" => value
            .checked_add_signed(Duration::minutes(rounded))
            .ok_or_else(|| TimeError::overflow("datetime")),
        "HOUR" => value
            .checked_add_signed(Duration::hours(rounded))
            .ok_or_else(|| TimeError::overflow("datetime")),
        "DAY" => value
            .checked_add_signed(Duration::days(rounded))
            .ok_or_else(|| TimeError::overflow("datetime")),
        "WEEK" => value
            .checked_add_signed(Duration::weeks(rounded))
            .ok_or_else(|| TimeError::overflow("datetime")),
        "MONTH" => add_months(value, rounded),
        "QUARTER" => add_months(
            value,
            rounded
                .checked_mul(3)
                .ok_or_else(|| TimeError::overflow("interval"))?,
        ),
        "YEAR" => add_months(
            value,
            rounded
                .checked_mul(12)
                .ok_or_else(|| TimeError::overflow("interval"))?,
        ),
        _ => Err(TimeError::invalid("time unit", unit)),
    }
}

/// 计算两时间戳之间的完整月数。
fn whole_months(left: NaiveDateTime, right: NaiveDateTime) -> i64 {
    let mut months =
        (right.year() - left.year()) as i64 * 12 + right.month() as i64 - left.month() as i64;
    let left_tail = (left.day(), left.time());
    let right_tail = (right.day(), right.time());
    if months > 0 && right_tail < left_tail {
        months -= 1;
    } else if months < 0 && right_tail > left_tail {
        months += 1;
    }
    months
}

/// TIMESTAMPDIFF：按单位计算差。
pub fn timestamp_diff(unit: &str, left: NaiveDateTime, right: NaiveDateTime) -> TimeResult<i64> {
    let micros = right
        .signed_duration_since(left)
        .num_microseconds()
        .ok_or_else(|| TimeError::overflow("timestamp_diff"))?;
    let result = match unit.to_ascii_uppercase().as_str() {
        "MICROSECOND" => micros,
        "SECOND" => micros / MICROS_PER_SECOND,
        "MINUTE" => micros / MICROS_PER_MINUTE,
        "HOUR" => micros / MICROS_PER_HOUR,
        "DAY" => micros / MICROS_PER_DAY,
        "WEEK" => micros / (7 * MICROS_PER_DAY),
        "MONTH" => whole_months(left, right),
        "QUARTER" => whole_months(left, right) / 3,
        "YEAR" => whole_months(left, right) / 12,
        _ => return Err(TimeError::invalid("time unit", unit)),
    };
    Ok(result)
}

/// 解析 `+HH:MM` / `-HH:MM` 固定偏移。
fn parse_offset(value: &str) -> Option<FixedOffset> {
    let bytes = value.as_bytes();
    if bytes.len() != 6 || !matches!(bytes[0], b'+' | b'-') || bytes[3] != b':' {
        return None;
    }
    let hour = value[1..3].parse::<i32>().ok()?;
    let minute = value[4..6].parse::<i32>().ok()?;
    if minute >= 60 || hour > 14 || (hour == 14 && minute != 0) {
        return None;
    }
    let seconds = (hour * 60 + minute) * 60 * if bytes[0] == b'-' { -1 } else { 1 };
    FixedOffset::east_opt(seconds)
}

/// 将本地墙钟时间按固定偏移解释为 UTC。
fn local_in_fixed(value: NaiveDateTime, offset: FixedOffset) -> TimeResult<DateTime<Utc>> {
    match offset.from_local_datetime(&value) {
        LocalResult::Single(value) => Ok(value.with_timezone(&Utc)),
        _ => Err(TimeError::invalid("local datetime", value)),
    }
}

/// 将本地墙钟时间按命名时区解释为 UTC。
fn local_in_tz(value: NaiveDateTime, timezone: Tz) -> TimeResult<DateTime<Utc>> {
    match timezone.from_local_datetime(&value) {
        LocalResult::Single(value) => Ok(value.with_timezone(&Utc)),
        // Go's time.Date resolves a fall-back overlap to the standard-time
        // occurrence, which is the later UTC instant returned by chrono-tz.
        LocalResult::Ambiguous(_, second) => Ok(second.with_timezone(&Utc)),
        LocalResult::None => {
            // TiDB normalizes a wall clock in a DST spring-forward gap to the
            // first valid clock value after the transition.
            for minutes in 1..=24 * 60 {
                let Some(candidate) = value.checked_add_signed(Duration::minutes(minutes)) else {
                    break;
                };
                match timezone.from_local_datetime(&candidate) {
                    LocalResult::Single(value) => return Ok(value.with_timezone(&Utc)),
                    LocalResult::Ambiguous(_, second) => return Ok(second.with_timezone(&Utc)),
                    LocalResult::None => {}
                }
            }
            Err(TimeError::invalid("local datetime", value))
        }
    }
}

/// CONVERT_TZ：在 from/to 时区之间转换。
pub fn convert_tz(value: NaiveDateTime, from: &str, to: &str) -> TimeResult<NaiveDateTime> {
    // from/to 可为固定偏移或 IANA 时区名；先解为 UTC 再投影到目标区。
    let utc = if let Some(offset) = parse_offset(from) {
        local_in_fixed(value, offset)?
    } else {
        let timezone = from
            .parse::<Tz>()
            .map_err(|_| TimeError::invalid("time zone", from))?;
        local_in_tz(value, timezone)?
    };
    if let Some(offset) = parse_offset(to) {
        Ok(utc.with_timezone(&offset).naive_local())
    } else {
        let timezone = to
            .parse::<Tz>()
            .map_err(|_| TimeError::invalid("time zone", to))?;
        Ok(utc.with_timezone(&timezone).naive_local())
    }
}

/// TiDB TSO physical time is a Unix millisecond value in the high 46 bits.
/// 解析 TSO（Timestamp Oracle）物理时间为 UTC 日期时间。
pub fn parse_tso(tso: i64) -> TimeResult<NaiveDateTime> {
    if tso <= 0 {
        return Err(TimeError::invalid("tso", tso));
    }
    let millis = tso >> TSO_LOGICAL_BITS;
    DateTime::<Utc>::from_timestamp_millis(millis)
        .map(|value| value.naive_utc())
        .ok_or_else(|| TimeError::invalid("tso", tso))
}

/// 提取 TSO 逻辑时钟部分。
pub fn parse_tso_logical(tso: i64) -> TimeResult<i64> {
    if tso <= 0 {
        return Err(TimeError::invalid("tso", tso));
    }
    Ok(tso & ((1_i64 << TSO_LOGICAL_BITS) - 1))
}

/// Clamps SafeTS to the requested bounded-staleness interval.
/// 有界陈旧读：将时间夹取到 [min, max]。
pub fn cal_appropriate_time(
    min_time: NaiveDateTime,
    max_time: NaiveDateTime,
    min_safe_time: NaiveDateTime,
) -> NaiveDateTime {
    if min_safe_time < min_time {
        min_time
    } else if min_safe_time > max_time {
        max_time
    } else {
        min_safe_time
    }
}

/// MySQL's day number, not Chrono's CE day number. This preserves TiDB's TO_DAYS epoch.
/// TO_DAYS：自公元 0 年起的天数。
pub fn to_days(value: NaiveDateTime) -> i64 {
    let mut year = value.year() as i64;
    let month = value.month() as i64;
    let mut days = 365 * year + 31 * (month - 1) + value.day() as i64;
    if month <= 2 {
        year -= 1;
    } else {
        days -= (month * 4 + 23) / 10;
    }
    days + year / 4 - ((year / 100 + 1) * 3) / 4
}

/// TO_SECONDS：自公元 0 年起的秒数。
pub fn to_seconds(value: NaiveDateTime) -> i64 {
    to_days(value) * 86_400
        + value.hour() as i64 * 3600
        + value.minute() as i64 * 60
        + value.second() as i64
}

/// UNIX_TIMESTAMP：指定时区下的 Unix 秒。
pub fn unix_timestamp(value: NaiveDateTime, timezone: &str) -> TimeResult<i64> {
    let utc = if let Some(offset) = parse_offset(timezone) {
        local_in_fixed(value, offset)?
    } else {
        local_in_tz(
            value,
            timezone
                .parse::<Tz>()
                .map_err(|_| TimeError::invalid("time zone", timezone))?,
        )?
    };
    Ok(utc.timestamp())
}

/// FROM_UNIXTIME：Unix 秒+微秒转本地墙钟时间。
pub fn from_unix_time(seconds: i64, micros: u32, timezone: &str) -> TimeResult<NaiveDateTime> {
    if micros >= 1_000_000 {
        return Err(TimeError::invalid("microseconds", micros));
    }
    let utc = DateTime::<Utc>::from_timestamp(seconds, micros * 1_000)
        .ok_or_else(|| TimeError::invalid("unix timestamp", seconds))?;
    if let Some(offset) = parse_offset(timezone) {
        Ok(utc.with_timezone(&offset).naive_local())
    } else {
        let timezone = timezone
            .parse::<Tz>()
            .map_err(|_| TimeError::invalid("time zone", timezone))?;
        Ok(utc.with_timezone(&timezone).naive_local())
    }
}

/// EXTRACT：按单位抽取日期时间部件。
pub fn extract(unit: &str, value: NaiveDateTime) -> TimeResult<i64> {
    let result = match unit.to_ascii_uppercase().as_str() {
        "MICROSECOND" => value.and_utc().timestamp_subsec_micros() as i64,
        "SECOND" => value.second() as i64,
        "MINUTE" => value.minute() as i64,
        "HOUR" => value.hour() as i64,
        "DAY" => value.day() as i64,
        "WEEK" => value.iso_week().week() as i64,
        "MONTH" => value.month() as i64,
        "QUARTER" => quarter(value) as i64,
        "YEAR" => value.year() as i64,
        "SECOND_MICROSECOND" => {
            value.second() as i64 * 1_000_000 + value.and_utc().timestamp_subsec_micros() as i64
        }
        "MINUTE_SECOND" => value.minute() as i64 * 100 + value.second() as i64,
        "HOUR_MINUTE" => value.hour() as i64 * 100 + value.minute() as i64,
        "DAY_HOUR" => value.day() as i64 * 100 + value.hour() as i64,
        "YEAR_MONTH" => value.year() as i64 * 100 + value.month() as i64,
        _ => return Err(TimeError::invalid("time unit", unit)),
    };
    Ok(result)
}

/// DATE_FORMAT：按 MySQL 风格掩码格式化。
pub fn date_format(value: NaiveDateTime, mask: &str) -> String {
    // 逐字符解释 % 掩码，未识别的 %X 原样输出。
    let mut output = String::with_capacity(mask.len() + 24);
    let mut chars = mask.chars();
    while let Some(ch) = chars.next() {
        if ch != '%' {
            output.push(ch);
            continue;
        }
        let Some(code) = chars.next() else {
            output.push('%');
            break;
        };
        match code {
            'Y' => output.push_str(&format!("{:04}", value.year())),
            'y' => output.push_str(&format!("{:02}", value.year().rem_euclid(100))),
            'm' => output.push_str(&format!("{:02}", value.month())),
            'c' => output.push_str(&value.month().to_string()),
            'M' => output.push_str(month_name(value)),
            'b' => output.push_str(&month_name(value)[..3]),
            'd' => output.push_str(&format!("{:02}", value.day())),
            'e' => output.push_str(&value.day().to_string()),
            'D' => output.push_str(&format!("{}{}", value.day(), ordinal_suffix(value.day()))),
            'j' => output.push_str(&format!("{:03}", value.ordinal())),
            'W' => output.push_str(day_name(value)),
            'a' => output.push_str(&day_name(value)[..3]),
            'w' => output.push_str(&value.weekday().num_days_from_sunday().to_string()),
            'U' => output.push_str(&format!("{:02}", week_number_sunday(value.date()))),
            'u' => output.push_str(&format!("{:02}", week_number_monday(value.date()))),
            'V' | 'v' => output.push_str(&format!("{:02}", value.iso_week().week())),
            'X' | 'x' => output.push_str(&format!("{:04}", value.iso_week().year())),
            code @ ('H' | 'k' | 'h' | 'I' | 'l' | 'i' | 's' | 'S' | 'f' | 'p' | 'r' | 'T') => {
                output.push_str(&format_mask(
                    &format!("%{code}"),
                    value.hour() as u64,
                    value.minute() as u64,
                    value.second() as u64,
                    value.and_utc().timestamp_subsec_micros() as u64,
                ));
            }
            '%' => output.push('%'),
            other => output.push(other),
        }
    }
    output
}

/// 英文序数后缀（st/nd/rd/th）。
fn ordinal_suffix(day: u32) -> &'static str {
    if (11..=13).contains(&(day % 100)) {
        "th"
    } else {
        match day % 10 {
            1 => "st",
            2 => "nd",
            3 => "rd",
            _ => "th",
        }
    }
}

/// 以周日为一周起始的周序号。
fn week_number_sunday(date: NaiveDate) -> u32 {
    let first = NaiveDate::from_ymd_opt(date.year(), 1, 1).expect("valid year");
    let before_first_sunday = (7 - first.weekday().num_days_from_sunday()) % 7;
    let ordinal = date.ordinal0();
    if ordinal < before_first_sunday {
        0
    } else {
        (ordinal - before_first_sunday) / 7 + 1
    }
}

/// 以周一为一周起始的周序号。
fn week_number_monday(date: NaiveDate) -> u32 {
    let first = NaiveDate::from_ymd_opt(date.year(), 1, 1).expect("valid year");
    let before_first_monday = (7 - first.weekday().num_days_from_monday()) % 7;
    let ordinal = date.ordinal0();
    if ordinal < before_first_monday {
        0
    } else {
        (ordinal - before_first_monday) / 7 + 1
    }
}

/// CURRENT_DATE：按会话时区取当前日期。
pub fn current_date(now: DateTime<Utc>, timezone: &str) -> TimeResult<NaiveDate> {
    if let Some(offset) = parse_offset(timezone) {
        Ok(now.with_timezone(&offset).date_naive())
    } else {
        let timezone = timezone
            .parse::<Tz>()
            .map_err(|_| TimeError::invalid("time zone", timezone))?;
        Ok(now.with_timezone(&timezone).date_naive())
    }
}

/// CURRENT_TIME：按会话时区与 fsp 取当前时间。
pub fn current_time(now: DateTime<Utc>, timezone: &str, fsp: u8) -> TimeResult<NaiveTime> {
    if fsp > 6 {
        return Err(TimeError::invalid("fsp", fsp));
    }
    let value = if let Some(offset) = parse_offset(timezone) {
        now.with_timezone(&offset).time()
    } else {
        let timezone = timezone
            .parse::<Tz>()
            .map_err(|_| TimeError::invalid("time zone", timezone))?;
        now.with_timezone(&timezone).time()
    };
    let factor = 10_u32.pow(9 - fsp as u32);
    let rounded = ((value.nanosecond() + factor / 2) / factor) * factor;
    if rounded >= 1_000_000_000 {
        NaiveTime::from_hms_nano_opt(value.hour(), value.minute(), value.second(), 0)
            .and_then(|time| time.overflowing_add_signed(Duration::seconds(1)).0.into())
            .ok_or_else(|| TimeError::overflow("current_time"))
    } else {
        value
            .with_nanosecond(rounded)
            .ok_or_else(|| TimeError::overflow("current_time"))
    }
}

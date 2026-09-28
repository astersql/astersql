// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 导入工具随机数与日期时间辅助。
//
// 使用原子 LCG 生成无锁伪随机数；提供整型/字符串/时长随机，以及
// civil 日换算（Howard Hinnant 算法）支撑 DATE/TIME/TIMESTAMP/YEAR 随机生成。

use crate::config::ImporterError;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 字符串随机字母表（数字 + 大小写字母，共 62 字符）。
pub const ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// 全局 LCG 状态；0 表示尚未用时间种子初始化。
static RANDOM_STATE: AtomicU64 = AtomicU64::new(0);

/// 显式设置随机种子（至少为 1）。
pub fn seed(seed: u64) {
    RANDOM_STATE.store(seed.max(1), Ordering::Release);
}

/// 无锁 LCG 步进，返回下一个伪随机 `u64`。
fn random_u64() -> u64 {
    let mut current = RANDOM_STATE.load(Ordering::Acquire);
    if current == 0 {
        current = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64
            | 1;
    }
    loop {
        let next = current
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        match RANDOM_STATE.compare_exchange_weak(current, next, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => return next,
            Err(actual) => current = actual,
        }
    }
}

/// 闭区间 `[minimum, maximum]` 内的随机 `i64`。
pub fn rand_i64(minimum: i64, maximum: i64) -> Result<i64, ImporterError> {
    if minimum > maximum {
        return Err(ImporterError::InvalidRange(format!(
            "{minimum}..={maximum}"
        )));
    }
    let width = (maximum as i128 - minimum as i128 + 1) as u128;
    Ok((minimum as i128 + (u128::from(random_u64()) % width) as i128) as i64)
}

/// 闭区间 `[minimum, maximum]` 内的随机 `usize`。
pub fn rand_usize(minimum: usize, maximum: usize) -> Result<usize, ImporterError> {
    if minimum > maximum {
        return Err(ImporterError::InvalidRange(format!(
            "{minimum}..={maximum}"
        )));
    }
    Ok(minimum + random_u64() as usize % (maximum - minimum + 1))
}

/// 先随机整型再按精度格式化为浮点。
pub fn rand_float64(minimum: i64, maximum: i64, precision: usize) -> Result<f64, ImporterError> {
    let value = rand_i64(minimum, maximum)? as f64;
    let rendered = format!("{value:.precision$}");
    rendered
        .parse()
        .map_err(|_| ImporterError::InvalidRange(rendered))
}

/// 随机布尔。
pub fn rand_bool() -> bool {
    random_u64() & 1 == 1
}

/// 从字母表随机生成指定长度字符串。
pub fn rand_string(length: usize) -> String {
    (0..length)
        .map(|_| ALPHABET[random_u64() as usize % ALPHABET.len()] as char)
        .collect()
}

/// 随机时长，不超过 `maximum`。
pub fn rand_duration(maximum: Duration) -> Duration {
    let maximum = maximum.as_nanos().min(u128::from(u64::MAX)) as u64;
    Duration::from_nanos(random_u64() % maximum.saturating_add(1))
}

fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// 将相对 Unix epoch 的天数转为公历年月日（civil date）。
pub(crate) fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let days = days + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year as i32, month as u32, day as u32)
}

/// 公历年月日转为相对 Unix epoch 的天数。
pub(crate) fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = i64::from(year) - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_prime = i64::from(month) + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * month_prime + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn parse_date(value: &str) -> Result<i64, ImporterError> {
    let values = value
        .split('-')
        .map(str::parse)
        .collect::<Result<Vec<i32>, _>>()
        .map_err(|_| ImporterError::InvalidRange(value.to_owned()))?;
    if values.len() != 3 {
        return Err(ImporterError::InvalidRange(value.to_owned()));
    }
    Ok(days_from_civil(
        values[0],
        values[1] as u32,
        values[2] as u32,
    ))
}

fn parse_time(value: &str) -> Result<i64, ImporterError> {
    let values = value
        .split(':')
        .map(str::parse)
        .collect::<Result<Vec<i64>, _>>()
        .map_err(|_| ImporterError::InvalidRange(value.to_owned()))?;
    if values.len() != 3 {
        return Err(ImporterError::InvalidRange(value.to_owned()));
    }
    Ok(values[0] * 3600 + values[1] * 60 + values[2])
}

fn format_date(days: i64) -> String {
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}")
}

fn format_time(seconds: i64) -> String {
    let seconds = seconds.rem_euclid(86_400);
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    )
}

/// 随机 DATE；无下界时用当前年份 + 随机月日（日限 1..=28）。
pub fn rand_date(minimum: &str, maximum: &str) -> Result<String, ImporterError> {
    if minimum.is_empty() {
        let (year, _, _) = civil_from_days(now_seconds() / 86_400);
        return Ok(format!(
            "{year:04}-{:02}-{:02}",
            rand_i64(1, 12)?,
            rand_i64(1, 28)?
        ));
    }
    let minimum = parse_date(minimum)?;
    let maximum = if maximum.is_empty() {
        minimum + 365
    } else {
        parse_date(maximum)?
    };
    Ok(format_date(rand_i64(minimum, maximum)?))
}

/// 随机 TIME；缺省区间时在一天内随机秒。
pub fn rand_time(minimum: &str, maximum: &str) -> Result<String, ImporterError> {
    if minimum.is_empty() || maximum.is_empty() {
        return Ok(format_time(rand_i64(0, 86_399)?));
    }
    Ok(format_time(rand_i64(
        parse_time(minimum)?,
        parse_time(maximum)?,
    )?))
}

/// 随机 TIMESTAMP（日期空格时钟）；无下界时分别随机 date/time。
pub fn rand_timestamp(minimum: &str, maximum: &str) -> Result<String, ImporterError> {
    if minimum.is_empty() {
        return Ok(format!("{} {}", rand_date("", "")?, rand_time("", "")?));
    }
    let (minimum_date, minimum_time) = minimum
        .split_once(' ')
        .ok_or_else(|| ImporterError::InvalidRange(minimum.to_owned()))?;
    let minimum = parse_date(minimum_date)? * 86_400 + parse_time(minimum_time)?;
    if maximum.is_empty() {
        let value = minimum + rand_i64(0, 365)? * 86_400;
        return Ok(format!(
            "{} {}",
            format_date(value.div_euclid(86_400)),
            format_time(value)
        ));
    }
    let maximum = {
        let (date, time) = maximum
            .split_once(' ')
            .ok_or_else(|| ImporterError::InvalidRange(maximum.to_owned()))?;
        parse_date(date)? * 86_400 + parse_time(time)?
    };
    let value = rand_i64(minimum, maximum)?;
    Ok(format!(
        "{} {}",
        format_date(value.div_euclid(86_400)),
        format_time(value)
    ))
}

/// 随机 YEAR；缺省时在当前年往前 0..=10 年。
pub fn rand_year(minimum: &str, maximum: &str) -> Result<String, ImporterError> {
    let (current, _, _) = civil_from_days(now_seconds() / 86_400);
    if minimum.is_empty() || maximum.is_empty() {
        return Ok(format!("{:04}", current - rand_i64(0, 10)? as i32));
    }
    let minimum: i32 = minimum
        .parse()
        .map_err(|_| ImporterError::InvalidRange(minimum.to_owned()))?;
    let maximum: i32 = maximum
        .parse()
        .map_err(|_| ImporterError::InvalidRange(maximum.to_owned()))?;
    let minimum = days_from_civil(minimum, 1, 1) * 86_400;
    let maximum = days_from_civil(maximum, 1, 1) * 86_400;
    let (year, _, _) = civil_from_days(rand_i64(minimum, maximum)?.div_euclid(86_400));
    Ok(format!("{year:04}"))
}
